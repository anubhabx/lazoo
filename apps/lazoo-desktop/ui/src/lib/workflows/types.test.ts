import { describe, expect, it } from "vitest";

import {
  NODE_CATALOG,
  NODE_KINDS,
  emptyDocument,
  parseDocument,
} from "./types";

describe("parseDocument", () => {
  it("turns null and junk into an empty document", () => {
    for (const junk of [null, undefined, 3, "x", [], {}]) {
      expect(parseDocument(junk)).toEqual(emptyDocument());
    }
  });

  it("keeps good data and drops bad nodes, edges, workflows and runs", () => {
    const doc = parseDocument({
      version: 1,
      workflows: [
        {
          id: "w1",
          name: "Good",
          enabled: true,
          nodes: [
            { id: "t", kind: "trigger.manual", position: { x: 5, y: 6 } },
            { id: "x", kind: "teleport" },
            { id: "n", kind: "notify", config: { title: "Hi", body: 3 } },
            { kind: "notify" },
            null,
          ],
          edges: [
            { id: "e1", source: "t", sourceHandle: "out", target: "n" },
            { id: "e2", source: "t", sourceHandle: "out", target: "gone" },
            { id: "e3", source: "t", sourceHandle: "sideways", target: "n" },
          ],
        },
        { name: "no id" },
        7,
      ],
      runs: [
        {
          id: "r1",
          workflowId: "w1",
          status: "succeeded",
          steps: [
            { nodeId: "t", status: "succeeded" },
            { nodeId: "t", status: "bogus" },
          ],
        },
        { id: "r2", workflowId: "missing", status: "failed" },
        { id: "r3", workflowId: "w1", status: "running" },
        { id: "r4", workflowId: "w1", status: "weird" },
      ],
      lastFired: { "w1:t": "2026-11-01T09:00:00.000Z", bad: "nope", worse: 4 },
    });
    expect(doc.workflows).toHaveLength(1);
    const [flow] = doc.workflows;
    expect(flow!.nodes.map((n) => n.id)).toEqual(["t", "n"]);
    expect(flow!.nodes[0]!.position).toEqual({ x: 5, y: 6 });
    expect(flow!.nodes[1]!.config).toEqual({ title: "Hi", body: "" });
    expect(flow!.edges.map((e) => e.id)).toEqual(["e1"]);
    expect(doc.runs.map((r) => [r.id, r.status])).toEqual([
      ["r1", "succeeded"],
      ["r3", "failed"],
    ]);
    expect(doc.runs[0]!.steps).toHaveLength(1);
    expect(doc.lastFired).toEqual({ "w1:t": "2026-11-01T09:00:00.000Z" });
  });

  it("repairs out-of-range config instead of rejecting the node", () => {
    const doc = parseDocument({
      workflows: [
        {
          id: "w",
          nodes: [
            {
              id: "s",
              kind: "trigger.schedule",
              config: { schedule: { mode: "interval", everyMinutes: 999999 } },
            },
            {
              id: "d",
              kind: "trigger.schedule",
              config: {
                schedule: { mode: "daily", time: "25:99", days: [] },
              },
            },
            {
              id: "a",
              kind: "agent.prompt",
              config: { authority: "root", timeoutMinutes: -4 },
            },
          ],
        },
      ],
    });
    const [s, d, a] = doc.workflows[0]!.nodes;
    expect(s!.config).toEqual({
      schedule: { mode: "interval", everyMinutes: 10080 },
    });
    expect(d!.config).toEqual({
      schedule: { mode: "daily", time: "09:00", days: [0, 1, 2, 3, 4, 5, 6] },
    });
    expect(a!.config).toMatchObject({
      authority: "ask",
      timeoutMinutes: 1,
      employeeId: null,
    });
  });
});

describe("NODE_CATALOG", () => {
  it("has an entry and a fresh default config for every kind", () => {
    for (const kind of NODE_KINDS) {
      const entry = NODE_CATALOG[kind];
      expect(entry.kind).toBe(kind);
      expect(entry.outputs.length).toBeGreaterThan(0);
      expect(entry.defaultConfig()).not.toBe(entry.defaultConfig());
    }
    expect(NODE_CATALOG["logic.condition"].outputs).toEqual(["true", "false"]);
  });
});
