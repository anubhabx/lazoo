//! Daemon RPC client: the method allowlist, deadlines and error mapping.
//!
//! The frontend never names an executable, a pipe or a credential. Every request opens a
//! fresh authenticated connection because the daemon closes it after one response.
use std::{future::Future, time::Duration};

use lazoo_ipc::{AuthenticatedConnection, IpcError, PipeClient};
use lazoo_protocol::{JsonRpcRequest, JsonRpcResponse};
use lazoo_runtime::{DataRoot, Installation, RuntimeError};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::OnceCell;
use uuid::Uuid;

pub const CLIENT_NAME: &str = "lazoo-desktop";
pub const READ_DEADLINE: Duration = Duration::from_secs(5);
pub const MUTATION_DEADLINE: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopFailure {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl DesktopFailure {
    #[must_use]
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum DesktopClientError {
    #[error("desktop IPC request timed out")]
    Timeout,
    #[error("desktop IPC failed")]
    Ipc(#[from] IpcError),
    #[error("desktop runtime failed")]
    Runtime(#[from] RuntimeError),
    #[error("daemon RPC failed with {code}")]
    Rpc {
        code: String,
        message: String,
        data: Option<Value>,
    },
    #[error("desktop response JSON was invalid")]
    Json(#[from] serde_json::Error),
    #[error("daemon response contained an invalid result/error combination")]
    InvalidResponse,
    #[error("the desktop does not permit that daemon method")]
    MethodRefused,
    #[error("terminal input exceeds the per-write limit")]
    InputTooLarge,
    #[error("terminal write was rejected: {0}")]
    TerminalWrite(String),
}

impl From<&DesktopClientError> for DesktopFailure {
    fn from(error: &DesktopClientError) -> Self {
        match error {
            DesktopClientError::Timeout | DesktopClientError::Ipc(IpcError::Io(_)) => Self::new(
                "LZ-DESKTOP-DAEMON-OFFLINE",
                "Lazoo cannot reach the local daemon. Start lazoo-keeper and retry.",
            ),
            DesktopClientError::Ipc(IpcError::Authentication) => Self::new(
                "LZ-IPC-AUTH",
                "The desktop could not authenticate with the local daemon.",
            ),
            DesktopClientError::Ipc(IpcError::ProtocolIncompatible) => Self::new(
                "LZ-PROTOCOL-INCOMPATIBLE",
                "The desktop and daemon use incompatible protocol versions.",
            ),
            DesktopClientError::Ipc(_) => Self::new(
                "LZ-DESKTOP-IPC",
                "The desktop could not read the local control channel.",
            ),
            DesktopClientError::Runtime(_) => Self::new(
                "LZ-DESKTOP-RUNTIME",
                "The Lazoo desktop runtime could not be initialized.",
            ),
            DesktopClientError::Rpc {
                code,
                message,
                data,
            } => Self {
                code: code.clone(),
                message: message.clone(),
                data: data.clone(),
            },
            DesktopClientError::Json(_) | DesktopClientError::InvalidResponse => Self::new(
                "LZ-DESKTOP-INVALID-RESPONSE",
                "The daemon returned a response the desktop could not safely read.",
            ),
            DesktopClientError::MethodRefused => Self::new(
                "LZ-DESKTOP-METHOD-REFUSED",
                "The desktop does not permit that daemon method.",
            ),
            DesktopClientError::InputTooLarge => Self::new(
                "LZ-DESKTOP-TERMINAL-INPUT-TOO-LARGE",
                "Terminal input is larger than one write allows.",
            ),
            DesktopClientError::TerminalWrite(message) => Self::new("LZ-TERMINAL-WRITE", message),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Read,
    Mutation,
}

impl Kind {
    #[must_use]
    pub const fn deadline(self) -> Duration {
        match self {
            Self::Read => READ_DEADLINE,
            Self::Mutation => MUTATION_DEADLINE,
        }
    }
}

/// The only daemon methods the webview may reach. Everything else, including shutdown,
/// raw terminal open, provider config apply, memory maintenance and agent-only tools,
/// stays unreachable from the frontend.
pub const ALLOWED: &[(&str, Kind)] = &[
    ("system.status", Kind::Read),
    ("system.resources", Kind::Read),
    ("system.budget", Kind::Read),
    ("desktop.snapshot", Kind::Read),
    ("project.list", Kind::Read),
    ("events.list", Kind::Read),
    ("employee.list", Kind::Read),
    ("employee.show", Kind::Read),
    ("roster.get", Kind::Read),
    ("provider.list", Kind::Read),
    ("provider.config", Kind::Read),
    ("provider.config.preview", Kind::Read),
    ("provider.config.operations.show", Kind::Read),
    ("memory.policy.show", Kind::Read),
    ("memory.search", Kind::Read),
    ("memory.queue.status", Kind::Read),
    ("memory.worker.show", Kind::Read),
    ("session.list", Kind::Read),
    ("session.show", Kind::Read),
    ("session.resources", Kind::Read),
    ("session.terminal.status", Kind::Read),
    ("terminal.list", Kind::Read),
    ("terminal.show", Kind::Read),
    ("conversation.list", Kind::Read),
    ("conversation.show", Kind::Read),
    ("turn.list", Kind::Read),
    ("turn.show", Kind::Read),
    ("task.list", Kind::Read),
    ("task.show", Kind::Read),
    ("approval.list", Kind::Read),
    ("approval.show", Kind::Read),
    ("incident.list", Kind::Read),
    ("incident.show", Kind::Read),
    ("incident.threshold.list", Kind::Read),
    ("inbox.list", Kind::Read),
    ("inbox.counts", Kind::Read),
    ("workspace.conversations", Kind::Read),
    ("workspace.launcher", Kind::Read),
    ("workspace.conversation", Kind::Read),
    ("native.trust.list", Kind::Read),
    ("native.trust.show", Kind::Read),
    ("native.trust.policy.show", Kind::Read),
    ("project.register", Kind::Mutation),
    ("employee.create", Kind::Mutation),
    ("employee.update", Kind::Mutation),
    ("employee.disable", Kind::Mutation),
    ("roster.refresh", Kind::Mutation),
    ("session.warm", Kind::Mutation),
    ("session.stop", Kind::Mutation),
    ("session.terminal.attach", Kind::Mutation),
    ("session.terminal.view", Kind::Mutation),
    ("terminal.resize", Kind::Mutation),
    ("terminal.stop", Kind::Mutation),
    ("conversation.create", Kind::Mutation),
    ("conversation.rename", Kind::Mutation),
    ("turn.submit", Kind::Mutation),
    ("turn.retry", Kind::Mutation),
    ("turn.cancel", Kind::Mutation),
    ("task.accept", Kind::Mutation),
    ("task.cancel", Kind::Mutation),
    ("task.resolve", Kind::Mutation),
    ("approval.decide", Kind::Mutation),
    ("incident.acknowledge", Kind::Mutation),
    ("incident.resolve", Kind::Mutation),
    ("native.trust.resolve", Kind::Mutation),
    ("native.trust.policy.set", Kind::Mutation),
    ("native.trust.policy.reset", Kind::Mutation),
];

#[must_use]
pub fn classify(method: &str) -> Option<Kind> {
    ALLOWED
        .iter()
        .find(|(name, _)| *name == method)
        .map(|(_, kind)| *kind)
}

pub fn build_request(
    id: impl Into<String>,
    method: &str,
    params: Value,
) -> Result<(JsonRpcRequest, Kind), DesktopClientError> {
    let kind = classify(method).ok_or(DesktopClientError::MethodRefused)?;
    Ok((
        JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: id.into(),
            method: method.to_owned(),
            params,
        },
        kind,
    ))
}

pub fn parse_response(
    response: JsonRpcResponse,
    expected_id: &str,
) -> Result<Value, DesktopClientError> {
    if response.jsonrpc != "2.0" || response.id != expected_id {
        return Err(DesktopClientError::InvalidResponse);
    }
    match (response.result, response.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(DesktopClientError::Rpc {
            code: error.code,
            message: error.message,
            data: error.data,
        }),
        _ => Err(DesktopClientError::InvalidResponse),
    }
}

pub async fn with_deadline<T, F>(duration: Duration, operation: F) -> Result<T, DesktopClientError>
where
    F: Future<Output = Result<T, DesktopClientError>>,
{
    tokio::time::timeout(duration, operation)
        .await
        .map_err(|_| DesktopClientError::Timeout)?
}

/// Resolves the installation once; credentials stay inside this process.
pub async fn installation(cell: &OnceCell<Installation>) -> Result<Installation, DesktopFailure> {
    cell.get_or_try_init(|| async {
        let root = DataRoot::resolve()?;
        Ok::<_, DesktopClientError>(Installation::load_or_create(&root)?)
    })
    .await
    .cloned()
    .map_err(|error| DesktopFailure::from(&error))
}

pub async fn connect(installation: &Installation) -> Result<AuthenticatedConnection, IpcError> {
    PipeClient::connect(installation, CLIENT_NAME, env!("CARGO_PKG_VERSION")).await
}

pub async fn connect_terminal(
    installation: &Installation,
) -> Result<AuthenticatedConnection, IpcError> {
    PipeClient::connect_terminal(installation, CLIENT_NAME, env!("CARGO_PKG_VERSION")).await
}

/// Sends one allowlisted request on a fresh connection and returns its `result`.
pub async fn call(
    installation: &Installation,
    method: &str,
    params: Value,
) -> Result<Value, DesktopFailure> {
    call_inner(installation, method, params)
        .await
        .map_err(|error| DesktopFailure::from(&error))
}

async fn call_inner(
    installation: &Installation,
    method: &str,
    params: Value,
) -> Result<Value, DesktopClientError> {
    let request_id = Uuid::new_v4().to_string();
    let (request, kind) = build_request(request_id.clone(), method, params)?;
    with_deadline(kind.deadline(), async {
        let mut connection = connect(installation).await?;
        connection.send_json(&request).await?;
        let response = connection.receive_json().await?;
        parse_response(response, &request_id)
    })
    .await
}

/// Opens `events.subscribe`, awaits its ack and hands back the live connection.
pub async fn subscribe(
    installation: &Installation,
    after: u64,
) -> Result<AuthenticatedConnection, DesktopFailure> {
    subscribe_inner(installation, after)
        .await
        .map_err(|error| DesktopFailure::from(&error))
}

async fn subscribe_inner(
    installation: &Installation,
    after: u64,
) -> Result<AuthenticatedConnection, DesktopClientError> {
    let request_id = Uuid::new_v4().to_string();
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: request_id.clone(),
        method: "events.subscribe".to_owned(),
        params: serde_json::json!({ "after": after, "limit": 512 }),
    };
    with_deadline(READ_DEADLINE, async {
        let mut connection = connect(installation).await?;
        connection.send_json(&request).await?;
        let response = connection.receive_json().await?;
        parse_response(response, &request_id)?;
        Ok(connection)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::{future::pending, time::Duration};

    use lazoo_protocol::{JsonRpcResponse, RpcError};
    use serde_json::json;

    use super::{
        ALLOWED, DesktopClientError, DesktopFailure, Kind, MUTATION_DEADLINE, READ_DEADLINE,
        build_request, classify, parse_response, with_deadline,
    };

    #[test]
    fn allowlist_refuses_privileged_and_agent_only_methods() {
        for method in [
            "system.shutdown",
            "terminal.open",
            "message.send",
            "message.reply",
            "provider.config.apply",
            "provider.config.rollback",
            "memory.cleanup.run",
            "memory.archive",
            "memory.policy.set",
            "memory.worker.configure",
            "task.delegate",
            "task.start",
            "task.report",
            "provider.probe",
            "provider.smoke",
            "incident.threshold.set",
            "incident.threshold.reset",
            "events.subscribe",
            "",
        ] {
            let error =
                build_request("id", method, json!({})).expect_err("privileged method must refuse");
            let public = DesktopFailure::from(&error);
            assert_eq!(public.code, "LZ-DESKTOP-METHOD-REFUSED", "{method}");
        }
    }

    #[test]
    fn allowlist_has_no_duplicates_and_the_expected_split() {
        let mut names: Vec<_> = ALLOWED.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALLOWED.len());
        let reads = ALLOWED
            .iter()
            .filter(|(_, kind)| *kind == Kind::Read)
            .count();
        assert_eq!(reads, 42);
        assert_eq!(ALLOWED.len() - reads, 25);
    }

    #[test]
    fn read_and_mutation_methods_select_their_deadline() {
        let (_, read) = build_request("id", "project.list", json!({})).expect("read");
        let (_, mutation) = build_request("id", "turn.submit", json!({})).expect("mutation");

        assert_eq!(read, Kind::Read);
        assert_eq!(read.deadline(), READ_DEADLINE);
        assert_eq!(read.deadline(), Duration::from_secs(5));
        assert_eq!(mutation, Kind::Mutation);
        assert_eq!(mutation.deadline(), MUTATION_DEADLINE);
        assert_eq!(mutation.deadline(), Duration::from_secs(600));
        assert_eq!(classify("approval.decide"), Some(Kind::Mutation));
        assert_eq!(classify("session.terminal.status"), Some(Kind::Read));
    }

    #[test]
    fn request_carries_the_allowlisted_method_and_params() {
        let (request, _) =
            build_request("desktop-test", "task.show", json!({"id": "x"})).expect("request");

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, "desktop-test");
        assert_eq!(request.method, "task.show");
        assert_eq!(request.params, json!({"id": "x"}));
    }

    #[test]
    fn valid_response_returns_the_result_value() {
        let response = JsonRpcResponse::success("desktop-test", json!({"projects": []}));

        assert_eq!(
            parse_response(response, "desktop-test").expect("result"),
            json!({"projects": []})
        );
    }

    #[test]
    fn rpc_failure_preserves_public_code_message_and_data() {
        let error = parse_response(
            JsonRpcResponse::failure(
                "desktop-test",
                RpcError {
                    code: "LZ-TURN-ACTIVE".to_owned(),
                    message: "a turn is active".to_owned(),
                    data: Some(json!({"turnId": "t-1"})),
                },
            ),
            "desktop-test",
        )
        .expect_err("RPC error must fail");

        let public = DesktopFailure::from(&error);
        assert_eq!(public.code, "LZ-TURN-ACTIVE");
        assert_eq!(public.message, "a turn is active");
        assert_eq!(public.data, Some(json!({"turnId": "t-1"})));
        let wire = serde_json::to_value(&public).expect("failure serializes");
        assert_eq!(wire["data"]["turnId"], "t-1");
    }

    #[test]
    fn failure_without_data_omits_the_field() {
        let wire = serde_json::to_value(DesktopFailure::new("LZ-X", "m")).expect("serializes");

        assert_eq!(wire, json!({"code": "LZ-X", "message": "m"}));
    }

    #[test]
    fn invalid_response_is_redacted() {
        let error = parse_response(
            JsonRpcResponse {
                jsonrpc: "2.0".to_owned(),
                id: "desktop-test".to_owned(),
                result: None,
                error: None,
            },
            "desktop-test",
        )
        .expect_err("invalid shape must fail");

        let public = DesktopFailure::from(&error);
        assert_eq!(public.code, "LZ-DESKTOP-INVALID-RESPONSE");
        assert!(!public.message.contains("nonce"));
    }

    #[test]
    fn response_id_and_json_rpc_version_must_match_the_request() {
        let wrong_id = JsonRpcResponse::success("another-request", json!({}));
        let wrong_version = JsonRpcResponse {
            jsonrpc: "1.0".to_owned(),
            id: "desktop-test".to_owned(),
            result: Some(json!({})),
            error: None,
        };

        for response in [wrong_id, wrong_version] {
            let error = parse_response(response, "desktop-test")
                .expect_err("mismatched response metadata must fail");
            assert_eq!(
                DesktopFailure::from(&error).code,
                "LZ-DESKTOP-INVALID-RESPONSE"
            );
        }
    }

    #[test]
    fn terminal_write_failures_use_their_contract_codes() {
        let too_large = DesktopFailure::from(&DesktopClientError::InputTooLarge);
        let rejected =
            DesktopFailure::from(&DesktopClientError::TerminalWrite("boot changed".into()));

        assert_eq!(too_large.code, "LZ-DESKTOP-TERMINAL-INPUT-TOO-LARGE");
        assert_eq!(rejected.code, "LZ-TERMINAL-WRITE");
        assert_eq!(rejected.message, "boot changed");
    }

    #[tokio::test]
    async fn stalled_exchange_times_out_as_daemon_offline() {
        let stalled = pending::<Result<(), DesktopClientError>>();

        let error = with_deadline(Duration::from_millis(10), stalled)
            .await
            .expect_err("stalled exchange must time out");
        let public = DesktopFailure::from(&error);

        assert_eq!(public.code, "LZ-DESKTOP-DAEMON-OFFLINE");
        assert!(public.message.contains("retry"));
    }
}
