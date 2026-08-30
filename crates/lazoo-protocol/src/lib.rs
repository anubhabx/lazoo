mod rpc;
mod types;

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
