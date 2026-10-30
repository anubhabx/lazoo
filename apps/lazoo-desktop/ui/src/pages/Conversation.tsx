import { AnimatePresence, motion } from "motion/react";
import { PanelRight } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { AttentionStrip } from "../components/AttentionStrip";
import { TerminalPane } from "../components/TerminalPane";
import { TranscriptPane } from "../components/TranscriptPane";
import {
  Button,
  EASE,
  Segmented,
  StatusWord,
  fadeFast,
} from "../components/primitives";
import { useAppState, useStore, useTerminals } from "../lib/context";
import type {
  AuthorityMode,
  InboxAction,
  LeadLifecycle,
  WorkerCard,
  WorkerState,
} from "../lib/protocol";
import type { PaneMode } from "../lib/store";
import { authorityLabel, providerLabel, shortId, words } from "../lib/util";

export function lifecycleTone(lifecycle: LeadLifecycle): string {
  switch (lifecycle) {
    case "busy":
      return "running";
    case "starting":
      return "starting";
    case "idle":
      return "idle";
    case "failed":
      return "failed";
    case "suspended":
    case "stopping":
    case "dormant":
      return lifecycle;
    default:
      return "unknown";
  }
}

export function workerTone(state: WorkerState): string {
  switch (state) {
    case "running":
      return "running";
    case "waiting":
      return "waiting";
    case "needs-approval":
      return "needs-approval";
    case "blocked":
      return "blocked";
    case "interrupted":
      return "interrupted";
    case "failed":
      return "failed";
    case "complete":
      return "complete";
    default:
      return "unknown";
  }
}

interface Tab {
  key: string;
  name: string;
  role: string;
  provider: string | null;
  tone: string;
  stateLabel: string;
  sessionId: string | null;
  done: boolean;
  worker: WorkerCard | null;
}

