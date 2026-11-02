// Activity: Lazoo's own record of a turn, derived from durable turn records.
// Harnesses do not report commands, diffs or tool arguments uniformly, so a
// turn is its request, the agent's reply, counts of what happened, and errors.

import type {
  ConversationTurnEvent,
  ConversationTurnRecord,
  ConversationTurnSummary,
  TurnState,
} from "./protocol";

export interface TranscriptBlock {
  id: string;
  author: "user" | "employee";
  authorId: string | null;
  taskId: string | null;
  state: TurnState;
  text: string;
  assistantText: string;
  /** Tool calls that finished (completed, failed or declined). */
  toolRuns: number;
  /** Lazoo collaboration calls (delegate, message, report) that finished. */
  collaboration: number;
  /** The harness streamed reasoning for this turn. */
  reasoned: boolean;
  errors: string[];
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
      return "queued";
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

const FINISHED = /complet|fail|declin|reject|error|cancel/;
/** Codex names Lazoo's dynamic tools `lazoo:*`; MCP clients use `mcp__lazoo__*`. */
const COLLABORATION = /^lazoo:|^mcp__lazoo/;

/** Fold provider events into a reply, counts and errors. Nothing is invented. */
export function foldEvents(events: ConversationTurnEvent[]): {
  assistantText: string;
  toolRuns: number;
  collaboration: number;
  reasoned: boolean;
  errors: string[];
} {
  let assistantText = "";
  let toolRuns = 0;
  let collaboration = 0;
  let reasoned = false;
  const errors: string[] = [];
  for (const event of events) {
    const kind = event.kind.toLowerCase();
    const payload = event.payload ?? {};
    if (kind.includes("text")) {
      assistantText += stringField(payload, [
        "text",
        "delta",
        "output",
        "message",
      ]);
    } else if (kind.includes("thought") || kind.includes("reasoning")) {
      reasoned = true;
    } else if (kind.includes("tool")) {
      const name = stringField(payload, ["toolName", "tool", "name"]);
      const status = stringField(payload, ["status", "state"]).toLowerCase();
      if (!FINISHED.test(status)) continue;
      if (COLLABORATION.test(name)) collaboration += 1;
      else toolRuns += 1;
    } else if (
      kind.includes("fail") ||
      kind.includes("error") ||
      kind.includes("reject")
    ) {
      errors.push(
        stringField(payload, ["message", "reason", "error"]) || event.kind,
      );
    }
  }
  return { assistantText, toolRuns, collaboration, reasoned, errors };
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
    toolRuns: 0,
    collaboration: 0,
    reasoned: false,
    errors: [],
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
    ...folded,
    failureCode: turn.failureCode,
    failureMessage: turn.failureMessage,
    completedAt: turn.completedAt ?? turn.terminalAt,
    truncated: turn.transcriptTruncated,
  };
}
