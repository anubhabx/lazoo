import { AnimatePresence, motion } from "motion/react";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { useTerminals } from "../lib/context";
import { Button, fadeFast } from "./primitives";

interface Props {
  sessionId: string | null;
  /** Changes whenever the conversation model refreshed; re-reads terminal status. */
  pulse: number;
  visible: boolean;
  agentName: string;
  isLead: boolean;
  onShowTranscript: () => void;
}

export function TerminalPane({
  sessionId,
  pulse,
  visible,
  agentName,
  isLead,
  onShowTranscript,
}: Props) {
  const terminals = useTerminals();
  useSyncExternalStore(
    terminals.subscribe,
    terminals.getVersion,
    terminals.getVersion,
  );
  const container = useRef<HTMLDivElement>(null);
  const [autoOpened, setAutoOpened] = useState<string | null>(null);
  const entry = sessionId ? terminals.get(sessionId) : undefined;

  useEffect(() => {
    if (!sessionId) return;
    terminals.ensure(sessionId);
    void terminals.refreshStatus(sessionId);
  }, [sessionId, pulse, terminals]);

  // Opening or closing a view emits no audit event, so a visible pane without a
  // live stream re-reads terminal status on a slow cadence (contract: bounded polling).
  useEffect(() => {
    if (!sessionId || !visible) return;
    const timer = setInterval(() => {
      const current = terminals.get(sessionId);
      if (!current?.stream) void terminals.refreshStatus(sessionId);
    }, 5000);
    return () => clearInterval(timer);
  }, [sessionId, visible, terminals]);

  useLayoutEffect(() => {
    const element = container.current;
    if (!sessionId || !element) return;
    terminals.mount(sessionId, element);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (visible) terminals.fit(sessionId);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [sessionId, terminals, visible]);

  useEffect(() => {
    if (!sessionId) return;
    void terminals.setFocused(sessionId, visible);
    if (visible) terminals.fit(sessionId);
  }, [sessionId, visible, terminals, entry?.record?.id]);

  const status = entry?.status ?? null;
  const live = Boolean(entry?.record && entry.live);
  const exited = Boolean(entry?.record && !entry.live);

  // Native mode means "show me the TUI": open the view once per session when the daemon allows it.
  useEffect(() => {
    if (!sessionId || !entry || !status || !visible) return;
    if (entry.record || entry.opening || status.viewOpening || status.opening)
      return;
    if (!status.viewable || autoOpened === sessionId) return;
    setAutoOpened(sessionId);
    void terminals.open(sessionId, "view", status.sessionRevision);
  }, [sessionId, entry, status, visible, autoOpened, terminals]);

  const noteKind =
    entry?.kind === "attach"
      ? "Attached · keys go to this terminal"
      : "Native view · read-only";

  return (
    <div className="terminal-layer" style={{ position: "absolute", inset: 0 }}>
      <div
        ref={container}
        className="terminal-host"
        data-visible={live || exited ? "true" : "false"}
        aria-label={`${agentName} terminal`}
        role="region"
      />
      <AnimatePresence>
        {sessionId && (live || exited) ? (
          <motion.div key="note" className="frame-note" {...fadeFast}>
            {exited ? (
              <>
                <span>
                  {entry?.kind === "attach" ? "Attachment" : "View"} closed
                  {entry?.exitCode !== null && entry?.exitCode !== undefined
                    ? ` · exit ${entry.exitCode}`
                    : ""}
                  {entry?.error
                    ? ` · ${entry.error.code}: ${entry.error.message}`
                    : ""}
                </span>
                {status?.viewable ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      void terminals.open(
                        sessionId,
                        "view",
                        status.sessionRevision,
                      )
                    }
                  >
                    Reopen
                  </Button>
                ) : null}
              </>
            ) : (
              <>
                <span>
                  {entry?.inputRefused
                    ? "Native view · input refused by the provider"
                    : noteKind}
                </span>
                {entry?.gap ? <span className="sep">·</span> : null}
                {entry?.gap ? <span>earlier output was lost</span> : null}
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void terminals.close(sessionId)}
                >
                  Close
                </Button>
              </>
            )}
          </motion.div>
        ) : null}
      </AnimatePresence>
      <AnimatePresence>
        {!live && !exited ? (
          <motion.div key="state" className="frame-state" {...fadeFast}>
            {!sessionId ? (
              <>
                <span className="frame-state__title">
                  No native session yet
                </span>
                <span>
                  {isLead
                    ? `${agentName} starts when you send the first request.`
                    : `${agentName} has not started a session for this task.`}
                </span>
              </>
            ) : entry?.opening || status?.viewOpening || status?.opening ? (
              <span className="frame-state__title">
                Opening the native view…
              </span>
            ) : entry?.error ? (
              <>
                <span className="frame-state__title">{entry.error.code}</span>
                <span>{entry.error.message}</span>
                <div className="frame-state__actions">
                  {status?.viewable ? (
                    <Button
                      size="sm"
                      onClick={() =>
                        void terminals.open(
                          sessionId,
                          "view",
                          status.sessionRevision,
                        )
                      }
                    >
                      Try again
                    </Button>
                  ) : null}
                  <Button size="sm" variant="ghost" onClick={onShowTranscript}>
                    Show transcript
                  </Button>
                </div>
              </>
            ) : status && !status.viewable ? (
              <>
                <span className="frame-state__title">
                  {status.attachable
                    ? "Session suspended"
                    : "Native view unavailable"}
                </span>
                <span>
                  {status.viewBlockingReason ?? status.blockingReason ?? ""}
                </span>
                <div className="frame-state__actions">
                  {status.attachable ? (
                    <Button
                      size="sm"
                      onClick={() =>
                        void terminals.open(
                          sessionId,
                          "attach",
                          status.sessionRevision,
                        )
                      }
                    >
                      Attach and take over
                    </Button>
                  ) : null}
                  <Button size="sm" variant="ghost" onClick={onShowTranscript}>
                    Show transcript
                  </Button>
                </div>
              </>
            ) : entry?.statusError ? (
              <>
                <span className="frame-state__title">
                  {entry.statusError.code}
                </span>
                <span>{entry.statusError.message}</span>
              </>
            ) : (
              <span className="frame-state__title">
                Reading terminal status…
              </span>
            )}
          </motion.div>
        ) : null}
      </AnimatePresence>
    </div>
  );
}
