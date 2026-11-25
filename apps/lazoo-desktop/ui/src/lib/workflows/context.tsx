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
