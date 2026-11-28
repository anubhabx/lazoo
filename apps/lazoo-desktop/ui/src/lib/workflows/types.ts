// The workflow model: the saved document, the node catalog and defensive parsing.
// Pure data; no React, no I/O. The desktop only stores the JSON (workflows_load/save).

import type { AuthorityMode } from "../protocol";

export type NodeKind =
  | "trigger.schedule"
  | "trigger.manual"
  | "agent.prompt"
  | "logic.condition"
  | "logic.wait"
  | "notify";

export type Schedule =
  | { mode: "interval"; everyMinutes: number } // 1..10080
  | { mode: "daily"; time: string; days: number[] }; // "HH:MM" local time, days 0=Sun..6=Sat, non-empty

export interface NodeConfigs {
  "trigger.schedule": { schedule: Schedule };
  "trigger.manual": Record<string, never>;
  "agent.prompt": {
    employeeId: string | null;
    spaceId: string | null;
    authority: AuthorityMode;
    prompt: string;
    waitForReply: boolean;
    timeoutMinutes: number;
  };
  "logic.condition": {
    op: "contains" | "not-contains" | "equals" | "is-empty";
    value: string;
  };
  "logic.wait": { minutes: number };
  notify: { title: string; body: string };
}

export interface WorkflowNode<K extends NodeKind = NodeKind> {
  id: string;
  kind: K;
  label: string;
  position: { x: number; y: number };
  config: NodeConfigs[K];
}

export type Handle = "out" | "true" | "false";

export interface WorkflowEdge {
  id: string;
  source: string;
  sourceHandle: Handle;
  target: string;
}

export interface Workflow {
  id: string;
  name: string;
  enabled: boolean;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  createdAt: string;
  updatedAt: string;
}

export type StepStatus = "running" | "succeeded" | "failed" | "skipped";

export interface RunStep {
  nodeId: string;
  status: StepStatus;
  startedAt: string;
  finishedAt: string | null;
  output: string | null;
  error: string | null;
  /** The conversation an agent step started, so the UI can link to it. */
  conversationId: string | null;
}

export type RunStatus = "running" | "succeeded" | "failed" | "cancelled";

export interface WorkflowRun {
  id: string;
  workflowId: string;
  trigger: "schedule" | "manual";
  triggerNodeId: string;
  status: RunStatus;
  startedAt: string;
  finishedAt: string | null;
  steps: RunStep[];
}

export interface WorkflowDocument {
  version: 1;
  workflows: Workflow[];
  runs: WorkflowRun[];
  /** `${workflowId}:${nodeId}` -> ISO time of the last schedule fire (or of enabling). */
  lastFired: Record<string, string>;
}

export type NodeGroup = "Triggers" | "Agents" | "Logic" | "Output";

export interface NodeCatalogEntry<K extends NodeKind = NodeKind> {
  kind: K;
  title: string;
  description: string;
  group: NodeGroup;
  /** Triggers start a run and take no input. */
  hasInput: boolean;
  outputs: Handle[];
  defaultConfig(): NodeConfigs[K];
}

export const NODE_CATALOG: { [K in NodeKind]: NodeCatalogEntry<K> } = {
  "trigger.schedule": {
    kind: "trigger.schedule",
    title: "Schedule",
    description: "Start on a repeating schedule while Lazoo is open.",
    group: "Triggers",
    hasInput: false,
    outputs: ["out"],
    defaultConfig: () => ({
      schedule: { mode: "daily", time: "09:00", days: [1, 2, 3, 4, 5] },
    }),
  },
  "trigger.manual": {
    kind: "trigger.manual",
    title: "Manual",
    description: "Start by pressing Run now.",
    group: "Triggers",
    hasInput: false,
    outputs: ["out"],
    defaultConfig: () => ({}),
  },
  "agent.prompt": {
    kind: "agent.prompt",
    title: "Ask an agent",
    description:
      "Start a conversation with an agent and optionally wait for its reply.",
    group: "Agents",
    hasInput: true,
    outputs: ["out"],
    defaultConfig: () => ({
      employeeId: null,
      spaceId: null,
      authority: "ask",
      prompt: "",
      waitForReply: true,
      timeoutMinutes: 30,
    }),
  },
  "logic.condition": {
    kind: "logic.condition",
    title: "Condition",
    description: "Branch on the previous step's output.",
    group: "Logic",
    hasInput: true,
    outputs: ["true", "false"],
    defaultConfig: () => ({ op: "contains", value: "" }),
  },
  "logic.wait": {
    kind: "logic.wait",
    title: "Wait",
    description: "Pause before the next step.",
    group: "Logic",
    hasInput: true,
    outputs: ["out"],
    defaultConfig: () => ({ minutes: 5 }),
  },
  notify: {
    kind: "notify",
    title: "Notify",
    description: "Show a notice in Lazoo.",
    group: "Output",
    hasInput: true,
    outputs: ["out"],
    defaultConfig: () => ({ title: "Workflow update", body: "{{input}}" }),
  },
};

