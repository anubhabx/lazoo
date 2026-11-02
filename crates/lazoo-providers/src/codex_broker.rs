#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use serde_json::json;
    use tokio::sync::{Mutex, oneshot, watch};

    use super::{
        BrokerMessageClass, BrokerQueues, QueueRoute, classify_message, next_controller_message,
        write_with_deadline,
    };

    use crate::ProviderError;

    #[test]
    fn server_requests_stay_with_the_controller() {
        assert_eq!(
            classify_message(&json!({"id": "approval-1", "method": "item/tool/call"})),
            BrokerMessageClass::ControllerOnly
        );
    }

    #[test]
    fn notifications_are_visible_to_a_read_only_viewer() {
        assert_eq!(
            classify_message(&json!({"method": "item/agentMessage/delta", "params": {}})),
            BrokerMessageClass::Notification
        );
    }

    #[test]
    fn responses_are_correlated_without_becoming_view_notifications() {
        assert_eq!(
            classify_message(&json!({"id": "request-1", "result": {}})),
            BrokerMessageClass::Response
        );
    }

    #[tokio::test]
    async fn finite_controller_burst_waits_for_a_slot_without_losing_events() {
        let (queues, mut controller) = BrokerQueues::fixture();
        for index in 0..super::MAX_CONTROLLER_MESSAGES {
            assert_eq!(
                queues
                    .route(json!({"method":"turn/started", "params":{"index":index}}))
                    .await,
                QueueRoute::Delivered
            );
        }
        let burst = queues.route(json!({"method":"turn/completed", "params":{}}));
        tokio::pin!(burst);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut burst)
                .await
                .is_err()
        );
        let first = controller.recv().await.unwrap();
        queues.release_controller_bytes(first.reserved_bytes);
        assert_eq!(burst.await, QueueRoute::Delivered);
        for _ in 0..super::MAX_CONTROLLER_MESSAGES - 1 {
            let message = controller.recv().await.unwrap();
            queues.release_controller_bytes(message.reserved_bytes);
            assert_eq!(message.payload.unwrap()["method"], "turn/started");
        }
        let last = controller.recv().await.unwrap();
        queues.release_controller_bytes(last.reserved_bytes);
        assert_eq!(last.payload.unwrap()["method"], "turn/completed");
        assert_eq!(
            queues
                .controller_bytes
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn stalled_and_closed_controllers_remain_bounded() {
        let (queues, controller) = BrokerQueues::fixture();
        for _ in 0..super::MAX_CONTROLLER_MESSAGES {
            assert_eq!(
                queues
                    .route(json!({"method":"turn/started", "params":{}}))
                    .await,
                QueueRoute::Delivered
            );
        }
        let before = queues
            .controller_bytes
            .load(std::sync::atomic::Ordering::Acquire);
        assert_eq!(
            queues
                .route(json!({"method":"turn/completed", "params":{}}))
                .await,
            QueueRoute::ControllerOverflow
        );
        assert_eq!(
            queues
                .controller_bytes
                .load(std::sync::atomic::Ordering::Acquire),
            before
        );
        drop(controller);
        assert_eq!(
            queues
                .route(json!({"method":"turn/completed", "params":{}}))
                .await,
            QueueRoute::ControllerClosed
        );
    }

    #[tokio::test]
    async fn interleaves_correlated_view_read_notification_and_server_request() {
        let (queues, mut controller) = BrokerQueues::fixture();
        let (reply_tx, reply_rx) = oneshot::channel();
        queues
            .insert_pending("view-1".to_owned(), reply_tx)
            .await
            .unwrap();
        let mut view = queues.subscribe_notifications();

        assert_eq!(
            queues
                .route(json!({"id": "view-1", "result": {"items": []}}))
                .await,
            QueueRoute::Delivered
        );
        assert_eq!(reply_rx.await.unwrap().unwrap(), json!({"items": []}));

        assert_eq!(
            queues
                .route(json!({"method": "item/agentMessage/delta", "params": {"delta": "hi"}}))
                .await,
            QueueRoute::Delivered
        );
        assert_eq!(
            queues
                .route(json!({"id": "approval-1", "method": "item/tool/call", "params": {}}))
                .await,
            QueueRoute::Delivered
        );

        assert_eq!(
            view.recv().await.unwrap()["method"],
            "item/agentMessage/delta"
        );
        assert_eq!(
            controller.recv().await.unwrap().payload.unwrap()["method"],
            "item/agentMessage/delta"
        );
        assert_eq!(
            controller.recv().await.unwrap().payload.unwrap()["id"],
            "approval-1"
        );
    }

    #[tokio::test]
    async fn slow_viewer_lags_without_blocking_controller() {
        let (queues, mut controller) = BrokerQueues::fixture();
        let mut slow_view = queues.subscribe_notifications();
        for index in 0..=super::MAX_VIEW_NOTIFICATIONS {
            assert_eq!(
                queues
                    .route(json!({"method": "turn/started", "params": {"index": index}}))
                    .await,
                QueueRoute::Delivered
            );
        }
        assert!(
            slow_view.recv().await.is_err(),
            "viewer must resync after lag"
        );
        assert_eq!(
            controller.recv().await.unwrap().payload.unwrap()["method"],
            "turn/started"
        );
    }

    #[tokio::test]
    async fn controller_byte_overflow_fails_closed_instead_of_queueing_unboundedly() {
        let (queues, _controller) = BrokerQueues::fixture();
        let oversized = json!({
            "id": "server-request",
            "method": "item/tool/call",
            "params": {"arguments": "x".repeat(super::MAX_CONTROLLER_QUEUE_BYTES)}
        });
        assert_eq!(
            queues.route(oversized).await,
            QueueRoute::ControllerOverflow
        );
    }

    #[tokio::test]
    async fn late_or_unknown_responses_never_fill_the_controller_queue() {
        let (queues, mut controller) = BrokerQueues::fixture();
        for index in 0..(super::MAX_CONTROLLER_MESSAGES * 2) {
            assert_eq!(
                queues
                    .route(json!({"id": format!("expired-{index}"), "result": {}}))
                    .await,
                QueueRoute::DroppedResponse
            );
        }
        assert!(controller.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancelled_read_releases_its_pending_slot() {
        let (queues, _controller) = BrokerQueues::fixture();
        let (reply_tx, reply_rx) = oneshot::channel();
        queues
            .insert_pending("cancelled".to_owned(), reply_tx)
            .await
            .unwrap();
        let cleanup = super::PendingCleanup {
            pending: Arc::downgrade(&queues.pending),
            id: "cancelled".to_owned(),
        };
        drop(reply_rx);
        drop(cleanup);
        tokio::task::yield_now().await;
        assert_eq!(queues.pending_len().await, 0);
    }

    #[tokio::test]
    async fn malformed_response_fails_closed_route() {
        let (queues, _controller) = BrokerQueues::fixture();
        assert_eq!(
            queues.route(json!({"result": {}})).await,
            QueueRoute::InvalidResponse
        );
    }

    #[tokio::test]
    async fn queued_completion_wins_over_an_already_closed_host() {
        let (queues, mut controller) = BrokerQueues::fixture();
        let (closed_tx, mut closed) = watch::channel(false);
        assert_eq!(
            queues
                .route(json!({"method": "turn/completed", "params": {}}))
                .await,
            QueueRoute::Delivered
        );
        let _ = closed_tx.send(true);
        assert_eq!(
            next_controller_message(
                &mut controller,
                &queues,
                &mut closed,
                Duration::from_millis(1),
                "test"
            )
            .await
            .unwrap()["method"],
            "turn/completed"
        );
    }

    #[tokio::test]
    async fn empty_controller_queue_wakes_on_host_close() {
        let (queues, mut controller) = BrokerQueues::fixture();
        let (closed_tx, mut closed) = watch::channel(false);
        let _ = closed_tx.send(true);
        assert!(matches!(
            next_controller_message(
                &mut controller,
                &queues,
                &mut closed,
                Duration::from_secs(1),
                "test"
            )
            .await,
            Err(ProviderError::ProcessExited { .. })
        ));
    }

    #[tokio::test]
    async fn blocked_writer_obeys_the_broker_write_deadline() {
        let (writer, _reader) = tokio::io::duplex(1);
        let writer = Mutex::new(writer);
        let held = writer.lock().await;
        assert!(
            write_with_deadline(&writer, b"{}\n", Duration::from_millis(1))
                .await
                .is_err()
        );
        drop(held);
    }
}
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use lazoo_runtime::{JobChild, JobCommand, JobObject};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdin, ChildStdout},
    sync::{Mutex, broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    CommandSpec, ProviderError,
    codex_frame::{BrokerLine, MAX_LINE_BYTES, read_broker_line},
    json_rpc::response_id,
};

const MAX_PENDING_REQUESTS: usize = 128;
const MAX_CONTROLLER_MESSAGES: usize = 32;
const MAX_CONTROLLER_QUEUE_BYTES: usize = 8 * 1024 * 1024;
/// Broadcast retains a single shared message per slot, so 16 accepted 512 KiB frames stay <= 8 MiB.
const MAX_VIEW_NOTIFICATIONS: usize = 16;
const WRITE_DEADLINE: Duration = Duration::from_secs(2);
const CONTROLLER_DRAIN_DEADLINE: Duration = Duration::from_secs(2);

type PendingResult = Result<Value, String>;

#[derive(Debug)]
struct ControllerMessage {
    payload: Result<Value, String>,
    reserved_bytes: usize,
}

#[derive(Debug)]
struct BrokerQueues {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<PendingResult>>>>,
    controller_tx: mpsc::Sender<ControllerMessage>,
    controller_bytes: AtomicUsize,
    notifications: broadcast::Sender<Value>,
}

struct PendingCleanup {
    pending: std::sync::Weak<Mutex<HashMap<String, oneshot::Sender<PendingResult>>>>,
    id: String,
}

impl Drop for PendingCleanup {
    fn drop(&mut self) {
        let Some(pending) = self.pending.upgrade() else {
            return;
        };
        let id = self.id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                pending.lock().await.remove(&id);
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueRoute {
    Delivered,
    DroppedResponse,
    ControllerOverflow,
    ControllerClosed,
    InvalidResponse,
}

/// The class determines which consumers may observe an app-server message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrokerMessageClass {
    Response,
    ControllerOnly,
    Notification,
}

#[must_use]
pub(crate) fn classify_message(message: &Value) -> BrokerMessageClass {
    if message.get("method").is_some() {
        if message.get("id").is_some() {
            BrokerMessageClass::ControllerOnly
        } else {
            BrokerMessageClass::Notification
        }
    } else {
        BrokerMessageClass::Response
    }
}

impl BrokerQueues {
    fn new() -> (Self, mpsc::Receiver<ControllerMessage>) {
        let (controller_tx, controller_rx) = mpsc::channel(MAX_CONTROLLER_MESSAGES);
        let (notifications, _) = broadcast::channel(MAX_VIEW_NOTIFICATIONS);
        (
            Self {
                pending: Arc::new(Mutex::new(HashMap::new())),
                controller_tx,
                controller_bytes: AtomicUsize::new(0),
                notifications,
            },
            controller_rx,
        )
    }

    #[cfg(test)]
    fn fixture() -> (Self, mpsc::Receiver<ControllerMessage>) {
        Self::new()
    }

    async fn insert_pending(
        &self,
        id: String,
        sender: oneshot::Sender<PendingResult>,
    ) -> Result<(), ProviderError> {
        let mut pending = self.pending.lock().await;
        if pending.len() >= MAX_PENDING_REQUESTS {
            return Err(ProviderError::Protocol(
                "Codex broker pending request limit exceeded".to_owned(),
            ));
        }
        pending.insert(id, sender);
        Ok(())
    }

    async fn remove_pending(&self, id: &str) {
        self.pending.lock().await.remove(id);
    }

    #[cfg(test)]
    async fn pending_len(&self) -> usize {
        self.pending.lock().await.len()
    }

    fn subscribe_notifications(&self) -> broadcast::Receiver<Value> {
        self.notifications.subscribe()
    }

    async fn route(&self, message: Value) -> QueueRoute {
        match classify_message(&message) {
            BrokerMessageClass::Response => {
                let Some(id) = response_id(&message) else {
                    return QueueRoute::InvalidResponse;
                };
                let response = if let Some(result) = message.get("result") {
                    Ok(result.clone())
                } else if let Some(error) = message.get("error") {
                    Err(format!("Codex request failed: {error}"))
                } else {
                    Err("Codex response contained neither result nor error".to_owned())
                };
                if let Some(waiter) = self.pending.lock().await.remove(&id) {
                    let _ = waiter.send(response);
                    QueueRoute::Delivered
                } else {
                    // Late correlated reads and replies to uncorrelated controller writes must
                    // not occupy the canonical driver's bounded queue.
                    QueueRoute::DroppedResponse
                }
            }
            BrokerMessageClass::ControllerOnly => self.enqueue_controller(Ok(message)).await,
            BrokerMessageClass::Notification => {
                let _ = self.notifications.send(message.clone());
                // Viewer/sidebar notifications unknown to the turn driver cannot fill its
                // queue while it is idle or awaiting a tool/approval decision.
                if crate::codex::normalize_codex_message(&message)
                    .is_ok_and(|event| event.is_none())
                {
                    return QueueRoute::Delivered;
                }
                self.enqueue_controller(Ok(message)).await
            }
        }
    }

    async fn enqueue_controller(&self, payload: Result<Value, String>) -> QueueRoute {
        let reserved_bytes = payload.as_ref().map_or(0, conservative_value_bytes);
        if reserved_bytes > MAX_CONTROLLER_QUEUE_BYTES
            || self
                .controller_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(reserved_bytes)
                        .filter(|total| *total <= MAX_CONTROLLER_QUEUE_BYTES)
                })
                .is_err()
        {
            return QueueRoute::ControllerOverflow;
        }
        let message = ControllerMessage {
            payload,
            reserved_bytes,
        };
        // A finite stdout burst can outrun the durable turn observer (especially while a
        // view bootstraps). A full channel is backpressure, not proof that the child failed.
        // Keep the byte reservation and one pending frame bounded, with a deadline for a
        // controller which has actually stopped consuming. Never drop meaningful events.
        match timeout(CONTROLLER_DRAIN_DEADLINE, self.controller_tx.send(message)).await {
            Ok(Ok(())) => QueueRoute::Delivered,
            result => {
                self.controller_bytes
                    .fetch_sub(reserved_bytes, Ordering::AcqRel);
                if result.is_err() {
                    QueueRoute::ControllerOverflow
                } else {
                    QueueRoute::ControllerClosed
                }
            }
        }
    }

    fn release_controller_bytes(&self, bytes: usize) {
        self.controller_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }
}

