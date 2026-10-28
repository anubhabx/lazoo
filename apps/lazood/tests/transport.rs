use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

use lazoo_ipc::{PipeClient, read_frame, write_frame};
use lazoo_protocol::{JsonRpcRequest, JsonRpcResponse};
use lazoo_runtime::{DataRoot, Installation};
use serde_json::json;
use tempfile::{TempDir, tempdir};
use tokio::{
    net::windows::named_pipe::ClientOptions,
    time::{sleep, timeout},
};

struct Daemon {
    child: Child,
    installation: Installation,
    _root: TempDir,
}

impl Daemon {
    async fn start() -> Self {
        let temporary = tempdir().unwrap();
        let installation =
            Installation::load_or_create(&DataRoot::from_path(temporary.path())).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lazood"))
            .env("LAZOO_DATA_ROOT", temporary.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self {
            child,
            installation,
            _root: temporary,
        };
        daemon.status().await;
        daemon
    }

    async fn status(&self) {
        let mut connection = PipeClient::connect(&self.installation, "transport-test", "0.1.0")
            .await
            .expect("daemon must remain available");
        connection
            .send_json(&JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: "health".to_owned(),
                method: "system.status".to_owned(),
                params: json!({}),
            })
            .await
            .unwrap();
        let response: JsonRpcResponse = timeout(Duration::from_secs(3), connection.receive_json())
            .await
            .expect("one stalled client must not block healthy clients")
            .unwrap();
        assert!(response.error.is_none(), "{response:?}");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_survives_rejected_and_malformed_client_handshakes() {
    let daemon = Daemon::start().await;
    let invalid = PipeClient::connect_using(
        daemon.installation.pipe_name(),
        daemon.installation.id(),
        &[7; 32],
        "wrong-nonce",
        "0.1.0",
        &[1],
    )
    .await;
    assert!(invalid.is_err());
    daemon.status().await;

    let mut malformed = ClientOptions::new()
        .open(daemon.installation.pipe_name())
        .unwrap();
    read_frame(&mut malformed).await.unwrap();
    write_frame(&mut malformed, b"not json").await.unwrap();
    drop(malformed);
    sleep(Duration::from_millis(20)).await;
    daemon.status().await;

    let mut abandoned = ClientOptions::new()
        .open(daemon.installation.pipe_name())
        .unwrap();
    read_frame(&mut abandoned).await.unwrap();
    drop(abandoned);
    sleep(Duration::from_millis(20)).await;
    daemon.status().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_handshake_does_not_block_another_client() {
    let daemon = Daemon::start().await;
    let mut stalled = ClientOptions::new()
        .open(daemon.installation.pipe_name())
        .unwrap();
    read_frame(&mut stalled).await.unwrap();
    timeout(Duration::from_secs(3), daemon.status())
        .await
        .expect("handshakes must be isolated per connection");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_survives_clients_disconnecting_before_server_hello() {
    let daemon = Daemon::start().await;
    for _ in 0..40 {
        if let Ok(client) = ClientOptions::new().open(daemon.installation.pipe_name()) {
            drop(client);
        }
        tokio::task::yield_now().await;
    }
    daemon.status().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overloaded_daemon_rejects_promptly_and_recovers_capacity() {
    let daemon = Daemon::start().await;
    let mut held = Vec::new();
    for _ in 0..64 {
        held.push(
            PipeClient::connect(&daemon.installation, "held-client", "0.1.0")
                .await
                .unwrap(),
        );
    }
    let overflow = timeout(
        Duration::from_secs(2),
        PipeClient::connect(&daemon.installation, "overflow", "0.1.0"),
    )
    .await;
    assert!(
        overflow
            .expect("overload must not leave a caller hanging")
            .is_err()
    );
    drop(held);
    // The server must reap closed tasks before admitting the next healthy request.
    sleep(Duration::from_millis(100)).await;
    daemon.status().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_reports_native_infrastructure_enforcement_without_claiming_desktop_coverage() {
    let daemon = Daemon::start().await;
    let mut connection = PipeClient::connect(&daemon.installation, "budget-test", "0.1.0")
        .await
        .unwrap();
    connection
        .send_json(&JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: "budget".to_owned(),
            method: "system.budget".to_owned(),
            params: json!({}),
        })
        .await
        .unwrap();
    let response: JsonRpcResponse = connection.receive_json().await.unwrap();
    assert!(response.error.is_none(), "{response:?}");
    let result = response.result.unwrap();
    assert_eq!(result["enforced"], true, "{result}");
    assert_eq!(result["hardCapPercent"], 20);
    assert_eq!(result["desktopIncluded"], false);
    assert_eq!(result["providerTreesIncluded"], false);
    assert_eq!(result["thirtySeconds"]["state"], "warming");
}
