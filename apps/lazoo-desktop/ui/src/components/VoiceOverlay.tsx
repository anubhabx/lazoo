// The dictation pill: all the `voice-overlay` window renders. No app chrome and
// no controls (the window never takes focus); it only shows what the main
// window pushes over `voice://overlay`.

import { CircleAlert, Check } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { Bridge } from "../lib/bridge";
import { installDitherFields } from "../lib/dither";
import type { OverlayUpdate } from "../lib/voice/types";
import { LevelMeter } from "./LevelMeter";

const HIDDEN: OverlayUpdate = { state: "hidden", detail: null, level: null };

export function VoiceOverlay({ bridge }: { bridge: Bridge }) {
  const [update, setUpdate] = useState<OverlayUpdate>(HIDDEN);
  const listeners = useRef(new Set<(level: number) => void>());
  const source = useCallback((listener: (level: number) => void) => {
    listeners.current.add(listener);
    return () => {
      listeners.current.delete(listener);
    };
  }, []);

  useEffect(() => {
    // The page background is transparent so only the pill shows.
    document.documentElement.dataset.overlay = "true";
    installDitherFields();
    let stop: (() => void) | null = null;
    let disposed = false;
    void bridge.voice
      .onOverlay((next) => {
        if (next.level !== null)
          for (const listener of listeners.current) listener(next.level);
        // Level-only updates arrive about 15 times a second; they repaint the
        // meter and leave the label alone.
        setUpdate((current) =>
          current.state === next.state && current.detail === next.detail
            ? current
            : next,
        );
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
      delete document.documentElement.dataset.overlay;
    };
  }, [bridge]);

  const { state, detail } = update;
  return (
    <div
      className="voice-pill"
      data-state={state}
      role="status"
      aria-live="polite"
      hidden={state === "hidden"}
    >
      {state === "recording" ? (
        <>
          <span className="voice-pill__dot" aria-hidden="true" />
          <span className="voice-pill__label">Recording</span>
          <LevelMeter source={source} columns={28} rows={6} cell={3} />
        </>
      ) : null}
      {state === "transcribing" ? (
        <>
          <span className="voice-pill__field" aria-hidden="true" />
          <span className="voice-pill__label">Transcribing</span>
        </>
      ) : null}
      {state === "done" ? (
        <>
          <Check size={15} aria-hidden="true" />
          <span className="voice-pill__label">{detail ?? "Done"}</span>
        </>
      ) : null}
      {state === "error" ? (
        <>
          <CircleAlert size={15} aria-hidden="true" />
          <span className="voice-pill__label voice-pill__label--error">
            {detail ?? "Dictation failed"}
          </span>
        </>
      ) : null}
    </div>
  );
}
