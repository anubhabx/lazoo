import { ArrowUp } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { markerTone } from "../components/Rail";
import {
  Avatar,
  Button,
  Dot,
  Skeleton,
  StatusWord,
} from "../components/primitives";
import { useAppState, useStore } from "../lib/context";
import type { AuthorityMode, LeadCandidate } from "../lib/protocol";
import {
  authorityLabel,
  providerLabel,
  relativeTime,
  words,
} from "../lib/util";

export function availabilityTone(candidate: LeadCandidate): string {
  switch (candidate.availability) {
    case "idle":
    case "dormant":
      return candidate.startable ? "healthy" : "unknown";
    case "starting":
      return "starting";
    case "suspended":
      return "suspended";
    case "failed":
      return "failed";
    case "disabled":
    case "offline":
      return "unknown";
    default:
      return "waiting";
  }
}

export function LauncherPage() {
  const store = useStore();
  const state = useAppState();
  const launcher = state.launcher;
  const space = state.projects.find((project) => project.id === state.spaceId);
  const [leadId, setLeadId] = useState<string | null>(null);
  const [authority, setAuthority] = useState<AuthorityMode | null>(null);
  const [text, setText] = useState("");
  const [validation, setValidation] = useState<string | null>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);

  const leads = launcher?.leads ?? [];
  const remembered = launcher?.rememberedLeadId ?? null;
  const selected = useMemo(
    () =>
      leads.find((lead) => lead.employeeId === leadId) ??
      leads.find((lead) => lead.employeeId === state.preferredLeadId) ??
      leads.find((lead) => lead.employeeId === remembered) ??
      (leads.length === 1 ? leads[0] : null) ??
      leads.find((lead) => lead.role === "lead" && lead.startable) ??
      null,
    [leads, leadId, remembered, state.preferredLeadId],
  );

  useEffect(() => {
    if (!selected) return;
    if (!authority || !selected.authorityModes.includes(authority)) {
      setAuthority(
        selected.authorityModes.includes(selected.defaultAuthority)
          ? selected.defaultAuthority
          : (selected.authorityModes[0] ?? null),
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.employeeId]);

  useEffect(() => {
    textarea.current?.focus();
  }, [launcher?.rosterRevision]);

  useEffect(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = "0px";
    element.style.height = `${Math.min(Math.max(element.scrollHeight, 76), 240)}px`;
  }, [text]);

  const busy = store.isBusy("start");

  function blockingReason(): string | null {
    if (!text.trim()) return "Type a request first.";
    if (!selected) return "Choose a lead.";
    if (!selected.startable)
      return selected.blockingReason ?? "This lead cannot start right now.";
    if (!authority)
      return (
        selected.authorityReason ??
        "No authority mode is available for this lead."
      );
    return null;
  }

  async function start() {
    const reason = blockingReason();
    if (reason) {
      setValidation(reason);
      return;
    }
    setValidation(null);
    await store.start({
      leadId: selected!.employeeId,
      authority: authority!,
      text,
    });
  }

  function onKey(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void start();
    }
  }

  const recent = space
    ? (state.conversations?.conversations ?? []).slice(0, 6)
    : [];
  const ready = Boolean(text.trim()) && Boolean(selected?.startable);

  return (
    <section className="launcher" aria-label="New work">
      <div className="launcher__column">
        <header className="launcher__intro">
          <h1 className="launcher__title">What should the team work on?</h1>
        </header>

        <div
          className="prompt"
          data-ready={ready}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) textarea.current?.focus();
          }}
        >
          <textarea
            ref={textarea}
            aria-label="What should the team work on?"
            placeholder={
              space
                ? `Describe the outcome in ${space.displayName}. The lead plans, delegates and reports back.`
                : "Describe the outcome. The lead plans, delegates and reports back."
            }
            value={text}
            onChange={(event) => {
              setText(event.target.value);
              if (validation) setValidation(null);
            }}
            onKeyDown={onKey}
          />
          <div className="prompt__controls">
            <LeadPicker
              leads={leads}
              selected={selected}
              onSelect={(id) => setLeadId(id)}
            />
            {selected && selected.authorityModes.length > 0 ? (
              <select
                className="select select--bare"
                aria-label="Authority"
                value={authority ?? ""}
                onChange={(event) =>
                  setAuthority(event.target.value as AuthorityMode)
                }
              >
                {selected.authorityModes.map((mode) => (
                  <option key={mode} value={mode}>
                    {authorityLabel(mode)}
                  </option>
                ))}
              </select>
            ) : null}
            <span className="spacer" />
            <Button
              variant="primary"
              size="sm"
              className="prompt__send"
              loading={busy}
              disabled={!launcher}
              onClick={() => void start()}
              aria-label="Start"
            >
              {busy ? null : <ArrowUp size={13} aria-hidden="true" />}
              Start
            </Button>
          </div>
        </div>

        <p className="launcher__truth" aria-live="polite">
          {validation ? <span className="warn">{validation}</span> : null}
          {state.composerError ? (
            <span className="warn">
              {state.composerError.code}: {state.composerError.message}
            </span>
          ) : null}
          {selected && !validation && !state.composerError ? (
            !selected.startable ? (
              <span className="warn">
                {selected.blockingReason ?? "This lead cannot start right now."}
              </span>
            ) : (
              <span>
                <strong>{selected.name}</strong>
                {remembered === selected.employeeId
                  ? " led your last conversation here. "
                  : " will lead. "}
                {authority === "ask"
                  ? "Commands, file changes and delegation wait for you."
                  : authority === "self-decision"
                    ? "In-scope work proceeds; scope expansions wait for you."
                    : "The lead and its workers proceed without approvals."}
                {!selected.collaborationToolsAvailable
                  ? " This lead cannot delegate."
                  : ""}
              </span>
            )
          ) : launcher && leads.length > 0 && !selected ? (
            <span>Choose who leads this conversation.</span>
          ) : !launcher &&
            (state.connection === "offline" ||
              state.connection === "uncertain") ? (
            <span className="warn">
              {state.connectionError?.message ?? "The daemon is not reachable."}
            </span>
          ) : null}
        </p>

        <section className="launcher__section" aria-label="Team">
          <header className="launcher__section-head">
            <h2>Team</h2>
            {leads.length > 0 ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => store.openSetup()}
              >
                Add agents
              </Button>
            ) : null}
          </header>
          {!launcher ? (
            <div className="team">
              {[0, 1, 2].map((index) => (
                <div key={index} className="member member--loading">
                  <Skeleton width={20} height={20} />
                  <Skeleton width="40%" height={10} />
                </div>
              ))}
            </div>
          ) : leads.length > 0 ? (
            <div className="team" role="radiogroup" aria-label="Lead">
              {leads.map((lead) => (
                <button
                  key={lead.employeeId}
                  type="button"
                  role="radio"
                  className="member"
                  aria-checked={selected?.employeeId === lead.employeeId}
                  aria-disabled={!lead.startable}
                  title={
                    lead.blockingReason ?? lead.availabilityReason ?? undefined
                  }
                  onClick={() => setLeadId(lead.employeeId)}
                >
                  <Avatar
                    name={lead.name}
                    provider={lead.provider}
                    size={20}
                    status={availabilityTone(lead)}
                  />
                  <span className="member__name">{lead.name}</span>
                  <span className="member__role">{lead.role ?? ""}</span>
                  <span className="member__meta">
                    {providerLabel(lead.provider)}
                    {lead.model ? ` · ${lead.model}` : ""}
                    {lead.reasoningEffort ? ` · ${lead.reasoningEffort}` : ""}
                  </span>
                  <span className="member__state">
                    {lead.startable
                      ? readiness(lead.availability)
                      : (lead.blockingReason ?? "blocked")}
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <div className="team-empty">
              <span className="team-empty__text">
                <strong>No employees yet.</strong>
                <span>
                  Lazoo ships a recommended team for the harnesses on this
                  machine. Review it and create it in one step.
                </span>
              </span>
              <Button
                variant="primary"
                size="sm"
                onClick={() => store.openSetup()}
              >
                Set up your team
              </Button>
            </div>
          )}
        </section>

        {recent.length > 0 ? (
          <section
            className="launcher__section"
            aria-label="Recent conversations"
          >
            <header className="launcher__section-head">
              <h2>Recent in {space?.displayName}</h2>
            </header>
            <div className="recent">
              {recent.map((row) => {
                const marker = markerTone(row, state.seen);
                return (
                  <button
                    key={row.id}
                    type="button"
                    className="recent__row"
                    onClick={() => store.openConversation(row.id)}
                  >
                    <span className="recent__marker">
                      {marker ? <Dot tone={marker.tone} /> : null}
                    </span>
                    <span className="recent__title truncate">{row.title}</span>
                    <span className="recent__meta">
                      {row.leadName ?? ""}
                      {row.workers.total
                        ? ` · ${row.workers.total} ${row.workers.total === 1 ? "worker" : "workers"}`
                        : ""}
                    </span>
                    <span className="recent__time">
                      {relativeTime(row.updatedAt)}
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        ) : null}
      </div>
    </section>
  );
}

/** What the availability means for someone about to send work. */
function readiness(availability: LeadCandidate["availability"]): string {
  switch (availability) {
    case "dormant":
      return "Ready";
    case "idle":
      return "Warm";
    case "starting":
      return "Starting";
    case "suspended":
      return "Suspended";
    default:
      return words(availability);
  }
}

function LeadPicker({
  leads,
  selected,
  onSelect,
}: {
  leads: LeadCandidate[];
  selected: LeadCandidate | null;
  onSelect: (id: string) => void;
}) {
  if (!leads.length) return <span className="chip">No lead available</span>;
  return (
    <span className="lead-pill">
      {selected ? (
        <Avatar
          name={selected.name}
          provider={selected.provider}
          size={20}
          status={availabilityTone(selected)}
        />
      ) : null}
      <select
        className="select select--bare"
        aria-label="Lead"
        value={selected?.employeeId ?? ""}
        onChange={(event) => onSelect(event.target.value)}
      >
        {!selected ? <option value="">Choose a lead</option> : null}
        {leads.map((lead) => (
          <option key={lead.employeeId} value={lead.employeeId}>
            {lead.name} · {providerLabel(lead.provider)}
            {lead.startable ? "" : " (blocked)"}
          </option>
        ))}
      </select>
    </span>
  );
}

export function LeadStatus({ lead }: { lead: LeadCandidate }) {
  return (
    <StatusWord
      tone={availabilityTone(lead)}
      label={words(lead.availability)}
    />
  );
}
