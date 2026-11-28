import { ReactFlowProvider, useReactFlow } from "@xyflow/react";
import { MoreHorizontal, Plus } from "lucide-react";
import { useCallback, useMemo, useState } from "react";

import {
  Badge,
  Button,
  Dialog,
  Dot,
  Empty,
  Page,
  PageHeader,
  Spinner,
  Switch,
  TableSkeleton,
  Menu,
} from "../components/primitives";
import {
  duplicateWorkflow,
  requestEnabled,
  requestRun,
} from "../components/workflow/actions";
import {
  ago,
  patchNode,
  plural,
  removeNodes,
  runTone,
  runWord,
  runsOf,
  stepCount,
  triggerSummary,
} from "../components/workflow/model";
import { useMinute } from "../components/workflow/useMinute";
import { NodeInspector } from "../components/workflow/NodeInspector";
import { RunsPanel } from "../components/workflow/RunsPanel";
import { WorkflowCanvas } from "../components/workflow/WorkflowCanvas";
import { WorkflowToolbar } from "../components/workflow/WorkflowToolbar";
import { useAppState, useStore } from "../lib/context";
import { validateWorkflow } from "../lib/workflows/graph";
import { useWorkflows, useWorkflowsState } from "../lib/workflows/context";
import { TEMPLATES } from "../lib/workflows/controller";
import type { Workflow, WorkflowRun } from "../lib/workflows/types";

export function WorkflowsPage() {
  const { route } = useAppState();
  const snapshot = useWorkflowsState();
  if (route.kind !== "workflows") return null;
  if (route.id === null) return <WorkflowsIndex />;
  const workflow = snapshot.document.workflows.find(
    (entry) => entry.id === route.id,
  );
  if (!workflow)
    return <MissingWorkflow loading={snapshot.status === "loading"} />;
  return <WorkflowEditor key={workflow.id} workflow={workflow} />;
}

function MissingWorkflow({ loading }: { loading: boolean }) {
  const store = useStore();
  return (
    <Page label="Workflows">
      {loading ? (
        <TableSkeleton rows={1} />
      ) : (
        <Empty
          title="This workflow is gone"
          detail="It was deleted or never existed."
          action={
            <Button
              onClick={() => store.navigate({ kind: "workflows", id: null })}
            >
              Back to workflows
            </Button>
          }
        />
      )}
    </Page>
  );
}

// ---- index --------------------------------------------------------------------

function formatNext(next: Date, now = new Date()): string {
  const clock = next.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  const day = (date: Date) =>
    new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((day(next) - day(now)) / 86_400_000);
  if (days <= 0) return `Next run ${clock}`;
  if (days === 1) return `Next run ${clock} tomorrow`;
  const weekday = next.toLocaleDateString(undefined, { weekday: "short" });
  return `Next run ${weekday} ${clock}`;
}

function WorkflowsIndex() {
  const store = useStore();
  const controller = useWorkflows();
  const snapshot = useWorkflowsState();
  const [deleting, setDeleting] = useState<Workflow | null>(null);
  const workflows = snapshot.document.workflows;

  const open = (id: string) => store.navigate({ kind: "workflows", id });
  const create = (template?: (typeof TEMPLATES)[number]) =>
    open(controller.create("", template).id);

  return (
    <Page label="Workflows">
      <PageHeader
        title="Workflows"
        description="Recurring work for your agents. Runs while Lazoo is open."
        actions={
          <Button variant="primary" onClick={() => create()}>
            <Plus size={14} aria-hidden="true" />
            New workflow
          </Button>
        }
        titleAccessory={<Badge>Preview</Badge>}
      />
      {snapshot.status === "loading" ? (
        <TableSkeleton rows={2} />
      ) : snapshot.status === "error" ? (
        <Empty
          title="Workflows could not be loaded"
          detail={`${snapshot.error?.code ?? ""} ${snapshot.error?.message ?? ""}`.trim()}
        />
      ) : workflows.length === 0 ? (
        <>
          <Empty
            title="No workflows yet"
            detail="Chain a schedule, an agent and a notice into work that repeats."
            action={
              <Button onClick={() => create()}>Start from scratch</Button>
            }
          />
          <div className="wf-templates" role="list" aria-label="Templates">
            {TEMPLATES.map((template) => (
              <div key={template.id} role="listitem" className="wf-template">
                <span className="wf-template__name">{template.name}</span>
                <span className="wf-template__desc">
                  {template.description}
                </span>
                <Button
                  size="sm"
                  aria-label={`Use template: ${template.name}`}
                  onClick={() => create(template)}
                >
                  Use template
                </Button>
              </div>
            ))}
          </div>
        </>
      ) : (
        <div className="ext-list wf-list" role="list" aria-label="Workflows">
          {workflows.map((workflow) => (
            <WorkflowRow
              key={workflow.id}
              workflow={workflow}
              onOpen={() => open(workflow.id)}
              onDelete={() => setDeleting(workflow)}
            />
          ))}
        </div>
      )}
      <Dialog
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title={`Delete ${deleting?.name ?? "workflow"}?`}
        description="Its steps and run history are removed. This cannot be undone."
        width={440}
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                if (deleting) controller.remove(deleting.id);
                setDeleting(null);
              }}
            >
              Delete
            </Button>
          </>
        }
      >
        {null}
      </Dialog>
    </Page>
  );
}

