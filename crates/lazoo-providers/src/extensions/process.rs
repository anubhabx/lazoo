use super::ExtensionError;
use lazoo_runtime::{JobChild, JobCommand, LaunchEnvironment};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};

pub const OUTPUT_CAP: u64 = 1024 * 1024;

async fn read_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, ExtensionError> {
    let mut bytes = Vec::new();
    reader.take(OUTPUT_CAP + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > OUTPUT_CAP {
        return Err(ExtensionError::Limit("command output"));
    }
    Ok(bytes)
}

/// Raw output is private to parsers; errors never include stdout, stderr or environment values.
pub async fn run_command(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    environment: &LaunchEnvironment,
    deadline: Duration,
) -> Result<Vec<u8>, ExtensionError> {
    let command = environment.apply_to_job(JobCommand::new(executable, cwd).args(args));
    let mut child = JobChild::spawn(&command)?;
    // Closing the pipe before the helper starts yields immediate EOF, without an interactive console.
    drop(child.take_stdin());
    let stdout = child.take_stdout().ok_or(ExtensionError::Command)?;
    let stderr = child.take_stderr().ok_or(ExtensionError::Command)?;
    let result = tokio::time::timeout(deadline, async {
        let (out, _, status) = tokio::try_join!(read_output(stdout), read_output(stderr), async {
            child.wait().await.map_err(ExtensionError::from)
        })?;
        if !status.success() {
            return Err(ExtensionError::Command);
        }
        Ok(out)
    })
    .await;
    let _ = child.terminate_tree(1);
    result.map_err(|_| ExtensionError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
    }
    #[tokio::test]
    async fn commands_close_stdin_and_enforce_output_and_deadline_limits() {
        let temp = tempfile::tempdir().unwrap();
        let env = LaunchEnvironment::default();
        let args =
            |script: &str| ["-NoProfile", "-NonInteractive", "-Command", script].map(str::to_owned);
        let empty = run_command(
            &shell(),
            &args("[Console]::Write([Console]::In.ReadToEnd().Length)"),
            temp.path(),
            &env,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(empty, b"0");
        assert!(matches!(
            run_command(
                &shell(),
                &args("[Console]::Write('x' * 1100000)"),
                temp.path(),
                &env,
                Duration::from_secs(10)
            )
            .await,
            Err(ExtensionError::Limit(_))
        ));
        assert!(matches!(
            run_command(
                &shell(),
                &args("Start-Sleep -Seconds 20"),
                temp.path(),
                &env,
                Duration::from_millis(100)
            )
            .await,
            Err(ExtensionError::Timeout)
        ));
        let error = run_command(
            &shell(),
            &args("[Console]::Error.Write('token=private-fixture'); exit 9"),
            temp.path(),
            &env,
            Duration::from_secs(10),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("private-fixture"));
    }
}
