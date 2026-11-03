import { useEffect, useMemo, useRef, useState } from "react";

import { useBridge } from "../lib/context";
import type {
  ConversationTurnRecord,
  ConversationTurnSummary,
  WorkspaceConversationResult,
} from "../lib/protocol";
import {
  blockFromRecord,
  blockFromSummary,
  stateWord,
  turnTone,
  type TranscriptBlock,
} from "../lib/transcript";
import { clockTime } from "../lib/util";
import { Button } from "./primitives";

const ACTIVE = new Set(["recorded", "submitting", "accepted"]);

interface Props {
  conversation: WorkspaceConversationResult;
  /** `null` shows the lead's turns; a task id shows that worker's turns. */
  taskId: string | null;
  /** Who answers in this list (the lead or the worker). */
  agentName: string;
  names: Map<string, string>;
  pulse: number;
}

/**
 * What Lazoo knows for certain about each turn: the request, the agent's reply,
 * how many tools and collaboration calls finished, and errors. The harness's
 * own TUI shows everything else.
 */
export function ActivityPane({
  conversation,
  taskId,
  agentName,
  names,
  pulse,
}: Props) {
  const bridge = useBridge();
  const [records, setRecords] = useState<Map<string, ConversationTurnRecord>>(
    new Map(),
  );
  const [older, setOlder] = useState<ConversationTurnSummary[]>([]);
  const [cursor, setCursor] = useState<number | null>(
    conversation.nextBeforeTurnSequence,
  );
  const [loadingOlder, setLoadingOlder] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const conversationId = conversation.conversation.id;

  useEffect(() => {
    setOlder([]);
    setCursor(conversation.nextBeforeTurnSequence);
    setRecords(new Map());
    stickToBottom.current = true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationId]);

  const turns = useMemo(() => {
    const merged = new Map<string, ConversationTurnSummary>();
    for (const turn of [...older, ...conversation.turns])
      merged.set(turn.id, turn);
    const rootId = conversation.rootTask?.id ?? null;
    return [...merged.values()]
      .filter((turn) =>
        taskId === null
          ? turn.taskId === null || turn.taskId === rootId
          : turn.taskId === taskId,
      )
      .sort((a, b) => a.sequence - b.sequence);
  }, [older, conversation.turns, conversation.rootTask, taskId]);

  // Fetch each turn's record once; re-fetch unfinished turns on every pulse.
  useEffect(() => {
    let cancelled = false;
    const wanted = turns.filter((turn) => {
      const known = records.get(turn.id);
      return (
        !known || ACTIVE.has(known.state) || known.revision !== turn.revision
      );
    });
    if (!wanted.length) return;
    (async () => {
      const queue = [...wanted];
      const worker = async () => {
        while (queue.length && !cancelled) {
          const turn = queue.shift()!;
          try {
            const record = await bridge.call<ConversationTurnRecord>(
              "turn.show",
              { id: turn.id },
            );
            if (cancelled) return;
            setRecords((previous) => new Map(previous).set(record.id, record));
          } catch {
            // The summary row still renders; the detail arrives on the next pulse.
          }
        }
      };
      await Promise.all([worker(), worker(), worker()]);
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [turns, pulse, bridge]);

  const blocks = useMemo<TranscriptBlock[]>(
    () =>
      turns.map((turn) => {
        const record = records.get(turn.id);
        return record ? blockFromRecord(record) : blockFromSummary(turn);
      }),
    [turns, records],
  );

  useEffect(() => {
    const element = scroller.current;
    if (element && stickToBottom.current)
      element.scrollTop = element.scrollHeight;
  }, [blocks]);

  async function loadOlder() {
    if (cursor === null || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await bridge.call<WorkspaceConversationResult>(
        "workspace.conversation",
        { id: conversationId, turnLimit: 50, beforeTurnSequence: cursor },
      );
      setOlder((previous) => [...page.turns, ...previous]);
      setCursor(page.nextBeforeTurnSequence);
      stickToBottom.current = false;
    } finally {
      setLoadingOlder(false);
    }
  }

  return (
    <div
      className="activity"
      ref={scroller}
      onScroll={() => {
        const element = scroller.current;
        if (element)
          stickToBottom.current =
            element.scrollHeight - element.scrollTop - element.clientHeight <
            48;
      }}
      aria-label="Activity"
    >
      {cursor !== null ? (
        <Button
          size="sm"
          variant="ghost"
          className="activity__older"
          onClick={() => void loadOlder()}
          disabled={loadingOlder}
        >
          {loadingOlder ? "Loading…" : "Load older turns"}
        </Button>
      ) : null}
      {blocks.length === 0 ? (
        <p className="activity__empty">
          {taskId === null
            ? "Requests and replies appear here once the first one is recorded."
            : "This worker's turns appear here once its task is dispatched."}
        </p>
      ) : null}
      {blocks.map((block) => (
        <Entry
          key={block.id}
          block={block}
          requester={
            block.author === "user"
              ? "You"
              : (names.get(block.authorId ?? "") ?? "Lead")
          }
          agentName={agentName}
        />
      ))}
    </div>
  );
}

function Entry({
  block,
  requester,
  agentName,
}: {
  block: TranscriptBlock;
  requester: string;
  agentName: string;
}) {
  const tone = turnTone(block.state);
  const counts = [
    block.toolRuns
      ? `${block.toolRuns} ${block.toolRuns === 1 ? "tool" : "tools"}`
      : "",
    block.collaboration
      ? `${block.collaboration} ${block.collaboration === 1 ? "team call" : "team calls"}`
      : "",
    block.reasoned ? "reasoned" : "",
  ].filter(Boolean);
  return (
    <article className="entry" data-tone={tone}>
      <header className="entry__head">
        <span className="entry__who">{requester}</span>
        <span className="entry__time">{clockTime(block.createdAt)}</span>
      </header>
      {block.text ? <p className="entry__request">{block.text}</p> : null}
      <div className="entry__reply">
        <header className="entry__head">
          <span className="entry__who">{agentName}</span>
          <span className="entry__state" data-tone={tone}>
            {stateWord(block.state)}
          </span>
          {counts.length ? (
            <span className="entry__counts">{counts.join(" · ")}</span>
          ) : null}
        </header>
        {block.assistantText ? (
          <Reply text={block.assistantText} />
        ) : tone === "active" ? (
          <p className="entry__pending">Working in the native view.</p>
        ) : null}
        {block.errors.map((error, index) => (
          <p key={index} className="entry__error">
            {error}
          </p>
        ))}
        {block.failureCode ? (
          <p className="entry__error">
            <span className="mono">{block.failureCode}</span>
            {block.failureMessage ? ` · ${block.failureMessage}` : ""}
          </p>
        ) : null}
        {block.truncated ? (
          <p className="entry__pending">Truncated by the daemon.</p>
        ) : null}
      </div>
    </article>
  );
}

const CLAMP = 480;

function Reply({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const long = text.length > CLAMP;
  return (
    <>
      <p className="entry__text">
        {long && !open ? `${text.slice(0, CLAMP).trimEnd()}…` : text}
      </p>
      {long ? (
        <button
          type="button"
          className="entry__more"
          onClick={() => setOpen((value) => !value)}
        >
          {open ? "Show less" : "Show all"}
        </button>
      ) : null}
    </>
  );
}
