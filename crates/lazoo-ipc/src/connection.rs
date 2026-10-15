use std::{ffi::c_void, io, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use lazoo_protocol::PROTOCOL_V1;
use lazoo_runtime::{Installation, OwnerSecurity};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;

use crate::{
    IpcError, ProofFields, compute_proof, read_frame,
    security::{PipeSecurity, effective_sddl, verify_client_owner},
    verify_proof, write_frame,
};

trait AsyncPipe: AsyncRead + AsyncWrite {}
impl<T> AsyncPipe for T where T: AsyncRead + AsyncWrite {}

pub struct AuthenticatedConnection {
    stream: Box<dyn AsyncPipe + Send + Unpin>,
    protocol_version: u32,
}

impl std::fmt::Debug for AuthenticatedConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedConnection")
            .field("protocol_version", &self.protocol_version)
            .finish_non_exhaustive()
    }
}

impl AuthenticatedConnection {
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), IpcError> {
        write_frame(&mut self.stream, bytes).await
    }

    pub async fn receive_bytes(&mut self, maximum: usize) -> Result<Vec<u8>, IpcError> {
        crate::read_frame_bounded(&mut self.stream, maximum).await
    }

    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> Result<(), IpcError> {
        let payload = serde_json::to_vec(value)?;
        write_frame(&mut self.stream, &payload).await
    }

    pub async fn receive_json<T: DeserializeOwned>(&mut self) -> Result<T, IpcError> {
        let payload = read_frame(&mut self.stream).await?;
        Ok(serde_json::from_slice(&payload)?)
    }

    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }
}

pub struct PipeServer {
    pending: Option<NamedPipeServer>,
    pipe_name: String,
    security: Arc<PipeSecurity>,
    owner_sid: String,
    installation_id: Uuid,
    nonce: Vec<u8>,
}

/// An accepted pipe whose peer has not yet authenticated. Keeping authentication separate
/// lets the listener accept healthy clients while another peer is slow or malformed.
pub struct PendingConnection {
    stream: NamedPipeServer,
    owner_sid: String,
    installation_id: Uuid,
    nonce: Vec<u8>,
}

impl PendingConnection {
    pub async fn authenticate(self) -> Result<AuthenticatedConnection, IpcError> {
        verify_client_owner(&self.stream, &self.owner_sid)?;
        timeout(
            Duration::from_secs(5),
            server_handshake(self.stream, self.installation_id, &self.nonce),
        )
        .await
        .map_err(|_| {
            IpcError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "pipe handshake timed out",
            ))
        })?
    }
}

impl PipeServer {
    pub fn bind(installation: &Installation, owner: &OwnerSecurity) -> Result<Self, IpcError> {
        Self::bind_name(installation, owner, installation.pipe_name().to_owned())
    }

    /// Terminal bytes use a distinct listener and connection budget from control RPCs.
    pub fn bind_terminal(
        installation: &Installation,
        owner: &OwnerSecurity,
    ) -> Result<Self, IpcError> {
        Self::bind_name(
            installation,
            owner,
            format!("{}.terminal.v1", installation.pipe_name()),
        )
    }

    fn bind_name(
        installation: &Installation,
        owner: &OwnerSecurity,
        pipe_name: String,
    ) -> Result<Self, IpcError> {
        let security = Arc::new(PipeSecurity::new(owner)?);
        let pending = create_server(&pipe_name, &security, true)?;
        Ok(Self {
            pending: Some(pending),
            pipe_name,
            security,
            owner_sid: owner.sid().to_owned(),
            installation_id: installation.id(),
            nonce: installation.unprotect_nonce()?,
        })
    }

    pub async fn accept(&mut self) -> Result<AuthenticatedConnection, IpcError> {
        self.accept_pending().await?.authenticate().await
    }