export const NODE_KINDS = Object.keys(NODE_CATALOG) as NodeKind[];

export function isTrigger(kind: NodeKind): boolean {
  return kind.startsWith("trigger.");
}

export function emptyDocument(): WorkflowDocument {
  return { version: 1, workflows: [], runs: [], lastFired: {} };
}

export function lastFiredKey(workflowId: string, nodeId: string): string {
  return `${workflowId}:${nodeId}`;
}

// ---- defensive parsing --------------------------------------------------

type Raw = Record<string, unknown>;

const isObject = (value: unknown): value is Raw =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const text = (value: unknown, fallback = ""): string =>
  typeof value === "string" ? value : fallback;

const isoOr = (value: unknown, fallback: string): string =>
  typeof value === "string" && !Number.isNaN(Date.parse(value))
    ? value
    : fallback;

const intIn = (value: unknown, min: number, max: number, fallback: number) =>
  typeof value === "number" && Number.isFinite(value)
    ? Math.min(max, Math.max(min, Math.round(value)))
    : fallback;

function parseSchedule(raw: unknown): Schedule {
  const fallback = NODE_CATALOG["trigger.schedule"].defaultConfig().schedule;
  if (!isObject(raw)) return fallback;
  if (raw.mode === "interval") {
    return {
      mode: "interval",
      everyMinutes: intIn(raw.everyMinutes, 1, 10080, 60),
    };
  }
  if (raw.mode === "daily") {
    const time =
      typeof raw.time === "string" && /^([01]\d|2[0-3]):[0-5]\d$/.test(raw.time)
        ? raw.time
        : "09:00";
    const days = Array.isArray(raw.days)
      ? [
          ...new Set(
            raw.days.filter(
              (day): day is number =>
                Number.isInteger(day) &&
                (day as number) >= 0 &&
                (day as number) <= 6,
            ),
          ),
        ].sort()
      : [];
    return {
      mode: "daily",
      time,
      days: days.length ? days : [0, 1, 2, 3, 4, 5, 6],
    };
  }
  return fallback;
}

const AUTHORITIES: AuthorityMode[] = ["ask", "self-decision", "full-autonomy"];
const CONDITION_OPS = [
  "contains",
  "not-contains",
  "equals",
  "is-empty",
] as const;

function parseConfig(kind: NodeKind, raw: unknown): NodeConfigs[NodeKind] {
  const config = isObject(raw) ? raw : {};
  switch (kind) {
    case "trigger.schedule":
      return { schedule: parseSchedule(config.schedule) };
    case "trigger.manual":
      return {};
    case "agent.prompt": {
      const authority = AUTHORITIES.find((mode) => mode === config.authority);
      return {
        employeeId:
          typeof config.employeeId === "string" ? config.employeeId : null,
        spaceId: typeof config.spaceId === "string" ? config.spaceId : null,
        authority: authority ?? "ask",
        prompt: text(config.prompt),
        waitForReply: config.waitForReply !== false,
        timeoutMinutes: intIn(config.timeoutMinutes, 1, 1440, 30),
      };
    }
    case "logic.condition":
      return {
        op: CONDITION_OPS.find((op) => op === config.op) ?? "contains",
        value: text(config.value),
      };
    case "logic.wait":
      return { minutes: intIn(config.minutes, 0, 10080, 5) };
    case "notify":
      return { title: text(config.title), body: text(config.body) };
  }
}

function parseNode(raw: unknown): WorkflowNode | null {
  if (!isObject(raw) || typeof raw.id !== "string" || !raw.id) return null;
  const kind = NODE_KINDS.find((candidate) => candidate === raw.kind);
  if (!kind) return null;
  const position = isObject(raw.position) ? raw.position : {};
  const coordinate = (value: unknown) =>
    typeof value === "number" && Number.isFinite(value) ? value : 0;
  return {
    id: raw.id,
    kind,
    label: text(raw.label, NODE_CATALOG[kind].title),
    position: { x: coordinate(position.x), y: coordinate(position.y) },
    config: parseConfig(kind, raw.config),
  } as WorkflowNode;
}

const HANDLES: Handle[] = ["out", "true", "false"];

