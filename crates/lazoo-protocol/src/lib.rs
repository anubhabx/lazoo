mod rpc;
mod types;

pub use rpc::{JsonRpcRequest, JsonRpcResponse, RpcError};
pub use types::{CheckoutMode, EventSummary, ProjectRecord, RegisterProjectResult};

pub const PROTOCOL_V1: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
