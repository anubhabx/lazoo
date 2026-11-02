import type {
  ConversationRow,
  ConversationTurnRecord,
  ConversationTurnSummary,
  EventSummary,
  InboxItem,
  LeadCandidate,
  ProjectRecord,
  SessionTerminalStatusResult,
  SystemStatus,
  TerminalRecord,
  WorkerCard,
  WorkspaceConversationResult,
  WorkspaceConversationsResult,
  WorkspaceLauncherResult,
} from "../lib/protocol";

export const NOW = "2026-10-31T10:00:00Z";
export const LEAD_ID = "11111111-1111-4111-8111-111111111111";
export const WORKER_ID = "22222222-2222-4222-8222-222222222222";
export const CONVERSATION_ID = "33333333-3333-4333-8333-333333333333";
export const PROJECT_ID = "44444444-4444-4444-8444-444444444444";
export const SESSION_ID = "55555555-5555-4555-8555-555555555555";
export const TASK_ID = "66666666-6666-4666-8666-666666666666";
export const ROOT_TASK_ID = "77777777-7777-4777-8777-777777777777";
export const BOOT_ID = "88888888-8888-4888-8888-888888888888";

export function status(overrides: Partial<SystemStatus> = {}): SystemStatus {
  return {
    daemonVersion: "0.1.0",
    bootId: BOOT_ID,
    uptimeMillis: 120_000,
    protocolVersion: 1,
    dataRoot: "C:\\Temp\\Lazoo",
    schemaVersion: 21,
    journalMode: "wal",
    projectCount: 1,
    eventHighWater: 40,
    pendingOutbox: 0,
    emergencyIncidentCount: 0,
    keeper: null,
    degraded: [],
    ...overrides,
  };
}

export function project(): ProjectRecord {
  return {
    id: PROJECT_ID,
    repositoryIdentity: "repo",
    canonicalPath: "C:\\workspace\\demo",
    displayName: "Desktop acceptance",
    checkoutMode: "isolated",
    configurationRevision: 1,
    createdAt: NOW,
  };
}

export function leadCandidate(
  overrides: Partial<LeadCandidate> = {},
): LeadCandidate {
  return {
    employeeId: LEAD_ID,
    name: "mira",
    provider: "codex",
    role: "lead",
    model: "gpt-5.4",
    reasoningEffort: "high",
    defaultAuthority: "ask",
    checkoutPolicy: "isolated",
    availability: "dormant",
    availabilityReason: null,
    authorityModes: ["ask", "self-decision", "full-autonomy"],
    authorityReason: null,
    collaborationToolsAvailable: true,
    nativeViewSupported: true,
    startable: true,
    blockingReason: null,
    configurationRevision: 1,
    ...overrides,
  };
}

export function launcher(
  overrides: Partial<WorkspaceLauncherResult> = {},
): WorkspaceLauncherResult {
  return {
    version: 1,
    project: null,
    rosterRevision: 1,
    leads: [leadCandidate()],
    rememberedLeadId: LEAD_ID,
    rememberedLeadBlockingReason: null,
    authorityModes: ["ask", "self-decision", "full-autonomy"],
    start: [
      {
        method: "conversation.create",
        params: { projectId: null, leadEmployeeId: null, title: null },
        fill: ["leadEmployeeId", "idempotencyKey"],
      },
      {
        method: "turn.submit",
        params: {
          conversationId: null,
          author: { kind: "user" },
          recipient: null,
          authorityMode: null,
          text: null,
          activateIfNeeded: true,
          maxAttempts: 3,
        },
        fill: ["conversationId", "authorityMode", "text", "idempotencyKey"],
      },
    ],
    eventHighWater: 40,
    ...overrides,
  };
}

