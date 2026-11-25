// The runs panel: newest first, the active run on top; a run opens to its steps.

import { ChevronRight } from "lucide-react";

import { useStore } from "../../lib/context";
import { formatDuration } from "../../lib/util";
import type { Workflow, WorkflowRun } from "../../lib/workflows/types";
import { Dot, Spinner } from "../primitives";
import { ago, runTone, runWord, stepTone, stepWord } from "./model";

function duration(run: WorkflowRun): string {
  const end = run.finishedAt ? Date.parse(run.finishedAt) : Date.now();
  const millis = end - Date.parse(run.startedAt);
  return Number.isFinite(millis) && millis >= 0 ? formatDuration(millis) : "";
}

export function RunsPanel({
  workflow,
  runs,
  expanded: open,
  onExpand,
}: {
  workflow: Workflow;
  runs: WorkflowRun[];
  /** Which runs are open; a live run is open unless closed. Lifted so the canvas can show it. */
  expanded: Record<string, boolean>;
  onExpand: (runId: string, value: boolean) => void;
}) {
  const store = useStore();
  return (
    <aside className="wf-panel" aria-label="Runs">
      <div className="wf-panel__head wf-panel__head--plain">
        <h2 className="wf-panel__heading">Runs</h2>
      </div>
      {runs.length === 0 ? (
        <p className="wf-note wf-note--pad">No runs yet.</p>
      ) : (
        <ul className="wf-runs">
          {runs.map((run) => {
            const expanded = open[run.id] ?? run.status === "running";
            return (
              <li key={run.id} className="wf-run">
                <button
                  type="button"
                  className="wf-run__head"
                  aria-expanded={expanded}
                  onClick={() => onExpand(run.id, !expanded)}
                >
                  <ChevronRight
                    size={13}
                    className="wf-run__chevron"
                    aria-hidden="true"
                  />
                  <span className="wf-run__main">
                    <span className="wf-run__status">
                      {run.status === "running" ? (
                        <span className="wf-run__spin">
                          <Spinner size={8} />
                        </span>
                      ) : (
                        <Dot tone={runTone(run.status)} />
                      )}
                      {runWord(run.status)}
                    </span>
                    <span className="wf-run__meta">
                      {run.trigger === "schedule" ? "Schedule" : "Manual"} ·{" "}
                      {ago(run.startedAt)}
                      {duration(run) ? ` · ${duration(run)}` : ""}
                    </span>
                  </span>
                </button>
                {expanded ? (
                  <ol className="wf-steps">
                    {run.steps.length === 0 ? (
                      <li className="wf-step wf-step--empty">No steps yet.</li>
                    ) : null}
                    {run.steps.map((step, index) => {
                      const node = workflow.nodes.find(
                        (entry) => entry.id === step.nodeId,
                      );
                      const text = step.error ?? step.output;
                      return (
                        <li key={`${step.nodeId}-${index}`} className="wf-step">
                          <span className="wf-step__row">
                            <span className="wf-step__name" title={node?.label}>
                              {node?.label ?? "Removed step"}
                            </span>
                            <span className="wf-step__status">
                              {step.status === "running" ? (
                                <span className="wf-run__spin">
                                  <Spinner size={8} />
                                </span>
                              ) : (
                                <Dot tone={stepTone(step.status)} />
                              )}
                              {stepWord(step.status)}
                            </span>
                          </span>
                          {text ? (
                            <span
                              className={
                                step.error
                                  ? "wf-step__text wf-step__text--error"
                                  : "wf-step__text mono"
                              }
                              title={text}
                            >
                              {text}
                            </span>
                          ) : null}
                          {step.conversationId ? (
                            <button
                              type="button"
                              className="wf-link"
                              onClick={() =>
                                store.openConversation(step.conversationId!)
                              }
                            >
                              Open conversation
                            </button>
                          ) : null}
                        </li>
                      );
                    })}
                  </ol>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
