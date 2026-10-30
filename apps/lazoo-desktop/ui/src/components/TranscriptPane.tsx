import { AnimatePresence, motion } from "motion/react";
import {
  AlertCircle,
  ChevronRight,
  Dot,
  SquareTerminal,
  Wrench,
} from "lucide-react";
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
  type TranscriptEvent,
} from "../lib/transcript";
import { clockTime } from "../lib/util";
import { Button, EASE } from "./primitives";

const ACTIVE = new Set(["recorded", "submitting", "accepted"]);

interface Props {
  conversation: WorkspaceConversationResult;
  /** `null` shows the lead's turns; a task id shows that worker's turns. */
  taskId: string | null;
  names: Map<string, string>;
  pulse: number;
}

export function TranscriptPane({ conversation, taskId, names, pulse }: Props) {
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
            setRecords((previous) => {
              const next = new Map(previous);
              next.set(record.id, record);
              return next;
            });
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
    if (!element || !stickToBottom.current) return;
    element.scrollTop = element.scrollHeight;
  }, [blocks]);

  function onScroll() {
    const element = scroller.current;
    if (!element) return;
    stickToBottom.current =
      element.scrollHeight - element.scrollTop - element.clientHeight < 48;
  }

  async function loadOlder() {
    if (cursor === null || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await bridge.call<WorkspaceConversationResult>(
        "workspace.conversation",
        {
          id: conversationId,
          turnLimit: 50,
          beforeTurnSequence: cursor,
        },
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
      className="transcript"
      ref={scroller}
      onScroll={onScroll}
      aria-label="Transcript"
    >
      {cursor !== null ? (
        <div className="transcript__older">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void loadOlder()}
            disabled={loadingOlder}
          >
            {loadingOlder ? "Loading…" : "Load older turns"}
          </Button>
        </div>
      ) : null}
      {blocks.length === 0 ? (
        <div className="empty">
          <span className="empty__title">No turns yet</span>
          <span>
            {taskId === null
              ? "The first request appears here as soon as it is recorded."
              : "This worker's turns appear here once the task is dispatched."}
          </span>
        </div>
      ) : null}
      <AnimatePresence initial={false}>
        {blocks.map((block) => (
          <motion.article
            key={block.id}
            className={`block block--${block.author === "user" ? "user" : "employee"} ${
              turnTone(block.state) === "active"
                ? "block--active"
                : turnTone(block.state) === "failed"
                  ? "block--failed"
                  : ""
            }`.trim()}
            layout="position"
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.22, ease: EASE }}
          >
            <div className="block__head">
              <strong>
                {block.author === "user"
                  ? "You"
                  : (names.get(block.authorId ?? "") ?? "Employee")}
              </strong>
              <span className="sep">·</span>
              <span>{stateWord(block.state)}</span>
              <span className="spacer" />
              <span>{clockTime(block.createdAt)}</span>
            </div>
            {block.text ? (
              <pre className="block__text">{block.text}</pre>
            ) : null}
            {block.thoughts.length ? (
              <Thoughts thoughts={block.thoughts} />
            ) : null}
            {block.assistantText ? (
              <pre className="block__text block__text--assistant">
                {block.assistantText}
              </pre>
            ) : null}
            {block.events.length ? (
              <div className="block__events">
                {block.events.map((event) => (
                  <EventRow key={event.key} event={event} />
                ))}
              </div>
            ) : null}
            {block.failureCode ? (
              <span className="block__failure">
                {block.failureCode}
                {block.failureMessage ? ` · ${block.failureMessage}` : ""}
              </span>
            ) : null}
            {block.truncated ? (
              <span className="meta">Transcript truncated by the daemon.</span>
            ) : null}
          </motion.article>
        ))}
      </AnimatePresence>
    </div>
  );
}

function Thoughts({ thoughts }: { thoughts: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button
        type="button"
        className="block__fold"
        onClick={() => setOpen((value) => !value)}
      >
        <ChevronRight
          size={12}
          aria-hidden="true"
          style={{
            display: "inline-block",
            verticalAlign: "-2px",
            transform: open ? "rotate(90deg)" : undefined,
            transition: "transform 150ms cubic-bezier(0.22, 1, 0.36, 1)",
          }}
        />{" "}
        {open ? "Hide" : "Show"} reasoning ({thoughts.length})
      </button>
      <AnimatePresence initial={false}>
        {open ? (
          <motion.pre
            className="block__text block__text--assistant"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.22, ease: EASE }}
            style={{ overflow: "hidden" }}
          >
            {thoughts.join("\n\n")}
          </motion.pre>
        ) : null}
      </AnimatePresence>
    </div>
  );
}

function EventRow({ event }: { event: TranscriptEvent }) {
  const icon =
    event.tone === "tool" ? (
      <Wrench size={12} aria-hidden="true" />
    ) : event.tone === "terminal" ? (
      <SquareTerminal size={12} aria-hidden="true" />
    ) : event.tone === "error" ? (
      <AlertCircle size={12} aria-hidden="true" />
    ) : (
      <Dot size={12} aria-hidden="true" />
    );
  return (
    <div className="event-row" data-tone={event.tone}>
      <span className="event-row__icon">{icon}</span>
      <span className="event-row__text">{event.text}</span>
      <span className="event-row__time">{clockTime(event.at)}</span>
    </div>
  );
}
