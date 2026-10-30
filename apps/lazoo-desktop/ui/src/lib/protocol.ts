// TypeScript mirror of the daemon contract the desktop binds (crates/lazoo-protocol).
// Shapes follow serde: camelCase fields, kebab-case enum values, `null` for Option.

export type Uuid = string;
export type Timestamp = string;

export type AuthorityMode = "ask" | "self-decision" | "full-autonomy";
export type CheckoutMode = "isolated" | "lead-main";
export type EmployeeCheckoutPolicy = "isolated" | "read-only";
export type ActivationPolicy = "manual" | "notice-turns";
export type EmployeeAvailability =
  | "disabled"
  | "offline"
  | "unverified"
  | "dormant"
  | "starting"
  | "idle"
  | "suspended"
  | "failed";
export type SessionLifecycleState =
  "starting" | "idle" | "stopping" | "suspended" | "failed";
export type TurnState =
  | "recorded"
  | "submitting"
  | "accepted"
  | "completed"
  | "retryable-failed"
  | "terminal-failed"
  | "cancelled"
  | "uncertain";
export type TaskState =
  | "proposed"
  | "accepted"
  | "running"
  | "cancel-requested"
  | "completed"
  | "failed"
  | "cancelled"
  | "uncertain";
export type TaskWorkspaceKind = "projectless" | "git-worktree";

export interface RpcFailure {
  code: string;
  message: string;
  data?: Record<string, unknown> | null;
}

export interface SystemStatus {
  daemonVersion: string;
  bootId: Uuid;
  uptimeMillis: number;
  protocolVersion: number;
  dataRoot: string;
  schemaVersion: number;
  journalMode: string;
  projectCount: number;
  eventHighWater: number;
  pendingOutbox: number;
  emergencyIncidentCount: number;
  keeper: unknown;
  degraded: string[];
}

export interface ProjectRecord {
  id: Uuid;
  repositoryIdentity: string;
  canonicalPath: string;
  displayName: string;
  checkoutMode: CheckoutMode;
  configurationRevision: number;
  createdAt: Timestamp;
}

export interface EventSummary {
  sequence: number;
  id: Uuid;
  eventType: string;
  payload: Record<string, unknown>;
  createdAt: Timestamp;
}

export interface EmployeeRecord {
  id: Uuid;
  name: string;
  provider: string;
  role: string;
  instructions: string;
  model: string | null;
  reasoningEffort: string | null;
  nativeSettings: Record<string, unknown>;
  authorityMode: AuthorityMode;
  checkoutPolicy: EmployeeCheckoutPolicy;
  activationPolicy: ActivationPolicy;
  idleTimeoutSeconds: number;
  enabled: boolean;
  configurationRevision: number;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  disabledAt: Timestamp | null;
}

export interface EmployeeSessionRecord {
  id: Uuid;
  employeeId: Uuid;
  employeeConfigurationRevision: number;
  provider: string;
  state: SessionLifecycleState;
  revision: number;
  ownerBootId: Uuid | null;
  nativeSessionId: string | null;
  nativeHostPid: number | null;
  failureCode: string | null;
  failureMessage: string | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  startedAt: Timestamp | null;
  stoppedAt: Timestamp | null;
}

export interface ProviderListEntry {
  provider: string;
  detected: boolean;
  executable: string | null;
  discoverySource: string | null;
}

// ---- Inbox --------------------------------------------------------------

export type InboxFilter =
  "all" | "approvals" | "permissions" | "incidents" | "resolved";
export type InboxKind =
  "approval" | "task-proposal" | "task-uncertain" | "incident" | "permission";
export type InboxAttention =
  "action-required" | "watching" | "acknowledged" | "resolved";
export type InboxSeverity = "priority" | "normal";

export interface InboxDeadline {
  kind: "expires" | "remediation";
  at: Timestamp;
}

export interface InboxScope {
  projectId: Uuid | null;
  conversationId: Uuid | null;
  employeeId: Uuid | null;
  employeeName: string | null;
  provider: string | null;
  taskId: Uuid | null;
  turnId: Uuid | null;
  sessionId: Uuid | null;
}

/** One action that applies now, as data naming a revision-checked RPC. */
export interface InboxAction {
  action: string;
  label: string;
  method: string;
  params: Record<string, unknown>;
}

export interface InboxItem {
  kind: InboxKind;
  id: Uuid;
  revision: number;
  attention: InboxAttention;
  severity: InboxSeverity;
  subtype: string;
  title: string;
  requiredAction: string;
  state: string;
  scope: InboxScope;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  deadline: InboxDeadline | null;
  detail: Record<string, unknown>;
  actions: InboxAction[];
}

export interface InboxCounts {
  unresolved: number;
  approvals: number;
  permissions: number;
  incidents: number;
  watching: number;
}

export interface InboxListResult {
  version: number;
  items: InboxItem[];
  counts: InboxCounts;
  eventHighWater: number;
  truncated: boolean;
  nextBefore: string | null;
}

