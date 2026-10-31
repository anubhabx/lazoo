// Development only: a scripted daemon so the UI can be looked at in a browser
// (`vite dev` + `?mock=1`). Never bundled into production (guarded by DEV).

import type { Bridge, TerminalStreamHandle } from "./bridge";
import type {
  EventFrame,
  EventSummary,
  InboxItem,
  LeadCandidate,
  ProjectRecord,
  TerminalFrame,
  WorkerCard,
  WorkspaceConversationResult,
} from "./protocol";
import {
  CONVERSATION_ID,
  SESSION_ID,
  conversation,
  conversations,
  conversationRow,
  inboxItem,
  launcher,
  leadCandidate,
  PROJECT_ID,
  project,
  status,
  terminalRecord,
  terminalStatus,
  turnRecord,
  turnSummary,
  workerCard,
} from "../test/fixtures";

export function createDevBridge(): Bridge {
  let sequence = 40;
  const listeners = new Set<(frame: EventFrame) => void>();
  const workers: WorkerCard[] = [];
  const attention: InboxItem[] = [];
  let turns = [turnSummary()];
  const trust: never[] = [];
  const liveTerminal = terminalRecord();
  // `?mock=1&fresh=1` starts with no employees to show first-run setup.
  const fresh = window.location.search.includes("fresh");
  const minutesAgo = (minutes: number) =>
    new Date(Date.now() - minutes * 60_000).toISOString();
  const leads: LeadCandidate[] = fresh
    ? []
    : [
        leadCandidate(),
        leadCandidate({
          employeeId: "bbbbbbbb-0000-4000-8000-000000000001",
          name: "rune",
          provider: "claude-code",
          role: "frontend",
          model: "sonnet",
          reasoningEffort: "medium",
          availability: "idle",
        }),
        leadCandidate({
          employeeId: "bbbbbbbb-0000-4000-8000-000000000002",
          name: "kit",
          provider: "antigravity",
          role: "research",
          model: null,
          reasoningEffort: null,
        }),
      ];
  const projects: ProjectRecord[] = [
    project(),
    {
      ...project(),
      id: "44444444-4444-4444-8444-444444444445",
      canonicalPath: "C:\\workspace\\lazoo-web",
      displayName: "lazoo-web",
    },
  ];
  const projectRows = [
    conversationRow({
      id: "33333333-3333-4333-8333-333333333336",
      title: "Sidebar keyboard navigation",
      marker: "needs-attention",
      updatedAt: minutesAgo(12),
    }),
    conversationRow({
      id: "33333333-3333-4333-8333-333333333337",
      title: "Migrate settings to the new tokens",
      marker: "completed",
      lastResultAt: minutesAgo(90),
      updatedAt: minutesAgo(90),
    }),
    conversationRow({
      id: "33333333-3333-4333-8333-333333333338",
      title: "Audit bundle size",
      marker: null,
      workers: { total: 0, running: 0, waiting: 0, failed: 0, completed: 0 },
      updatedAt: minutesAgo(60 * 26),
    }),
  ];

  function emit(eventType: string) {
    sequence += 1;
    const event: EventSummary = {
      sequence,
      id: `dev-${sequence}`,
      eventType,
      payload: {},
      createdAt: new Date().toISOString(),
    };
    for (const listener of listeners) listener({ kind: "event", event });
  }

  // A little life: a worker appears, then needs an approval, then another worker finishes.
  setTimeout(() => {
    workers.push(workerCard());
    emit("task.started");
  }, 2500);
  setTimeout(() => {
    attention.push(inboxItem());
    emit("approval.requested");
  }, 5000);
  setTimeout(() => {
    workers.push(
      workerCard({
        taskId: "aaaaaaaa-0000-4000-8000-000000000002",
        employeeId: "bbbbbbbb-0000-4000-8000-000000000002",
        name: "kit",
        provider: "antigravity",
        model: null,
        label: "Audit shared temp dirs",
        state: "complete",
        stateReason: "the task reported completion",
        taskState: "completed",
        session: null,
        terminalAt: new Date().toISOString(),
        actions: [],
      }),
    );
    emit("task.completed");
  }, 8000);

  function current(): WorkspaceConversationResult {
    return conversation({
      workers: [...workers],
      attention: [...attention],
      turns,
      eventHighWater: sequence,
    });
  }

  const handlers: Record<string, (params: Record<string, unknown>) => unknown> =
    {
      "system.status": () => status({ eventHighWater: sequence }),
      "project.list": () => [...projects],
      "project.register": (params) => {
        const record = {
          ...project(),
          id: `44444444-4444-4444-8444-${String(projects.length).padStart(12, "0")}`,
          canonicalPath: String(params.path),
          displayName:
            (params.displayName as string | null) ??
            String(params.path).split(/[\\/]/).pop() ??
            "project",
        };
        projects.push(record);
        emit("project.registered");
        return { project: record };
      },
      "employee.create": (params) => {
        const lead = leadCandidate({
          employeeId: `cccccccc-0000-4000-8000-${String(leads.length).padStart(12, "0")}`,
          name: String(params.name),
          provider: String(params.provider),
          role: String(params.role),
          model: (params.model as string | null) ?? null,
          reasoningEffort: (params.reasoningEffort as string | null) ?? null,
          defaultAuthority:
            params.authorityMode as LeadCandidate["defaultAuthority"],
        });
        leads.push(lead);
        emit("employee.created");
        return {
          employee: { id: lead.employeeId, name: lead.name, enabled: true },
          rosterRevision: leads.length,
          eventSequence: sequence,
        };
      },
      "workspace.conversations": (params) =>
        params.projectId === PROJECT_ID
          ? { ...conversations(projectRows), projectId: PROJECT_ID }
          : params.projectId
            ? { ...conversations([]), projectId: params.projectId as string }
            : conversations([
                conversationRow(),
                conversationRow({
                  id: "33333333-3333-4333-8333-333333333334",
                  title: "add retry budget to dispatch",
                  marker: "needs-attention",
                  updatedAt: new Date(
                    Date.now() - 3 * 86_400_000,
                  ).toISOString(),
                }),
                conversationRow({
                  id: "33333333-3333-4333-8333-333333333335",
                  title: "doc pass on NEXT-SESSION",
                  marker: null,
                  updatedAt: new Date(
                    Date.now() - 12 * 86_400_000,
                  ).toISOString(),
                }),
              ]),
      "workspace.launcher": (params) =>
        launcher({
          eventHighWater: sequence,
          project:
            projects.find((entry) => entry.id === params.projectId) ?? null,
          leads: [...leads],
          rememberedLeadId: fresh ? null : (leads[0]?.employeeId ?? null),
        }),
      "workspace.conversation": () => current(),
      "inbox.counts": () => ({
        version: 1,
        counts: {
          unresolved: attention.length,
          approvals: attention.length,
          permissions: 0,
          incidents: 0,
          watching: 0,
        },
        eventHighWater: sequence,
      }),
      "inbox.list": () => ({
        version: 1,
        items: [...attention],
        counts: {
          unresolved: attention.length,
          approvals: attention.length,
          permissions: 0,
          incidents: 0,
          watching: 0,
        },
        eventHighWater: sequence,
        truncated: false,
        nextBefore: null,
      }),
      "native.trust.list": () => trust,
      "turn.show": () => turnRecord(),
      "session.terminal.status": (params) =>
        params.id === SESSION_ID
          ? terminalStatus({ viewTerminal: liveTerminal })
          : terminalStatus({
              sessionId: String(params.id),
              viewable: false,
              viewBlockingReason:
                "the provider does not support a native read-only view",
            }),
      "session.terminal.view": () => ({
        sessionId: SESSION_ID,
        terminal: liveTerminal,
      }),
      "terminal.resize": (params) => ({
        ...liveTerminal,
        columns: params.columns,
        rows: params.rows,
      }),
      "terminal.stop": () => ({ ...liveTerminal, live: false }),
      "approval.decide": () => {
        attention.splice(0, attention.length);
        emit("approval.decided");
        return { ok: true };
      },
      "turn.submit": (params) => {
        turns = [
          ...turns,
          turnSummary({
            id: `turn-${turns.length + 1}`,
            sequence: turns.length + 1,
            state: "accepted",
            createdAt: new Date().toISOString(),
          }),
        ];
        emit("turn.accepted");
        return { turn: { id: `turn-${turns.length}`, text: params.text } };
      },
      "conversation.create": () => ({ conversation: { id: CONVERSATION_ID } }),
      "conversation.rename": () => ({ ok: true }),
      "task.cancel": () => ({ ok: true }),
      "employee.list": () =>
        leads.map((lead) => ({
          id: lead.employeeId,
          name: lead.name,
          provider: lead.provider,
          enabled: true,
        })),
      "provider.list": () => [
        {
          provider: "codex",
          detected: true,
          executable: "C:\\nvm4w\\codex.cmd",
          discoverySource: "path",
        },
        {
          provider: "claude-code",
          detected: true,
          executable: "C:\\Users\\dev\\.local\\bin\\claude.exe",
          discoverySource: "path",
        },
        {
          provider: "opencode",
          detected: false,
          executable: null,
          discoverySource: null,
        },
        {
          provider: "antigravity",
          detected: true,
          executable: "C:\\Users\\dev\\AppData\\Local\\agy\\bin\\agy.exe",
          discoverySource: "path",
        },
      ],
      "roster.get": () => ({ revision: 3, entries: [] }),
      "native.trust.policy.show": (params) => ({
        provider: params.provider,
        autoAcceptManagedNextTime: false,
        revision: 1,
        updatedAt: new Date().toISOString(),
      }),
      "memory.policy.show": () => ({
        retentionDays: 30,
        quotaBytes: 1073741824,
      }),
      "memory.queue.status": () => ({ pending: 0, archived: 12 }),
      "memory.worker.show": () => ({ configured: false }),
      "system.resources": () => ({
        daemon: {
          pid: 4242,
          bootId: status().bootId,
          uptimeMillis: 3_600_000,
          cpuMillis: 12_000,
          userCpuMillis: 9_000,
          kernelCpuMillis: 3_000,
          workingSetBytes: 13_631_488,
          peakWorkingSetBytes: 20_971_520,
          handleCount: 312,
          liveSessions: 1,
          sampledAt: new Date().toISOString(),
        },
      }),
      "system.budget": () => ({
        cpuCapPercent: 20,
        backgroundThrottlePercent: 15,
      }),
      "session.resources": () => ({
        sessionId: SESSION_ID,
        employeeId: "x",
        provider: "codex",
        nativeHostPid: 5151,
        activeProcesses: 3,
        totalProcesses: 5,
        cpuMillis: 48_000,
        userCpuMillis: 40_000,
        kernelCpuMillis: 8_000,
        peakProcessMemoryBytes: 180_000_000,
        peakJobMemoryBytes: 310_000_000,
        sampledAt: new Date().toISOString(),
      }),
    };

  const sample = [
    "\u001b[1;37mOpenAI Codex\u001b[0m \u001b[2mv0.165 · gpt-5.4 · C:\\workspace\\demo\\.lazoo\\worktrees\\task-9c1e\u001b[0m",
    "",
    "\u001b[2m›\u001b[0m Fix the flaky daemon test turn_retry_recovers_after_provider_timeout.",
    "",
    "• Reproduced locally in 4 of 20 runs.",
    "• Ran \u001b[1mcargo test -p lazood --test daemon turn_retry -- --test-threads=4\u001b[0m",
    "  \u001b[2m└ 4 failed · 16 passed · 38s\u001b[0m",
    "• Two tests share temp_data_root(). The second inherits a stale WAL.",
    '• Called \u001b[1mtask_delegate\u001b[0m(rune, "Reproduce across 20 runs")',
    "  \u001b[2m└ task 7f2a · accepted · claude code\u001b[0m",
    "• Waiting on ash's test run before touching the store.",
    "",
    "\u001b[2m────────────────────────────────────────────────────────\u001b[0m",
    "\u001b[2m› Viewing sub-agent · prompts and approvals go through Lazoo\u001b[0m",
  ];

  return {
    async call<T>(method: string, params: unknown = {}) {
      await new Promise((resolve) => setTimeout(resolve, 40));
      const handler = handlers[method];
      if (!handler)
        throw {
          code: "LZ-DEV-UNSCRIPTED",
          message: `no dev handler for ${method}`,
        };
      return handler(params as Record<string, unknown>) as T;
    },
    async subscribeEvents(_after, onFrame) {
      listeners.add(onFrame);
      return async () => {
        listeners.delete(onFrame);
      };
    },
    async startTerminalStream(_args, onFrame): Promise<TerminalStreamHandle> {
      let line = 0;
      let offset = 0;
      const timer = setInterval(() => {
        if (line >= sample.length) {
          clearInterval(timer);
          return;
        }
        const text = `${sample[line]}\r\n`;
        line += 1;
        const data = btoa(unescape(encodeURIComponent(text)));
        const frame: TerminalFrame = {
          kind: "output",
          start: offset,
          end: offset + text.length,
          gap: false,
          live: true,
          exitCode: null,
          data,
        };
        offset += text.length;
        onFrame(frame);
      }, 120);
      return {
        stop: async () => clearInterval(timer),
        focus: async () => undefined,
      };
    },
    async writeTerminal() {
      return undefined;
    },
    window: {
      close: async () => undefined,
      minimize: async () => undefined,
      toggleMaximize: async () => undefined,
      startDragging: async () => undefined,
      isMaximized: async () => false,
    },
  };
}
