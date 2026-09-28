mod approval;
mod conversation;
mod incident;
mod message;
mod rpc;
mod task;
mod types;

pub use approval::{
    APPROVAL_CONTRACT_V1, ApprovalDecider, ApprovalDecision, ApprovalKind, ApprovalListParams,
    ApprovalMutationResult, ApprovalReconcileResult, ApprovalRecord, ApprovalShowParams,
    ApprovalState, DecideApprovalParams,
};
pub use conversation::{
    CONVERSATION_CONTRACT_V1, CancelTurnParams, ConversationDetail, ConversationListParams,
    ConversationMutationResult, ConversationRecord, ConversationShowParams, ConversationSummary,
    ConversationTurnEvent, ConversationTurnRecord, ConversationTurnSummary,
    CreateConversationParams, RetryTurnParams, SubmitTurnParams, TurnListParams,
    TurnMutationResult, TurnReconcileResult, TurnShowParams, TurnState,
};
pub use incident::{
    AcknowledgeIncidentParams, INCIDENT_CONTRACT_V1, IncidentListParams, IncidentMutationResult,
    IncidentPriority, IncidentRecord, IncidentShowParams, IncidentSource, IncidentState,
    ResolveIncidentParams,
};
pub use message::{
    AcknowledgeMessageParams, CancelMessageParams, ClaimMessageParams, FailMessageParams,
    MESSAGE_ENVELOPE_V1, MESSAGE_ENVELOPE_V2, MessageActivationIntent, MessageDeadLetter,
    MessageDelivery, MessageDeliveryAttempt, MessageDeliveryState, MessageEndpoint,
    MessageEnvelope, MessageKind, MessageListParams, MessageMutationResult, MessageReconcileResult,
    MessageRecord, MessageReference, MessageSender, MessageShowParams, MessageSummary,
    RetryMessageParams, SendMessageParams, UserMessageEndpoint, UserMessageKind,
};
pub use rpc::{JsonRpcRequest, JsonRpcResponse, RpcError};
pub use task::{
    AcceptTaskParams, CancelTaskParams, DelegateTaskParams, ReportTaskParams, ResolveTaskParams,
    StartTaskParams, TASK_CONTRACT_V1, TaskDetail, TaskDispatchFailureResult, TaskDispatchRecord,
    TaskDispatchState, TaskEvidence, TaskEvidenceKind, TaskListParams, TaskMutationResult,
    TaskReconcileResult, TaskRecord, TaskReportKind, TaskResolution, TaskRole, TaskShowParams,
    TaskState, TaskSummary, TaskWorkspaceKind,
};
pub use types::{
    ActivationPolicy, AuthorityMode, CheckoutMode, CreateEmployeeParams, DesktopSnapshot,
    DisableEmployeeParams, EmployeeAvailability, EmployeeCheckoutPolicy, EmployeeMutationResult,
    EmployeeRecord, EmployeeSessionRecord, EmployeeShowParams, EventSummary, EventsListParams,
    ProjectRecord, ProviderListEntry, ProviderObservation, ProviderProbeParams,
    ProviderSmokeParams, ProviderSmokeResult, RegisterProjectParams, RegisterProjectResult,
    RosterEntry, RosterRefreshParams, RosterRefreshResult, RosterSnapshot, SessionLifecycleState,
    SessionMutationResult, SessionReconcileResult, SessionShowParams, StopSessionParams,
    SystemStatus, UpdateEmployeeParams, WarmSessionParams,
};

pub const PROTOCOL_V1: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