export function ConversationPage() {
  const store = useStore();
  const terminals = useTerminals();
  const state = useAppState();
  const route = state.route;
  const conversation = state.conversation;
  const routeTab = route.kind === "conversation" ? route.tab : "lead";
  const conversationId = route.kind === "conversation" ? route.id : null;

  const tabs = useMemo<Tab[]>(() => {
    if (!conversation) return [];
    const { lead, workers } = conversation;
    return [
      {
        key: "lead",
        name: lead.name,
        role: "lead",
        provider: lead.provider,
        tone: lifecycleTone(lead.lifecycle),
        stateLabel: words(lead.lifecycle),
        sessionId: lead.session?.id ?? null,
        done: false,
        worker: null,
      },
      ...workers.map((worker) => ({
        key: worker.taskId,
        name: worker.name ?? shortId(worker.employeeId),
        role: worker.label,
        provider: worker.provider,
        tone: workerTone(worker.state),
        stateLabel: words(worker.state),
        sessionId: worker.session?.id ?? null,
        done: worker.state === "complete" || worker.taskState === "cancelled",
        worker,
      })),
    ];
  }, [conversation]);

  const active = tabs.find((tab) => tab.key === routeTab) ?? tabs[0] ?? null;
  const mode: PaneMode = conversationId
    ? store.paneMode(conversationId)
    : "native";

  // Terminals for tabs that left, or for another conversation, are released.
  useEffect(() => {
    const keep = new Set(
      tabs
        .map((tab) => tab.sessionId)
        .filter((id): id is string => Boolean(id)),
    );
    void terminals.releaseAllExcept(keep);
  }, [tabs, terminals]);

  const names = useMemo(() => {
    const map = new Map<string, string>();
    if (conversation) {
      map.set(conversation.lead.employeeId, conversation.lead.name);
      for (const worker of conversation.workers) {
        if (worker.name) map.set(worker.employeeId, worker.name);
      }
    }
    return map;
  }, [conversation]);

  if (!conversation) {
    return (
      <div className="conv" aria-busy={!state.conversationError}>
        <div />
        <div />
        <div className="conv__frame">
          <div className="frame-state">
            {state.conversationError ? (
              <>
                <span className="frame-state__title">
                  {state.conversationError.code}
                </span>
                <span>{state.conversationError.message}</span>
                <Button
                  size="sm"
                  onClick={() => store.navigate({ kind: "launcher" })}
                >
                  Back to new work
                </Button>
              </>
            ) : (
              <span className="frame-state__title">Opening conversation…</span>
            )}
          </div>
        </div>
        <div />
        <div />
      </div>
    );
  }

  const pulse = conversation.eventHighWater;
  const isLead = active?.key === "lead";

  return (
    <div className="conv">
      <div className="tabs">
        <div className="tabs__list" role="tablist" aria-label="Agents">
          <AnimatePresence initial={false}>
            {tabs.map((tab) => (
              <motion.button
                key={tab.key}
                type="button"
                role="tab"
                className={`tab ${tab.done ? "tab--done" : ""}`.trim()}
                aria-selected={active?.key === tab.key}
                aria-label={`${tab.name}, ${tab.role}, ${tab.stateLabel}`}
                title={`${tab.name} · ${providerLabel(tab.provider)} · ${tab.stateLabel}`}
                onClick={() => store.focusTab(tab.key)}
                layout
                initial={{ opacity: 0, x: -6 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: -6 }}
                transition={{ duration: 0.22, ease: EASE }}
              >
                <span className="dot" data-tone={tab.tone} aria-hidden="true" />
                <span className="tab__name">{tab.name}</span>
                {tab.key === "lead" ? (
                  <span className="tab__role">main</span>
                ) : null}
                {active?.key === tab.key ? (
                  <motion.span
                    className="tab__indicator"
                    layoutId="tab-indicator"
                    transition={{ duration: 0.15, ease: EASE }}
                  />
                ) : null}
              </motion.button>
            ))}
          </AnimatePresence>
        </div>
        <div className="tabs__end">
          <Segmented
            name="pane-mode"
            label="Pane mode"
            value={mode}
            options={[
              { value: "native", label: "Native" },
              { value: "transcript", label: "Transcript" },
            ]}
            onChange={(value) =>
              conversationId && store.setPaneMode(conversationId, value)
            }
          />
          <Button
            variant="ghost"
            size="sm"
            icon
            aria-label={state.detailsOpen ? "Close details" : "Open details"}
            aria-pressed={state.detailsOpen}
            onClick={() => store.toggleDetails()}
          >
            <PanelRight size={14} aria-hidden="true" />
          </Button>
        </div>
      </div>

      <Header tab={active} />

      <div className="conv__frame" data-focused={mode === "native" && isLead}>
        <AnimatePresence mode="wait" initial={false}>
          {mode === "native" ? (
            <motion.div
              key="native"
              style={{ position: "absolute", inset: 0 }}
              {...fadeFast}
            >
              {tabs.map((tab) => (
                <PaneSlot key={tab.key} visible={active?.key === tab.key}>
                  <TerminalPane
                    sessionId={tab.sessionId}
                    pulse={pulse}
                    visible={active?.key === tab.key}
                    agentName={tab.name}
                    isLead={tab.key === "lead"}
                    onShowTranscript={() =>
                      conversationId &&
                      store.setPaneMode(conversationId, "transcript")
                    }
                  />
                </PaneSlot>
              ))}
            </motion.div>
          ) : (
            <motion.div
              key="transcript"
              style={{ position: "absolute", inset: 0 }}
              {...fadeFast}
            >
              <TranscriptPane
                conversation={conversation}
                taskId={active?.worker?.taskId ?? null}
                names={names}
                pulse={pulse}
              />
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <AttentionStrip conversation={conversation} />

      {isLead ? (
        <Composer />
      ) : active?.worker ? (
        <div className="conv__footer-note">
          {active.worker.stateReason}
          {active.worker.failureCode ? ` · ${active.worker.failureCode}` : ""}
        </div>
      ) : null}
    </div>
  );
}

function PaneSlot({
  visible,
  children,
}: {
  visible: boolean;
  children: React.ReactNode;
}) {
  const [everVisible, setEverVisible] = useState(visible);
  useEffect(() => {
    if (visible) setEverVisible(true);
  }, [visible]);
  if (!everVisible) return null;
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        visibility: visible ? "visible" : "hidden",
        pointerEvents: visible ? "auto" : "none",
      }}
      aria-hidden={!visible}
    >
      {children}
    </div>
  );
}

function Header({ tab }: { tab: Tab | null }) {
  const store = useStore();
  const { conversation } = useAppState();
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  if (!conversation || !tab) return <div className="conv__header" />;
  const { lead } = conversation;
  const actions = tab.worker
    ? tab.worker.actions
    : lead.actions.filter((a) => a.action !== "rename");

  function startRename() {
    setDraft(conversation!.title);
    setRenaming(true);
  }

  async function commitRename() {
    setRenaming(false);
    const next = draft.trim();
    if (!next || next === conversation!.title) return;
    await store.rename(next);
  }

  return (
    <div className="conv__header">
      {renaming ? (
        <input
          className="input"
          style={{ maxWidth: 320, height: 26 }}
          aria-label="Conversation title"
          value={draft}
          autoFocus
          onChange={(event) => setDraft(event.target.value)}
          onBlur={() => void commitRename()}
          onKeyDown={(event) => {
            if (event.key === "Enter") void commitRename();
            if (event.key === "Escape") setRenaming(false);
          }}
        />
      ) : (
        <button
          type="button"
          className="btn btn--ghost btn--sm"
          style={{ maxWidth: 320 }}
          title="Rename conversation"
          aria-label={`Rename conversation: ${conversation.title}`}
          onClick={startRename}
        >
          <span className="truncate">{conversation.title}</span>
        </button>
      )}
      <span className="sep">/</span>
      <span className="conv__identity">
        <strong>{tab.name}</strong>
        <span>{tab.key === "lead" ? "lead" : "worker"}</span>
        <span className="sep">·</span>
        <span>{providerLabel(tab.provider)}</span>
        {tab.key === "lead" && lead.model ? (
          <>
            <span className="sep">·</span>
            <span className="truncate">{lead.model}</span>
          </>
        ) : null}
        {tab.worker?.model ? (
          <>
            <span className="sep">·</span>
            <span className="truncate">{tab.worker.model}</span>
          </>
        ) : null}
        <span className="sep">·</span>
        <span>
          {authorityLabel(
            tab.worker
              ? tab.worker.effectiveAuthority
              : (lead.authority ?? "ask"),
          )}
        </span>
        <span className="sep">·</span>
        <StatusWord tone={tab.tone} label={tab.stateLabel} />
      </span>
      <div className="conv__actions">
        {actions.map((action) => (
          <ActionButton key={action.action} action={action} />
        ))}
      </div>
    </div>
  );
}

