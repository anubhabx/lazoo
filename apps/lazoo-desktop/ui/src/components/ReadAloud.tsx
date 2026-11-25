// Read replies aloud. The lead's reply is spoken as it streams: the same
// `assistantText` Activity shows, folded from turn events, cut into sentences
// as it grows and flushed when the turn ends. Only the lead's turns are spoken,
// and only their reply: never the user's request, never a worker's output.

import { RotateCcw, Square, Volume2, VolumeX } from "lucide-react";
import { useEffect, useRef } from "react";

import { useAppState, useBridge } from "../lib/context";
import type {
  ConversationTurnRecord,
  ConversationTurnSummary,
  WorkspaceConversationResult,
} from "../lib/protocol";
import { blockFromRecord } from "../lib/transcript";
import { SentenceChunker } from "../lib/voice/chunker";
import { useVoice, useVoiceState } from "../lib/voice/context";
import { toSpeakable } from "../lib/voice/speech";
import { Button } from "./primitives";

const ACTIVE = new Set(["recorded", "submitting", "accepted"]);

/** The lead's own turns: the root task's, or none. Workers' turns carry their task id. */
export function leadTurns(
  conversation: WorkspaceConversationResult,
): ConversationTurnSummary[] {
  const rootId = conversation.rootTask?.id ?? null;
  return conversation.turns
    .filter((turn) => turn.taskId === null || turn.taskId === rootId)
    .sort((a, b) => a.sequence - b.sequence);
}

interface Tracked {
  chunker: SentenceChunker;
  done: boolean;
}

interface Session {
  baseline: Set<string>;
  tracked: Map<string, Tracked>;
}

/** A headless controller: renders nothing, speaks the lead's reply. */
export function ReadAloud() {
  const voice = useVoice();
  const bridge = useBridge();
  const { conversation } = useAppState();
  const { state } = useVoiceState();
  const enabled = Boolean(
    state && state.settings.readReplies && state.settings.tts.engine !== "off",
  );
  const conversationId = conversation?.conversation.id ?? null;
  const latest = useRef(conversation);
  latest.current = conversation;
  const session = useRef<Session>({ baseline: new Set(), tracked: new Map() });
  const running = useRef(false);
  const again = useRef(false);

  // A new conversation, or read-aloud switched on: what is already there is
  // history and stays silent.
  useEffect(() => {
    const current = latest.current;
    session.current = {
      baseline: new Set(current ? leadTurns(current).map((t) => t.id) : []),
      tracked: new Map(),
    };
    if (!enabled) voice.stopSpeaking();
  }, [conversationId, enabled, voice]);

  async function process() {
    if (running.current) {
      again.current = true;
      return;
    }
    running.current = true;
    try {
      do {
        again.current = false;
        const sess = session.current;
        const current = latest.current;
        if (!enabled || !current) break;
        for (const turn of leadTurns(current)) {
          if (sess.baseline.has(turn.id)) continue;
          let entry = sess.tracked.get(turn.id);
          if (!entry) {
            entry = { chunker: new SentenceChunker(), done: false };
            sess.tracked.set(turn.id, entry);
          }
          if (entry.done) continue;
          let record: ConversationTurnRecord;
          try {
            record = await bridge.call<ConversationTurnRecord>("turn.show", {
              id: turn.id,
            });
          } catch {
            break; // the next pulse tries again
          }
          if (session.current !== sess) break;
          const chunks = entry.chunker.push(
            blockFromRecord(record).assistantText,
          );
          const finished = !ACTIVE.has(record.state);
          if (finished) {
            chunks.push(...entry.chunker.flush());
            entry.done = true;
          }
          for (const chunk of chunks) {
            const speakable = toSpeakable(chunk);
            if (speakable) voice.speak(speakable);
          }
          // A later turn is spoken only after this one is.
          if (!finished) break;
        }
      } while (again.current);
    } finally {
      running.current = false;
    }
  }

  const turns = conversation?.turns;
  const pulse = conversation?.eventHighWater;
  useEffect(() => {
    if (enabled) void process();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, turns, pulse]);

  // Replay: the latest lead reply that has any text.
  const hasReply = conversation ? leadTurns(conversation).length > 0 : false;
  useEffect(() => {
    if (!hasReply) {
      voice.registerReplay(null);
      return;
    }
    voice.registerReplay(() => {
      void (async () => {
        const current = latest.current;
        if (!current) return;
        const candidates = leadTurns(current).reverse().slice(0, 4);
        for (const turn of candidates) {
          try {
            const record = await bridge.call<ConversationTurnRecord>(
              "turn.show",
              { id: turn.id },
            );
            const text = blockFromRecord(record).assistantText;
            if (!text.trim()) continue;
            voice.stopSpeaking();
            const chunker = new SentenceChunker();
            const chunks = [...chunker.push(text), ...chunker.flush()];
            for (const chunk of chunks) {
              const speakable = toSpeakable(chunk);
              if (speakable) voice.speak(speakable);
            }
            return;
          } catch {
            return;
          }
        }
      })();
    });
    return () => voice.registerReplay(null);
  }, [hasReply, voice, bridge]);

  // Leaving the conversation ends the reading.
  useEffect(() => () => voice.stopSpeaking(), [voice]);

  return null;
}

/** The agent bar's voice controls: the toggle, and Stop or Replay beside it. */
export function ReadAloudControls() {
  const voice = useVoice();
  const { state, speaking, canReplay } = useVoiceState();
  const settings = state?.settings;
  if (!settings || settings.tts.engine === "off") return null;
  const on = settings.readReplies;
  return (
    <span className="readaloud">
      {speaking ? (
        <span className="readaloud__speaking" role="status">
          <span className="readaloud__field" aria-hidden="true" />
          Speaking
        </span>
      ) : null}
      {speaking ? (
        <Button
          variant="ghost"
          size="sm"
          icon
          tip="Stop speaking  Esc"
          aria-label="Stop speaking"
          onClick={() => voice.stopSpeaking()}
        >
          <Square size={12} aria-hidden="true" />
        </Button>
      ) : canReplay ? (
        <Button
          variant="ghost"
          size="sm"
          icon
          tip="Replay last reply"
          aria-label="Replay last reply"
          onClick={() => voice.replayLast()}
        >
          <RotateCcw size={14} aria-hidden="true" />
        </Button>
      ) : null}
      <Button
        variant="ghost"
        size="sm"
        icon
        tip={on ? "Reading replies aloud" : "Read replies aloud"}
        aria-label="Read replies aloud"
        aria-pressed={on}
        onClick={() => void voice.setReadReplies(!on)}
      >
        {on ? (
          <Volume2 size={14} aria-hidden="true" />
        ) : (
          <VolumeX size={14} aria-hidden="true" />
        )}
      </Button>
    </span>
  );
}