function WorkflowRow({
  workflow,
  onOpen,
  onDelete,
}: {
  workflow: Workflow;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const store = useStore();
  const controller = useWorkflows();
  const snapshot = useWorkflowsState();
  useMinute();
  const active = snapshot.activeRuns[workflow.id];
  const last = runsOf(snapshot.document.runs, workflow.id, active)[0];
  const next = controller.nextRun(workflow);
  const steps = stepCount(workflow);
  return (
    <div className="wf-row" role="listitem">
      <div className="wf-row__main">
        <button type="button" className="wf-row__open" onClick={onOpen}>
          {workflow.name}
        </button>
        <span className="wf-row__summary">
          {triggerSummary(workflow)} · {plural(steps, "step")}
        </span>
      </div>
      <span className="wf-row__next" data-off={!next || undefined}>
        {next ? formatNext(next) : "Off"}
      </span>
      <span className="wf-row__last">
        <LastRun run={last} />
      </span>
      <span className="wf-row__control">
        <Switch
          label={`Enable ${workflow.name}`}
          checked={workflow.enabled}
          onChange={(enabled) =>
            requestEnabled(controller, store, workflow, enabled)
          }
        />
      </span>
      <span className="wf-row__control">
        <Menu
          label={`Actions for ${workflow.name}`}
          items={[
            {
              label: "Run now",
              disabled: active !== undefined,
              onSelect: () => requestRun(controller, store, workflow),
            },
            {
              label: "Duplicate",
              onSelect: () => duplicateWorkflow(controller, workflow),
            },
            { label: "Delete", danger: true, onSelect: onDelete },
          ]}
        >
          <MoreHorizontal size={15} aria-hidden="true" />
        </Menu>
      </span>
    </div>
  );
}

function LastRun({ run }: { run: WorkflowRun | undefined }) {
  if (!run) return <span className="wf-muted">Never run</span>;
  return (
    <>
      <span className="wf-status">
        {run.status === "running" ? (
          <span className="wf-run__spin">
            <Spinner size={8} />
          </span>
        ) : (
          <Dot tone={runTone(run.status)} />
        )}
        {runWord(run.status)}
      </span>
      <span className="wf-muted">{ago(run.startedAt)}</span>
    </>
  );
}

// ---- editor -------------------------------------------------------------------

function WorkflowEditor({ workflow }: { workflow: Workflow }) {
  return (
    <ReactFlowProvider>
      <EditorBody workflow={workflow} />
    </ReactFlowProvider>
  );
}

function EditorBody({ workflow }: { workflow: Workflow }) {
  const store = useStore();
  const app = useAppState();
  const controller = useWorkflows();
  const snapshot = useWorkflowsState();
  const flow = useReactFlow();
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [runsOpen, setRunsOpen] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const id = workflow.id;

  // Every edit reads the freshest workflow, so two edits in one tick both land.
  const commit = useCallback(
    (change: (current: Workflow) => Workflow) => {
      const current = controller
        .getSnapshot()
        .document.workflows.find((entry) => entry.id === id);
      if (current) controller.update(change(current));
    },
    [controller, id],
  );

  const issues = useMemo(() => validateWorkflow(workflow), [workflow]);
  const active = snapshot.activeRuns[id];
  const runs = useMemo(
    () => runsOf(snapshot.document.runs, id, active),
    [snapshot.document.runs, id, active],
  );
  const context = useMemo(
    () => ({ employees: app.employees, projects: app.projects }),
    [app.employees, app.projects],
  );
  // Nodes stay neutral unless a run is live, or a run is open in the Runs panel.
  const overlay =
    active ??
    (runsOpen ? runs.find((run) => expanded[run.id] === true) : undefined) ??
    null;
  const focusNode = (nodeId: string) => {
    setSelectedIds([nodeId]);
    void flow.fitView({ nodes: [{ id: nodeId }], maxZoom: 1, duration: 0 });
  };
  const present = selectedIds.filter((entry) =>
    workflow.nodes.some((node) => node.id === entry),
  );
  const selected =
    present.length === 1
      ? workflow.nodes.find((node) => node.id === present[0])
      : undefined;

  return (
    <div className="wf-editor">
      <WorkflowToolbar
        workflow={workflow}
        issues={issues}
        saving={snapshot.saving}
        running={active !== undefined}
        runsOpen={runsOpen}
        onBack={() => store.navigate({ kind: "workflows", id: null })}
        onRename={(name) => commit((current) => ({ ...current, name }))}
        onIssue={(issue) => {
          if (issue.nodeId) focusNode(issue.nodeId);
        }}
        onToggleRuns={() => {
          setRunsOpen((value) => !value);
          setSelectedIds([]);
        }}
        onEnabled={(enabled) =>
          requestEnabled(controller, store, workflow, enabled)
        }
        onRun={() => {
          const issue = requestRun(controller, store, workflow);
          if (issue?.nodeId) focusNode(issue.nodeId);
        }}
        onCancel={() => controller.cancelRun(id)}
      />
      <div className="wf-editor__body">
        <WorkflowCanvas
          workflow={workflow}
          issues={issues}
          run={overlay}
          selectedIds={present}
          onSelect={setSelectedIds}
          commit={commit}
          context={context}
        />
        {selected ? (
          <NodeInspector
            key={selected.id}
            node={selected}
            onLabel={(label) =>
              commit((current) => patchNode(current, selected.id, { label }))
            }
            onConfig={(config) =>
              commit((current) => patchNode(current, selected.id, { config }))
            }
            onDelete={() => {
              commit((current) => removeNodes(current, new Set([selected.id])));
              setSelectedIds([]);
            }}
          />
        ) : runsOpen ? (
          <RunsPanel
            workflow={workflow}
            runs={runs}
            expanded={expanded}
            onExpand={(runId, value) =>
              setExpanded((current) => ({ ...current, [runId]: value }))
            }
          />
        ) : null}
      </div>
    </div>
  );
}
