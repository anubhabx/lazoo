use lazoo_ipc::{IpcError, PipeClient};
use lazoo_protocol::{DesktopSnapshot, JsonRpcRequest, JsonRpcResponse};
use lazoo_runtime::{DataRoot, Installation, RuntimeError};
use serde::Serialize;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
enum DesktopClientError {
    #[error("desktop IPC failed")]
    Ipc(#[from] IpcError),
    #[error("desktop runtime failed")]
    Runtime(#[from] RuntimeError),
    #[error("daemon RPC failed with {code}")]
    Rpc { code: String, message: String },
    #[error("desktop response JSON was invalid")]
    Json(#[from] serde_json::Error),
    #[error("daemon response contained an invalid result/error combination")]
    InvalidResponse,
}

impl From<&DesktopClientError> for DesktopFailure {
    fn from(error: &DesktopClientError) -> Self {
        match error {
            DesktopClientError::Ipc(IpcError::Authentication) => Self {
                code: "LZ-IPC-AUTH".to_owned(),
                message: "The desktop could not authenticate with the local daemon.".to_owned(),
            },
            DesktopClientError::Ipc(IpcError::ProtocolIncompatible) => Self {
                code: "LZ-PROTOCOL-INCOMPATIBLE".to_owned(),
                message: "The desktop and daemon use incompatible protocol versions.".to_owned(),
            },
            DesktopClientError::Ipc(IpcError::Io(_)) => Self {
                code: "LZ-DESKTOP-DAEMON-OFFLINE".to_owned(),
                message: "Lazoo cannot reach the local daemon. Start lazoo-keeper and retry."
                    .to_owned(),
            },
            DesktopClientError::Ipc(_) => Self {
                code: "LZ-DESKTOP-IPC".to_owned(),
                message: "The desktop could not read the local control channel.".to_owned(),
            },
            DesktopClientError::Runtime(_) => Self {
                code: "LZ-DESKTOP-RUNTIME".to_owned(),
                message: "The Lazoo desktop runtime could not be initialized.".to_owned(),
            },
            DesktopClientError::Rpc { code, message } => Self {
                code: code.clone(),
                message: message.clone(),
            },
            DesktopClientError::Json(_) | DesktopClientError::InvalidResponse => Self {
                code: "LZ-DESKTOP-INVALID-RESPONSE".to_owned(),
                message: "The daemon returned a response the desktop could not safely read."
                    .to_owned(),
            },
        }
    }
}

fn snapshot_request(id: impl Into<String>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: id.into(),
        method: "desktop.snapshot".to_owned(),
        params: json!({}),
    }
}

fn parse_snapshot_response(
    response: JsonRpcResponse,
) -> Result<DesktopSnapshot, DesktopClientError> {
    match (response.result, response.error) {
        (Some(result), None) => Ok(serde_json::from_value(result)?),
        (None, Some(error)) => Err(DesktopClientError::Rpc {
            code: error.code,
            message: error.message,
        }),
        _ => Err(DesktopClientError::InvalidResponse),
    }
}

async fn fetch_snapshot_inner() -> Result<DesktopSnapshot, DesktopClientError> {
    let root = DataRoot::resolve()?;
    let installation = Installation::load_or_create(&root)?;
    let mut connection =
        PipeClient::connect(&installation, "lazoo-desktop", env!("CARGO_PKG_VERSION")).await?;
    let request = snapshot_request(Uuid::new_v4().to_string());
    connection.send_json(&request).await?;
    let response = connection.receive_json().await?;
    parse_snapshot_response(response)
}

pub async fn fetch_snapshot() -> Result<DesktopSnapshot, DesktopFailure> {
    fetch_snapshot_inner()
        .await
        .map_err(|error| DesktopFailure::from(&error))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lazoo_protocol::{DesktopSnapshot, JsonRpcResponse, PROTOCOL_V1, RpcError, SystemStatus};
    use serde_json::json;
    use uuid::Uuid;

    use super::{DesktopFailure, parse_snapshot_response, snapshot_request};

    fn snapshot() -> DesktopSnapshot {
        DesktopSnapshot {
            captured_at: "2026-08-28T00:00:00Z".to_owned(),
            status: SystemStatus {
                daemon_version: "0.1.0".to_owned(),
                boot_id: Uuid::nil(),
                uptime_millis: 1000,
                protocol_version: PROTOCOL_V1,
                data_root: PathBuf::from(r"C:\Temp\Lazoo"),
                schema_version: 1,
                journal_mode: "wal".to_owned(),
                project_count: 0,
                event_high_water: 0,
                pending_outbox: 0,
                emergency_incident_count: 0,
                keeper: None,
                degraded: Vec::new(),
            },
            projects: Vec::new(),
            events: Vec::new(),
        }
    }

    #[test]
    fn snapshot_request_cannot_select_an_arbitrary_method() {
        let request = snapshot_request("desktop-test");

        assert_eq!(request.id, "desktop-test");
        assert_eq!(request.method, "desktop.snapshot");
        assert_eq!(request.params, json!({}));
    }

    #[test]
    fn valid_snapshot_response_decodes_typed_result() {
        let expected = snapshot();
        let response = JsonRpcResponse::success(
            "desktop-test",
            serde_json::to_value(&expected).expect("snapshot serializes"),
        );

        assert_eq!(
            parse_snapshot_response(response).expect("snapshot response"),
            expected
        );
    }

    #[test]
    fn rpc_failure_preserves_public_code_and_message() {
        let error = parse_snapshot_response(JsonRpcResponse::failure(
            "desktop-test",
            RpcError {
                code: "LZ-STORE".to_owned(),
                message: "store unavailable".to_owned(),
                data: None,
            },
        ))
        .expect_err("RPC error must fail");

        let public = DesktopFailure::from(&error);
        assert_eq!(public.code, "LZ-STORE");
        assert_eq!(public.message, "store unavailable");
    }

    #[test]
    fn invalid_response_is_redacted() {
        let error = parse_snapshot_response(JsonRpcResponse {
            jsonrpc: "2.0".to_owned(),
            id: "desktop-test".to_owned(),
            result: None,
            error: None,
        })
        .expect_err("invalid shape must fail");

        let public = DesktopFailure::from(&error);
        assert_eq!(public.code, "LZ-DESKTOP-INVALID-RESPONSE");
        assert!(!public.message.contains("nonce"));
    }
}
