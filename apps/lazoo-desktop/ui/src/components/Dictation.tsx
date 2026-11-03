// The composer's microphone: start, stop, cancel; a compact recording strip;
// review-before-send or direct send. The microphone itself is the voice
// controller's (it is shared with global push-to-talk).

import { Mic, MicOff, Square } from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
} from "react";

import { useStore } from "../lib/context";
import type { RpcFailure } from "../lib/protocol";
import { asFailure } from "../lib/bridge";
import { useVoice, useVoiceState } from "../lib/voice/context";
import { endpointHost } from "../lib/voice/types";
import { Button } from "./primitives";
import { LevelMeter } from "./LevelMeter";

/** Text spliced in at the caret, spaced so words do not run together. */
export function spliceAtCaret(
  value: string,
  start: number,
  end: number,
  insert: string,
): { value: string; caret: number } {
  const before = value.slice(0, start);
  const after = value.slice(end);
  const lead = before && !/\s$/.test(before) ? " " : "";
  const tail = after && !/^\s/.test(after) ? " " : "";
  const next = `${before}${lead}${insert}${tail}${after}`;
  return { value: next, caret: before.length + lead.length + insert.length };
}

export function useDictation({
  textarea,
  setText,
  send,
}: {
  textarea: RefObject<HTMLTextAreaElement | null>;
  setText: (value: string) => void;
  /** Submits the transcript as the turn; false when it was refused. */
  send: (text: string) => Promise<boolean>;
}) {
  const voice = useVoice();
  const store = useStore();
  const snapshot = useVoiceState();
  const [error, setError] = useState<RpcFailure | null>(null);
  const { state, phase, owner } = snapshot;
  const mine = owner === "composer";

  const insert = useCallback(
    (text: string) => {
      const element = textarea.current;
      const value = element?.value ?? "";
      const start = element?.selectionStart ?? value.length;
      const end = element?.selectionEnd ?? value.length;
      const next = spliceAtCaret(value, start, end, text);
      setText(next.value);
      setTimeout(() => {
        const target = textarea.current;
        if (!target) return;
        target.focus();
        target.setSelectionRange(next.caret, next.caret);
      }, 0);
    },
    [textarea, setText],
  );

  const stop = useCallback(async () => {
    setError(null);
    try {
      const result = await voice.finishRecording("conversation");
      if (!result) return;
      const text = result.text.trim();
      if (!text) {
        setError({ code: "LZ-VOICE-EMPTY", message: "Nothing was heard." });
        return;
      }
      if (voice.getSnapshot().state?.settings.reviewBeforeSend ?? true) {
        insert(text);
        return;
      }
      // Direct send; a refused send keeps the words for correction.
      if (!(await send(text))) insert(text);
    } catch (failure) {
      setError(asFailure(failure));
    }
  }, [voice, insert, send]);

  const start = useCallback(async () => {
    setError(null);
    const failure = await voice.beginRecording("composer");
    if (failure) setError(failure);
  }, [voice]);

  const toggle = useCallback(() => {
    const current = voice.getSnapshot();
    // Voice not loaded, or speech to text is off: the shortcut does nothing.
    if (!current.state || current.state.settings.stt.engine === "off") return;
    if (current.phase === "recording" && current.owner === "composer")
      void stop();
    else if (current.phase === "idle") void start();
  }, [voice, start, stop]);

  const cancel = useCallback(() => {
    voice.cancelRecording();
    setError(null);
  }, [voice]);

  // Keys: Ctrl+Shift+Space toggles while Lazoo is focused; while recording,
  // Enter stops and Esc cancels.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (
        event.ctrlKey &&
        event.shiftKey &&
        !event.altKey &&
        event.code === "Space"
      ) {
        event.preventDefault();
        toggle();
        return;
      }
      const current = voice.getSnapshot();
      if (current.phase !== "recording" || current.owner !== "composer") return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        cancel();
      } else if (
        event.key === "Enter" &&
        !event.shiftKey &&
        !(event.target as HTMLElement | null)?.closest?.("button")
      ) {
        event.preventDefault();
        event.stopPropagation();
        void stop();
      }
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [voice, toggle, cancel, stop]);

  // The length limit ends a recording the way Stop would.
  const limitHits = useRef(snapshot.limitHits);
  useEffect(() => {
    if (snapshot.limitHits === limitHits.current) return;
    limitHits.current = snapshot.limitHits;
    if (mine) void stop();
  }, [snapshot.limitHits, mine, stop]);

  // Dictation the focused app refused lands here (global push-to-talk).
  const pending = snapshot.pendingInsert;
  useEffect(() => {
    if (!pending) return;
    insert(pending.text);
    voice.takeInsert(pending.id);
  }, [pending, insert, voice]);

  // Leaving the conversation releases the microphone.
  useEffect(
    () => () => {
      const current = voice.getSnapshot();
      if (current.phase === "recording" && current.owner === "composer")
        voice.cancelRecording();
    },
    [voice],
  );

  const settings = state?.settings ?? null;
  const external =
    settings?.stt.engine === "endpoint" && state?.endpointExternal
      ? (endpointHost(settings.endpoint.baseUrl) ?? "unknown host")
      : null;

  return {
    /** The voice state has loaded; the mic is shown or explained. */
    loaded: state !== null,
    off: settings?.stt.engine === "off",
    phase,
    mine,
    /** Another surface (push-to-talk) holds the microphone. */
    elsewhere: phase !== "idle" && !mine,
    startedAt: snapshot.startedAt,
    external,
    error,
    toggle,
    cancel,
    openSettings: () => store.navigate({ kind: "settings", section: "voice" }),
  };
}

