import { describe, expect, it } from "vitest";

import { edge, node, workflow } from "./fixtures";
import { outgoing, validateWorkflow } from "./graph";

const agent = { employeeId: "e1", prompt: "go" };

describe("validateWorkflow", () => {
  it("accepts a connected workflow, even with a one-sided condition", () => {
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("a", "agent.prompt", agent),
        node("c", "logic.condition", { value: "x" }),
        node("n", "notify"),
      ],
      [edge("t", "a"), edge("a", "c"), edge("c", "n", "true")],
    );
    expect(validateWorkflow(flow)).toEqual([]);
  });

  it("asks for a trigger", () => {
    const issues = validateWorkflow(workflow([node("n", "notify")], []));
    expect(issues[0]).toEqual({
      nodeId: null,
      message: "Add a trigger to start this workflow.",
    });
  });

  it("flags agent steps without an agent or prompt", () => {
    const flow = workflow(
      [node("t", "trigger.manual"), node("a", "agent.prompt")],
      [edge("t", "a")],
    );
    const messages = validateWorkflow(flow).map((issue) => issue.message);
    expect(messages).toContain("Choose an agent for this step.");
    expect(messages).toContain("Write a prompt for this step.");
  });

  it("flags unreachable nodes", () => {
    const flow = workflow(
      [node("t", "trigger.manual"), node("n1", "notify"), node("n2", "notify")],
      [edge("t", "n1")],
    );
    expect(validateWorkflow(flow)).toEqual([
      { nodeId: "n2", message: "This step is not connected to a trigger." },
    ]);
  });

  it("flags cycles", () => {
    const flow = workflow(
      [
        node("t", "trigger.manual"),
        node("w1", "logic.wait"),
        node("w2", "logic.wait"),
      ],
      [edge("t", "w1"), edge("w1", "w2"), edge("w2", "w1")],
    );
    expect(validateWorkflow(flow).map((issue) => issue.message)).toContain(
      "This step is part of a loop.",
    );
  });
});

describe("outgoing", () => {
  it("filters by handle and keeps edge order", () => {
    const flow = workflow(
      [node("c", "logic.condition"), node("a", "notify"), node("b", "notify")],
      [edge("c", "a", "true"), edge("c", "b", "false"), edge("c", "b", "true")],
    );
    expect(outgoing(flow, "c", "true").map((e) => e.target)).toEqual([
      "a",
      "b",
    ]);
    expect(outgoing(flow, "c", "false").map((e) => e.target)).toEqual(["b"]);
    expect(outgoing(flow, "c")).toHaveLength(3);
  });
});