fn conservative_value_bytes(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(MAX_CONTROLLER_QUEUE_BYTES, |bytes| {
        bytes.len().saturating_mul(4)
    })
}

#[derive(Debug)]
struct BrokerInner {
    diagnostic: Arc<ExitDiagnostic>,
    view_request: Arc<tokio::sync::Semaphore>,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Mutex<JobChild>,
    queues: BrokerQueues,
    next_id: AtomicU64,
    closed: AtomicBool,
    closed_tx: watch::Sender<bool>,
}

/// Only fixed reason labels and numeric process facts enter diagnostics, never RPC payloads.
#[derive(Debug)]
struct ExitDiagnostic {
    host_pid: Option<u32>,
    job_id: uuid::Uuid,
    reason: std::sync::Mutex<Option<&'static str>>,
    status: std::sync::Mutex<Option<std::process::ExitStatus>>,
}

impl ExitDiagnostic {
    fn record(&self, reason: &'static str) {
        let mut first = self.reason.lock().unwrap();
        let status = *self.status.lock().unwrap();
        let first_reason = *first.get_or_insert(if status.is_some() {
            "native-process-exit"
        } else {
            reason
        });
        tracing::warn!(host_pid = self.host_pid, job_id = %self.job_id, reason, first_reason,
            exit_code = ?status.and_then(|status| status.code()),
            status_observed = status.is_some(), "Codex child lifecycle");
    }

