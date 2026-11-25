// Pure helpers the workflow editor shares: summaries, status vocabulary and graph edits.
// No React; the data model itself lives in lib/workflows.

import type {
  AuthorityMode,
  EmployeeRecord,
  ProjectRecord,
} from "../../lib/protocol";
import { authorityLabel, relativeTime } from "../../lib/util";
import { describeSchedule } from "../../lib/workflows/schedule";
import {
  NODE_CATALOG,
  isTrigger,
  type Handle,
  type NodeConfigs,
  type NodeKind,
  type RunStatus,
  type StepStatus,
  type Workflow,
  type WorkflowEdge,
  type WorkflowNode,
  type WorkflowRun,
} from "../../lib/workflows/types";

export const NODE_WIDTH = 232;
/** Horizontal pitch between a node and the one added after it (an 88 px gap). */
export const NODE_GAP = 320;

export const HANDLE_LABEL: Record<Handle, string> = {
  out: "Next",
  true: "True",
  false: "False",
};

export const AUTHORITY_MEANING: Record<AuthorityMode, string> = {
  ask: "Commands, file changes and delegation wait for you.",
  "self-decision": "In-scope work proceeds; scope expansions wait for you.",
  "full-autonomy": "The agent and its workers proceed without approvals.",
};

export const CONDITION_OPS: {
  value: NodeConfigs["logic.condition"]["op"];
  label: string;
}[] = [
  { value: "contains", label: "Contains" },
  { value: "not-contains", label: "Does not contain" },
  { value: "equals", label: "Equals" },
  { value: "is-empty", label: "Is empty" },
];

export function plural(count: number, one: string, many = `${one}s`) {
  return `${count} ${count === 1 ? one : many}`;
}

// ---- run vocabulary ---------------------------------------------------------

export function stepTone(status: StepStatus): string {
  switch (status) {
    case "running":
      return "running";
    case "succeeded":
      return "completed";
    case "failed":
      return "failed";
    case "skipped":
      return "dormant";
  }
}

export function stepWord(status: StepStatus): string {
  switch (status) {
    case "running":
      return "Running";
    case "succeeded":
      return "Succeeded";
    case "failed":
      return "Failed";
    case "skipped":
      return "Skipped";
  }
}

export function runTone(status: RunStatus): string {
  switch (status) {
    case "running":
      return "running";
    case "succeeded":
      return "completed";
    case "failed":
      return "failed";
    case "cancelled":
      return "dormant";
  }
}

export function runWord(status: RunStatus): string {
  switch (status) {
    case "running":
      return "Running";
    case "succeeded":
      return "Succeeded";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
  }
}

/** "5m ago", "just now"; dates older than a week keep the short date. */
export function ago(iso: string | null | undefined): string {
  const value = relativeTime(iso);
  if (!value) return "";
  if (value === "now") return "just now";
  return /^\d+[mhd]$/.test(value) ? `${value} ago` : value;
}

export function runsOf(
  runs: WorkflowRun[],
  workflowId: string,
  active?: WorkflowRun,
): WorkflowRun[] {
  const finished = runs
    .filter((run) => run.workflowId === workflowId)
    .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt));
  return active ? [active, ...finished] : finished;
}

// ---- summaries ----------------------------------------------------------------

export function triggerSummary(workflow: Workflow): string {
  const trigger = workflow.nodes.find((node) => isTrigger(node.kind));
  if (!trigger) return "No trigger";
  if (trigger.kind === "trigger.schedule") {
    return describeSchedule(
      (trigger.config as NodeConfigs["trigger.schedule"]).schedule,
    );
  }
  return "Manual";
}

export function stepCount(workflow: Workflow): number {
  return workflow.nodes.filter((node) => !isTrigger(node.kind)).length;
}

export interface SummaryContext {
  employees: EmployeeRecord[] | null;
  projects: ProjectRecord[];
}

export function employeeOf(
  context: SummaryContext,
  id: string | null,
): EmployeeRecord | null {
  return (id && context.employees?.find((entry) => entry.id === id)) || null;
}

export function projectName(
  context: SummaryContext,
  id: string | null,
): string {
  if (!id) return "Home";
  return (
    context.projects.find((entry) => entry.id === id)?.displayName ?? "Home"
  );
}

const OP_TEXT: Record<NodeConfigs["logic.condition"]["op"], string> = {
  contains: "contains",
  "not-contains": "does not contain",
  equals: "equals",
  "is-empty": "is empty",
};

