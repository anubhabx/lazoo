//! The desktop Inbox read model: `inbox.list` and `inbox.counts`, derived on every call from
//! the approval, task, and incident ledgers. Nothing is stored and no mutation exists here;
//! every action an item carries names an existing revision-checked RPC.

use lazoo_domain::{
    InboxCursor, InboxValidationError, approval_inbox_item, inbox_counts, inbox_filter_matches,
    incident_inbox_item, sort_inbox_archive, sort_unresolved_inbox, task_inbox_item,
    validate_inbox_limit,
};
use lazoo_protocol::{
    INBOX_CONTRACT_V1, InboxCountsResult, InboxFilter, InboxItem, InboxListParams, InboxListResult,
    RpcError,
};
use lazoo_store::{InboxArchive, InboxSources};
use serde_json::Value;

use super::{App, store_error, to_value};

impl App {
    pub(super) fn inbox_list(&self, value: &Value) -> Result<Value, RpcError> {
        let params: InboxListParams = serde_json::from_value(value.clone())
            .map_err(|error| inbox_invalid(error.to_string()))?;
        validate_inbox_limit(params.limit).map_err(|error| inbox_invalid(error.to_string()))?;
        let sources = self.store.inbox_sources().map_err(store_error)?;
        let unresolved = unresolved_items(&sources);
        let counts = inbox_counts(&unresolved);
        let limit = params.limit as usize;
        let (mut items, event_high_water) = if params.filter == InboxFilter::Resolved {
            let before = params
                .before
                .as_deref()
                .map(InboxCursor::parse)
                .transpose()
                .map_err(|error| inbox_invalid(error.to_string()))?;
            let archive = self
                .store
                .inbox_archive(
                    before
                        .as_ref()
                        .map(|cursor| (cursor.updated_at.as_str(), cursor.id)),
                    params.limit,
                )
                .map_err(store_error)?;
            let mut items = archive_items(&archive);
            sort_inbox_archive(&mut items);
            (
                items,
                archive.event_high_water.max(sources.event_high_water),
            )
        } else {
            if params.before.is_some() {
                return Err(inbox_invalid(
                    InboxValidationError::CursorFilter.to_string(),
                ));
            }
            let items = unresolved
                .into_iter()
                .filter(|item| inbox_filter_matches(params.filter, item))
                .collect();
            (items, sources.event_high_water)
        };
        let truncated = items.len() > limit;
        items.truncate(limit);
        let next_before = (params.filter == InboxFilter::Resolved && truncated)
            .then(|| items.last().map(InboxCursor::of))
            .flatten()
            .map(|cursor| cursor.render());
        Ok(to_value(InboxListResult {
            version: INBOX_CONTRACT_V1,
            items,
            counts,
            event_high_water,
            truncated,
            next_before,
        }))
    }

    pub(super) fn inbox_counts(&self, value: &Value) -> Result<Value, RpcError> {
        super::require_empty_params(value, "inbox.counts")?;
        let sources = self.store.inbox_sources().map_err(store_error)?;
        Ok(to_value(InboxCountsResult {
            version: INBOX_CONTRACT_V1,
            counts: inbox_counts(&unresolved_items(&sources)),
            event_high_water: sources.event_high_water,
        }))
    }
}

fn unresolved_items(sources: &InboxSources) -> Vec<InboxItem> {
    let employees = &sources.employees;
    let mut items: Vec<InboxItem> = sources
        .approvals
        .iter()
        .map(|approval| approval_inbox_item(approval, employees))
        .collect();
    items.extend(
        sources
            .tasks
            .iter()
            .filter_map(|task| task_inbox_item(task, false, employees)),
    );
    items.extend(
        sources
            .incidents
            .iter()
            .filter_map(|(incident, conversation_id)| {
                incident_inbox_item(incident, *conversation_id, employees)
            }),
    );
    sort_unresolved_inbox(&mut items);
    items
}

fn archive_items(archive: &InboxArchive) -> Vec<InboxItem> {
    let employees = &archive.employees;
    let mut items: Vec<InboxItem> = archive
        .approvals
        .iter()
        .map(|approval| approval_inbox_item(approval, employees))
        .collect();
    items.extend(
        archive
            .tasks
            .iter()
            .filter_map(|(task, once_uncertain)| task_inbox_item(task, *once_uncertain, employees)),
    );
    items.extend(
        archive
            .incidents
            .iter()
            .filter_map(|(incident, conversation_id)| {
                incident_inbox_item(incident, *conversation_id, employees)
            }),
    );
    items
}

fn inbox_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-INBOX-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}