    fn exited(&self, status: std::io::Result<std::process::ExitStatus>) {
        match status {
            Ok(status) => {
                *self.status.lock().unwrap() = Some(status);
                // EOF is a transport fact, not evidence that Lazoo killed the child.
                let reason = self.reason.lock().unwrap().unwrap_or("native-process-exit");
                tracing::warn!(host_pid = self.host_pid, job_id = %self.job_id, reason,
                    exit_code = ?status.code(), success = status.success(),
                    "Codex child exit observed");
            }
            Err(error) => tracing::warn!(host_pid = self.host_pid, job_id = %self.job_id,
                reason = "exit-status-query-failed", os_error = ?error.raw_os_error(),
                "Codex child exit status unavailable"),
        }
    }
}

/// Controller-owned app-server transport. It has exactly one stdout reader.
#[derive(Debug)]
pub(crate) struct CodexBroker {
    inner: Arc<BrokerInner>,
    controller_rx: Mutex<mpsc::Receiver<ControllerMessage>>,
    reader_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    job: Arc<JobObject>,
    native_host_pid: Option<u32>,
}

/// Write authority remains exclusively with the canonical Codex turn driver.
#[derive(Clone, Debug)]
pub(crate) struct CodexBrokerControl {
    inner: Arc<BrokerInner>,
}