/** The muted lines under a node's title: a headline and an optional detail. */
export function nodeSummary(
  node: WorkflowNode,
  context: SummaryContext,
): { headline: string; detail: string | null; muted: boolean } {
  switch (node.kind) {
    case "trigger.schedule":
      return {
        headline: describeSchedule(
          (node.config as NodeConfigs["trigger.schedule"]).schedule,
        ),
        detail: null,
        muted: false,
      };
    case "trigger.manual":
      return { headline: "Started with Run now", detail: null, muted: false };
    case "agent.prompt": {
      const config = node.config as NodeConfigs["agent.prompt"];
      const employee = employeeOf(context, config.employeeId);
      const prompt = config.prompt.trim().split(/\r?\n/)[0] ?? "";
      if (!employee) {
        return {
          headline: "Choose an agent",
          detail: prompt || null,
          muted: true,
        };
      }
      return {
        headline: `${employee.name} · ${projectName(context, config.spaceId)} · ${authorityLabel(config.authority)}`,
        detail: prompt || "No prompt yet",
        muted: false,
      };
    }
    case "logic.condition": {
      const config = node.config as NodeConfigs["logic.condition"];
      return {
        headline:
          config.op === "is-empty"
            ? "Output is empty"
            : `Output ${OP_TEXT[config.op]} "${config.value}"`,
        detail: null,
        muted: false,
      };
    }
    case "logic.wait": {
      const minutes = (node.config as NodeConfigs["logic.wait"]).minutes;
      return {
        headline: minutes === 0 ? "No delay" : plural(minutes, "minute"),
        detail: null,
        muted: false,
      };
    }
    case "notify": {
      const title = (node.config as NodeConfigs["notify"]).title.trim();
      return {
        headline: title || "No title",
        detail: null,
        muted: !title,
      };
    }
  }
}

// ---- graph edits -----------------------------------------------------------------

export function edgeId(source: string, handle: Handle, target: string) {
  return `${source}-${handle}-${target}`;
}

/** True when connecting source to target would close a loop (or loop onto itself). */
export function wouldCycle(
  workflow: Workflow,
  source: string,
  target: string,
): boolean {
  if (source === target) return true;
  const seen = new Set<string>();
  const stack = [target];
  while (stack.length) {
    const id = stack.pop()!;
    if (id === source) return true;
    if (seen.has(id)) continue;
    seen.add(id);
    for (const edge of workflow.edges) {
      if (edge.source === id) stack.push(edge.target);
    }
  }
  return false;
}

export function canConnect(
  workflow: Workflow,
  source: string,
  handle: Handle,
  target: string,
): boolean {
  const to = workflow.nodes.find((node) => node.id === target);
  if (!to || !NODE_CATALOG[to.kind].hasInput) return false;
  if (
    workflow.edges.some((edge) => edge.id === edgeId(source, handle, target))
  ) {
    return false;
  }
  return !wouldCycle(workflow, source, target);
}

export function connect(
  workflow: Workflow,
  source: string,
  handle: Handle,
  target: string,
): Workflow {
  if (!canConnect(workflow, source, handle, target)) return workflow;
  const edge: WorkflowEdge = {
    id: edgeId(source, handle, target),
    source,
    sourceHandle: handle,
    target,
  };
  return { ...workflow, edges: [...workflow.edges, edge] };
}

/** The first output of a node that nothing leaves from yet. */
export function freeHandle(
  workflow: Workflow,
  node: WorkflowNode,
): Handle | null {
  return (
    NODE_CATALOG[node.kind].outputs.find(
      (handle) =>
        !workflow.edges.some(
          (edge) => edge.source === node.id && edge.sourceHandle === handle,
        ),
    ) ?? null
  );
}

/** Nudges a position down until it no longer sits on top of another node. */
export function clearSpot(
  workflow: Workflow,
  position: { x: number; y: number },
): { x: number; y: number } {
  const next = { ...position };
  while (
    workflow.nodes.some(
      (node) =>
        Math.abs(node.position.x - next.x) < NODE_WIDTH / 2 &&
        Math.abs(node.position.y - next.y) < 80,
    )
  ) {
    next.y += 120;
  }
  return next;
}

export function snap(value: number): number {
  return Math.round(value / 8) * 8;
}

export function addNode(
  workflow: Workflow,
  id: string,
  kind: NodeKind,
  position: { x: number; y: number },
  from?: { nodeId: string; handle: Handle },
): Workflow {
  const entry = NODE_CATALOG[kind];
  const node = {
    id,
    kind,
    label: entry.title,
    position: clearSpot(workflow, { x: snap(position.x), y: snap(position.y) }),
    config: entry.defaultConfig(),
  } as WorkflowNode;
  const next = { ...workflow, nodes: [...workflow.nodes, node] };
  return from && entry.hasInput
    ? connect(next, from.nodeId, from.handle, id)
    : next;
}

export function removeNodes(workflow: Workflow, ids: Set<string>): Workflow {
  return {
    ...workflow,
    nodes: workflow.nodes.filter((node) => !ids.has(node.id)),
    edges: workflow.edges.filter(
      (edge) => !ids.has(edge.source) && !ids.has(edge.target),
    ),
  };
}

export function patchNode(
  workflow: Workflow,
  id: string,
  patch: { label?: string; config?: object },
): Workflow {
  return {
    ...workflow,
    nodes: workflow.nodes.map((node) =>
      node.id === id
        ? ({
            ...node,
            label: patch.label ?? node.label,
            config: patch.config
              ? { ...node.config, ...patch.config }
              : node.config,
          } as WorkflowNode)
        : node,
    ),
  };
}
