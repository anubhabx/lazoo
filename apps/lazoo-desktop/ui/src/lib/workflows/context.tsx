import { createContext, useContext, useSyncExternalStore } from "react";

import type { WorkflowsController, WorkflowsSnapshot } from "./controller";

const WorkflowsContext = createContext<WorkflowsController | null>(null);

export const WorkflowsProvider = WorkflowsContext.Provider;

export function useWorkflows(): WorkflowsController {
  const workflows = useContext(WorkflowsContext);
  if (!workflows) throw new Error("WorkflowsProvider is missing");
  return workflows;
}

export function useWorkflowsState(): WorkflowsSnapshot {
  const workflows = useWorkflows();
  return useSyncExternalStore(
    workflows.subscribe,
    workflows.getSnapshot,
    workflows.getSnapshot,
  );
}

const NO_SUBSCRIPTION = () => () => {};
const NO_SNAPSHOT = () => null;

/** The state when a controller is mounted, else null; for surfaces that may render without one. */
export function useOptionalWorkflowsState(): WorkflowsSnapshot | null {
  const workflows = useContext(WorkflowsContext);
  return useSyncExternalStore(
    workflows ? workflows.subscribe : NO_SUBSCRIPTION,
    workflows ? workflows.getSnapshot : NO_SNAPSHOT,
    workflows ? workflows.getSnapshot : NO_SNAPSHOT,
  );
}
