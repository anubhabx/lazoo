mod message;
mod rpc;
mod types;

pub use message::{
    AcknowledgeMessageParams, CancelMessageParams, ClaimMessageParams, FailMessageParams,
    MESSAGE_ENVELOPE_V1, MessageActivationIntent, MessageDeadLetter, MessageDelivery,
    MessageDeliveryAttempt, MessageDeliveryState, MessageEndpoint, MessageEnvelope, MessageKind,
    MessageListParams, MessageMutationResult, MessageReconcileResult, MessageRecord,
    MessageReference, MessageShowParams, MessageSummary, RetryMessageParams, SendMessageParams,
};
pub use rpc::{JsonRpcRequest, JsonRpcResponse, RpcError};
pub use types::{
    AuthorityMode, CheckoutMode, CreateEmployeeParams, DesktopSnapshot, DisableEmployeeParams,
    EmployeeAvailability, EmployeeCheckoutPolicy, EmployeeMutationResult, EmployeeRecord,
    EmployeeSessionRecord, EmployeeShowParams, EventSummary, EventsListParams, ProjectRecord,
    ProviderListEntry, ProviderObservation, ProviderProbeParams, ProviderSmokeParams,
    ProviderSmokeResult, RegisterProjectParams, RegisterProjectResult, RosterEntry,
    RosterRefreshParams, RosterRefreshResult, RosterSnapshot, SessionLifecycleState,
    SessionMutationResult, SessionReconcileResult, SessionShowParams, StopSessionParams,
    SystemStatus, UpdateEmployeeParams, WarmSessionParams,
};

pub const PROTOCOL_V1: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
