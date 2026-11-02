//! Terminal byte streaming over the dedicated terminal pipe.
//!
//! Every exchange is a fresh authenticated connection (the daemon serves one request per
//! connection), so a "stream" is a bounded poller that remembers the next byte offset.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use lazoo_protocol::{
    TERMINAL_INPUT_MAX_BYTES, TERMINAL_OUTPUT_MAX_BYTES, TerminalIoRequest, TerminalIoResponse,
    TerminalOutputMetadata,
};
use lazoo_runtime::Installation;
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::{sync::Notify, task::JoinHandle, time::sleep};
use uuid::Uuid;

use crate::client::{
    self, DesktopClientError, DesktopFailure, READ_DEADLINE, connect_terminal, with_deadline,
};

const READ_LIMIT: usize = TERMINAL_OUTPUT_MAX_BYTES;
const MIN_DELAY: Duration = Duration::from_millis(16);
const FOCUSED_CEILING: Duration = Duration::from_millis(200);
const UNFOCUSED_CEILING: Duration = Duration::from_millis(1000);
const TRANSPORT_RETRIES: u32 = 5;
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// One message on the terminal channel. `data` is standard base64 of the raw bytes; frame
/// boundaries are not UTF-8 or escape-sequence boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalFrame {
    Output {
        start: u64,
        end: u64,
        gap: bool,
        live: bool,
        exit_code: Option<u32>,
        data: String,
    },
    Closed {
        reason: String,
    },
}

/// What the UI last saw, so an empty read is only forwarded when lifecycle state changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Emitted {
    live: bool,
    exit_code: Option<u32>,
}

impl Emitted {
    const fn initial() -> Self {
        Self {
            live: true,
            exit_code: None,
        }
    }
}

fn frame_for(
    metadata: &TerminalOutputMetadata,
    bytes: &[u8],
    last: &mut Emitted,
) -> Option<TerminalFrame> {
    let changed = metadata.live != last.live || metadata.exit_code != last.exit_code;
    if bytes.is_empty() && !changed && !metadata.gap {
        return None;
    }
    *last = Emitted {
        live: metadata.live,
        exit_code: metadata.exit_code,
    };
    Some(TerminalFrame::Output {
        start: metadata.start,
        end: metadata.end,
        gap: metadata.gap,
        live: metadata.live,
        exit_code: metadata.exit_code,
        data: STANDARD.encode(bytes),
    })
}

/// A drained exited terminal has no more output to read; a full read may leave more behind.
fn finished(metadata: &TerminalOutputMetadata) -> bool {
    !metadata.live && metadata.length < READ_LIMIT
}

enum Outcome {
    Output {
        metadata: TerminalOutputMetadata,
        bytes: Vec<u8>,
    },
    Remote(String),
}

async fn read_once(
    installation: &Installation,
    id: Uuid,
    boot_id: Uuid,
    after: u64,
) -> Result<Outcome, DesktopClientError> {
    with_deadline(READ_DEADLINE, async {
        let mut connection = connect_terminal(installation).await?;
        connection
            .send_json(&TerminalIoRequest::Read {
                id,
                expected_boot_id: boot_id,
                after,
                limit: READ_LIMIT,
            })
            .await?;
        let response: TerminalIoResponse = connection.receive_json().await?;
        match (response.error, response.output) {
            (Some(reason), None) => Ok(Outcome::Remote(reason)),
            (None, Some(metadata)) => {
                // The byte frame follows even when `length` is zero.
                let bytes = connection.receive_bytes(READ_LIMIT).await?;
                if bytes.len() == metadata.length {
                    Ok(Outcome::Output { metadata, bytes })
                } else {
                    Err(DesktopClientError::InvalidResponse)
                }
            }
            _ => Err(DesktopClientError::InvalidResponse),
        }
    })
    .await
}

/// A running poller and the knobs the UI may turn.
pub struct StreamHandle {
    task: JoinHandle<()>,
    focused: Arc<AtomicBool>,
    wake: Arc<Notify>,
}

impl StreamHandle {
    /// Focus selects the polling ceiling; regaining focus restarts at the fastest cadence.
    pub fn set_focus(&self, focused: bool) {
        self.focused.store(focused, Ordering::Relaxed);
        if focused {
            self.wake.notify_one();
        }
    }

