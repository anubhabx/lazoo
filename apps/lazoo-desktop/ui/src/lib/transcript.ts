// Lazoo's own rendering of a turn: blocks derived from durable turn records.

import type {
  ConversationTurnEvent,
  ConversationTurnRecord,
  ConversationTurnSummary,
  TurnState,
} from "./protocol";

export type EventTone =
  "text" | "thought" | "tool" | "terminal" | "state" | "error";

export interface TranscriptEvent {
  key: string;
  tone: EventTone;
  text: string;
  at: string;
}

export interface TranscriptBlock {
  id: string;
  author: "user" | "employee";
  authorId: string | null;
  taskId: string | null;
  state: TurnState;
  text: string;
  assistantText: string;
  thoughts: string[];
  events: TranscriptEvent[];
  failureCode: string | null;
  failureMessage: string | null;
  createdAt: string;
  completedAt: string | null;
  truncated: boolean;
}

export function isUserAuthor(
  author: ConversationTurnSummary["author"],
): boolean {
  return "kind" in author && author.kind === "user";
}

export function turnTone(
  state: TurnState,
): "active" | "failed" | "done" | "quiet" {
  switch (state) {
    case "recorded":
    case "submitting":
    case "accepted":
      return "active";
    case "retryable-failed":
    case "terminal-failed":
    case "uncertain":
      return "failed";
    case "completed":
      return "done";
    default:
      return "quiet";
  }
}

export function stateWord(state: TurnState): string {
  switch (state) {
    case "recorded":
      return "recorded";
    case "submitting":
      return "sending";
    case "accepted":
      return "working";
    case "completed":
      return "completed";
    case "retryable-failed":
      return "failed, retryable";
    case "terminal-failed":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "uncertain":
      return "uncertain";
    default:
      return state;
  }
}

function stringField(payload: Record<string, unknown>, keys: string[]): string {
  for (const key of keys) {
    const value = payload[key];
    if (typeof value === "string" && value.length) return value;
  }
  return "";
}

/** Fold raw provider events into a readable sequence. Nothing is invented. */
export function foldEvents(events: ConversationTurnEvent[]): {
  assistantText: string;
  thoughts: string[];
  rows: TranscriptEvent[];
} {
  let assistantText = "";
  let thought = "";
  const thoughts: string[] = [];
  const rows: TranscriptEvent[] = [];
  for (const event of events) {
    const kind = event.kind.toLowerCase();
    const payload = event.payload ?? {};
    const key = `${event.sequence}`;
    if (kind.includes("text")) {
      assistantText += stringField(payload, [
        "text",
        "delta",
        "output",
        "message",
      ]);
      continue;
    }
    if (kind.includes("thought") || kind.includes("reasoning")) {
      thought += stringField(payload, ["text", "delta"]);
      continue;
    }
    if (thought) {
      thoughts.push(thought);
      thought = "";
    }
    if (kind.includes("tool")) {
      const name = stringField(payload, ["toolName", "tool", "name"]) || "tool";
      const status = stringField(payload, ["status", "state"]);
      rows.push({
        key,
        tone: "tool",
        text: status ? `${name} · ${status}` : name,
        at: event.createdAt,
      });
      continue;
    }
    if (kind.includes("terminal")) {
      const output = stringField(payload, ["output", "text"]);
      if (output.trim()) {
        rows.push({
          key,
          tone: "terminal",
          text: output.trimEnd(),
          at: event.createdAt,
        });
      }
      continue;
    }
    if (
      kind.includes("fail") ||
      kind.includes("error") ||
      kind.includes("reject")
    ) {
      rows.push({
        key,
        tone: "error",
        text:
          stringField(payload, ["message", "reason", "error"]) || event.kind,
        at: event.createdAt,
      });
      continue;
    }
    rows.push({
      key,
      tone: "state",
      text: event.kind.replace(/[-_.]/g, " "),
      at: event.createdAt,
    });
  }
  if (thought) thoughts.push(thought);
  return { assistantText, thoughts, rows };
}

export function blockFromSummary(
  turn: ConversationTurnSummary,
): TranscriptBlock {
  const user = isUserAuthor(turn.author);
  return {
    id: turn.id,
    author: user ? "user" : "employee",
    authorId: user ? null : (turn.author as { employeeId: string }).employeeId,
    taskId: turn.taskId,
    state: turn.state,
    text: "",
    assistantText: "",
    thoughts: [],
    events: [],
    failureCode: null,
    failureMessage: null,
    createdAt: turn.createdAt,
    completedAt: null,
    truncated: false,
  };
}

export function blockFromRecord(turn: ConversationTurnRecord): TranscriptBlock {
  const base = blockFromSummary(turn);
  const folded = foldEvents(turn.events);
  return {
    ...base,
    text: turn.text,
    assistantText: folded.assistantText,
    thoughts: folded.thoughts,
    events: folded.rows,
    failureCode: turn.failureCode,
    failureMessage: turn.failureMessage,
    completedAt: turn.completedAt ?? turn.terminalAt,
    truncated: turn.transcriptTruncated,
  };
}
