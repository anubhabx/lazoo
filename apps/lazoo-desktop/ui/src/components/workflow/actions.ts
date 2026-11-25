// Actions the index and the editor share.

import type { Store } from "../../lib/store";
import {
  validateWorkflow,
  type WorkflowIssue,
} from "../../lib/workflows/graph";
import type { WorkflowsController } from "../../lib/workflows/controller";
import type { Workflow } from "../../lib/workflows/types";

/** Turning a workflow on is refused while it has problems; the first one is shown. */
export function requestEnabled(
  controller: WorkflowsController,
  store: Store,
  workflow: Workflow,
  enabled: boolean,
): boolean {
  if (enabled) {
    const issues = validateWorkflow(workflow);
    if (issues.length) {
      store.toast({
        tone: "error",
        title: `${workflow.name} was not turned on`,
        detail: issues[0]!.message,
      });
      return false;
    }
  }
  controller.setEnabled(workflow.id, enabled);
  return true;
}

export function duplicateWorkflow(
  controller: WorkflowsController,
  workflow: Workflow,
): Workflow {
  return controller.create(`${workflow.name} copy`, {
    id: workflow.id,
    name: workflow.name,
    description: "",
    nodes: workflow.nodes,
    edges: workflow.edges,
  });
}

/** Run now is refused like enabling is: the first problem is shown and returned. */
export function requestRun(
  controller: WorkflowsController,
  store: Store,
  workflow: Workflow,
): WorkflowIssue | null {
  const issues = validateWorkflow(workflow);
  if (issues.length) {
    store.toast({
      tone: "error",
      title: `${workflow.name} was not run`,
      detail: issues[0]!.message,
    });
    return issues[0]!;
  }
  void controller.runNow(workflow.id);
  return null;
}
