import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";

import { useTerminals } from "../lib/context";
import { Button } from "./primitives";

interface Props {
  sessionId: string | null;
  /** Changes whenever the conversation model refreshed; re-reads terminal status. */
  pulse: number;
  visible: boolean;
  agentName: string;
  isLead: boolean;
  /** The worker's task reached a terminal state; its session is gone or suspended. */
  finished?: boolean;
  /** The agent's session is starting (trust, warm-up); the view opens once it is ready. */
  starting?: boolean;
  onShowActivity: () => void;
}

/**
 * The genuine harness TUI, edge to edge. Lazoo adds nothing on top of it;
 * when there is no live view, one dithered field says why and offers at most
 * one way forward.
 */
export function TerminalPane({
  sessionId,
  pulse,
  visible,
  agentName,
  isLead,
  finished = false,
  starting = false,
  onShowActivity,
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

  // A refused keystroke is worth one quiet note, not a permanent badge.
  const refused = Boolean(entry?.inputRefused);
  const [showRefused, setShowRefused] = useState(false);
  useEffect(() => {
    if (!refused) return;
    setShowRefused(true);
    const timer = setTimeout(() => setShowRefused(false), 4000);
    return () => clearTimeout(timer);
  }, [refused]);

  // Native first: open the view once per session whenever the daemon allows it.
  useEffect(() => {
    if (!sessionId || !entry || !status || !visible) return;
    if (entry.record || entry.opening || status.viewOpening || status.opening)
      return;
    if (!status.viewable || autoOpened === sessionId) return;
    setAutoOpened(sessionId);
    void terminals.open(sessionId, "view", status.sessionRevision);
  }, [sessionId, entry, status, visible, autoOpened, terminals]);

  const reopen = () =>
    sessionId &&
    status &&
    void terminals.open(sessionId, "view", status.sessionRevision);

  let field: ReactNode = null;
  // A closed view keeps its last screen readable; a quiet bar says so.
  const closedNote = exited ? (
    <div className="terminal-note" role="status">
      <span>
        {entry?.kind === "attach" ? "Attachment" : "View"} closed
        {entry?.exitCode !== null && entry?.exitCode !== undefined
          ? ` · exit ${entry.exitCode}`
          : ""}
        {entry?.error ? ` · ${entry.error.code}` : ""}
      </span>
      {status?.viewable ? (
        <Button size="sm" variant="ghost" onClick={reopen}>
          Reopen
        </Button>
      ) : null}
    </div>
  ) : null;
  if (exited) {
    field = null;
  } else if (!live) {
    if (!sessionId) {
      field = (
        <Field
          title={finished ? "No live session" : "No native session yet"}
          detail={
            finished
              ? `${agentName} finished this task.`
              : isLead
                ? `${agentName} starts when you send the first request.`
                : `${agentName} has not started a session for this task.`
          }
          action={
            finished ? (
              <Button size="sm" variant="ghost" onClick={onShowActivity}>
                Show activity
              </Button>
            ) : null
          }
        />
      );
    } else if (entry?.opening || status?.viewOpening || status?.opening) {
      field = <Field title="Opening the native view…" busy />;
    } else if (starting && status && !status.viewable) {
      field = (
        <Field
          title={`Starting ${agentName}…`}
          detail="The native view opens as soon as the session is ready."
          busy
        />
      );
    } else if (entry?.error) {
      field = (
        <Field
          title={entry.error.code}
          detail={entry.error.message}
          action={
            status?.viewable ? (
              <Button size="sm" onClick={reopen}>
                Try again
              </Button>
            ) : (
              <Button size="sm" variant="ghost" onClick={onShowActivity}>
                Show activity
              </Button>
            )
          }
        />
      );
    } else if (status && !status.viewable) {
      field = (
        <Field
          title={
            status.attachable ? "Session suspended" : "Native view unavailable"
          }
          detail={status.viewBlockingReason ?? status.blockingReason ?? ""}
          action={
            status.attachable ? (
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
            ) : (
              <Button size="sm" variant="ghost" onClick={onShowActivity}>
                Show activity
              </Button>
            )
          }
        />
      );
    } else if (entry?.statusError) {
      field = (
        <Field
          title={entry.statusError.code}
          detail={entry.statusError.message}
        />
      );
    } else {
      field = <Field title="Reading terminal status…" busy />;
    }
  }

  return (
    <div className="terminal-layer">
      <div
        ref={container}
        className="terminal-host"
        data-visible={live || exited ? "true" : "false"}
        aria-label={`${agentName} terminal`}
        role="region"
      />
      {live && (showRefused || entry?.gap) ? (
        <div className="terminal-note" role="status">
          {showRefused
            ? "Read-only view · type in the composer below"
            : "Earlier output was lost"}
        </div>
      ) : null}
      {closedNote}
      {field}
    </div>
  );
}

function Field({
  title,
  detail,
  action,
  busy = false,
}: {
  title: string;
  detail?: string | null;
  action?: ReactNode;
  busy?: boolean;
}) {
  return (
    <div className="frame-state" data-busy={busy || undefined}>
      <span className="frame-state__title">{title}</span>
      {detail ? <span>{detail}</span> : null}
      {action ? <div className="frame-state__actions">{action}</div> : null}
    </div>
  );
}
