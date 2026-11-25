// Test helpers for workflow tests.
import type {
  Handle,
  NodeConfigs,
  NodeKind,
  Workflow,
  WorkflowEdge,
  WorkflowNode,
} from "./types";
import { NODE_CATALOG } from "./types";

export function node<K extends NodeKind>(
  id: string,
  kind: K,
  config: Partial<NodeConfigs[K]> = {},
): WorkflowNode {
  return {
    id,
    kind,
    label: id,
    position: { x: 0, y: 0 },
    config: { ...NODE_CATALOG[kind].defaultConfig(), ...config },
  } as WorkflowNode;
}

export function edge(
  source: string,
  target: string,
  handle: Handle = "out",
): WorkflowEdge {
  return {
    id: `${source}-${handle}-${target}`,
    source,
    sourceHandle: handle,
    target,
  };
}

export function workflow(
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
  overrides: Partial<Workflow> = {},
): Workflow {
  return {
    id: "wf",
    name: "Test",
    enabled: true,
    nodes,
    edges,
    createdAt: "2026-11-01T00:00:00.000Z",
    updatedAt: "2026-11-01T00:00:00.000Z",
    ...overrides,
  };
}
