// One workflows controller for the main window: loads and saves the document through the
// desktop bridge, ticks the scheduler, and runs workflows. No React; the UI reads it with
// `useWorkflowsState()`.
//
// ponytail: schedules only fire while the Lazoo window is open. When workflows must run
// with the window closed, move this scheduler into lazood (it already outlives the window).

import type { Bridge } from "../bridge";
import { asFailure } from "../bridge";
import type { RpcFailure } from "../protocol";
import type { Store } from "../store";
import { newKey } from "../util";
import { runWorkflow, type EngineDeps } from "./engine";
import { nextFire } from "./schedule";
import {
  NODE_CATALOG,
  emptyDocument,
  isTrigger,
  lastFiredKey,
  parseDocument,
  type NodeConfigs,
  type NodeKind,
  type Workflow,
  type WorkflowDocument,
  type WorkflowEdge,
  type WorkflowNode,
  type WorkflowRun,
} from "./types";

export interface WorkflowsSnapshot {
  status: "loading" | "ready" | "error";
  error: RpcFailure | null;
  document: WorkflowDocument;
  activeRuns: Record<string, WorkflowRun>;
  saving: boolean;
}

export interface WorkflowsDeps {
  now: () => Date;
  sleep: (ms: number, signal: AbortSignal) => Promise<void>;
  newId: () => string;
}

export const TICK_MS = 30_000;
export const SAVE_DEBOUNCE_MS = 400;
export const RUNS_KEPT = 20;

export interface WorkflowTemplate {
  id: string;
  name: string;
  description: string;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
}

function node<K extends NodeKind>(
  id: string,
  kind: K,
  label: string,
  x: number,
  y: number,
  config: Partial<NodeConfigs[K]> = {},
): WorkflowNode {
  return {
    id,
    kind,
    label,
    position: { x, y },
    config: { ...NODE_CATALOG[kind].defaultConfig(), ...config },
  } as WorkflowNode;
}

export const TEMPLATES: WorkflowTemplate[] = [
  {
    id: "daily-summary",
    name: "Daily summary",
    description:
      "Every weekday morning, ask the lead to summarise yesterday's changes and show the result.",
    nodes: [
      node("schedule", "trigger.schedule", "Weekdays at 09:00", 0, 0, {
        schedule: { mode: "daily", time: "09:00", days: [1, 2, 3, 4, 5] },
      }),
      node("agent", "agent.prompt", "Summarise yesterday", 320, 0, {
        prompt:
          "Summarise yesterday's changes in this project: what was merged or finished, what is still in progress, and anything that needs my attention. Keep it short.",
      }),
      node("notify", "notify", "Show the summary", 640, 0, {
        title: "Daily summary",
        body: "{{input}}",
      }),
    ],
    edges: [
      { id: "e1", source: "schedule", sourceHandle: "out", target: "agent" },
      { id: "e2", source: "agent", sourceHandle: "out", target: "notify" },
    ],
  },
  {
    id: "nightly-checks",
    name: "Nightly checks",
    description:
      "Every night, have an agent run the test suite and raise a notice when something fails.",
    nodes: [
      node("schedule", "trigger.schedule", "Every day at 22:00", 0, 0, {
        schedule: {
          mode: "daily",
          time: "22:00",
          days: [0, 1, 2, 3, 4, 5, 6],
        },
      }),
      node("agent", "agent.prompt", "Run the tests", 320, 0, {
        prompt:
          "Run the project's test suite and report the result. Say 'all passing' when everything passes; otherwise list each failing test with the reason.",
        timeoutMinutes: 60,
      }),
      node("condition", "logic.condition", "Did it fail?", 640, 0, {
        op: "contains",
        value: "fail",
      }),
      node("notify", "notify", "Checks failed", 960, -70, {
        title: "Checks failed",
        body: "{{input}}",
      }),
    ],
    edges: [
      { id: "e1", source: "schedule", sourceHandle: "out", target: "agent" },
      { id: "e2", source: "agent", sourceHandle: "out", target: "condition" },
      { id: "e3", source: "condition", sourceHandle: "true", target: "notify" },
    ],
  },
];

const defaultSleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    if (signal.aborted) {
      reject(new Error("Cancelled"));
      return;
    }
    const done = () => {
      clearTimeout(timer);
      reject(new Error("Cancelled"));
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", done);
      resolve();
    }, ms);
    signal.addEventListener("abort", done, { once: true });
  });

export class WorkflowsController {
  private snapshot: WorkflowsSnapshot = {
    status: "loading",
    error: null,
    document: emptyDocument(),
    activeRuns: {},
    saving: false,
  };
  private readonly listeners = new Set<() => void>();
  private readonly deps: WorkflowsDeps;
  private readonly aborts = new Map<string, AbortController>();
  private revision = 0;
  private tickTimer: ReturnType<typeof setInterval> | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private saveRunning: Promise<void> | null = null;
  private dirty = false;
  private started = false;
  private disposed = false;
  /** Bumped by every start and dispose, so a start that was disposed mid-load stands down. */
  private generation = 0;

  constructor(
    private readonly bridge: Bridge,
    private readonly store: Store,
    deps: Partial<WorkflowsDeps> = {},
  ) {
    this.deps = {
      now: () => new Date(),
      sleep: defaultSleep,
      newId: newKey,
      ...deps,
    };
  }

  // ---- subscription ---------------------------------------------------

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;

