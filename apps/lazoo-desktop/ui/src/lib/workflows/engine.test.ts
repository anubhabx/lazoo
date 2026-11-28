import { describe, expect, it } from "vitest";

import {
  CONVERSATION_ID,
  LEAD_ID,
  launcher,
  turnRecord,
} from "../../test/fixtures";
import type { TurnState } from "../protocol";
import { STEP_CAP, runWorkflow, type EngineDeps } from "./engine";
import { edge, node, workflow } from "./fixtures";
import type { WorkflowRun } from "./types";

interface Harness {
  deps: EngineDeps;
  calls: { method: string; params: Record<string, unknown> }[];
  notices: { title: string; body: string }[];
  sleeps: number[];
  updates: WorkflowRun[];
  abort: AbortController;
  /** Turn states `turn.show` answers with, one per poll (the last repeats). */
  turnStates: TurnState[];
  onSleep: (ms: number) => void;
}

function harness(): Harness {
  let clock = Date.parse("2026-11-02T09:00:00.000Z");
  let ids = 0;
  const abort = new AbortController();
  const h: Harness = {
    calls: [],
    notices: [],
    sleeps: [],
    updates: [],
    abort,
    turnStates: ["completed"],
    onSleep: () => undefined,
    deps: {
      signal: abort.signal,
      now: () => new Date(clock),
      newId: () => `run-${(ids += 1)}`,
      notify: (title, body) => h.notices.push({ title, body }),
      sleep: async (ms, signal) => {
        if (signal.aborted) throw new Error("Cancelled");
        h.sleeps.push(ms);
        clock += ms;
        h.onSleep(ms);
        if (signal.aborted) throw new Error("Cancelled");
      },
      call: async <T>(method: string, params: unknown) => {
        const p = params as Record<string, unknown>;
        h.calls.push({ method, params: p });
        switch (method) {
          case "workspace.launcher":
            return launcher() as T;
          case "conversation.create":
            return { conversation: { id: CONVERSATION_ID } } as T;
          case "turn.submit":
            return { turn: { id: "turn-1" } } as T;
          case "turn.show": {
            const state =
              h.turnStates.length > 1
                ? h.turnStates.shift()!
                : h.turnStates[0]!;
            return turnRecord({
              state,
              failureMessage:
                state === "terminal-failed" ? "provider died" : null,
            }) as T;
          }
        }
        throw { code: "LZ-TEST", message: `unscripted ${method}` };
      },
    },
  };
  return h;
}

const run = (h: Harness, flow: ReturnType<typeof workflow>, trigger = "t") =>
  runWorkflow(flow, trigger, h.deps, (r) => h.updates.push(r));

const agent = {
  employeeId: LEAD_ID,
  prompt: "summarise",
  authority: "ask",
} as const;