/// Cloneable, read-only bridge for the native TUI view.
#[derive(Clone)]
pub(crate) struct CodexBrokerReadHandle {
    inner: Arc<BrokerInner>,
    session_id: Arc<str>,
    initialize: Arc<Value>,
    new_empty_thread: Arc<AtomicBool>,
}

impl fmt::Debug for CodexBrokerReadHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexBrokerReadHandle")
            .field("session_id", &self.session_id)
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct CodexNotificationSubscription {
    receiver: broadcast::Receiver<Value>,
}

impl CodexBroker {
    pub(crate) fn spawn(spec: &CommandSpec) -> Result<Self, ProviderError> {
        let mut command = JobCommand::new(&spec.executable, &spec.cwd)
            .args(spec.args.clone())
            .envs(spec.env.clone());
        for name in &spec.env_remove {
            command = command.env_remove(name);
        }
        let mut child = JobChild::spawn(&command)?;
        let stdin = child
            .take_stdin()
            .ok_or_else(|| ProviderError::Protocol("Codex stdin was not piped".to_owned()))?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| ProviderError::Protocol("Codex stdout was not piped".to_owned()))?;
        let stderr = child
            .take_stderr()
            .ok_or_else(|| ProviderError::Protocol("Codex stderr was not piped".to_owned()))?;
        let native_host_pid = child.id();
        let job = child.job();
        let diagnostic = Arc::new(ExitDiagnostic {
            host_pid: native_host_pid,
            job_id: job.id(),
            reason: std::sync::Mutex::new(None),
            status: std::sync::Mutex::new(None),
        });
        let exit_diagnostic = Arc::clone(&diagnostic);
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        child.observe_exit(move |status| {
            tracing::dispatcher::with_default(&dispatch, || exit_diagnostic.exited(status));
        })?;
        tracing::debug!(host_pid = native_host_pid, job_id = %job.id(), "Codex child started");
        let (queues, controller_rx) = BrokerQueues::new();
        let (closed_tx, _) = watch::channel(false);
        let inner = Arc::new(BrokerInner {
            diagnostic,
            view_request: Arc::new(tokio::sync::Semaphore::new(1)),
            stdin: Arc::new(Mutex::new(stdin)),
            child: Mutex::new(child),
            queues,
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            closed_tx,
        });
        let reader_task = tokio::spawn(read_stdout(Arc::clone(&inner), stdout));
        let stderr_task = tokio::spawn(drain_stderr(stderr));
        Ok(Self {
            inner,
            controller_rx: Mutex::new(controller_rx),
            reader_task,
            stderr_task,
            job,
            native_host_pid,
        })
    }

    pub(crate) fn control(&self) -> CodexBrokerControl {
        CodexBrokerControl {
            inner: Arc::clone(&self.inner),
        }
    }

    pub(crate) fn native_host_pid(&self) -> Option<u32> {
        self.native_host_pid
    }

    pub(crate) fn job(&self) -> Arc<JobObject> {
        Arc::clone(&self.job)
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        request(&self.inner, method, params, wait, phase).await
    }

    pub(crate) async fn next_message(
        &self,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        let mut receiver = self.controller_rx.lock().await;
        let mut closed = self.inner.closed_tx.subscribe();
        let result =
            next_controller_message(&mut receiver, &self.inner.queues, &mut closed, wait, phase)
                .await;
        if matches!(result, Err(ProviderError::ProcessExited { .. })) {
            let mut child = self.inner.child.lock().await;
            let code = match timeout(Duration::from_millis(250), child.wait()).await {
                Ok(Ok(status)) => status.code(),
                _ => None,
            };
            return Err(ProviderError::ProcessExited { code });
        }
        result
    }

    pub(crate) fn read_handle(
        &self,
        session_id: impl Into<Arc<str>>,
        initialize: Value,
        new_empty_thread: Arc<AtomicBool>,
    ) -> CodexBrokerReadHandle {
        CodexBrokerReadHandle {
            inner: Arc::clone(&self.inner),
            session_id: session_id.into(),
            initialize: Arc::new(initialize),
            new_empty_thread,
        }
    }

    pub(crate) async fn stop(&self, wait: Duration) -> Result<(), ProviderError> {
        self.inner.close("controller-stop").await;
        let mut child = self.inner.child.lock().await;
        if timeout(wait, child.wait()).await.is_err() {
            self.inner
                .diagnostic
                .record("stop-deadline-job-termination");
            child.terminate_tree(1)?;
            let _ = timeout(wait, child.wait()).await;
        }
        self.reader_task.abort();
        self.stderr_task.abort();
        Ok(())
    }
}

