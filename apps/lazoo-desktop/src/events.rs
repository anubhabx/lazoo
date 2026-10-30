//! `events.subscribe` forwarding: durable replay then live notifications to one channel.
use lazoo_protocol::EventSummary;
use lazoo_runtime::Installation;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tokio::task::JoinHandle;

use crate::client::{self, DesktopClientError, DesktopFailure};

/// One message on the event channel. The UI keeps the last `sequence` it consumed and
/// resubscribes from there after `Closed`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EventFrame {
    Event { event: EventSummary },
    Closed { reason: String },
}

/// Extracts the event from one `events.event` notification; frames without `params` are ignored.
fn notification_event(frame: &Value) -> Result<Option<EventSummary>, serde_json::Error> {
    match frame.get("params") {
        None | Some(Value::Null) => Ok(None),
        Some(params) => serde_json::from_value(params.clone()).map(Some),
    }
}

/// Acks the subscription, then forwards notifications until the connection fails.
pub async fn start(
    installation: &Installation,
    after: u64,
    channel: Channel<EventFrame>,
) -> Result<JoinHandle<()>, DesktopFailure> {
    let mut connection = client::subscribe(installation, after).await?;
    Ok(tokio::spawn(async move {
        let reason = loop {
            let frame = match connection.receive_json::<Value>().await {
                Ok(frame) => frame,
                Err(error) => break DesktopFailure::from(&DesktopClientError::from(error)).message,
            };
            match notification_event(&frame) {
                Ok(Some(event)) => {
                    if channel.send(EventFrame::Event { event }).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => break "The daemon sent an event the desktop could not read.".to_owned(),
            }
        };
        let _ = channel.send(EventFrame::Closed { reason });
    }))
}

#[cfg(test)]
mod tests {
    use lazoo_protocol::EventSummary;
    use serde_json::json;
    use uuid::Uuid;

    use super::{EventFrame, notification_event};

    fn summary() -> EventSummary {
        EventSummary {
            sequence: 7,
            id: Uuid::nil(),
            event_type: "turn.completed".to_owned(),
            payload: json!({"turnId": "t-1"}),
            created_at: "2026-10-31T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn event_frame_serializes_with_a_kind_tag() {
        let wire = serde_json::to_value(EventFrame::Event { event: summary() }).expect("frame");

        assert_eq!(
            wire,
            json!({
                "kind": "event",
                "event": {
                    "sequence": 7,
                    "id": "00000000-0000-0000-0000-000000000000",
                    "eventType": "turn.completed",
                    "payload": {"turnId": "t-1"},
                    "createdAt": "2026-10-31T00:00:00Z"
                }
            })
        );
    }

    #[test]
    fn closed_frame_serializes_its_reason() {
        let wire = serde_json::to_value(EventFrame::Closed {
            reason: "daemon offline".to_owned(),
        })
        .expect("frame");

        assert_eq!(wire, json!({"kind": "closed", "reason": "daemon offline"}));
    }

    #[test]
    fn notification_params_become_the_event() {
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "events.event",
            "params": serde_json::to_value(summary()).expect("summary")
        });

        assert_eq!(notification_event(&frame).expect("parses"), Some(summary()));
    }

    #[test]
    fn frames_without_params_are_ignored_and_bad_params_fail() {
        assert_eq!(
            notification_event(&json!({"jsonrpc": "2.0"})).expect("ignored"),
            None
        );
        assert!(notification_event(&json!({"params": {"sequence": "x"}})).is_err());
    }
}
