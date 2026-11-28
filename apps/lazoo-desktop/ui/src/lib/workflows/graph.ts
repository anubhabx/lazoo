// Structure checks and edge helpers. Pure functions over a workflow.

import type { Handle, Workflow, WorkflowEdge } from "./types";
import { isTrigger } from "./types";

export interface WorkflowIssue {
  nodeId: string | null;
  message: string;
}

/** Edges leaving `nodeId`, in edge order; only those on `handle` when it is given. */
export function outgoing(
  workflow: Workflow,
  nodeId: string,
  handle?: Handle,
): WorkflowEdge[] {
  return workflow.edges.filter(
    (edge) =>
      edge.source === nodeId &&
      (handle === undefined || edge.sourceHandle === handle),
  );
}

/**
 * Problems that would stop or confuse a run. A condition with only one branch wired
 * is allowed: the other branch simply ends the run.
 */
export function validateWorkflow(workflow: Workflow): WorkflowIssue[] {
  const issues: WorkflowIssue[] = [];
  const triggers = workflow.nodes.filter((node) => isTrigger(node.kind));
  if (!triggers.length) {
    issues.push({
      nodeId: null,
      message: "Add a trigger to start this workflow.",
    });
  }
  for (const trigger of triggers) {
    if (!outgoing(workflow, trigger.id).length) {
      issues.push({
        nodeId: trigger.id,
        message: "Connect the trigger to a step.",
      });
    }
  }
  for (const node of workflow.nodes) {
    if (node.kind !== "agent.prompt") continue;
    const config = node.config as { employeeId: string | null; prompt: string };
    if (!config.employeeId) {
      issues.push({
        nodeId: node.id,
        message: "Choose an agent for this step.",
      });
    }
    if (!config.prompt.trim()) {
      issues.push({
        nodeId: node.id,
        message: "Write a prompt for this step.",
      });
    }
  }

  const reachable = new Set<string>();
  const stack = triggers.map((node) => node.id);
  while (stack.length) {
    const id = stack.pop()!;
    if (reachable.has(id)) continue;
    reachable.add(id);
    for (const edge of outgoing(workflow, id)) stack.push(edge.target);
  }
  for (const node of workflow.nodes) {
    if (!reachable.has(node.id)) {
      issues.push({
        nodeId: node.id,
        message: "This step is not connected to a trigger.",
      });
    }
  }

  // Cycles: a depth-first walk that meets a node already on the current path.
  const state = new Map<string, "path" | "done">();
  const cyclic = new Set<string>();
  const walk = (id: string) => {
    state.set(id, "path");
    for (const edge of outgoing(workflow, id)) {
      const seen = state.get(edge.target);
      if (seen === "path") cyclic.add(edge.target);
      else if (!seen) walk(edge.target);
    }
    state.set(id, "done");
  };
  for (const node of workflow.nodes) if (!state.has(node.id)) walk(node.id);
  for (const id of cyclic) {
    issues.push({ nodeId: id, message: "This step is part of a loop." });
  }
  return issues;
}
