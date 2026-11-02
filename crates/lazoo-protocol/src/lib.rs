mod accounts;
mod approval;
mod conversation;
mod inbox;
mod incident;
mod memory;
mod message;
mod native_trust;
mod rpc;
mod task;
mod terminal;
mod terminal_stream;
mod types;
mod workspace;

pub use accounts::*;
pub use approval::{
    APPROVAL_CONTRACT_V1, ApprovalDecider, ApprovalDecision, ApprovalKind, ApprovalListParams,
    ApprovalMutationResult, ApprovalReconcileResult, ApprovalRecord, ApprovalShowParams,
    ApprovalState, DecideApprovalParams,
};
pub use conversation::{
    CONVERSATION_CONTRACT_V1, CancelTurnParams, ConversationDetail, ConversationListParams,
    ConversationMutationResult, ConversationRecord, ConversationShowParams, ConversationSummary,
    ConversationTurnEvent, ConversationTurnRecord, ConversationTurnSummary,
    CreateConversationParams, RenameConversationParams, RetryTurnParams, SubmitTurnParams,
    TurnListParams, TurnMutationResult, TurnReconcileResult, TurnShowParams, TurnState,
};
pub use inbox::{
    INBOX_CONTRACT_V1, InboxAction, InboxAttention, InboxCounts, InboxCountsResult, InboxDeadline,
    InboxDeadlineKind, InboxFilter, InboxItem, InboxKind, InboxListParams, InboxListResult,
    InboxScope, InboxSeverity,
};
pub use incident::{
    AcknowledgeIncidentParams, INCIDENT_CONTRACT_V1, IncidentEscalation, IncidentEscalationReason,
    IncidentListParams, IncidentMutationResult, IncidentPriority, IncidentRecord,
    IncidentRemedyKind, IncidentRemedyRecord, IncidentShowParams, IncidentSource, IncidentState,
    IncidentThresholdMutationResult, IncidentThresholdRecord, IncidentThresholdSource,
    IncidentUserNotification, ResetIncidentThresholdParams, ResolveIncidentParams,
    SetIncidentThresholdParams,
};
pub use memory::{
    MEMORY_CONTRACT_V1, MemoryCleanupPreview, MemoryContextPack, MemoryPolicy, MemoryQueueState,
    MemoryRecord, MemoryRecordType, MemorySearchParams, MemorySourceArchive, MemorySummaryJob,
};
pub use message::{
    AcknowledgeMessageParams, CancelMessageParams, ClaimMessageParams, FailMessageParams,
    MESSAGE_ENVELOPE_V1, MESSAGE_ENVELOPE_V2, MessageActivationIntent, MessageDeadLetter,
    MessageDelivery, MessageDeliveryAttempt, MessageDeliveryState, MessageEndpoint,
    MessageEnvelope, MessageKind, MessageListParams, MessageMutationResult, MessagePriority,
    MessageReconcileResult, MessageRecord, MessageReference, MessageSender, MessageShowParams,
    MessageSummary, RetryMessageParams, SendMessageParams, UserMessageEndpoint, UserMessageKind,
};
pub use native_trust::{
    NATIVE_TRUST_CONTRACT_V1, NativeTrustDecider, NativeTrustDecision, NativeTrustListParams,
    NativeTrustMutationResult, NativeTrustNotificationState, NativeTrustPolicy,
    NativeTrustPolicyParams, NativeTrustReconcileResult, NativeTrustRequestRecord,
    NativeTrustShowParams, NativeTrustState, ResetNativeTrustPolicyParams,
    ResolveNativeTrustParams, SetNativeTrustPolicyParams,
};
pub use rpc::{JsonRpcRequest, JsonRpcResponse, RpcError};
pub use task::{
    AcceptTaskParams, CancelTaskParams, DelegateTaskParams, ReportTaskParams, ResolveTaskParams,
    StartTaskParams, TASK_CONTRACT_V1, TaskDetail, TaskDispatchFailureResult, TaskDispatchRecord,
    TaskDispatchState, TaskEvidence, TaskEvidenceKind, TaskListParams, TaskMutationResult,
    TaskReconcileResult, TaskRecord, TaskReportKind, TaskResolution, TaskRole, TaskShowParams,
    TaskState, TaskSummary, TaskWorkspaceKind,
};
pub use terminal::*;
pub use terminal_stream::*;
pub use types::{
    ActivationPolicy, AuthorityMode, CheckoutMode, CreateEmployeeParams, DaemonResources,
    DesktopSnapshot, DisableEmployeeParams, EmployeeAvailability, EmployeeCheckoutPolicy,
    EmployeeMutationResult, EmployeeRecord, EmployeeSessionRecord, EmployeeShowParams,
    EventSummary, EventsListParams, ProjectRecord, ProviderListEntry, ProviderObservation,
    ProviderProbeParams, ProviderSmokeParams, ProviderSmokeResult, RegisterProjectParams,
    RegisterProjectResult, RosterEntry, RosterRefreshParams, RosterRefreshResult, RosterSnapshot,
    SessionLifecycleState, SessionMutationResult, SessionReconcileResult, SessionResources,
    SessionResourcesParams, SessionShowParams, StopSessionParams, SystemStatus,
    UpdateEmployeeParams, WarmSessionParams,
};

pub use workspace::{
    ConversationMarker, ConversationRow, LeadCandidate, LeadHeader, LeadLifecycle,
    LeadSessionSummary, WORKSPACE_CONTRACT_V1, WorkerCard, WorkerCounts, WorkerSessionSummary,
    WorkerState, WorkerWorkspace, WorkspaceConversationParams, WorkspaceConversationResult,
    WorkspaceConversationsParams, WorkspaceConversationsResult, WorkspaceLauncherParams,
    WorkspaceLauncherResult, WorkspaceScope, WorkspaceStartAction,
};

pub const PROTOCOL_V1: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
