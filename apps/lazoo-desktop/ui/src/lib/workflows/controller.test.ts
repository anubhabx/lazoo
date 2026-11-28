import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createFakeBridge, type FakeBridge } from "../../test/fakeBridge";
import type { Store } from "../store";
import {
  RUNS_KEPT,
  SAVE_DEBOUNCE_MS,
  TEMPLATES,
  TICK_MS,
  WorkflowsController,
} from "./controller";
import { edge, node, workflow } from "./fixtures";
import type { Workflow, WorkflowDocument, WorkflowRun } from "./types";

const MINUTE = 60_000;
let bridge: FakeBridge;
let toast: ReturnType<typeof vi.fn>;
let controller: WorkflowsController;
let ids: number;

function document(
  workflows: Workflow[],
  extra: Partial<WorkflowDocument> = {},
) {
  return { version: 1, workflows, runs: [], lastFired: {}, ...extra };
}

/** A schedule trigger every 30 minutes, then a notice. */
function scheduled(overrides: Partial<Workflow> = {}) {
  return workflow(
    [
      node("t", "trigger.schedule", {
        schedule: { mode: "interval", everyMinutes: 30 },
      }),
      node("n", "notify", { title: "Ping", body: "hello" }),
    ],
    [edge("t", "n")],
    overrides,
  );
}

function build() {
  controller = new WorkflowsController(bridge, { toast } as unknown as Store, {
    sleep: async () => undefined,
    newId: () => `id-${(ids += 1)}`,
  });
  return controller;
}

const pings = () =>
  toast.mock.calls.filter(([entry]) => entry.title === "Ping").length;

async function settle() {
  await vi.advanceTimersByTimeAsync(0);
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 2, 9, 0, 0));
  bridge = createFakeBridge();
  toast = vi.fn();
  ids = 0;
});

afterEach(() => {
  controller?.dispose();
  vi.useRealTimers();
});

describe("WorkflowsController scheduling", () => {
  it("fires a due trigger once and not again within the interval", async () => {
    const lastFired = {
      "wf:t": new Date(Date.now() - 31 * MINUTE).toISOString(),
    };
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([scheduled()], { lastFired });
    build();
    await controller.start();
    await settle();
    expect(pings()).toBe(1);
    expect(controller.getSnapshot().document.runs).toHaveLength(1);
    expect(controller.getSnapshot().document.runs[0]).toMatchObject({
      trigger: "schedule",
      status: "succeeded",
    });

    await vi.advanceTimersByTimeAsync(25 * MINUTE);
    expect(pings()).toBe(1);
    await vi.advanceTimersByTimeAsync(6 * MINUTE);
    expect(pings()).toBe(2);
  });

  it("does not replay missed occurrences", async () => {
    const lastFired = {
      "wf:t": new Date(Date.now() - 24 * 60 * MINUTE).toISOString(),
    };
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([scheduled()], { lastFired });
    build();
    await controller.start();
    await vi.advanceTimersByTimeAsync(5 * TICK_MS);
    expect(pings()).toBe(1);
  });

  it("does not fire immediately when a workflow is enabled", async () => {
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([
      scheduled({ enabled: false }),
    ]);
    build();
    await controller.start();
    controller.setEnabled("wf", true);
    await vi.advanceTimersByTimeAsync(29 * MINUTE);
    expect(pings()).toBe(0);
    expect(
      controller.nextRun(controller.getSnapshot().document.workflows[0]!),
    ).not.toBeNull();
    await vi.advanceTimersByTimeAsync(2 * MINUTE);
    expect(pings()).toBe(1);
  });

  it("skips a workflow that already has an active run", async () => {
    const flow = workflow(
      [
        node("t", "trigger.schedule", {
          schedule: { mode: "interval", everyMinutes: 1 },
        }),
        node("w", "logic.wait", { minutes: 10 }),
      ],
      [edge("t", "w")],
    );
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([flow], {
      lastFired: { "wf:t": new Date(Date.now() - 2 * MINUTE).toISOString() },
    });
    let release!: () => void;
    controller = new WorkflowsController(
      bridge,
      { toast } as unknown as Store,
      {
        sleep: () => new Promise<void>((resolve) => (release = resolve)),
        newId: () => `id-${(ids += 1)}`,
      },
    );
    await controller.start();
    await settle();
    expect(Object.keys(controller.getSnapshot().activeRuns)).toEqual(["wf"]);
    await vi.advanceTimersByTimeAsync(5 * MINUTE);
    expect(ids).toBe(1);
    release();
    await settle();
    expect(controller.getSnapshot().document.runs).toHaveLength(1);
  });
});