    pub fn abort(&self) {
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

pub fn start(
    installation: Installation,
    id: Uuid,
    boot_id: Uuid,
    after: u64,
    channel: Channel<TerminalFrame>,
) -> StreamHandle {
    let focused = Arc::new(AtomicBool::new(true));
    let wake = Arc::new(Notify::new());
    let task = tokio::spawn(poll(
        installation,
        id,
        boot_id,
        after,
        channel,
        Arc::clone(&focused),
        Arc::clone(&wake),
    ));
    StreamHandle {
        task,
        focused,
        wake,
    }
}

async fn poll(
    installation: Installation,
    id: Uuid,
    boot_id: Uuid,
    mut after: u64,
    channel: Channel<TerminalFrame>,
    focused: Arc<AtomicBool>,
    wake: Arc<Notify>,
) {
    let close = |reason: String| {
        let _ = channel.send(TerminalFrame::Closed { reason });
    };
    let mut emitted = Emitted::initial();
    let mut delay = MIN_DELAY;
    let mut failures = 0_u32;
    loop {
        match read_once(&installation, id, boot_id, after).await {
            Ok(Outcome::Remote(reason)) => return close(reason),
            Ok(Outcome::Output { metadata, bytes }) => {
                failures = 0;
                after = metadata.end;
                if let Some(frame) = frame_for(&metadata, &bytes, &mut emitted)
                    && channel.send(frame).is_err()
                {
                    return;
                }
                if finished(&metadata) {
                    return close("exited".to_owned());
                }
                if !bytes.is_empty() {
                    delay = MIN_DELAY;
                    continue;
                }
            }
            Err(error) => {
                failures += 1;
                if failures > TRANSPORT_RETRIES {
                    return close(DesktopFailure::from(&error).message);
                }
                sleep(RETRY_BACKOFF).await;
                continue;
            }
        }
        let ceiling = if focused.load(Ordering::Relaxed) {
            FOCUSED_CEILING
        } else {
            UNFOCUSED_CEILING
        };
        tokio::select! {
            () = sleep(delay.min(ceiling)) => delay = (delay * 2).min(ceiling),
            () = wake.notified() => delay = MIN_DELAY,
        }
    }
}

fn check_input(data: &str) -> Result<(), DesktopClientError> {
    if data.len() > TERMINAL_INPUT_MAX_BYTES {
        Err(DesktopClientError::InputTooLarge)
    } else {
        Ok(())
    }
}

/// Sends one input write. It is never retried: a lost acknowledgement does not prove the
/// bytes did not reach the process.
pub async fn write(
    installation: &Installation,
    id: Uuid,
    boot_id: Uuid,
    data: &str,
) -> Result<(), DesktopFailure> {
    write_inner(installation, id, boot_id, data)
        .await
        .map_err(|error| DesktopFailure::from(&error))
}

async fn write_inner(
    installation: &Installation,
    id: Uuid,
    boot_id: Uuid,
    data: &str,
) -> Result<(), DesktopClientError> {
    check_input(data)?;
    with_deadline(client::READ_DEADLINE, async {
        let mut connection = connect_terminal(installation).await?;
        connection
            .send_json(&TerminalIoRequest::Write {
                id,
                expected_boot_id: boot_id,
            })
            .await?;
        connection.send_bytes(data.as_bytes()).await?;
        let response: TerminalIoResponse = connection.receive_json().await?;
        match response.error {
            Some(reason) => Err(DesktopClientError::TerminalWrite(reason)),
            None => Ok(()),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use lazoo_protocol::TerminalOutputMetadata;
    use serde_json::json;

    use super::{Emitted, TerminalFrame, check_input, finished, frame_for};

    fn metadata(length: usize, live: bool, exit_code: Option<u32>) -> TerminalOutputMetadata {
        TerminalOutputMetadata {
            start: 10,
            end: 10 + length as u64,
            length,
            gap: false,
            live,
            exit_code,
        }
    }

    #[test]
    fn output_frame_serializes_camel_case_with_a_kind_tag() {
        let wire = serde_json::to_value(TerminalFrame::Output {
            start: 1,
            end: 3,
            gap: false,
            live: true,
            exit_code: None,
            data: "aGk=".to_owned(),
        })
        .expect("frame");

        assert_eq!(
            wire,
            json!({
                "kind": "output",
                "start": 1,
                "end": 3,
                "gap": false,
                "live": true,
                "exitCode": null,
                "data": "aGk="
            })
        );
    }

    #[test]
    fn closed_frame_serializes_its_reason() {
        let wire = serde_json::to_value(TerminalFrame::Closed {
            reason: "exited".to_owned(),
        })
        .expect("frame");

        assert_eq!(wire, json!({"kind": "closed", "reason": "exited"}));
    }

    #[test]
    fn chunk_survives_a_base64_round_trip_including_split_utf8() {
        let chunk = [0x1b, b'[', b'3', b'1', b'm', 0xe2, 0x82];
        let mut emitted = Emitted::initial();

        let frame = frame_for(&metadata(chunk.len(), true, None), &chunk, &mut emitted)
            .expect("bytes always emit");
        let TerminalFrame::Output { data, .. } = frame else {
            panic!("expected an output frame");
        };

        assert_eq!(STANDARD.decode(data).expect("valid base64"), chunk);
    }

    #[test]
    fn empty_unchanged_reads_are_not_forwarded() {
        let mut emitted = Emitted::initial();

        assert!(frame_for(&metadata(0, true, None), &[], &mut emitted).is_none());
    }

    #[test]
    fn lifecycle_change_emits_once_even_without_bytes() {
        let mut emitted = Emitted::initial();

        let frame = frame_for(&metadata(0, false, Some(3)), &[], &mut emitted)
            .expect("exit state change emits");
        assert!(matches!(
            frame,
            TerminalFrame::Output {
                live: false,
                exit_code: Some(3),
                ..
            }
        ));
        assert!(frame_for(&metadata(0, false, Some(3)), &[], &mut emitted).is_none());
    }

    #[test]
    fn gap_is_always_reported() {
        let mut emitted = Emitted::initial();
        let mut gap = metadata(0, true, None);
        gap.gap = true;

        assert!(frame_for(&gap, &[], &mut emitted).is_some());
    }

    #[test]
    fn exited_terminal_is_finished_only_when_a_read_is_not_full() {
        assert!(finished(&metadata(0, false, Some(0))));
        assert!(!finished(&metadata(0, true, None)));
        assert!(!finished(&metadata(super::READ_LIMIT, false, Some(0))));
    }

    #[test]
    fn oversized_input_is_refused_before_any_connection() {
        let largest = "a".repeat(16 * 1024);
        let too_large = "a".repeat(16 * 1024 + 1);

        assert!(check_input(&largest).is_ok());
        assert!(check_input(&too_large).is_err());
    }
}
