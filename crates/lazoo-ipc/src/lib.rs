mod auth;
mod connection;
mod frame;
mod security;

pub use auth::{ProofFields, compute_proof, verify_proof};
pub use connection::{AuthenticatedConnection, PendingConnection, PipeClient, PipeServer};
pub use frame::{read_frame, read_frame_bounded, write_frame};

use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("IPC I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("IPC JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC frame length {actual} exceeds maximum {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("IPC authentication failed")]
    Authentication,
    #[error("no compatible protocol version")]
    ProtocolIncompatible,
    #[error("IPC base64 value is invalid: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("runtime security failed: {0}")]
    Runtime(#[from] lazoo_runtime::RuntimeError),
    #[error("Windows IPC operation failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("Windows returned invalid UTF-16 text: {0}")]
    InvalidUtf16(#[from] std::string::FromUtf16Error),
    #[error("Windows IPC operation returned error code {0}")]
    WindowsCode(u32),
    #[error("HMAC key is invalid")]
    InvalidHmacKey,
}

impl IpcError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::FrameTooLarge { .. } => "LZ-IPC-FRAME",
            Self::Authentication => "LZ-IPC-AUTH",
            Self::ProtocolIncompatible => "LZ-PROTOCOL-INCOMPATIBLE",
            Self::Io(_)
            | Self::Json(_)
            | Self::Base64(_)
            | Self::Runtime(_)
            | Self::Windows(_)
            | Self::WindowsCode(_)
            | Self::InvalidHmacKey
            | Self::InvalidUtf16(_) => "LZ-IPC",
        }
    }
}