describe("WorkflowsController running", () => {
  it("runNow prefers the manual trigger and keeps only the newest 20 runs", async () => {
    const flow = workflow(
      [
        node("s", "trigger.schedule"),
        node("m", "trigger.manual"),
        node("n", "notify", { title: "Ping", body: "x" }),
      ],
      [edge("s", "n"), edge("m", "n")],
    );
    const old: WorkflowRun[] = Array.from({ length: 25 }, (_, i) => ({
      id: `old-${i}`,
      workflowId: "wf",
      trigger: "manual",
      triggerNodeId: "m",
      status: "succeeded",
      startedAt: new Date(2026, 9, 1, 0, 25 - i).toISOString(),
      finishedAt: null,
      steps: [],
    }));
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([flow], { runs: old });
    build();
    await controller.start();
    const run = await controller.runNow("wf");
    expect(run).toMatchObject({ triggerNodeId: "m", status: "succeeded" });
    const { runs } = controller.getSnapshot().document;
    expect(runs).toHaveLength(RUNS_KEPT);
    expect(runs[0]!.id).toBe(run!.id);
    expect(runs[1]!.id).toBe("old-0");
    expect(controller.getSnapshot().activeRuns).toEqual({});
  });

  it("cancels an active run", async () => {
    const flow = workflow(
      [node("m", "trigger.manual"), node("w", "logic.wait", { minutes: 10 })],
      [edge("m", "w")],
    );
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([flow]);
    controller = new WorkflowsController(
      bridge,
      { toast } as unknown as Store,
      {
        sleep: (_ms, signal) =>
          new Promise<void>((_resolve, reject) =>
            signal.addEventListener("abort", () =>
              reject(new Error("Cancelled")),
            ),
          ),
        newId: () => `id-${(ids += 1)}`,
      },
    );
    await controller.start();
    const pending = controller.runNow("wf");
    await settle();
    expect(controller.getSnapshot().activeRuns.wf).toBeDefined();
    controller.cancelRun("wf");
    expect((await pending)!.status).toBe("cancelled");
  });
});

describe("WorkflowsController persistence", () => {
  it("starts empty when nothing is stored and surfaces load errors", async () => {
    build();
    await controller.start();
    expect(controller.getSnapshot()).toMatchObject({
      status: "ready",
      error: null,
    });
    expect(controller.getSnapshot().document.workflows).toEqual([]);

    controller.dispose();
    bridge.workflows.load.mockRejectedValueOnce({
      code: "LZ-WORKFLOWS-CORRUPT",
      message: "unreadable",
    });
    build();
    await controller.start();
    expect(controller.getSnapshot()).toMatchObject({
      status: "error",
      error: { code: "LZ-WORKFLOWS-CORRUPT" },
    });
  });

  it("debounces saves, bumps updatedAt and checks the revision", async () => {
    build();
    await controller.start();
    const created = controller.create("From template", TEMPLATES[0]);
    controller.update({ ...created, name: "Renamed" });
    controller.update({ ...created, name: "Renamed again" });
    expect(bridge.workflows.save).not.toHaveBeenCalled();
    expect(controller.getSnapshot().saving).toBe(true);
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect(bridge.workflows.save).toHaveBeenCalledTimes(1);
    expect(bridge.workflows.save.mock.calls[0]![1]).toBe(0);
    expect(bridge.workflows.stored.revision).toBe(1);
    expect(controller.getSnapshot().saving).toBe(false);
    const [saved] = (bridge.workflows.stored.document as WorkflowDocument)
      .workflows;
    expect(saved).toMatchObject({ name: "Renamed again", enabled: false });
    expect(saved!.nodes).toHaveLength(3);

    controller.remove(created.id);
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect(bridge.workflows.save.mock.calls[1]![1]).toBe(1);
    expect(
      (bridge.workflows.stored.document as WorkflowDocument).workflows,
    ).toEqual([]);
  });

  it("reloads and says so when the file changed elsewhere", async () => {
    bridge.workflows.stored.revision = 1;
    bridge.workflows.stored.document = document([]);
    build();
    await controller.start();
    bridge.workflows.stored.revision = 7;
    bridge.workflows.stored.document = document([
      scheduled({ id: "other", enabled: false }),
    ]);
    controller.create("Mine");
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect(
      controller.getSnapshot().document.workflows.map((w) => w.id),
    ).toEqual(["other"]);
    expect(toast).toHaveBeenCalledWith(
      expect.objectContaining({ tone: "info", title: "Workflows reloaded" }),
    );
    // The next edit saves against the new revision.
    controller.create("Again");
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
    expect(bridge.workflows.stored.revision).toBe(8);
  });
});

describe("TEMPLATES", () => {
  it("are connected left to right with a trigger first", () => {
    expect(TEMPLATES.map((t) => t.name)).toEqual([
      "Daily summary",
      "Nightly checks",
    ]);
    for (const template of TEMPLATES) {
      const xs = template.nodes.map((n) => n.position.x);
      expect(xs).toEqual([...xs].sort((a, b) => a - b));
      expect(template.nodes[0]!.kind).toBe("trigger.schedule");
      expect(template.edges.length).toBeGreaterThanOrEqual(
        template.nodes.length - 1,
      );
    }
  });
});
