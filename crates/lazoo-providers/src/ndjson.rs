use std::{
    collections::VecDeque, future::poll_fn, pin::Pin, sync::Arc, task::Poll, time::Duration,
};

use lazoo_runtime::{JobChild, JobCommand};
use serde_json::Value;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStderr, ChildStdout},
    sync::Mutex,
    task::JoinHandle,
    time::timeout,
};

use crate::{CommandSpec, ProviderError};

const MAX_PROVIDER_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_LINES: usize = 100;
const MAX_STDERR_LINE_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct NdjsonControl {
    child: Arc<Mutex<JobChild>>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
}

impl NdjsonControl {
    pub(crate) async fn send(&self, message: &Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(message)?;
        if bytes.len() > MAX_PROVIDER_LINE_BYTES {
            return Err(ProviderError::Protocol(
                "provider input record exceeded the line limit".to_owned(),
            ));
        }
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub(crate) async fn kill(&self) -> Result<(), ProviderError> {
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            child.terminate_tree(1)?;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), ProviderError> {
        self.stdin.lock().await.shutdown().await?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct NdjsonProcess {
    child: Arc<Mutex<JobChild>>,
    stdout: BufReader<ChildStdout>,
    control: NdjsonControl,
    stderr_lines: Arc<Mutex<VecDeque<String>>>,
    stderr_task: JoinHandle<()>,
}

impl NdjsonProcess {
    pub(crate) fn spawn(spec: &CommandSpec) -> Result<Self, ProviderError> {
        let command = JobCommand::new(&spec.executable, &spec.cwd)
            .args(spec.args.clone())
            .envs(spec.env.clone());
        let mut child = JobChild::spawn(&command)?;
        let stdin = child
            .take_stdin()
            .ok_or_else(|| ProviderError::Protocol("provider stdin was not piped".to_owned()))?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| ProviderError::Protocol("provider stdout was not piped".to_owned()))?;
        let stderr = child
            .take_stderr()
            .ok_or_else(|| ProviderError::Protocol("provider stderr was not piped".to_owned()))?;
        let child = Arc::new(Mutex::new(child));
        let stdin = Arc::new(Mutex::new(stdin));
        let stderr_lines = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_task = tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_lines)));
        Ok(Self {
            child: Arc::clone(&child),
            stdout: BufReader::new(stdout),
            control: NdjsonControl { child, stdin },
            stderr_lines,
            stderr_task,
        })
    }

    pub(crate) fn control(&self) -> NdjsonControl {
        self.control.clone()
    }

    /// Waits briefly for unsolicited bytes without consuming them. Unlike `next_message`, a
    /// timeout preserves a partially received line in the reader buffer.
    pub(crate) async fn wait_for_ready_output(
        &mut self,
        wait: Duration,
    ) -> Result<bool, ProviderError> {
        match timeout(
            wait,
            poll_fn(
                |context| match Pin::new(&mut self.stdout).poll_fill_buf(context) {
                    Poll::Ready(Ok(bytes)) => Poll::Ready(Ok(!bytes.is_empty())),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(ProviderError::Io(error))),
                    Poll::Pending => Poll::Pending,
                },
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(false),
        }
    }

    pub(crate) fn native_host_pid(&self) -> Option<u32> {
        self.child.try_lock().ok().and_then(|child| child.id())
    }

    pub(crate) fn job(&self) -> Option<std::sync::Arc<lazoo_runtime::JobObject>> {
        self.child.try_lock().ok().map(|child| child.job())
    }

    pub(crate) async fn next_message(
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
                let code = self
                    .child
                    .lock()
                    .await
                    .try_wait()?
                    .and_then(|status| status.code());
                return Err(ProviderError::ProcessExited { code });
            };
            if line.is_empty() {
                continue;
            }
            return serde_json::from_slice(&line).map_err(ProviderError::from);
        }
    }

    pub(crate) async fn stop(&mut self, wait: Duration) -> Result<(), ProviderError> {
        let _ = self.control.close().await;
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            if let Ok(status) = timeout(wait, child.wait()).await {
                status?;
            } else {
                child.terminate_tree(1)?;
                let _ = timeout(wait, child.wait()).await;
            }
        }
        self.stderr_task.abort();
        self.stderr_lines.lock().await.clear();
        Ok(())
    }
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
