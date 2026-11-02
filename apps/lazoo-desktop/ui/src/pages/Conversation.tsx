import { ArrowUp, Ellipsis, PanelRight } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
} from "react";

import { AttentionStrip } from "../components/AttentionStrip";
import { TerminalPane } from "../components/TerminalPane";
import {
  Avatar,
  Button,
  Dialog,
  Menu,
  Skeleton,
  type MenuItem,
} from "../components/primitives";
import { useAppState, useStore, useTerminals } from "../lib/context";
import type {
  AuthorityMode,
  InboxAction,
  LeadLifecycle,
  WorkerCard,
  WorkerState,
} from "../lib/protocol";
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

export interface Tab {
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

/** The lead first, then each worker, with the session its pane shows. */
export function useAgentTabs(): Tab[] {
  const state = useAppState();
  const conversation = state.conversation;
  const sessions = state.sessions;
  return useMemo<Tab[]>(() => {
    if (!conversation) return [];
    const { lead, workers } = conversation;
    // A finished worker's card carries no session, but the daemon keeps its suspended
    // session, which `session.terminal.attach` can resume in the pane.
    const suspendedFor = (employeeId: string) =>
      sessions
        .filter((s) => s.employeeId === employeeId && s.state === "suspended")
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))[0]?.id ?? null;
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
        sessionId:
          worker.session?.id ??
          (worker.state === "complete" || worker.state === "failed"
            ? suspendedFor(worker.employeeId)
            : null),
        done: worker.state === "complete" || worker.taskState === "cancelled",
        worker,
      })),
    ];
  }, [conversation, sessions]);
}