export type Dictation = ReturnType<typeof useDictation>;

export function MicButton({ dictation }: { dictation: Dictation }) {
  if (!dictation.loaded) return null;
  if (dictation.off) {
    return (
      <Button
        variant="ghost"
        size="sm"
        icon
        tip="Speech to text is off. Settings, Voice"
        aria-label="Speech to text is off. Open Voice settings"
        onClick={dictation.openSettings}
      >
        <MicOff size={14} aria-hidden="true" />
      </Button>
    );
  }
  const recording = dictation.mine && dictation.phase === "recording";
  const transcribing = dictation.mine && dictation.phase === "transcribing";
  return (
    <>
      {dictation.external && !recording && !transcribing ? (
        <span
          className="composer__external"
          title={`Audio is sent to ${dictation.external} for transcription`}
        >
          External · {dictation.external}
        </span>
      ) : null}
      <Button
        variant="ghost"
        size="sm"
        icon
        className="composer__mic"
        data-recording={recording || undefined}
        tip={recording ? "Stop  Enter" : "Dictate  Ctrl Shift Space"}
        aria-label={
          recording
            ? "Stop dictation"
            : transcribing
              ? "Transcribing"
              : "Dictate a message"
        }
        aria-pressed={recording}
        disabled={transcribing || dictation.elsewhere}
        onClick={dictation.toggle}
      >
        {recording ? (
          <Square size={12} aria-hidden="true" />
        ) : (
          <Mic size={14} aria-hidden="true" />
        )}
      </Button>
    </>
  );
}

function formatElapsed(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function Elapsed({ startedAt }: { startedAt: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, []);
  return (
    <span className="composer__elapsed mono" aria-hidden="true">
      {formatElapsed(Math.max(0, now - startedAt))}
    </span>
  );
}

/** The recording or transcribing strip above the box. Announced politely. */
export function RecordingStrip({ dictation }: { dictation: Dictation }) {
  const voice = useVoice();
  const recording = dictation.mine && dictation.phase === "recording";
  const transcribing = dictation.mine && dictation.phase === "transcribing";
  return (
    <div className="composer__strip-slot" role="status" aria-live="polite">
      {recording ? (
        <div className="composer__strip" data-state="recording">
          <span className="composer__rec-dot" aria-hidden="true" />
          <span className="composer__strip-label">Recording</span>
          <LevelMeter source={voice.onLevel} />
          {dictation.startedAt !== null ? (
            <Elapsed startedAt={dictation.startedAt} />
          ) : null}
          {dictation.external ? (
            <span className="composer__external">
              External · {dictation.external}
            </span>
          ) : null}
          <span className="spacer" />
          <span className="composer__strip-hint">
            Enter to stop · Esc to cancel
          </span>
        </div>
      ) : transcribing ? (
        <div className="composer__strip" data-state="transcribing">
          <span className="composer__strip-label">Transcribing…</span>
          {dictation.external ? (
            <span className="composer__external">
              External · {dictation.external}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