function parseWorkflow(raw: unknown, now: string): Workflow | null {
  if (!isObject(raw) || typeof raw.id !== "string" || !raw.id) return null;
  const nodes: WorkflowNode[] = [];
  const seen = new Set<string>();
  for (const candidate of Array.isArray(raw.nodes) ? raw.nodes : []) {
    const node = parseNode(candidate);
    if (node && !seen.has(node.id)) {
      seen.add(node.id);
      nodes.push(node);
    }
  }
  const edges: WorkflowEdge[] = [];
  const edgeIds = new Set<string>();
  for (const candidate of Array.isArray(raw.edges) ? raw.edges : []) {
    if (!isObject(candidate)) continue;
    const handle = HANDLES.find((value) => value === candidate.sourceHandle);
    if (
      typeof candidate.id !== "string" ||
      edgeIds.has(candidate.id) ||
      typeof candidate.source !== "string" ||
      typeof candidate.target !== "string" ||
      !handle ||
      !seen.has(candidate.source) ||
      !seen.has(candidate.target)
    ) {
      continue;
    }
    edgeIds.add(candidate.id);
    edges.push({
      id: candidate.id,
      source: candidate.source,
      sourceHandle: handle,
      target: candidate.target,
    });
  }
  const createdAt = isoOr(raw.createdAt, now);
  return {
    id: raw.id,
    name: text(raw.name, "Untitled workflow"),
    enabled: raw.enabled === true,
    nodes,
    edges,
    createdAt,
    updatedAt: isoOr(raw.updatedAt, createdAt),
  };
}

const STEP_STATUSES: StepStatus[] = [
  "running",
  "succeeded",
  "failed",
  "skipped",
];
const RUN_STATUSES: RunStatus[] = [
  "running",
  "succeeded",
  "failed",
  "cancelled",
];

function parseStep(raw: unknown, fallback: string): RunStep | null {
  if (!isObject(raw) || typeof raw.nodeId !== "string") return null;
  const status = STEP_STATUSES.find((value) => value === raw.status);
  if (!status) return null;
  return {
    nodeId: raw.nodeId,
    status,
    startedAt: isoOr(raw.startedAt, fallback),
    finishedAt: typeof raw.finishedAt === "string" ? raw.finishedAt : null,
    output: typeof raw.output === "string" ? raw.output : null,
    error: typeof raw.error === "string" ? raw.error : null,
    conversationId:
      typeof raw.conversationId === "string" ? raw.conversationId : null,
  };
}

function parseRun(raw: unknown, now: string): WorkflowRun | null {
  if (
    !isObject(raw) ||
    typeof raw.id !== "string" ||
    typeof raw.workflowId !== "string"
  ) {
    return null;
  }
  const status = RUN_STATUSES.find((value) => value === raw.status);
  if (!status) return null;
  const startedAt = isoOr(raw.startedAt, now);
  // A run that was still "running" when saved did not survive the window closing.
  const interrupted = status === "running";
  return {
    id: raw.id,
    workflowId: raw.workflowId,
    trigger: raw.trigger === "schedule" ? "schedule" : "manual",
    triggerNodeId: text(raw.triggerNodeId),
    status: interrupted ? "failed" : status,
    startedAt,
    finishedAt: interrupted
      ? startedAt
      : typeof raw.finishedAt === "string"
        ? raw.finishedAt
        : null,
    steps: (Array.isArray(raw.steps) ? raw.steps : [])
      .map((step) => parseStep(step, startedAt))
      .filter((step): step is RunStep => step !== null),
  };
}

/** Validates a stored document. Bad entries are dropped, the rest kept; it never throws. */
export function parseDocument(
  raw: unknown,
  now = new Date(),
): WorkflowDocument {
  if (!isObject(raw)) return emptyDocument();
  const stamp = now.toISOString();
  const workflows: Workflow[] = [];
  const ids = new Set<string>();
  for (const candidate of Array.isArray(raw.workflows) ? raw.workflows : []) {
    const workflow = parseWorkflow(candidate, stamp);
    if (workflow && !ids.has(workflow.id)) {
      ids.add(workflow.id);
      workflows.push(workflow);
    }
  }
  const runs = (Array.isArray(raw.runs) ? raw.runs : [])
    .map((run) => parseRun(run, stamp))
    .filter(
      (run): run is WorkflowRun => run !== null && ids.has(run.workflowId),
    );
  const lastFired: Record<string, string> = {};
  if (isObject(raw.lastFired)) {
    for (const [key, value] of Object.entries(raw.lastFired)) {
      if (typeof value === "string" && !Number.isNaN(Date.parse(value))) {
        lastFired[key] = value;
      }
    }
  }
  return { version: 1, workflows, runs, lastFired };
}