async fn next_controller_message(
    receiver: &mut mpsc::Receiver<ControllerMessage>,
    queues: &BrokerQueues,
    closed: &mut watch::Receiver<bool>,
    wait: Duration,
    phase: &'static str,
) -> Result<Value, ProviderError> {
    if let Ok(message) = receiver.try_recv() {
        queues.release_controller_bytes(message.reserved_bytes);
        return message.payload.map_err(ProviderError::Protocol);
    }
    if *closed.borrow() {
        return Err(ProviderError::ProcessExited { code: None });
    }
    timeout(wait, async {
        tokio::select! {
            biased;
            message = receiver.recv() => match message {
                Some(message) => {
                    queues.release_controller_bytes(message.reserved_bytes);
                    message.payload.map_err(ProviderError::Protocol)
                }
                None => Err(ProviderError::Protocol(
                    "Codex controller receiver closed unexpectedly".to_owned(),
                )),
            },
            changed = closed.changed() => {
                match changed {
                    Ok(()) if *closed.borrow() => Err(ProviderError::ProcessExited { code: None }),
                    Ok(()) => Err(ProviderError::Protocol(
                        "Codex broker close watch returned an unexpected state".to_owned(),
                    )),
                    Err(_) => Err(ProviderError::ProcessExited { code: None }),
                }
            }
        }
    })
    .await
    .map_err(|_| ProviderError::Timeout { phase })?
}

