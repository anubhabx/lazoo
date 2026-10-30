mod client;
mod events;
mod terminal;

use std::{
    collections::HashMap,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use lazoo_runtime::Installation;
use serde_json::Value;
use tauri::{State, ipc::Channel};
use tokio::{sync::OnceCell, task::JoinHandle};
use uuid::Uuid;

use client::DesktopFailure;
use events::EventFrame;
use terminal::{StreamHandle, TerminalFrame};

/// Process-wide state: the lazily resolved installation and the live background tasks.
#[derive(Default)]
struct DesktopState {
    installation: OnceCell<Installation>,
    next_id: AtomicU64,
    subscriptions: Mutex<HashMap<u64, JoinHandle<()>>>,
    streams: Mutex<HashMap<u64, StreamHandle>>,
}

impl DesktopState {
    async fn installation(&self) -> Result<Installation, DesktopFailure> {
        client::installation(&self.installation).await
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn subscriptions(&self) -> MutexGuard<'_, HashMap<u64, JoinHandle<()>>> {
        self.subscriptions.lock().expect("subscription registry")
    }

    fn streams(&self) -> MutexGuard<'_, HashMap<u64, StreamHandle>> {
        self.streams.lock().expect("stream registry")
    }

    fn abort_all(&self) {
        for (_, task) in self.subscriptions().drain() {
            task.abort();
        }
        for (_, stream) in self.streams().drain() {
            stream.abort();
        }
    }
}

/// Contract rule: only allowlisted daemon methods are reachable, with a 5 s deadline for
/// reads and 600 s for mutations; everything else is refused before any connection opens.
#[tauri::command]
async fn daemon_call(
    state: State<'_, DesktopState>,
    method: String,
    params: Value,
) -> Result<Value, DesktopFailure> {
    if client::classify(&method).is_none() {
        return Err(DesktopFailure::new(
            "LZ-DESKTOP-METHOD-REFUSED",
            "The desktop does not permit that daemon method.",
        ));
    }
    let installation = state.installation().await?;
    client::call(&installation, &method, params).await
}

/// Contract rule: subscribe from the last safely consumed cursor; the ack must succeed
/// before any frame is forwarded, and a lost connection is reported as a `closed` frame.
#[tauri::command]
async fn events_subscribe(
    state: State<'_, DesktopState>,
    after: u64,
    on_event: Channel<EventFrame>,
) -> Result<u64, DesktopFailure> {
    let installation = state.installation().await?;
    let task = events::start(&installation, after, on_event).await?;
    let id = state.next_id();
    let mut subscriptions = state.subscriptions();
    subscriptions.retain(|_, task| !task.is_finished());
    subscriptions.insert(id, task);
    Ok(id)
}

/// Contract rule: the UI owns subscription lifetime; unsubscribing drops the connection.
#[tauri::command]
async fn events_unsubscribe(state: State<'_, DesktopState>, id: u64) -> Result<(), DesktopFailure> {
    if let Some(task) = state.subscriptions().remove(&id) {
        task.abort();
    }
    Ok(())
}

/// Contract rule: terminal bytes use the dedicated pipe, one read per connection, tracked
/// by the caller's terminal ID, boot ID and byte offset; the poller backs off when empty.
#[tauri::command]
async fn terminal_stream_start(
    state: State<'_, DesktopState>,
    terminal_id: Uuid,
    boot_id: Uuid,
    after: u64,
    on_chunk: Channel<TerminalFrame>,
) -> Result<u64, DesktopFailure> {
    let installation = state.installation().await?;
    let stream = terminal::start(installation, terminal_id, boot_id, after, on_chunk);
    let id = state.next_id();
    let mut streams = state.streams();
    streams.retain(|_, stream| !stream.is_finished());
    streams.insert(id, stream);
    Ok(id)
}

/// Contract rule: stopping a stream only stops reading; it never stops the terminal.
#[tauri::command]
async fn terminal_stream_stop(
    state: State<'_, DesktopState>,
    id: u64,
) -> Result<(), DesktopFailure> {
    if let Some(stream) = state.streams().remove(&id) {
        stream.abort();
    }
    Ok(())
}

/// Contract rule: an unfocused terminal polls at most once per second; refocusing resumes
/// the fastest cadence.
#[tauri::command]
async fn terminal_stream_focus(
    state: State<'_, DesktopState>,
    id: u64,
    focused: bool,
) -> Result<(), DesktopFailure> {
    if let Some(stream) = state.streams().get(&id) {
        stream.set_focus(focused);
    }
    Ok(())
}

/// Contract rule: input is at most 16 KiB per write, bound to the boot ID, and never
/// retried automatically because a lost acknowledgement may still have reached the process.
#[tauri::command]
async fn terminal_write(
    state: State<'_, DesktopState>,
    terminal_id: Uuid,
    boot_id: Uuid,
    data: String,
) -> Result<(), DesktopFailure> {
    let installation = state.installation().await?;
    terminal::write(&installation, terminal_id, boot_id, &data).await
}

pub fn run() {
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            daemon_call,
            events_subscribe,
            events_unsubscribe,
            terminal_stream_start,
            terminal_stream_stop,
            terminal_stream_focus,
            terminal_write,
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                tauri::Manager::state::<DesktopState>(window).abort_all();
            }
        })
        .run(tauri::generate_context!())
        .expect("Lazoo desktop runtime failed");
}
