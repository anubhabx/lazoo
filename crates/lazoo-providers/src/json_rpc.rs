use std::{
    collections::VecDeque,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};

use crate::{CommandSpec, ProviderError};

const MAX_PROVIDER_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_LINES: usize = 100;
const MAX_STDERR_LINE_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct JsonRpcControl {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    next_id: Arc<AtomicU64>,
    standard_envelope: bool,
}

impl JsonRpcControl {
    pub(crate) async fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<String, ProviderError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let mut message = json!({"id": id, "method": method, "params": params});
        self.add_standard_envelope(&mut message);
        self.write(&message).await?;
        Ok(id)
    }

    pub(crate) async fn send_uncorrelated_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(), ProviderError> {
        self.send_request(method, params).await.map(|_| ())
    }

    pub(crate) async fn notify(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), ProviderError> {
        let mut message = params.map_or_else(
            || json!({"method": method}),
            |params| json!({"method": method, "params": params}),
        );
        self.add_standard_envelope(&mut message);
        self.write(&message).await
    }

    pub(crate) async fn respond(
        &self,
        id: Value,
        result: Result<Value, (i64, &'static str)>,
    ) -> Result<(), ProviderError> {
        let mut message = match result {
            Ok(result) => json!({"id": id, "result": result}),
            Err((code, message)) => {
                json!({"id": id, "error": {"code": code, "message": message}})
            }
        };
        self.add_standard_envelope(&mut message);
        self.write(&message).await
    }

    fn add_standard_envelope(&self, message: &mut Value) {
        if self.standard_envelope {
            message
                .as_object_mut()
                .expect("JSON-RPC messages are objects")
                .insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
        }
    }

    async fn write(&self, message: &Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(message)?;
        if bytes.len() > MAX_PROVIDER_LINE_BYTES {
            return Err(ProviderError::Protocol(
                "provider request exceeded the line limit".to_owned(),
            ));
        }
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub(crate) async fn close(&self) -> Result<(), ProviderError> {
        self.stdin.lock().await.shutdown().await?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct JsonRpcProcess {
    child: Child,
    stdout: BufReader<ChildStdout>,
    control: JsonRpcControl,
    queued: VecDeque<Value>,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    stderr_task: JoinHandle<()>,
}

impl JsonRpcProcess {
    pub(crate) fn spawn(spec: &CommandSpec) -> Result<Self, ProviderError> {
        Self::spawn_with_envelope(spec, false)
    }

    pub(crate) fn spawn_standard(spec: &CommandSpec) -> Result<Self, ProviderError> {
        Self::spawn_with_envelope(spec, true)
    }

    fn spawn_with_envelope(
        spec: &CommandSpec,
        standard_envelope: bool,
    ) -> Result<Self, ProviderError> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Protocol("provider stdin was not piped".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Protocol("provider stdout was not piped".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::Protocol("provider stderr was not piped".to_owned()))?;
        let stderr_lines = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_task = tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_lines)));
        Ok(Self {
            child,
            stdout: BufReader::new(stdout),
            control: JsonRpcControl {
                stdin: Arc::new(Mutex::new(stdin)),
                next_id: Arc::new(AtomicU64::new(1)),
                standard_envelope,
            },
            queued: VecDeque::new(),
            stderr_lines,
            stderr_task,
        })
    }

    pub(crate) fn control(&self) -> JsonRpcControl {
        self.control.clone()
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        let request_id = self.control.send_request(method, params).await?;
        loop {
            let message = self.read_wire_message(wait, phase).await?;
            if response_id(&message).is_some_and(|id| id == request_id) {
                if let Some(result) = message.get("result") {
                    return Ok(result.clone());
                }
                if let Some(error) = message.get("error") {
                    let code = error
                        .get("code")
                        .map_or_else(|| "unknown".to_owned(), Value::to_string);
                    return Err(ProviderError::Protocol(format!(
                        "provider request {method} failed with code {code}"
                    )));
                }
                return Err(ProviderError::Protocol(format!(
                    "provider response for {method} contained neither result nor error"
                )));
            }
            self.queued.push_back(message);
        }
    }

    pub(crate) async fn next_message(
        &mut self,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        if let Some(message) = self.queued.pop_front() {
            return Ok(message);
        }
        self.read_wire_message(wait, phase).await
    }

    pub(crate) fn discard_queued(&mut self) -> usize {
        let count = self.queued.len();
        self.queued.clear();
        count
    }

    async fn read_wire_message(
        &mut self,
        wait: Duration,
        phase: &'static str,
    ) -> Result<Value, ProviderError> {
        loop {
            let line = timeout(
                wait,
                read_bounded_line(&mut self.stdout, MAX_PROVIDER_LINE_BYTES),
            )
            .await
            .map_err(|_| ProviderError::Timeout { phase })??;
            let Some(line) = line else {
                let code = self.child.try_wait()?.and_then(|status| status.code());
                return Err(ProviderError::ProcessExited { code });
            };
            if line.is_empty() {
                continue;
            }
            return serde_json::from_slice(&line).map_err(ProviderError::from);
        }
    }

    pub(crate) async fn stderr_tail(&self) -> Vec<String> {
        self.stderr_lines.lock().await.iter().cloned().collect()
    }

    pub(crate) async fn stop(&mut self, wait: Duration) -> Result<(), ProviderError> {
        let _ = self.control.close().await;
        if let Ok(status) = timeout(wait, self.child.wait()).await {
            status?;
        } else {
            self.child.start_kill()?;
            let _ = timeout(wait, self.child.wait()).await;
        }
        self.stderr_task.abort();
        Ok(())
    }
}

pub(crate) fn response_id(message: &Value) -> Option<String> {
    message.get("id").and_then(|id| match id {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

async fn drain_stderr(stderr: ChildStderr, lines: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let Ok(Some(bytes)) = read_bounded_line(&mut reader, MAX_STDERR_LINE_BYTES).await else {
            break;
        };
        let line = String::from_utf8_lossy(&bytes).into_owned();
        let mut lines = lines.lock().await;
        if lines.len() == MAX_STDERR_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut output = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if output.is_empty() {
                Ok(None)
            } else {
                Ok(Some(output))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);
        let content = newline.map_or(buffer, |position| &buffer[..position]);
        if output.len().saturating_add(content.len()) > maximum {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "provider output line exceeded the limit",
            ));
        }
        output.extend_from_slice(content);
        reader.consume(consumed);
        if newline.is_some() {
            if output.last() == Some(&b'\r') {
                output.pop();
            }
            return Ok(Some(output));
        }
    }
}