impl Drop for CodexBroker {
    fn drop(&mut self) {
        self.inner.diagnostic.record("controller-drop");
        self.reader_task.abort();
        self.stderr_task.abort();
        self.inner.closed.store(true, Ordering::Release);
        self.inner.closed_tx.send_replace(true);
        // Never let a concurrently held child mutex suppress process-tree cleanup.
        let _ = self.job.terminate(1);
    }
}

impl CodexBrokerControl {
    pub(crate) async fn send_uncorrelated_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(), ProviderError> {
        let _ = self.send_request(method, params).await?;
        Ok(())
    }

    pub(crate) async fn notify(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), ProviderError> {
        let message = params.map_or_else(
            || json!({"method": method}),
            |params| json!({"method": method, "params": params}),
        );
        self.inner.write(message).await
    }

    pub(crate) async fn respond(
        &self,
        id: Value,
        result: Result<Value, (i64, &'static str)>,
    ) -> Result<(), ProviderError> {
        let message = match result {
            Ok(result) => json!({"id": id, "result": result}),
            Err((code, message)) => json!({"id": id, "error": {"code": code, "message": message}}),
        };
        self.inner.write(message).await
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<String, ProviderError> {
        let id = self
            .inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        self.inner
            .write(json!({"id": id, "method": method, "params": params}))
            .await?;
        Ok(id)
    }
}

impl CodexBrokerReadHandle {
    pub(crate) fn native_host_pid(&self) -> Option<u32> {
        self.inner.diagnostic.host_pid
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        request_view(&self.inner, method, params, wait, phase).await
    }

    #[must_use]
    pub(crate) fn subscribe_notifications(&self) -> CodexNotificationSubscription {
        CodexNotificationSubscription {
            receiver: self.inner.queues.subscribe_notifications(),
        }
    }

    #[must_use]
    pub(crate) fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// A live view can await this to close its loopback listener when the host dies.
    #[must_use]
    pub(crate) fn closed_subscription(&self) -> watch::Receiver<bool> {
        self.inner.closed_tx.subscribe()
    }

    #[must_use]
    pub(crate) fn cached_initialize(&self) -> Value {
        (*self.initialize).clone()
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Only a just-started broker session can safely synthesize an empty history page.
    #[must_use]
    pub(crate) fn is_new_empty_thread(&self) -> bool {
        self.new_empty_thread.load(Ordering::Acquire)
    }
}

impl CodexNotificationSubscription {
    pub(crate) async fn next(
        &mut self,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        match timeout(wait, self.receiver.recv()).await {
            Err(_) => Err(ProviderError::Timeout { phase }),
            Ok(Ok(message)) => Ok(message),
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => Err(ProviderError::Protocol(
                "Codex viewer notification stream requires resync".to_owned(),
            )),
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                Err(ProviderError::ProcessExited { code: None })
            }
        }
    }
}

impl BrokerInner {
    async fn write(&self, message: Value) -> Result<(), ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessExited { code: None });
        }
        let mut bytes = serde_json::to_vec(&message)?;
        if bytes.len() > MAX_LINE_BYTES {
            return Err(ProviderError::Protocol(
                "Codex broker request exceeded the line limit".to_owned(),
            ));
        }
        bytes.push(b'\n');
        if write_with_deadline(&self.stdin, &bytes, WRITE_DEADLINE)
            .await
            .is_ok()
        {
            return Ok(());
        }
        self.fail_closed("controller-write-failed-or-timed-out")
            .await;
        Err(ProviderError::Protocol(
            "Codex broker stdin write exceeded its deadline or failed; transport closed".to_owned(),
        ))
    }

    async fn close(&self, reason: &'static str) {
        self.diagnostic.record(reason);
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.closed_tx.send_replace(true);
        let mut pending = self.queues.pending.lock().await;
        for (_, waiter) in pending.drain() {
            let _ = waiter.send(Err("Codex broker closed".to_owned()));
        }
        drop(pending);
        let stdin = Arc::clone(&self.stdin);
        tokio::spawn(async move {
            let _ = timeout(WRITE_DEADLINE, async {
                let mut stdin = stdin.lock().await;
                stdin.shutdown().await
            })
            .await;
        });
    }

    async fn fail_closed(&self, reason: &'static str) {
        self.close(reason).await;
        let child = self.child.lock().await;
        let _ = child.terminate_tree(1);
    }
}