export interface InboxCountsResult {
  version: number;
  counts: InboxCounts;
  eventHighWater: number;
}

// ---- Workspace ----------------------------------------------------------

export type ConversationMarker =
  "needs-attention" | "active" | "failed" | "completed";

export interface WorkerCounts {
  total: number;
  running: number;
  waiting: number;
  failed: number;
  completed: number;
}

export interface ConversationRow {
  id: Uuid;
  revision: number;
  title: string;
  titleDerived: boolean;
  leadEmployeeId: Uuid;
  leadName: string | null;
  provider: string | null;
  marker: ConversationMarker | null;
  markerReason: string | null;
  unresolved: number;
  workers: WorkerCounts;
  rootTaskState: TaskState | null;
  lastTurnState: TurnState | null;
  lastResultAt: Timestamp | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
}

export interface WorkspaceConversationsResult {
  version: number;
  projectId: Uuid | null;
  conversations: ConversationRow[];
  rememberedLeadId: Uuid | null;
  eventHighWater: number;
  truncated: boolean;
  nextBefore: string | null;
}

export interface LeadCandidate {
  employeeId: Uuid;
  name: string;
  provider: string;
  role: string;
  model: string | null;
  reasoningEffort: string | null;
  defaultAuthority: AuthorityMode;
  checkoutPolicy: EmployeeCheckoutPolicy;
  availability: EmployeeAvailability;
  availabilityReason: string | null;
  authorityModes: AuthorityMode[];
  authorityReason: string | null;
  collaborationToolsAvailable: boolean;
  nativeViewSupported: boolean;
  startable: boolean;
  blockingReason: string | null;
  configurationRevision: number;
}

export interface WorkspaceStartAction {
  method: string;
  params: Record<string, unknown>;
  fill: string[];
}

export interface WorkspaceLauncherResult {
  version: number;
  project: ProjectRecord | null;
  rosterRevision: number;
  leads: LeadCandidate[];
  rememberedLeadId: Uuid | null;
  rememberedLeadBlockingReason: string | null;
  authorityModes: AuthorityMode[];
  start: WorkspaceStartAction[];
  eventHighWater: number;
}

export type LeadLifecycle =
  | "dormant"
  | "starting"
  | "busy"
  | "idle"
  | "stopping"
  | "suspended"
  | "failed"
  | "unknown";

export type WorkerState =
  | "running"
  | "waiting"
  | "needs-approval"
  | "blocked"
  | "failed"
  | "complete"
  | "interrupted"
  | "unknown";

export interface LeadSessionSummary {
  id: Uuid;
  state: SessionLifecycleState;
  nativeHostPid: number | null;
  workingDirectory: string | null;
}

export interface LeadHeader {
  employeeId: Uuid;
  name: string;
  role: string;
  provider: string;
  model: string | null;
  reasoningEffort: string | null;
  availability: EmployeeAvailability;
  availabilityReason: string | null;
  authority: AuthorityMode | null;
  session: LeadSessionSummary | null;
  lifecycle: LeadLifecycle;
  activeTurnId: Uuid | null;
  actions: InboxAction[];
}

export interface WorkspaceScope {
  projectId: Uuid | null;
  projectName: string | null;
  canonicalPath: string | null;
  checkoutMode: CheckoutMode | null;
}

export interface WorkerWorkspace {
  kind: TaskWorkspaceKind | null;
  path: string | null;
  branchName: string | null;
  baseCommit: string | null;
}

export interface WorkerCard {
  taskId: Uuid;
  revision: number;
  rootTaskId: Uuid;
  parentTaskId: Uuid | null;
  depth: number;
  employeeId: Uuid;
  name: string | null;
  provider: string | null;
  model: string | null;
  label: string;
  state: WorkerState;
  stateReason: string;
  taskState: TaskState;
  effectiveAuthority: AuthorityMode;
  session: { id: Uuid; state: SessionLifecycleState } | null;
  activeTurnId: Uuid | null;
  workspace: WorkerWorkspace | null;
  failureCode: string | null;
  failureMessage: string | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  terminalAt: Timestamp | null;
  actions: InboxAction[];
}

export interface ConversationRecord {
  sequence: number;
  version: number;
  id: Uuid;
  projectId: Uuid | null;
  leadEmployeeId: Uuid;
  leadEmployeeConfigurationRevision: number;
  title: string | null;
  revision: number;
  createdAt: Timestamp;
  updatedAt: Timestamp;
}

export type MessageSender =
  { employeeId: Uuid; sessionId: Uuid | null } | { kind: "user" };

export interface MessageEndpoint {
  employeeId: Uuid;
  sessionId: Uuid | null;
}

export interface ConversationTurnSummary {
  sequence: number;
  id: Uuid;
  conversationId: Uuid;
  taskId: Uuid | null;
  author: MessageSender;
  recipient: MessageEndpoint;
  state: TurnState;
  revision: number;
  nativeTurnId: string | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
}

