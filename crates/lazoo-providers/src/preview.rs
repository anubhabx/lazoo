//! The configuration migration preview (brief section 16.1 step 6), derived from the
//! read-only discovery report. It says what Lazoo would manage if the user asked it to:
//! one candidate per distinct integration, whether it is a plain import, an exact duplicate
//! across providers to merge, a same-name conflict that needs the user's choice (section
//! 16.3), or unsupported. Nothing is selected, written, or stored.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ProviderKind,
    config::{
        DiscoveredIntegration, IntegrationKind, IntegrationTransport, ProviderConfigurationReport,
    },
};

pub const PROVIDER_MIGRATION_PREVIEW_V1: u32 = 1;

/// What the preview proposes for one candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationProposal {
    /// One provider holds it; Lazoo would manage it as is.
    Manage,
    /// Several providers hold the same integration (equal identity); Lazoo would manage one
    /// entry for all of them.
    Merge,
    /// Another integration shares its name with a different identity; the user must choose
    /// one or give a provider-specific alias (section 16.3).
    Choose,
}

/// Where one candidate was found.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MigrationSource {
    pub provider: ProviderKind,
    pub name: String,
    pub file: String,
    pub enabled: bool,
}

/// One integration Lazoo could manage, grouped by identity across providers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MigrationCandidate {
    pub identity: String,
    pub name: String,
    pub kind: IntegrationKind,
    pub transport: IntegrationTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// Names only, the union over the sources; values are never read into the preview.
    pub secret_keys: Vec<String>,
    pub proposal: MigrationProposal,
    /// Lazoo's words for the proposal.
    pub reason: String,
    pub sources: Vec<MigrationSource>,
    /// Providers with a known configuration that already hold this integration.
    pub present_in: Vec<ProviderKind>,
    /// Providers with a known configuration that do not hold it yet.
    pub missing_from: Vec<ProviderKind>,
    /// `provider/name` labels of integrations that share the name with another identity.
    pub conflicts_with: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MigrationSummary {
    pub manage: u64,
    pub merge: u64,
    pub choose: u64,
    pub unsupported: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MigrationPreview {
    pub version: u32,
    pub scanned_at: String,
    /// Providers whose configuration Lazoo can read, in roster order.
    pub providers: Vec<ProviderKind>,
    pub candidates: Vec<MigrationCandidate>,
    /// The discovery report's unsupported entries, unchanged.
    pub unsupported: Vec<String>,
    pub summary: MigrationSummary,
}

/// Derive the preview from a discovery report. Candidates are ordered by name, then identity.
#[must_use]
pub fn migration_preview(report: &ProviderConfigurationReport) -> MigrationPreview {
    let providers: Vec<ProviderKind> = report.files.iter().map(|file| file.provider).collect();
    let mut by_identity: BTreeMap<&str, Vec<&DiscoveredIntegration>> = BTreeMap::new();
    for integration in &report.integrations {
        by_identity
            .entry(integration.identity.as_str())
            .or_default()
            .push(integration);
    }
    let mut candidates: Vec<MigrationCandidate> = by_identity
        .values()
        .map(|group| candidate(group, &providers, &report.integrations))
        .collect();
    candidates.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.identity.cmp(&right.identity))
    });
    let mut summary = MigrationSummary {
        unsupported: report.unsupported.len() as u64,
        ..MigrationSummary::default()
    };
    for candidate in &candidates {
        match candidate.proposal {
            MigrationProposal::Manage => summary.manage += 1,
            MigrationProposal::Merge => summary.merge += 1,
            MigrationProposal::Choose => summary.choose += 1,
        }
    }
    MigrationPreview {
        version: PROVIDER_MIGRATION_PREVIEW_V1,
        scanned_at: report.scanned_at.clone(),
        providers,
        candidates,
        unsupported: report.unsupported.clone(),
        summary,
    }
}

fn candidate(
    group: &[&DiscoveredIntegration],
    providers: &[ProviderKind],
    all: &[DiscoveredIntegration],
) -> MigrationCandidate {
    let first = group[0];
    let names: BTreeSet<String> = group
        .iter()
        .map(|integration| integration.name.to_lowercase())
        .collect();
    let conflicts_with: Vec<String> = all
        .iter()
        .filter(|other| {
            other.identity != first.identity && names.contains(&other.name.to_lowercase())
        })
        .map(|other| format!("{}/{}", other.provider, other.name))
        .collect();
    let present: BTreeSet<ProviderKind> = group
        .iter()
        .map(|integration| integration.provider)
        .collect();
    let present_in: Vec<ProviderKind> = providers
        .iter()
        .copied()
        .filter(|provider| present.contains(provider))
        .collect();
    let missing_from: Vec<ProviderKind> = providers
        .iter()
        .copied()
        .filter(|provider| !present.contains(provider))
        .collect();
    let (proposal, reason) = if !conflicts_with.is_empty() {
        (
            MigrationProposal::Choose,
            format!(
                "{} shares its name with a different integration ({}): choose one or give it a provider-specific alias",
                first.name,
                conflicts_with.join(", ")
            ),
        )
    } else if present_in.len() > 1 {
        (
            MigrationProposal::Merge,
            format!(
                "{} is configured identically in {}: Lazoo would manage one entry for all of them",
                first.name,
                present_in
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else {
        (
            MigrationProposal::Manage,
            format!(
                "{} is configured only in {}: Lazoo would manage it as is",
                first.name, first.provider
            ),
        )
    };
    let secret_keys: BTreeSet<String> = group
        .iter()
        .flat_map(|integration| integration.secret_keys.iter().cloned())
        .collect();
    MigrationCandidate {
        identity: first.identity.clone(),
        name: first.name.clone(),
        kind: first.kind,
        transport: first.transport,
        command: first.command.clone(),
        args: first.args.clone(),
        url: first.url.clone(),
        secret_keys: secret_keys.into_iter().collect(),
        proposal,
        reason,
        sources: group
            .iter()
            .map(|integration| MigrationSource {
                provider: integration.provider,
                name: integration.name.clone(),
                file: integration.source.to_string_lossy().into_owned(),
                enabled: integration.enabled,
            })
            .collect(),
        present_in,
        missing_from,
        conflicts_with,
    }
}