async fn write_with_deadline<W: AsyncWrite + Unpin>(
    writer: &Mutex<W>,
    bytes: &[u8],
    deadline: Duration,
) -> Result<(), ()> {
    let mut writer = timeout(deadline, writer.lock()).await.map_err(|_| ())?;
    timeout(deadline, async {
        writer.write_all(bytes).await.map_err(|_| ())?;
        writer.flush().await.map_err(|_| ())
    })
    .await
    .map_err(|_| ())?
}

async fn request(
    inner: &Arc<BrokerInner>,
    method: &str,
    params: Value,
    wait: Duration,
    phase: &'static str,
) -> Result<Value, ProviderError> {
    if inner.closed.load(Ordering::Acquire) {
        return Err(ProviderError::ProcessExited { code: None });
    }
    let id = inner.next_id.fetch_add(1, Ordering::Relaxed).to_string();
    let (sender, receiver) = oneshot::channel();
    inner.queues.insert_pending(id.clone(), sender).await?;
    let _cleanup = PendingCleanup {
        pending: Arc::downgrade(&inner.queues.pending),
        id: id.clone(),
    };
    if let Err(error) = inner
        .write(json!({"id": id, "method": method, "params": params}))
        .await
    {
        inner.queues.remove_pending(&id).await;
        return Err(error);
    }
    let outcome = timeout(wait, receiver)
        .await
        .map_err(|_| ProviderError::Timeout { phase });
    if outcome.is_err() {
        inner.queues.remove_pending(&id).await;
    }
    match outcome? {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(detail)) => Err(ProviderError::Protocol(detail)),
        Err(_) => Err(ProviderError::ProcessExited { code: None }),
    }
}

/// A viewer has one small, non-cancellable write in flight at most. Timing out or dropping
/// its caller never cancels a partially written NDJSON frame or closes the canonical host.
async fn request_view(
    inner: &Arc<BrokerInner>,
    method: &str,
    params: Value,
    wait: Duration,
    phase: &'static str,
) -> Result<Value, ProviderError> {
    tracing::debug!(
        host_pid = inner.diagnostic.host_pid,
        phase,
        "Codex viewer read"
    );
    let permit = Arc::clone(&inner.view_request)
        .try_acquire_owned()
        .map_err(|_| {
            ProviderError::Protocol(
                "Codex viewer read already in flight; resync required".to_owned(),
            )
        })?;
    if inner.closed.load(Ordering::Acquire) {
        return Err(ProviderError::ProcessExited { code: None });
    }
    let id = format!("view:{}", inner.next_id.fetch_add(1, Ordering::Relaxed));
    let mut bytes = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))?;
    if bytes.len() > 4096 {
        return Err(ProviderError::Protocol(
            "Codex viewer request exceeded its byte budget".to_owned(),
        ));
    }
    bytes.push(b'\n');
    let (sender, receiver) = oneshot::channel();
    inner.queues.insert_pending(id.clone(), sender).await?;
    let inner = Arc::clone(inner);
    let task = tokio::spawn(async move {
        let _permit = permit;
        let _cleanup = PendingCleanup {
            pending: Arc::downgrade(&inner.queues.pending),
            id,
        };
        // Only waiting for the writer can time out. Once started, finish the frame even if
        // the viewer disconnects. No viewer path calls fail_closed or terminate_tree.
        let mut writer = timeout(WRITE_DEADLINE, inner.stdin.lock())
            .await
            .map_err(|_| ProviderError::Timeout { phase })?;
        writer.write_all(&bytes).await?;
        writer.flush().await?;
        drop(writer);
        match timeout(wait, receiver)
            .await
            .map_err(|_| ProviderError::Timeout { phase })?
        {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(ProviderError::Protocol(error)),
            Err(_) => Err(ProviderError::ProcessExited { code: None }),
        }
    });
    timeout(wait, task)
        .await
        .map_err(|_| ProviderError::Timeout { phase })?
        .map_err(|_| ProviderError::Protocol("Codex viewer read task ended".to_owned()))?
}