export function ConversationPage() {
  const store = useStore();
  const terminals = useTerminals();
  const state = useAppState();
  const route = state.route;
  const conversation = state.conversation;
  const routeTab = route.kind === "conversation" ? route.tab : "lead";
  const tabs = useAgentTabs();
  const active = tabs.find((tab) => tab.key === routeTab) ?? tabs[0] ?? null;

  // Terminals for tabs that left, or for another conversation, are released.
  useEffect(() => {
    const keep = new Set(
      tabs
        .map((tab) => tab.sessionId)
        .filter((id): id is string => Boolean(id)),
    );
    void terminals.releaseAllExcept(keep);
  }, [tabs, terminals]);

  if (!conversation) {
    return (
      <div className="conv" aria-busy={!state.conversationError}>
        <div className="agentbar" />
        <div className="conv__frame">
          <div className="frame-state">
            {state.conversationError ? (
              <>
                <span className="frame-state__title">
                  {state.conversationError.code}
                </span>
                <span>{state.conversationError.message}</span>
                <div className="frame-state__actions">
                  <Button
                    size="sm"
                    onClick={() => store.navigate({ kind: "launcher" })}
                  >
                    Back to new work
                  </Button>
                </div>
              </>
            ) : (
              <div className="frame-loading" role="status">
                <span className="sr-only">Opening conversation…</span>
                <Skeleton width="38%" height={10} />
                <Skeleton width="72%" height={10} />
                <Skeleton width="64%" height={10} />
              </div>
            )}
          </div>
        </div>
      </div>
    );
  }

  const pulse = conversation.eventHighWater;
  const isLead = active?.key === "lead";

  return (
    <div className="conv">
      <div className="agentbar">
        <div className="agentbar__tabs" role="tablist" aria-label="Agents">
          {tabs.map((tab) => (
            <button
              key={tab.key}
              type="button"
              role="tab"
              className="agent-tab"
              data-done={tab.done || undefined}
              aria-selected={active?.key === tab.key}
              aria-label={`${tab.name}, ${tab.role}, ${tab.stateLabel}`}
              title={`${tab.name} · ${providerLabel(tab.provider)} · ${tab.stateLabel}`}
              onClick={() => store.focusTab(tab.key)}
            >
              <Avatar
                name={tab.name}
                provider={tab.provider}
                size={20}
                status={tab.tone}
              />
              <span className="agent-tab__name">{tab.name}</span>
              {tab.key === "lead" ? (
                <span className="agent-tab__role">lead</span>
              ) : null}
            </button>
          ))}
        </div>
        <AgentFacts tab={active} />
      </div>

      <div className="conv__frame">
        {tabs.map((tab) => (
          <PaneSlot key={tab.key} visible={active?.key === tab.key}>
            <TerminalPane
              sessionId={tab.sessionId}
              pulse={pulse}
              visible={active?.key === tab.key}
              agentName={tab.name}
              isLead={tab.key === "lead"}
              finished={tab.done || tab.worker?.state === "failed"}
              onShowActivity={() => store.toggleDetails(true)}
            />
          </PaneSlot>
        ))}
      </div>

      <AttentionStrip conversation={conversation} />

      {isLead ? (
        <Composer />
      ) : active?.worker ? (
        <div className="conv__footer-note">
          <span className="truncate">{active.worker.stateReason}</span>
          {active.worker.failureCode ? (
            <span className="mono">{active.worker.failureCode}</span>
          ) : null}
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
    <div className="pane-slot" data-visible={visible} aria-hidden={!visible}>
      {children}
    </div>
  );
}

const DESTRUCTIVE = [
  "stop-session",
  "cancel",
  "cancel-turn",
  "decline",
  "resolve-failed",
];

/** The active agent's machine facts, one inline action, the rest in a menu. */
function AgentFacts({ tab }: { tab: Tab | null }) {
  const store = useStore();
  const terminals = useTerminals();
  useSyncExternalStore(
    terminals.subscribe,
    terminals.getVersion,
    terminals.getVersion,
  );
  const state = useAppState();
  const { conversation } = state;
  const [confirm, setConfirm] = useState<InboxAction | null>(null);
  if (!conversation || !tab) return <div className="agentbar__end" />;
  const { lead } = conversation;
  const actions = tab.worker
    ? tab.worker.actions
    : lead.actions.filter((a) => a.action !== "rename");
  const model = tab.worker ? tab.worker.model : lead.model;
  const authority = tab.worker
    ? tab.worker.effectiveAuthority
    : (lead.authority ?? "ask");
  const [inline, ...rest] = actions;
  const view = tab.sessionId ? terminals.get(tab.sessionId) : undefined;
  const items: MenuItem[] = rest.map((action) => ({
    label: action.label,
    danger: DESTRUCTIVE.includes(action.action),
    onSelect: () =>
      DESTRUCTIVE.includes(action.action)
        ? setConfirm(action)
        : void store.act(action),
  }));
  if (view?.record && view.live && tab.sessionId) {
    const id = tab.sessionId;
    items.push({
      label: "Close native view",
      onSelect: () => void terminals.close(id),
    });
  }
  items.push({
    label: state.detailsOpen ? "Hide activity" : "Show activity",
    onSelect: () => store.toggleDetails(),
  });
  const readOnly = Boolean(view?.record && view.live && view.kind !== "attach");

  return (
    <div className="agentbar__end">
      <span className="agentbar__facts">
        {providerLabel(tab.provider)}
        {model ? ` · ${model}` : ""} · {authorityLabel(authority)}
        {readOnly ? " · read-only" : ""}
      </span>
      {inline ? <ActionButton action={inline} /> : null}
      <Menu label={`More for ${tab.name}`} items={items}>
        <Ellipsis size={15} aria-hidden="true" />
      </Menu>
      <Button
        variant="ghost"
        size="sm"
        icon
        tip="Activity  Ctrl I"
        tipSide="left"
        aria-label={state.detailsOpen ? "Close details" : "Open details"}
        aria-pressed={state.detailsOpen}
        onClick={() => store.toggleDetails()}
      >
        <PanelRight size={15} aria-hidden="true" />
      </Button>
      <Dialog
        open={confirm !== null}
        onClose={() => setConfirm(null)}
        title={`${confirm?.label ?? ""}?`}
        description={`This applies to ${tab.name} immediately.`}
        width={400}
        footer={
          <>
            <span className="spacer" />
            <Button variant="ghost" onClick={() => setConfirm(null)}>
              Keep it
            </Button>
            <Button
              variant="danger"
              data-autofocus
              onClick={() => {
                const action = confirm;
                setConfirm(null);
                if (action) void store.act(action);
              }}
            >
              {confirm?.label}
            </Button>
          </>
        }
      >
        {null}
      </Dialog>
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
  const destructive = DESTRUCTIVE.includes(action.action);
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
    element.style.height = `${Math.min(element.scrollHeight, 200)}px`;
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
        ? `${lead?.name} is working · your message queues`
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
        <textarea
          ref={textarea}
          rows={1}
          aria-label="Message to the lead"
          placeholder={placeholder}
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKey}
        />
        {modes.length > 1 ? (
          <select
            className="select select--bare"
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
          <span className="composer__authority">
            {authorityLabel(authority)}
          </span>
        )}
        <Button
          type="submit"
          variant="primary"
          size="sm"
          icon
          className="composer__send"
          loading={busy}
          disabled={!text.trim()}
          aria-label={busy ? "Sending" : "Send"}
        >
          {busy ? null : <ArrowUp size={14} aria-hidden="true" />}
        </Button>
      </div>
      {state.composerError ? (
        <p className="composer__error" role="alert">
          <span className="mono">{state.composerError.code}</span>{" "}
          {state.composerError.message}
        </p>
      ) : null}
    </form>
  );
}
