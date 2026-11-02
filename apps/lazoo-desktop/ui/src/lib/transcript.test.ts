import { describe, expect, test } from "vitest";

import { blockFromRecord, foldEvents, stateWord, turnTone } from "./transcript";
import { turnRecord } from "../test/fixtures";

describe("activity folding", () => {
  test("text deltas join into one reply; finished tools are counted", () => {
    const folded = foldEvents(turnRecord().events);
    expect(folded.assistantText).toBe("Reproduced locally in 4 of 20 runs.");
    expect(folded.toolRuns).toBe(1);
    expect(folded.collaboration).toBe(0);
  });

  test("reasoning is noted, terminal output is not listed, errors are kept", () => {
    const record = turnRecord({
      events: [
        {
          sequence: 1,
          kind: "thought-delta",
          provider: "codex",
          nativeSessionId: null,
          nativeTurnId: null,
          payload: { text: "Think " },
          createdAt: "2026-10-31T10:00:00Z",
        },
        {
          sequence: 2,
          kind: "thought-delta",
          provider: "codex",
          nativeSessionId: null,
          nativeTurnId: null,
          payload: { text: "harder" },
          createdAt: "2026-10-31T10:00:00Z",
        },
        {
          sequence: 3,
          kind: "terminal-update",
          provider: "codex",
          nativeSessionId: null,
          nativeTurnId: null,
          payload: { output: "cargo test\n" },
          createdAt: "2026-10-31T10:00:00Z",
        },
        {
          sequence: 4,
          kind: "turn-failed",
          provider: "codex",
          nativeSessionId: null,
          nativeTurnId: null,
          payload: { message: "stalled" },
          createdAt: "2026-10-31T10:00:00Z",
        },
      ],
    });
    const block = blockFromRecord(record);
    expect(block.reasoned).toBe(true);
    expect(block.toolRuns).toBe(0);
    expect(block.errors).toEqual(["stalled"]);
  });

  test("Lazoo collaboration calls count apart from tools; unfinished calls do not count", () => {
    const tool = (sequence: number, toolName: string, status: string) => ({
      sequence,
      kind: "tool-update",
      provider: "claude-code",
      nativeSessionId: null,
      nativeTurnId: null,
      payload: { toolName, status },
      createdAt: "2026-10-31T10:00:00Z",
    });
    const folded = foldEvents([
      tool(1, "Bash", "permission-requested"),
      tool(2, "Bash", "completed"),
      tool(3, "lazoo:task_delegate", "inProgress"),
      tool(4, "lazoo:task_delegate", "completed"),
      tool(5, "Edit", "failed"),
    ]);
    expect(folded.toolRuns).toBe(2);
    expect(folded.collaboration).toBe(1);
  });

  test("a block never invents content: empty events stay empty", () => {
    const block = blockFromRecord(turnRecord({ events: [], text: "hi" }));
    expect(block.assistantText).toBe("");
    expect(block.toolRuns).toBe(0);
    expect(block.errors).toEqual([]);
    expect(block.text).toBe("hi");
    expect(block.author).toBe("user");
  });

  test("tones and words follow the turn state", () => {
    expect(turnTone("accepted")).toBe("active");
    expect(turnTone("uncertain")).toBe("failed");
    expect(turnTone("completed")).toBe("done");
    expect(stateWord("retryable-failed")).toBe("failed, retryable");
    expect(stateWord("recorded")).toBe("queued");
  });
});