  private set(patch: Partial<WorkflowsSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch };
    for (const listener of this.listeners) listener();
  }

  private setDocument(document: WorkflowDocument) {
    this.set({ document });
  }

  // ---- lifecycle ------------------------------------------------------

  async start() {
    if (this.started) return;
    this.started = true;
    // React StrictMode runs effects twice (start, dispose, start): a disposed
    // controller must be startable again.
    this.disposed = false;
    const generation = ++this.generation;
    await this.load();
    if (this.disposed || generation !== this.generation) return;
    this.tickTimer = setInterval(() => this.tick(), TICK_MS);
    this.tick();
  }

  dispose() {
    this.disposed = true;
    this.started = false;
    this.generation += 1;
    if (this.tickTimer) clearInterval(this.tickTimer);
    this.tickTimer = null;
    for (const abort of this.aborts.values()) abort.abort();
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
      void this.flush();
    }
  }

  private async load() {
    try {
      const loaded = await this.bridge.workflows.load();
      this.revision = loaded.revision;
      this.set({
        status: "ready",
        error: null,
        document: parseDocument(loaded.document, this.deps.now()),
      });
    } catch (error) {
      this.set({ status: "error", error: asFailure(error) });
    }
  }

  // ---- editing --------------------------------------------------------

  create(name: string, template?: WorkflowTemplate): Workflow {
    const stamp = this.deps.now().toISOString();
    const nodes = template
      ? structuredClone(template.nodes)
      : [node("trigger", "trigger.schedule", "Schedule", 0, 0)];
    const workflow: Workflow = {
      id: this.deps.newId(),
      name: name.trim() || template?.name || "Untitled workflow",
      enabled: false,
      nodes,
      edges: template ? structuredClone(template.edges) : [],
      createdAt: stamp,
      updatedAt: stamp,
    };
    this.setDocument({
      ...this.snapshot.document,
      workflows: [...this.snapshot.document.workflows, workflow],
    });
    this.scheduleSave();
    return workflow;
  }

  update(workflow: Workflow) {
    const stamp = this.deps.now().toISOString();
    const { document } = this.snapshot;
    if (!document.workflows.some((entry) => entry.id === workflow.id)) return;
    this.setDocument({
      ...document,
      workflows: document.workflows.map((entry) =>
        entry.id === workflow.id ? { ...workflow, updatedAt: stamp } : entry,
      ),
    });
    this.scheduleSave();
  }

  remove(id: string) {
    this.aborts.get(id)?.abort();
    const { document } = this.snapshot;
    const prefix = `${id}:`;
    this.setDocument({
      ...document,
      workflows: document.workflows.filter((entry) => entry.id !== id),
      runs: document.runs.filter((run) => run.workflowId !== id),
      lastFired: Object.fromEntries(
        Object.entries(document.lastFired).filter(
          ([key]) => !key.startsWith(prefix),
        ),
      ),
    });
    this.scheduleSave();
  }

  /** Enabling records "now" for each schedule trigger, so nothing fires immediately. */
  setEnabled(id: string, enabled: boolean) {
    const { document } = this.snapshot;
    const workflow = document.workflows.find((entry) => entry.id === id);
    if (!workflow || workflow.enabled === enabled) return;
    const lastFired = { ...document.lastFired };
    if (enabled) {
      const stamp = this.deps.now().toISOString();
      for (const entry of workflow.nodes) {
        if (entry.kind === "trigger.schedule")
          lastFired[lastFiredKey(id, entry.id)] = stamp;
      }
    }
    this.setDocument({
      ...document,
      workflows: document.workflows.map((entry) =>
        entry.id === id
          ? { ...entry, enabled, updatedAt: this.deps.now().toISOString() }
          : entry,
      ),
      lastFired,
    });
    this.scheduleSave();
  }

  // ---- running --------------------------------------------------------

  /** The manual trigger if there is one, else the first trigger. */
  async runNow(id: string): Promise<WorkflowRun | null> {
    const workflow = this.snapshot.document.workflows.find(
      (entry) => entry.id === id,
    );
    if (!workflow || this.snapshot.activeRuns[id]) return null;
    const trigger =
      workflow.nodes.find((entry) => entry.kind === "trigger.manual") ??
      workflow.nodes.find((entry) => isTrigger(entry.kind));
    if (!trigger) {
      this.store.toast({
        tone: "error",
        title: "Nothing to run",
        detail: `${workflow.name} has no trigger.`,
      });
      return null;
    }
    return this.execute(workflow, trigger.id);
  }

  cancelRun(workflowId: string) {
    this.aborts.get(workflowId)?.abort();
  }

  /** When the workflow next fires, or null when it is off or has no schedule. */
  nextRun(workflow: Workflow): Date | null {
    if (!workflow.enabled) return null;
    const now = this.deps.now();
    let next: Date | null = null;
    for (const entry of workflow.nodes) {
      if (entry.kind !== "trigger.schedule") continue;
      const config = entry.config as NodeConfigs["trigger.schedule"];
      const base =
        this.snapshot.document.lastFired[lastFiredKey(workflow.id, entry.id)];
      let candidate = nextFire(config.schedule, base ? new Date(base) : now);
      if (candidate < now) candidate = now;
      if (!next || candidate < next) next = candidate;
    }
    return next;
  }

  private async execute(
    workflow: Workflow,
    triggerNodeId: string,
  ): Promise<WorkflowRun> {
    const abort = new AbortController();
    this.aborts.set(workflow.id, abort);
    const deps: EngineDeps = {
      call: (method, params) => this.bridge.call(method, params),
      now: this.deps.now,
      sleep: this.deps.sleep,
      notify: (title, body) =>
        this.store.toast({ tone: "info", title, detail: body }),
      signal: abort.signal,
      newId: this.deps.newId,
    };
    const track = (run: WorkflowRun) => {
      if (this.disposed) return;
      this.set({
        activeRuns: { ...this.snapshot.activeRuns, [workflow.id]: run },
      });
    };
    // runWorkflow reports its first state synchronously, which claims the workflow's slot.
    const run = await runWorkflow(workflow, triggerNodeId, deps, track);
    this.aborts.delete(workflow.id);
    const activeRuns = Object.fromEntries(
      Object.entries(this.snapshot.activeRuns).filter(
        ([id]) => id !== workflow.id,
      ),
    );
    const document = this.snapshot.document;
    this.set({
      activeRuns,
      document: {
        ...document,
        runs: pruneRuns([run, ...document.runs]),
      },
    });
    if (run.status === "failed") {
      const failed = run.steps.find((step) => step.status === "failed");
      this.store.toast({
        tone: "error",
        title: `${workflow.name} failed`,
        detail: failed?.error ?? undefined,
      });
    }
    this.scheduleSave();
    return run;
  }

  // ---- scheduler ------------------------------------------------------

  /** Fires due schedule triggers. Missed occurrences are never replayed. */
  private tick() {
    if (this.disposed || this.snapshot.status !== "ready") return;
    const now = this.deps.now();
    const stamp = now.toISOString();
    const { document } = this.snapshot;
    const lastFired = { ...document.lastFired };
    let changed = false;
    const due: { workflow: Workflow; triggerId: string }[] = [];
    for (const workflow of document.workflows) {
      if (!workflow.enabled) continue;
      let claimed = this.snapshot.activeRuns[workflow.id] !== undefined;
      for (const entry of workflow.nodes) {
        if (entry.kind !== "trigger.schedule") continue;
        const key = lastFiredKey(workflow.id, entry.id);
        const base = lastFired[key];
        if (!base) {
          // A trigger added while enabled starts counting from now.
          lastFired[key] = stamp;
          changed = true;
          continue;
        }
        const config = entry.config as NodeConfigs["trigger.schedule"];
        if (nextFire(config.schedule, new Date(base)) > now) continue;
        lastFired[key] = stamp;
        changed = true;
        // One run per workflow at a time; a busy workflow skips this occurrence.
        if (claimed) continue;
        claimed = true;
        due.push({ workflow, triggerId: entry.id });
      }
    }
    if (changed) {
      this.setDocument({ ...document, lastFired });
      this.scheduleSave();
    }
    for (const { workflow, triggerId } of due) {
      void this.execute(workflow, triggerId);
    }
  }

  // ---- persistence ----------------------------------------------------

  private scheduleSave() {
    this.dirty = true;
    if (this.disposed) {
      void this.flush();
      return;
    }
    this.set({ saving: true });
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      void this.flush();
    }, SAVE_DEBOUNCE_MS);
  }

  /** Saves now; saves never overlap, and a change made mid-save is saved after it. */
  flush(): Promise<void> {
    if (this.saveRunning) return this.saveRunning;
    this.saveRunning = (async () => {
      try {
        while (this.dirty) {
          this.dirty = false;
          try {
            const saved = await this.bridge.workflows.save(
              this.snapshot.document,
              this.revision,
            );
            this.revision = saved.revision;
          } catch (error) {
            const failure = asFailure(error);
            if (failure.code === "LZ-WORKFLOWS-CONFLICT") {
              this.dirty = false;
              await this.load();
              this.store.toast({
                tone: "info",
                title: "Workflows reloaded",
                detail:
                  "They were changed elsewhere, so your last edit was not saved.",
              });
            } else {
              this.store.toast({
                tone: "error",
                title: failure.code,
                detail: `${failure.message} Your workflows were not saved.`,
              });
            }
            break;
          }
        }
      } finally {
        this.saveRunning = null;
        this.set({ saving: this.dirty || this.saveTimer !== null });
      }
    })();
    return this.saveRunning;
  }
}

/** Newest `RUNS_KEPT` finished runs per workflow; running ones are never dropped. */
function pruneRuns(runs: WorkflowRun[]): WorkflowRun[] {
  const kept = new Map<string, number>();
  return runs.filter((run) => {
    if (run.status === "running") return true;
    const count = (kept.get(run.workflowId) ?? 0) + 1;
    kept.set(run.workflowId, count);
    return count <= RUNS_KEPT;
  });
}