async fn read_stdout(inner: Arc<BrokerInner>, stdout: ChildStdout) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let line = match read_broker_line(&mut stdout).await {
            Ok(Some(BrokerLine::Message(line))) => line,
            Ok(Some(BrokerLine::Oversized {
                id: Some(id),
                has_method: false,
            })) if id.starts_with("view:") => {
                if let Some(waiter) = inner.queues.pending.lock().await.remove(&id) {
                    let _ = waiter.send(Err(
                        "Codex viewer response exceeded the line limit; resync required".to_owned(),
                    ));
                }
                continue;
            }
            Ok(Some(BrokerLine::Oversized { .. })) => {
                fail_closed(&inner, "controller-frame-overflow").await;
                return;
            }
            Ok(None) => {
                inner.close("reader-eof").await;
                return;
            }
            Err(error) => {
                inner.close("reader-io-error").await;
                let _ = inner
                    .queues
                    .enqueue_controller(Err(format!("Codex broker stdout failed: {error}")))
                    .await;
                return;
            }
        };
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_slice(&line) {
            Ok(message) => message,
            Err(error) => {
                inner.close("reader-invalid-json").await;
                let _ = inner
                    .queues
                    .enqueue_controller(Err(format!("Codex broker received invalid JSON: {error}")))
                    .await;
                return;
            }
        };
        let method = diagnostic_method(&message);
        tracing::debug!(host_pid = inner.diagnostic.host_pid, method,
            frame_bytes = line.len(), class = ?classify_message(&message),
            "Codex stdout frame");
        match inner.queues.route(message).await {
            QueueRoute::Delivered | QueueRoute::DroppedResponse => {}
            route => {
                let reason = match route {
                    QueueRoute::ControllerOverflow => "controller-queue-overflow",
                    QueueRoute::ControllerClosed => "controller-receiver-closed",
                    _ => "invalid-response-envelope",
                };
                tracing::warn!(
                    host_pid = inner.diagnostic.host_pid,
                    method,
                    reason,
                    controller_messages =
                        MAX_CONTROLLER_MESSAGES - inner.queues.controller_tx.capacity(),
                    controller_bytes = inner.queues.controller_bytes.load(Ordering::Acquire),
                    "Codex controller delivery failed"
                );
                fail_closed(&inner, reason).await;
                return;
            }
        }
    }
}

fn diagnostic_method(message: &Value) -> &'static str {
    // Match to literals rather than logging provider-controlled strings or response ids.
    match message.get("method").and_then(Value::as_str) {
        Some("turn/started") => "turn/started",
        Some("turn/completed") => "turn/completed",
        Some("item/started") => "item/started",
        Some("item/completed") => "item/completed",
        Some("item/agentMessage/delta") => "item/agentMessage/delta",
        Some("item/commandExecution/outputDelta") => "item/commandExecution/outputDelta",
        Some("thread/tokenUsage/updated") => "thread/tokenUsage/updated",
        Some("account/rateLimits/updated") => "account/rateLimits/updated",
        Some("item/tool/call") => "item/tool/call",
        Some("item/commandExecution/requestApproval") => "item/commandExecution/requestApproval",
        Some("error") => "error",
        Some("process/exited") => "process/exited",
        None => "response",
        _ => "other",
    }
}

async fn fail_closed(inner: &Arc<BrokerInner>, reason: &'static str) {
    inner.fail_closed(reason).await;
}

async fn drain_stderr(stderr: ChildStderr) {
    let mut stderr = BufReader::new(stderr);
    while read_line(&mut stderr)
        .await
        .is_ok_and(|line| line.is_some())
    {}
}

async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);
        let content = newline.map_or(buffer, |position| &buffer[..position]);
        if line.len().saturating_add(content.len()) > MAX_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Codex output line exceeded the limit",
            ));
        }
        line.extend_from_slice(content);
        reader.consume(consumed);
        if newline.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}