describe("runWorkflow", () => {
  it("runs a linear flow and passes output on as {{input}}", async () => {
    const h = harness();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("a", "agent.prompt", { ...agent, prompt: "Summarise it" }),
        node("n", "notify", { title: "Done", body: "Result: {{input}}" }),
      ],
      [edge("t", "a"), edge("a", "n")],
    );
    const result = await run(h, flow);
    expect(result.status).toBe("succeeded");
    expect(result.steps.map((s) => [s.nodeId, s.status])).toEqual([
      ["t", "succeeded"],
      ["a", "succeeded"],
      ["n", "succeeded"],
    ]);
    expect(h.notices).toEqual([
      { title: "Done", body: "Result: Reproduced locally in 4 of 20 runs." },
    ]);
    expect(result.steps[1]!.conversationId).toBe(CONVERSATION_ID);
    expect(result.finishedAt).not.toBeNull();
    expect(h.calls.map((c) => c.method)).toEqual([
      "workspace.launcher",
      "conversation.create",
      "turn.submit",
      "turn.show",
    ]);
    const [, create, submit] = h.calls;
    expect(create!.params).toMatchObject({
      leadEmployeeId: LEAD_ID,
      idempotencyKey: "wf-run-1-a-create",
    });
    expect(submit!.params).toMatchObject({
      conversationId: CONVERSATION_ID,
      authorityMode: "ask",
      text: "Summarise it",
      idempotencyKey: "wf-run-1-a-submit",
    });
  });

  it("calls onUpdate with fresh objects after state changes", async () => {
    const h = harness();
    const flow = workflow(
      [node("t", "trigger.manual"), node("n", "notify")],
      [edge("t", "n")],
    );
    await run(h, flow);
    expect(h.updates.length).toBeGreaterThan(3);
    expect(new Set(h.updates).size).toBe(h.updates.length);
    expect(h.updates[0]!.status).toBe("running");
    expect(h.updates.at(-1)!.status).toBe("succeeded");
    expect(h.updates[1]!.steps[0]!.status).toBe("running");
  });

  it("polls every three seconds until the turn finishes", async () => {
    const h = harness();
    h.turnStates = ["accepted", "accepted", "completed"];
    const flow = workflow(
      [node("t", "trigger.manual"), node("a", "agent.prompt", agent)],
      [edge("t", "a")],
    );
    expect((await run(h, flow)).status).toBe("succeeded");
    expect(h.sleeps).toEqual([3000, 3000]);
  });

  it("returns the conversation id without waiting when told not to", async () => {
    const h = harness();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("a", "agent.prompt", { ...agent, waitForReply: false }),
      ],
      [edge("t", "a")],
    );
    const result = await run(h, flow);
    expect(result.steps[1]!.output).toBe(CONVERSATION_ID);
    expect(h.calls.some((c) => c.method === "turn.show")).toBe(false);
  });

  it("fails the step on a failed turn or a timeout", async () => {
    const failed = harness();
    failed.turnStates = ["terminal-failed"];
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("a", "agent.prompt", { ...agent, timeoutMinutes: 1 }),
        node("n", "notify"),
      ],
      [edge("t", "a"), edge("a", "n")],
    );
    const result = await run(failed, flow);
    expect(result.status).toBe("failed");
    expect(result.steps.at(-1)).toMatchObject({
      nodeId: "a",
      status: "failed",
      error: "provider died",
    });
    expect(failed.notices).toEqual([]);

    const slow = harness();
    slow.turnStates = ["accepted"];
    const timedOut = await run(slow, flow);
    expect(timedOut.status).toBe("failed");
    expect(timedOut.steps.at(-1)!.error).toBe("No reply within 1 minutes.");
    expect(slow.sleeps.length).toBe(20);
  });

  it("branches on a condition, running only the taken side", async () => {
    const make = (value: string) =>
      workflow(
        [
          node("t", "trigger.manual"),
          node("a", "agent.prompt", agent),
          node("c", "logic.condition", { op: "contains", value }),
          node("yes", "notify", { title: "yes", body: "" }),
          node("no", "notify", { title: "no", body: "" }),
        ],
        [
          edge("t", "a"),
          edge("a", "c"),
          edge("c", "yes", "true"),
          edge("c", "no", "false"),
        ],
      );
    const hit = harness();
    await run(hit, make("REPRODUCED"));
    expect(hit.notices.map((n) => n.title)).toEqual(["yes"]);
    const miss = harness();
    const result = await run(miss, make("nothing like it"));
    expect(miss.notices.map((n) => n.title)).toEqual(["no"]);
    expect(result.steps.map((s) => s.nodeId)).toEqual(["t", "a", "c", "no"]);
  });

  it("ends quietly when the taken handle has no edge", async () => {
    const h = harness();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("c", "logic.condition", { op: "is-empty", value: "" }),
        node("n", "notify"),
      ],
      [edge("t", "c"), edge("c", "n", "false")],
    );
    const result = await run(h, flow);
    expect(result.status).toBe("succeeded");
    expect(h.notices).toEqual([]);
  });

  it("stops at the first failing step", async () => {
    const h = harness();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("a", "agent.prompt", { ...agent, employeeId: null }),
        node("n", "notify"),
      ],
      [edge("t", "a"), edge("a", "n")],
    );
    const result = await run(h, flow);
    expect(result.status).toBe("failed");
    expect(result.steps.map((s) => s.nodeId)).toEqual(["t", "a"]);
    expect(h.notices).toEqual([]);
  });

  it("fails when an RPC fails", async () => {
    const h = harness();
    h.deps.call = async () => {
      throw { code: "LZ-X", message: "daemon offline" };
    };
    const flow = workflow(
      [node("t", "trigger.manual"), node("a", "agent.prompt", agent)],
      [edge("t", "a")],
    );
    const result = await run(h, flow);
    expect(result.steps[1]).toMatchObject({
      status: "failed",
      error: "daemon offline",
    });
  });

  it("waits with sleep", async () => {
    const h = harness();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("w", "logic.wait", { minutes: 5 }),
        node("n", "notify", { title: "t", body: "{{input}}" }),
      ],
      [edge("t", "w"), edge("w", "n")],
    );
    const result = await run(h, flow);
    expect(h.sleeps).toEqual([300_000]);
    expect(result.status).toBe("succeeded");
  });

  it("is cancelled by abort, mid-wait", async () => {
    const h = harness();
    h.onSleep = () => h.abort.abort();
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("w", "logic.wait", { minutes: 5 }),
        node("n", "notify"),
      ],
      [edge("t", "w"), edge("w", "n")],
    );
    const result = await run(h, flow);
    expect(result.status).toBe("cancelled");
    expect(result.steps.map((s) => s.nodeId)).toEqual(["t", "w"]);
    expect(h.notices).toEqual([]);
  });

  it("runs each node at most once and tolerates cycles", async () => {
    const h = harness();
    const flow = workflow(
      [node("t", "trigger.manual"), node("n1", "notify"), node("n2", "notify")],
      [edge("t", "n1"), edge("t", "n2"), edge("n1", "n2"), edge("n2", "n1")],
    );
    const result = await run(h, flow);
    expect(result.status).toBe("succeeded");
    expect(result.steps.map((s) => s.nodeId)).toEqual(["t", "n1", "n2"]);
  });

  it("stops at the step cap", async () => {
    const h = harness();
    const count = STEP_CAP + 5;
    const nodes = [
      node("t", "trigger.manual"),
      ...Array.from({ length: count }, (_, i) => node(`n${i}`, "notify")),
    ];
    const edges = [
      edge("t", "n0"),
      ...Array.from({ length: count - 1 }, (_, i) =>
        edge(`n${i}`, `n${i + 1}`),
      ),
    ];
    const result = await run(h, workflow(nodes, edges));
    expect(result.status).toBe("failed");
    expect(result.steps.filter((s) => s.status === "succeeded")).toHaveLength(
      STEP_CAP,
    );
    expect(result.steps.at(-1)!.error).toContain("limited to 100 steps");
  });
});
