use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

use lazoo_ipc::PipeClient;
use lazoo_protocol::{JsonRpcRequest, JsonRpcResponse, TerminalIoRequest, TerminalIoResponse};
use lazoo_runtime::{DataRoot, Installation};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio::time::{sleep, timeout};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_terminal_pipe_round_trips_and_rejects_stale_boot_without_blocking_controls() {
    let daemon = Daemon::start().await;
    let terminal = daemon.call("terminal.open", json!({"executable": r"C:\Windows\System32\cmd.exe", "args": ["/Q", "/K"], "cwd": std::env::current_dir().unwrap(), "columns": 80, "rows": 25, "idempotencyKey": "terminal-binary"})).await.unwrap();
    let id = serde_json::from_value(terminal["id"].clone()).unwrap();
    let boot = serde_json::from_value(terminal["bootId"].clone()).unwrap();
    let mut pipe =
        PipeClient::connect_terminal(&daemon.installation, "terminal-bytes-test", "0.1.0")
            .await
            .unwrap();
    pipe.send_json(&TerminalIoRequest::Write {
        id,
        expected_boot_id: boot,
    })
    .await
    .unwrap();
    pipe.send_bytes(b"echo LAZOO_BINARY_PIPE_OK\r\n")
        .await
        .unwrap();
    let response: TerminalIoResponse = pipe.receive_json().await.unwrap();
    assert!(response.error.is_none());
    let mut after = 0;
    let mut all = Vec::new();
    timeout(Duration::from_secs(4), async {
        loop {
            let mut pipe =
                PipeClient::connect_terminal(&daemon.installation, "terminal-bytes-test", "0.1.0")
                    .await
                    .unwrap();
            pipe.send_json(&TerminalIoRequest::Read {
                id,
                expected_boot_id: boot,
                after,
                limit: 64 * 1024,
            })
            .await
            .unwrap();
            let response: TerminalIoResponse = pipe.receive_json().await.unwrap();
            assert!(response.error.is_none());
            let metadata = response.output.unwrap();
            let bytes = pipe.receive_bytes(64 * 1024).await.unwrap();
            assert_eq!(bytes.len(), metadata.length);
            assert_eq!(metadata.end - metadata.start, bytes.len() as u64);
            after = metadata.end;
            all.extend(bytes);
            if String::from_utf8_lossy(&all).contains("LAZOO_BINARY_PIPE_OK") {
                break;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let mut stale = PipeClient::connect_terminal(&daemon.installation, "stale-boot", "0.1.0")
        .await
        .unwrap();
    stale
        .send_json(&TerminalIoRequest::Read {
            id,
            expected_boot_id: uuid::Uuid::new_v4(),
            after: 0,
            limit: 64,
        })
        .await
        .unwrap();
    let response: TerminalIoResponse = stale.receive_json().await.unwrap();
    assert!(response.error.is_some());
    // Sixteen idle terminal peers must consume only the terminal connection budget.
    let mut stalled = Vec::new();
    for _ in 0..16 {
        if let Ok(pipe) =
            PipeClient::connect_terminal(&daemon.installation, "terminal-idle", "0.1.0").await
        {
            stalled.push(pipe);
        }
    }
    daemon.call("system.status", json!({})).await.unwrap();
    daemon
        .call(
            "terminal.stop",
            json!({"id": id, "expectedBootId": boot, "expectedRevision": 1}),
        )
        .await
        .unwrap();
}

struct Daemon {
    child: Child,
    installation: Installation,
    _root: TempDir,
}
impl Daemon {
    async fn start() -> Self {
        let root = tempdir().unwrap();
        let installation = Installation::load_or_create(&DataRoot::from_path(root.path())).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lazood"))
            .env("LAZOO_DATA_ROOT", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self {
            child,
            installation,
            _root: root,
        };
        for _ in 0..80 {
            if daemon.call("system.status", json!({})).await.is_ok() {
                return daemon;
            }
            sleep(Duration::from_millis(25)).await;
        }
        panic!("daemon did not start");
    }
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut connection =
            PipeClient::connect(&self.installation, "terminal-service-test", "0.1.0")
                .await
                .map_err(|error| error.to_string())?;
        connection
            .send_json(&JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: "terminal-test".into(),
                method: method.into(),
                params,
            })
            .await
            .map_err(|error| error.to_string())?;
        let response: JsonRpcResponse = timeout(Duration::from_secs(3), connection.receive_json())
            .await
            .map_err(|_| "RPC timed out".to_owned())?
            .map_err(|error| error.to_string())?;
        response.error.map_or_else(
            || Ok(response.result.unwrap_or(Value::Null)),
            |error| Err(error.message),
        )
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_control_plane_is_ephemeral_idempotent_and_revisioned() {
    let daemon = Daemon::start().await;
    let open = json!({"executable": r"C:\Windows\System32\cmd.exe", "args": ["/Q", "/K"], "cwd": std::env::current_dir().unwrap(), "columns": 80, "rows": 25, "idempotencyKey": "terminal-service-test-key"});
    let first = daemon.call("terminal.open", open.clone()).await.unwrap();
    let second = daemon.call("terminal.open", open).await.unwrap();
    assert_eq!(first["id"], second["id"]);
    let id = first["id"].clone();
    let listed = daemon.call("terminal.list", json!({})).await.unwrap();
    assert_eq!(listed["terminals"].as_array().unwrap().len(), 1);
    assert_eq!(listed["bootId"], first["bootId"]);
    let boot_id = first["bootId"].clone();
    let resized = daemon
        .call(
            "terminal.resize",
            json!({"id": id, "columns": 100, "rows": 40, "expectedRevision": 1, "expectedBootId": boot_id}),
        )
        .await
        .unwrap();
    assert_eq!(resized["revision"], 2);
    assert!(
        daemon
            .call(
                "terminal.stop",
                json!({"id": resized["id"], "expectedRevision": 1, "expectedBootId": boot_id})
            )
            .await
            .is_err()
    );
    let stopped = daemon
        .call(
            "terminal.stop",
            json!({"id": resized["id"], "expectedRevision": 2, "expectedBootId": boot_id}),
        )
        .await
        .unwrap();
    assert_eq!(stopped["live"], false);
    let shown = daemon
        .call("terminal.show", json!({"id": stopped["id"]}))
        .await
        .unwrap();
    assert_eq!(shown, stopped);
    let listed = daemon.call("terminal.list", json!({})).await.unwrap();
    assert_eq!(listed["terminals"], json!([stopped]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_terminal_records_do_not_consume_the_live_terminal_limit() {
    let daemon = Daemon::start().await;
    for index in 0..10 {
        let record = daemon
            .call(
                "terminal.open",
                json!({
                    "executable": r"C:\Windows\System32\cmd.exe",
                    "args": ["/Q", "/K"],
                    "cwd": std::env::current_dir().unwrap(),
                    "columns": 80,
                    "rows": 25,
                    "idempotencyKey": format!("terminal-churn-{index}"),
                }),
            )
            .await
            .unwrap();
        daemon
            .call(
                "terminal.stop",
                json!({
                    "id": record["id"],
                    "expectedBootId": record["bootId"],
                    "expectedRevision": 1,
                }),
            )
            .await
            .unwrap();
    }
    let listed = daemon.call("terminal.list", json!({})).await.unwrap();
    assert_eq!(listed["terminals"].as_array().unwrap().len(), 10);
    assert!(
        listed["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|terminal| terminal["live"] == false)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_open_client_does_not_strand_the_reserved_terminal_slot() {
    let daemon = Daemon::start().await;
    let mut connection =
        PipeClient::connect(&daemon.installation, "abandoned-terminal-open", "0.1.0")
            .await
            .unwrap();
    connection
        .send_json(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "abandoned-open".into(),
            method: "terminal.open".into(),
            params: json!({
                "executable": r"C:\Windows\System32\cmd.exe",
                "args": ["/Q", "/K"],
                "cwd": std::env::current_dir().unwrap(),
                "columns": 80,
                "rows": 25,
                "idempotencyKey": "abandoned-terminal-open",
            }),
        })
        .await
        .unwrap();
    drop(connection);
    let record = timeout(Duration::from_secs(3), async {
        loop {
            let list = daemon.call("terminal.list", json!({})).await.unwrap();
            if let Some(record) = list["terminals"].as_array().and_then(|items| items.first()) {
                break record.clone();
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    daemon
        .call(
            "terminal.stop",
            json!({
                "id": record["id"],
                "expectedBootId": record["bootId"],
                "expectedRevision": record["revision"],
            }),
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_during_stop_cannot_reap_the_stop_owned_session() {
    let daemon = Daemon::start().await;
    for index in 0..6 {
        let record = daemon
            .call(
                "terminal.open",
                json!({
                    "executable": r"C:\Windows\System32\cmd.exe",
                    "args": ["/Q", "/K"],
                    "cwd": std::env::current_dir().unwrap(),
                    "columns": 80,
                    "rows": 25,
                    "idempotencyKey": format!("terminal-stop-list-race-{index}"),
                }),
            )
            .await
            .unwrap();
        let stop = daemon.call(
            "terminal.stop",
            json!({
                "id": record["id"],
                "expectedBootId": record["bootId"],
                "expectedRevision": 1,
            }),
        );
        let list = daemon.call("terminal.list", json!({}));
        let (stopped, listed) = tokio::join!(stop, list);
        assert!(stopped.is_ok(), "stop must retain ownership: {stopped:?}");
        assert!(listed.is_ok());
    }
}