    /// Cancellation before a connection arrives leaves the listening instance intact.
    pub async fn accept_pending(&mut self) -> Result<PendingConnection, IpcError> {
        loop {
            let listening = self.pending.as_ref().ok_or_else(|| {
                IpcError::Io(io::Error::other("pipe listener has no pending instance"))
            })?;
            let result = listening.connect().await;
            let connected = self.pending.take().expect("listening instance");
            self.pending = Some(create_server(&self.pipe_name, &self.security, false)?);
            match result {
                Ok(()) => {
                    return Ok(PendingConnection {
                        stream: connected,
                        owner_sid: self.owner_sid.clone(),
                        installation_id: self.installation_id,
                        nonce: self.nonce.clone(),
                    });
                }
                // A peer may open and close before ConnectNamedPipe completes. That consumes
                // this pipe instance, not the listener; keep the fresh instance for the next peer.
                Err(error) if matches!(error.raw_os_error(), Some(109 | 232 | 233)) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub fn effective_sddl(&self) -> Result<String, IpcError> {
        let pending = self.pending.as_ref().ok_or_else(|| {
            IpcError::Io(io::Error::other("pipe listener has no pending instance"))
        })?;
        effective_sddl(pending)
    }
}

pub struct PipeClient;

impl PipeClient {
    pub async fn connect_terminal(
        installation: &Installation,
        client_name: &str,
        client_version: &str,
    ) -> Result<AuthenticatedConnection, IpcError> {
        Self::connect_using(
            &format!("{}.terminal.v1", installation.pipe_name()),
            installation.id(),
            &installation.unprotect_nonce()?,
            client_name,
            client_version,
            &[PROTOCOL_V1],
        )
        .await
    }

    pub async fn connect(
        installation: &Installation,
        client_name: &str,
        client_version: &str,
    ) -> Result<AuthenticatedConnection, IpcError> {
        let nonce = installation.unprotect_nonce()?;
        Self::connect_using(
            installation.pipe_name(),
            installation.id(),
            &nonce,
            client_name,
            client_version,
            &[PROTOCOL_V1],
        )
        .await
    }

    pub async fn connect_using(
        pipe_name: &str,
        installation_id: Uuid,
        nonce: &[u8],
        client_name: &str,
        client_version: &str,
        offered_versions: &[u32],
    ) -> Result<AuthenticatedConnection, IpcError> {
        let stream = open_client(pipe_name).await?;
        timeout(
            Duration::from_secs(5),
            client_handshake(
                stream,
                installation_id,
                nonce,
                client_name,
                client_version,
                offered_versions,
            ),
        )
        .await
        .map_err(|_| {
            IpcError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "server handshake timed out",
            ))
        })?
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ServerHello {
    challenge: String,
    supported_versions: Vec<u32>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeRequest {
    installation_id: Uuid,
    client_name: String,
    client_version: String,
    offered_versions: Vec<u32>,
    proof: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeResponse {
    accepted: bool,
    selected_version: Option<u32>,
    error_code: Option<String>,
}

async fn server_handshake(
    mut stream: NamedPipeServer,
    installation_id: Uuid,
    nonce: &[u8],
) -> Result<AuthenticatedConnection, IpcError> {
    let challenge = challenge_bytes();
    let hello = ServerHello {
        challenge: STANDARD.encode(challenge),
        supported_versions: vec![PROTOCOL_V1],
    };
    send_json(&mut stream, &hello).await?;
    let request: InitializeRequest = receive_json(&mut stream).await?;
    let proof = STANDARD.decode(&request.proof)?;
    let fields = ProofFields {
        challenge: &challenge,
        installation_id: &request.installation_id.to_string(),
        client_name: &request.client_name,
        client_version: &request.client_version,
        offered_versions: &request.offered_versions,
    };
    let compatible = request.offered_versions.contains(&PROTOCOL_V1);
    let authenticated =
        request.installation_id == installation_id && verify_proof(nonce, &fields, &proof);
    if !compatible || !authenticated {
        let error = if compatible {
            IpcError::Authentication
        } else {
            IpcError::ProtocolIncompatible
        };
        let response = InitializeResponse {
            accepted: false,
            selected_version: None,
            error_code: Some(error.code().to_owned()),
        };
        let _ = send_json(&mut stream, &response).await;
        return Err(error);
    }
    send_json(
        &mut stream,
        &InitializeResponse {
            accepted: true,
            selected_version: Some(PROTOCOL_V1),
            error_code: None,
        },
    )
    .await?;
    Ok(AuthenticatedConnection {
        stream: Box::new(stream),
        protocol_version: PROTOCOL_V1,
    })
}

async fn client_handshake(
    mut stream: NamedPipeClient,
    installation_id: Uuid,
    nonce: &[u8],
    client_name: &str,
    client_version: &str,
    offered_versions: &[u32],
) -> Result<AuthenticatedConnection, IpcError> {
    let hello: ServerHello = receive_json(&mut stream).await?;
    let challenge = STANDARD.decode(&hello.challenge)?;
    let _server_has_compatible_version = hello
        .supported_versions
        .iter()
        .any(|version| offered_versions.contains(version));
    let installation_text = installation_id.to_string();
    let fields = ProofFields {
        challenge: &challenge,
        installation_id: &installation_text,
        client_name,
        client_version,
        offered_versions,
    };
    let proof = STANDARD.encode(compute_proof(nonce, &fields)?);
    send_json(
        &mut stream,
        &InitializeRequest {
            installation_id,
            client_name: client_name.to_owned(),
            client_version: client_version.to_owned(),
            offered_versions: offered_versions.to_vec(),
            proof,
        },
    )
    .await?;
    let response: InitializeResponse = receive_json(&mut stream).await?;
    if !response.accepted {
        return match response.error_code.as_deref() {
            Some("LZ-PROTOCOL-INCOMPATIBLE") => Err(IpcError::ProtocolIncompatible),
            _ => Err(IpcError::Authentication),
        };
    }
    let protocol_version = response
        .selected_version
        .ok_or(IpcError::ProtocolIncompatible)?;
    Ok(AuthenticatedConnection {
        stream: Box::new(stream),
        protocol_version,
    })
}

fn create_server(
    name: &str,
    security: &PipeSecurity,
    first: bool,
) -> Result<NamedPipeServer, IpcError> {
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    let server = unsafe {
        options.create_with_security_attributes_raw(name, security.as_raw().cast::<c_void>())
    }?;
    Ok(server)
}

async fn open_client(name: &str) -> Result<NamedPipeClient, IpcError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(error) if Instant::now() < deadline && is_retryable_open_error(&error) => {
                sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(IpcError::Io(error)),
        }
    }
}

async fn send_json<S, T>(stream: &mut S, value: &T) -> Result<(), IpcError>
where
    S: AsyncWrite + Unpin,
    T: Serialize,
{
    write_frame(stream, &serde_json::to_vec(value)?).await
}

async fn receive_json<S, T>(stream: &mut S) -> Result<T, IpcError>
where
    S: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    Ok(serde_json::from_slice(&read_frame(stream).await?)?)
}

fn challenge_bytes() -> [u8; 32] {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut challenge = [0_u8; 32];
    challenge[..16].copy_from_slice(first.as_bytes());
    challenge[16..].copy_from_slice(second.as_bytes());
    challenge
}

fn is_retryable_open_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error() == Some(windows::Win32::Foundation::ERROR_PIPE_BUSY.0.cast_signed())
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::is_retryable_open_error;

    #[test]
    fn client_retries_busy_and_not_found_pipe_errors() {
        let busy = io::Error::from_raw_os_error(
            windows::Win32::Foundation::ERROR_PIPE_BUSY.0.cast_signed(),
        );
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);

        assert!(is_retryable_open_error(&busy));
        assert!(is_retryable_open_error(&missing));
        assert!(!is_retryable_open_error(&denied));
    }
}