export function ActionButton({
  action,
  primary = false,
}: {
  action: InboxAction;
  primary?: boolean;
}) {
  const store = useStore();
  useAppState();
  const destructive = [
    "stop-session",
    "cancel",
    "cancel-turn",
    "decline",
    "resolve-failed",
  ].includes(action.action);
  const [confirming, setConfirming] = useState(false);
  const target = String(action.params.id ?? action.params.conversationId ?? "");
  const busy = store.isBusy(`${action.action}:${target}`);

  useEffect(() => {
    if (!confirming) return;
    const timer = setTimeout(() => setConfirming(false), 4000);
    return () => clearTimeout(timer);
  }, [confirming]);

  async function run() {
    if (destructive && !confirming) {
      setConfirming(true);
      return;
    }
    setConfirming(false);
    await store.act(action);
  }

  return (
    <Button
      size="sm"
      variant={
        confirming
          ? "danger"
          : primary
            ? "primary"
            : destructive
              ? "ghost"
              : "default"
      }
      disabled={busy}
      aria-label={confirming ? `Confirm: ${action.label}` : action.label}
      onClick={() => void run()}
    >
      {confirming ? `Confirm ${action.label.toLowerCase()}` : action.label}
    </Button>
  );
}

function Composer() {
  const store = useStore();
  const state = useAppState();
  const conversation = state.conversation;
  const [text, setText] = useState("");
  const textarea = useRef<HTMLTextAreaElement>(null);
  const lead = conversation?.lead;
  const candidate = state.launcher?.leads.find(
    (item) => item.employeeId === lead?.employeeId,
  );
  const modes: AuthorityMode[] = candidate?.authorityModes.length
    ? candidate.authorityModes
    : lead?.authority
      ? [lead.authority]
      : ["ask"];
  const [authority, setAuthority] = useState<AuthorityMode>(
    lead?.authority ?? modes[0],
  );
  const busy = store.isBusy("turn");

  useEffect(() => {
    if (lead?.authority && modes.includes(lead.authority))
      setAuthority(lead.authority);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lead?.authority]);

  useEffect(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = "0px";
    element.style.height = `${Math.min(element.scrollHeight, 160)}px`;
  }, [text]);

  async function send() {
    if (!text.trim() || busy) return;
    const ok = await store.submitTurn(text, authority);
    if (ok) setText("");
  }

  function onKey(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send();
    }
  }

  const lifecycle = lead?.lifecycle ?? "dormant";
  const placeholder =
    lifecycle === "dormant"
      ? `Send the first request to ${lead?.name ?? "the lead"}`
      : lifecycle === "busy"
        ? `${lead?.name} is working; your next message is queued`
        : `Message ${lead?.name ?? "the lead"}`;

  return (
    <form
      className="composer"
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
    >
      <div className="composer__box">
        <span className="composer__chevron" aria-hidden="true">
          ›
        </span>
        <textarea
          ref={textarea}
          rows={1}
          aria-label="Message to the lead"
          placeholder={placeholder}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKey}
        />
        <Button
          type="submit"
          variant="primary"
          size="sm"
          disabled={busy || !text.trim()}
        >
          {busy ? "Sending…" : "Send"}
        </Button>
      </div>
      <div className="composer__row">
        {modes.length > 1 ? (
          <select
            className="select"
            style={{
              height: 22,
              minHeight: 22,
              width: "auto",
              fontSize: 11,
              padding: "0 24px 0 8px",
            }}
            aria-label="Authority for this request"
            value={authority}
            onChange={(event) =>
              setAuthority(event.target.value as AuthorityMode)
            }
          >
            {modes.map((mode) => (
              <option key={mode} value={mode}>
                {authorityLabel(mode)}
              </option>
            ))}
          </select>
        ) : (
          <span>{authorityLabel(authority)}</span>
        )}
        <span className="sep">·</span>
        <span>Enter sends, Shift+Enter adds a line</span>
        {state.composerError ? (
          <>
            <span className="sep">·</span>
            <span className="composer__error" role="alert">
              {state.composerError.code}: {state.composerError.message}
            </span>
          </>
        ) : null}
      </div>
    </form>
  );
}