export interface ConversationTurnEvent {
  sequence: number;
  kind: string;
  provider: string;
  nativeSessionId: string | null;
  nativeTurnId: string | null;
  payload: Record<string, unknown>;
  createdAt: Timestamp;
}

export interface ConversationTurnRecord extends ConversationTurnSummary {
  version: number;
  recipientEmployeeConfigurationRevision: number;
  text: string;
  messageId: Uuid;
  messageAttemptId: Uuid | null;
  correlationId: Uuid;
  ownerBootId: Uuid | null;
  provider: string | null;
  nativeSessionId: string | null;
  failureCode: string | null;
  failureMessage: string | null;
  transcriptTruncated: boolean;
  submittedAt: Timestamp | null;
  acceptedAt: Timestamp | null;
  completedAt: Timestamp | null;
  cancelRequestedAt: Timestamp | null;
  terminalAt: Timestamp | null;
  events: ConversationTurnEvent[];
}

export interface TaskSummary {
  sequence: number;
  id: Uuid;
  rootTaskId: Uuid;
  parentTaskId: Uuid | null;
  depth: number;
  projectId: Uuid | null;
  conversationId: Uuid;
  assignedEmployeeId: Uuid;
  role: string;
  objective: string;
  requestedAuthority: AuthorityMode;
  effectiveAuthority: AuthorityMode;
  state: TaskState;
  revision: number;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  terminalAt: Timestamp | null;
}

export interface WorkspaceConversationResult {
  version: number;
  conversation: ConversationRecord;
  title: string;
  titleDerived: boolean;
  scope: WorkspaceScope;
  lead: LeadHeader;
  rootTask: TaskSummary | null;
  turns: ConversationTurnSummary[];
  turnsTruncated: boolean;
  nextBeforeTurnSequence: number | null;
  workers: WorkerCard[];
  attention: InboxItem[];
  eventHighWater: number;
}

// ---- Terminals ----------------------------------------------------------

export interface TerminalRecord {
  id: Uuid;
  bootId: Uuid;
  pid: number;
  executable: string;
  args: string[];
  cwd: string;
  columns: number;
  rows: number;
  revision: number;
  live: boolean;
  exitCode: number | null;
}

export interface SessionTerminalStatusResult {
  bootId: Uuid;
  sessionId: Uuid;
  sessionRevision: number;
  opening: boolean;
  terminal: TerminalRecord | null;
  attachable: boolean;
  blockingCode: string | null;
  blockingReason: string | null;
  viewable: boolean;
  viewBlockingCode: string | null;
  viewBlockingReason: string | null;
  viewOpening: boolean;
  viewTerminal: TerminalRecord | null;
}

export interface SessionTerminalOpenResult {
  sessionId: Uuid;
  terminal: TerminalRecord;
}

// ---- Native trust -------------------------------------------------------

export type NativeTrustState =
  "pending" | "approved" | "declined" | "cancelled";

export interface NativeTrustRequestRecord {
  sequence: number;
  version: number;
  id: Uuid;
  provider: string;
  canonicalWorkspacePath: string;
  managedWorkspace: boolean;
  sessionId: Uuid;
  employeeId: Uuid;
  ownerBootId: Uuid;
  state: NativeTrustState;
  decider: "user" | "policy" | "system" | null;
  revision: number;
  notificationState: "pending" | "claimed" | "settled";
  notificationClaimId: Uuid | null;
  createdAt: Timestamp;
  updatedAt: Timestamp;
  decidedAt: Timestamp | null;
}

export interface NativeTrustPolicy {
  provider: string;
  autoAcceptManagedNextTime: boolean;
  revision: number;
  updatedAt: Timestamp;
}

// ---- Resources ----------------------------------------------------------

export interface DaemonResources {
  pid: number;
  bootId: Uuid;
  uptimeMillis: number;
  cpuMillis: number;
  userCpuMillis: number;
  kernelCpuMillis: number;
  workingSetBytes: number;
  peakWorkingSetBytes: number;
  handleCount: number;
  liveSessions: number;
  sampledAt: Timestamp;
}

export interface SessionResources {
  sessionId: Uuid;
  employeeId: Uuid;
  provider: string;
  nativeHostPid: number | null;
  activeProcesses: number;
  totalProcesses: number;
  cpuMillis: number;
  userCpuMillis: number;
  kernelCpuMillis: number;
  peakProcessMemoryBytes: number;
  peakJobMemoryBytes: number;
  sampledAt: Timestamp;
}

// ---- Frames from the Rust side -----------------------------------------

export type EventFrame =
  { kind: "event"; event: EventSummary } | { kind: "closed"; reason: string };

export type TerminalFrame =
  | {
      kind: "output";
      start: number;
      end: number;
      gap: boolean;
      live: boolean;
      exitCode: number | null;
      data: string;
    }
  | { kind: "closed"; reason: string };
