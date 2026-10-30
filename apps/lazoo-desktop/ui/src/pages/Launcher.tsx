import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { Button, Empty, StatusWord } from "../components/primitives";
import { useAppState, useStore } from "../lib/context";
import type { AuthorityMode, LeadCandidate } from "../lib/protocol";
import { authorityLabel, providerLabel, words } from "../lib/util";

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
      leads.find((lead) => lead.employeeId === remembered) ??
      (leads.length === 1 ? leads[0] : null) ??
      null,
    [leads, leadId, remembered],
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

  return (
    <section className="launcher" aria-label="New work">
      <div className="launcher__card">
        <div>
          <div className="launcher__eyebrow">
            {space ? space.displayName : "Home"}
          </div>
          <h1 className="launcher__title">What should the team work on?</h1>
        </div>

        <div className="launcher__prompt">
          <textarea
            ref={textarea}
            aria-label="What should the team work on?"
            placeholder="Describe the outcome. The lead plans, delegates, and reports back here."
            value={text}
            onChange={(event) => {
              setText(event.target.value);
              if (validation) setValidation(null);
            }}
            onKeyDown={onKey}
          />
          <div className="launcher__controls">
            <LeadPicker
              leads={leads}
              selected={selected}
              onSelect={(id) => setLeadId(id)}
            />
            {selected && selected.authorityModes.length > 0 ? (
              <select
                className="select"
                style={{ width: "auto", height: 28 }}
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
              disabled={busy || !launcher}
              onClick={() => void start()}
              aria-label="Start"
            >
              {busy ? "Starting…" : "Start"}
            </Button>
          </div>
        </div>

        <div className="launcher__truth" aria-live="polite">
          {validation ? <span className="warn">{validation}</span> : null}
          {state.composerError ? (
            <span className="warn">
              {state.composerError.code}: {state.composerError.message}
            </span>
          ) : null}
          {selected ? (
            <>
              <span>
                <strong>{selected.name}</strong>
                {remembered === selected.employeeId
                  ? " led your last conversation here"
                  : ""}{" "}
                · {providerLabel(selected.provider)}
                {selected.model ? ` · ${selected.model}` : ""}
                {selected.reasoningEffort
                  ? ` · ${selected.reasoningEffort}`
                  : ""}{" "}
                · {words(selected.availability)}
                {selected.availabilityReason
                  ? ` (${selected.availabilityReason})`
                  : ""}
              </span>
              <span>
                {!selected.startable ? (
                  <span className="warn">
                    {selected.blockingReason ?? "Not startable right now"}
                  </span>
                ) : authority === "ask" ? (
                  "Ask: commands, file changes, and delegation wait for you in the Inbox."
                ) : authority === "self-decision" ? (
                  "Self-decision: in-scope work proceeds; scope expansions wait for you."
                ) : (
                  "Full autonomy: the lead and its workers proceed without approvals."
                )}
                {!selected.collaborationToolsAvailable
                  ? " This lead cannot delegate to workers."
                  : ""}
                {selected.nativeViewSupported
                  ? ""
                  : " No live native view for this harness."}
              </span>
            </>
          ) : launcher && leads.length === 0 ? (
            <span>
              No employees yet.{" "}
              <a
                onClick={() =>
                  store.navigate({ kind: "settings", section: "employees" })
                }
              >
                Create one in Settings
              </a>
              .
            </span>
          ) : launcher ? (
            <span>Choose who leads this conversation.</span>
          ) : state.connection === "offline" ||
            state.connection === "uncertain" ? (
            <span className="warn">
              {state.connectionError?.message ?? "The daemon is not reachable."}
            </span>
          ) : (
            <span>Reading the roster…</span>
          )}
        </div>

        {leads.length > 0 ? (
          <div className="leads" role="radiogroup" aria-label="Lead">
            {leads.map((lead) => (
              <button
                key={lead.employeeId}
                type="button"
                role="radio"
                className="lead-option"
                aria-checked={selected?.employeeId === lead.employeeId}
                aria-disabled={!lead.startable}
                title={
                  lead.blockingReason ?? lead.availabilityReason ?? undefined
                }
                onClick={() => setLeadId(lead.employeeId)}
              >
                <span
                  className="dot"
                  data-tone={availabilityTone(lead)}
                  aria-hidden="true"
                />
                <span className="lead-option__name">
                  <strong>{lead.name}</strong>
                  <span className="meta">
                    {lead.role ? `${lead.role} · ` : ""}
                    {providerLabel(lead.provider)}
                    {lead.model ? ` · ${lead.model}` : ""}
                  </span>
                </span>
                <span className="meta">
                  {lead.startable
                    ? words(lead.availability)
                    : (lead.blockingReason ?? "blocked")}
                </span>
              </button>
            ))}
          </div>
        ) : launcher ? (
          <Empty
            title="No leads to choose from"
            detail="An employee becomes a lead candidate once its harness is detected and verified."
          />
        ) : null}
      </div>
    </section>
  );
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
    <select
      className="select"
      style={{ width: "auto", height: 28 }}
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
