//! Event bodies remain intact in the audit log. Wire projections have an explicit size budget.
use lazoo_protocol::EventSummary;
use serde_json::json;

const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_PAGE_BYTES: usize = 768 * 1024;

pub(super) fn bounded_event(mut event: EventSummary) -> EventSummary {
    let bytes = serde_json::to_vec(&event)
        .expect("event is serializable")
        .len();
    if bytes > MAX_EVENT_BYTES {
        event.payload = json!({
            "_lazooOmitted": true,
            "code": "LZ-EVENT-PAYLOAD-TOO-LARGE",
            "eventBytes": bytes,
        });
    }
    event
}

pub(super) fn bounded_page(events: Vec<EventSummary>) -> Vec<EventSummary> {
    let mut bytes = 2;
    events
        .into_iter()
        .map(bounded_event)
        .take_while(|event| {
            bytes += serde_json::to_vec(event)
                .expect("event is serializable")
                .len()
                + 1;
            bytes <= MAX_PAGE_BYTES
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_event_preserves_cursor_and_explicitly_omits_only_wire_payload() {
        let event = EventSummary {
            sequence: 123,
            id: uuid::Uuid::new_v4(),
            event_type: "task.progressed".to_owned(),
            payload: json!({"text": "界".repeat(400_000)}),
            created_at: "2026-11-03".to_owned(),
        };
        let wire = bounded_event(event.clone());
        assert_eq!(wire.sequence, event.sequence);
        assert_eq!(wire.id, event.id);
        assert_eq!(wire.event_type, event.event_type);
        assert_eq!(wire.payload["code"], "LZ-EVENT-PAYLOAD-TOO-LARGE");
        assert!(
            serde_json::to_vec(&json!({"jsonrpc":"2.0", "method":"events.event", "params":wire}))
                .unwrap()
                .len()
                < lazoo_protocol::MAX_FRAME_BYTES
        );
        assert!(event.payload["text"].as_str().unwrap().len() > lazoo_protocol::MAX_FRAME_BYTES);
    }
}
