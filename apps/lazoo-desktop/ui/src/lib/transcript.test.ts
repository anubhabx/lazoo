import { describe, expect, test } from "vitest";

import { blockFromRecord, foldEvents, stateWord, turnTone } from "./transcript";
import { turnRecord } from "../test/fixtures";

describe("transcript folding", () => {
  test("text deltas join into one assistant text; tools become rows", () => {
    const folded = foldEvents(turnRecord().events);
    expect(folded.assistantText).toBe("Reproduced locally in 4 of 20 runs.");
    expect(folded.rows).toHaveLength(1);
    expect(folded.rows[0]).toMatchObject({
      tone: "tool",
      text: "task_delegate · completed",
    });
  });

  test("thoughts fold separately and flush before the next row", () => {
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
    expect(block.thoughts).toEqual(["Think harder"]);
    expect(block.events.map((row) => row.tone)).toEqual(["terminal", "error"]);
    expect(block.events[0]!.text).toBe("cargo test");
    expect(block.events[1]!.text).toBe("stalled");
  });

  test("a block never invents content: empty events stay empty", () => {
    const block = blockFromRecord(turnRecord({ events: [], text: "hi" }));
    expect(block.assistantText).toBe("");
    expect(block.events).toEqual([]);
    expect(block.text).toBe("hi");
    expect(block.author).toBe("user");
  });

  test("tones and words follow the turn state", () => {
    expect(turnTone("accepted")).toBe("active");
    expect(turnTone("uncertain")).toBe("failed");
    expect(turnTone("completed")).toBe("done");
    expect(stateWord("retryable-failed")).toBe("failed, retryable");
  });
});