export function conversationRow(
  overrides: Partial<ConversationRow> = {},
): ConversationRow {
  return {
    id: CONVERSATION_ID,
    revision: 2,
    title: "fix flaky daemon test",
    titleDerived: true,
    leadEmployeeId: LEAD_ID,
    leadName: "mira",
    provider: "codex",
    marker: "active",
    markerReason: "the lead is working",
    unresolved: 0,
    workers: { total: 1, running: 1, waiting: 0, failed: 0, completed: 0 },
    rootTaskState: "running",
    lastTurnState: "accepted",
    lastResultAt: null,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

export function conversations(
  rows: ConversationRow[] = [conversationRow()],
): WorkspaceConversationsResult {
  return {
    version: 1,
    projectId: null,
    conversations: rows,
    rememberedLeadId: LEAD_ID,
    eventHighWater: 40,
    truncated: false,
    nextBefore: null,
  };
}

export function turnSummary(
  overrides: Partial<ConversationTurnSummary> = {},
): ConversationTurnSummary {
  return {
    sequence: 1,
    id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1",
    conversationId: CONVERSATION_ID,
    taskId: null,
    author: { kind: "user" },
    recipient: { employeeId: LEAD_ID, sessionId: SESSION_ID },
    state: "accepted",
    revision: 1,
    nativeTurnId: "t1",
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

export function turnRecord(
  overrides: Partial<ConversationTurnRecord> = {},
): ConversationTurnRecord {
  return {
    ...turnSummary(),
    version: 1,
    recipientEmployeeConfigurationRevision: 1,
    text: "Fix the flaky daemon test without widening timeouts.",
    messageId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1",
    messageAttemptId: null,
    correlationId: "cccccccc-cccc-4ccc-8ccc-ccccccccccc1",
    ownerBootId: BOOT_ID,
    provider: "codex",
    nativeSessionId: "thread-1",
    failureCode: null,
    failureMessage: null,
    transcriptTruncated: false,
    submittedAt: NOW,
    acceptedAt: NOW,
    completedAt: null,
    cancelRequestedAt: null,
    terminalAt: null,
    events: [
      {
        sequence: 1,
        kind: "text-delta",
        provider: "codex",
        nativeSessionId: "thread-1",
        nativeTurnId: "t1",
        payload: { text: "Reproduced locally in 4 of 20 runs." },
        createdAt: NOW,
      },
      {
        sequence: 2,
        kind: "tool-update",
        provider: "codex",
        nativeSessionId: "thread-1",
        nativeTurnId: "t1",
        payload: { toolName: "task_delegate", status: "completed" },
        createdAt: NOW,
      },
    ],
    ...overrides,
  };
}

export function workerCard(overrides: Partial<WorkerCard> = {}): WorkerCard {
  return {
    taskId: TASK_ID,
    revision: 1,
    rootTaskId: ROOT_TASK_ID,
    parentTaskId: ROOT_TASK_ID,
    depth: 1,
    employeeId: WORKER_ID,
    name: "rune",
    provider: "claude-code",
    model: "claude-opus-5-5",
    label: "Reproduce across 20 runs",
    state: "running",
    stateReason: "the worker's turn is accepted",
    taskState: "running",
    effectiveAuthority: "ask",
    session: {
      id: "99999999-9999-4999-8999-999999999999",
      state: "idle",
      account: null,
    },
    activeTurnId: null,
    workspace: {
      kind: "git-worktree",
      path: "C:\\workspace\\demo\\.lazoo\\wt",
      branchName: "task/rune",
      baseCommit: "abcdef0123456789",
    },
    failureCode: null,
    failureMessage: null,
    createdAt: NOW,
    updatedAt: NOW,
    terminalAt: null,
    actions: [
      {
        action: "cancel",
        label: "Cancel",
        method: "task.cancel",
        params: { id: TASK_ID, expectedRevision: 1 },
      },
    ],
    ...overrides,
  };
}

export function inboxItem(overrides: Partial<InboxItem> = {}): InboxItem {
  return {
    kind: "approval",
    id: "dddddddd-dddd-4ddd-8ddd-ddddddddddd1",
    revision: 3,
    attention: "action-required",
    severity: "normal",
    subtype: "command-execution",
    title: "wants to run cargo test --workspace",
    requiredAction: "Approve once or decline",
    state: "pending",
    scope: {
      projectId: null,
      conversationId: CONVERSATION_ID,
      employeeId: WORKER_ID,
      employeeName: "rune",
      provider: "claude-code",
      taskId: TASK_ID,
      turnId: null,
      sessionId: null,
    },
    createdAt: NOW,
    updatedAt: NOW,
    deadline: { kind: "expires", at: "2026-10-31T10:15:00Z" },
    detail: {},
    actions: [
      {
        action: "approve",
        label: "Approve once",
        method: "approval.decide",
        params: {
          id: "dddddddd-dddd-4ddd-8ddd-ddddddddddd1",
          expectedRevision: 3,
          decision: "approve",
        },
      },
      {
        action: "decline",
        label: "Decline",
        method: "approval.decide",
        params: {
          id: "dddddddd-dddd-4ddd-8ddd-ddddddddddd1",
          expectedRevision: 3,
          decision: "decline",
        },
      },
    ],
    ...overrides,
  };
}

export function conversation(
  overrides: Partial<WorkspaceConversationResult> = {},
): WorkspaceConversationResult {
  return {
    version: 1,
    conversation: {
      sequence: 1,
      version: 1,
      id: CONVERSATION_ID,
      projectId: null,
      leadEmployeeId: LEAD_ID,
      leadEmployeeConfigurationRevision: 1,
      title: null,
      revision: 2,
      createdAt: NOW,
      updatedAt: NOW,
    },
    title: "fix flaky daemon test",
    titleDerived: true,
    scope: {
      projectId: null,
      projectName: null,
      canonicalPath: null,
      checkoutMode: null,
    },
    lead: {
      employeeId: LEAD_ID,
      name: "mira",
      role: "lead",
      provider: "codex",
      model: "gpt-5.4",
      reasoningEffort: "high",
      availability: "idle",
      availabilityReason: null,
      authority: "ask",
      session: {
        account: null,
        id: SESSION_ID,
        state: "idle",
        nativeHostPid: 4242,
        workingDirectory: "C:\\Temp\\Lazoo\\sessions\\s1",
      },
      lifecycle: "busy",
      activeTurnId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1",
      actions: [
        {
          action: "rename",
          label: "Rename",
          method: "conversation.rename",
          params: { id: CONVERSATION_ID, expectedRevision: 2 },
        },
        {
          action: "cancel-turn",
          label: "Cancel turn",
          method: "turn.cancel",
          params: {
            id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1",
            expectedRevision: 1,
          },
        },
      ],
    },
    rootTask: {
      sequence: 1,
      id: ROOT_TASK_ID,
      rootTaskId: ROOT_TASK_ID,
      parentTaskId: null,
      depth: 0,
      projectId: null,
      conversationId: CONVERSATION_ID,
      assignedEmployeeId: LEAD_ID,
      role: "lead",
      objective: "Fix the flaky daemon test",
      requestedAuthority: "ask",
      effectiveAuthority: "ask",
      state: "running",
      revision: 1,
      createdAt: NOW,
      updatedAt: NOW,
      terminalAt: null,
    },
    turns: [turnSummary()],
    turnsTruncated: false,
    nextBeforeTurnSequence: null,
    workers: [],
    attention: [],
    eventHighWater: 40,
    ...overrides,
  };
}

export function terminalRecord(
  overrides: Partial<TerminalRecord> = {},
): TerminalRecord {
  return {
    id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeee1",
    bootId: BOOT_ID,
    pid: 5151,
    executable: "codex",
    args: ["--remote"],
    cwd: "C:\\Temp\\Lazoo\\sessions\\s1",
    columns: 120,
    rows: 36,
    revision: 1,
    live: true,
    exitCode: null,
    ...overrides,
  };
}

export function terminalStatus(
  overrides: Partial<SessionTerminalStatusResult> = {},
): SessionTerminalStatusResult {
  return {
    bootId: BOOT_ID,
    sessionId: SESSION_ID,
    sessionRevision: 3,
    opening: false,
    terminal: null,
    attachable: false,
    blockingCode: "LZ-SESSION-STATE",
    blockingReason: "only suspended sessions attach",
    viewable: true,
    viewBlockingCode: null,
    viewBlockingReason: null,
    viewOpening: false,
    viewTerminal: null,
    ...overrides,
  };
}

export function event(sequence: number, eventType: string): EventSummary {
  return {
    sequence,
    id: `event-${sequence}`,
    eventType,
    payload: {},
    createdAt: NOW,
  };
}
