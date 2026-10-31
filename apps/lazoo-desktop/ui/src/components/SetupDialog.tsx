import { AnimatePresence, motion } from "motion/react";
import { Check, ChevronDown, CircleSlash, Sparkles, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useAppState, useStore } from "../lib/context";
import {
  EFFORTS,
  MODEL_SUGGESTIONS,
  recommendTeam,
  whyChoice,
  type AgentDraft,
  PRESETS,
} from "../lib/presets";
import type { AuthorityMode } from "../lib/protocol";
import { authorityLabel, providerLabel } from "../lib/util";
import {
  Avatar,
  Badge,
  Button,
  Dialog,
  EASE,
  Field,
  SPRING,
  Spinner,
} from "./primitives";

type Outcome = "creating" | "created" | "failed";
const HARNESSES = [
  "claude-code",
  "codex",
  "opencode",
  "antigravity",
  "github-copilot",
];

export function SetupDialog() {
  const store = useStore();
  const state = useAppState();
  const open = state.setupOpen;
  const providers = state.providers ?? [];
  const detected = providers.filter((entry) => entry.detected);
  const [drafts, setDrafts] = useState<AgentDraft[]>([]);
  const [editing, setEditing] = useState<string | null>(null);
  const [outcomes, setOutcomes] = useState<Record<string, Outcome>>({});
  const [phase, setPhase] = useState<"edit" | "creating" | "done">("edit");
  const [createdCount, setCreatedCount] = useState(0);

  const taken = useMemo(
    () =>
      (state.employees ?? [])
        .filter((employee) => employee.enabled)
        .map((employee) => employee.name),
    [state.employees],
  );
  const detectedKey = detected.map((entry) => entry.provider).join(",");

  // A fresh recommendation each time the dialog opens or detection changes.
  useEffect(() => {
    if (!open || phase !== "edit") return;
    setDrafts(recommendTeam(providers, taken));
    setOutcomes({});
    setEditing(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, detectedKey]);

  function close() {
    store.openSetup(false);
    if (phase !== "creating") {
      setPhase("edit");
      setCreatedCount(0);
    }
  }

  function update(key: string, patch: Partial<AgentDraft>) {
    setDrafts((list) =>
      list.map((draft) => {
        if (draft.key !== key) return draft;
        const next = { ...draft, ...patch };
        if (patch.provider && patch.provider !== draft.provider) {
          // A new harness brings its own recommended model and effort.
          const preset = PRESETS.find((entry) => entry.role === draft.key);
          const choice = preset?.choices.find(
            (entry) => entry.provider === patch.provider,
          );
          next.model = choice?.model ?? "";
          next.reasoningEffort = choice?.reasoningEffort ?? "";
          next.why = choice
            ? whyChoice(draft.key, choice)
            : "Uses the harness's own default model.";
        }
        if (patch.name !== undefined) {
          next.exists = taken.some(
            (name) => name.toLowerCase() === patch.name!.trim().toLowerCase(),
          );
        }
        return next;
      }),
    );
  }

  const chosen = drafts.filter(
    (draft) => draft.include && draft.name.trim() && !draft.exists,
  );

  async function create() {
    if (!chosen.length) return;
    setPhase("creating");
    setEditing(null);
    const count = await store.createTeam(chosen, (key, outcome) =>
      setOutcomes((current) => ({ ...current, [key]: outcome })),
    );
    setCreatedCount(count);
    setPhase(count > 0 ? "done" : "edit");
    if (count > 0) store.navigate({ kind: "launcher" });
  }

  const harnessNames = detected.map((entry) => providerLabel(entry.provider));
  const description =
    phase === "done"
      ? `${createdCount} ${createdCount === 1 ? "agent is" : "agents are"} ready. Describe what you want built and the lead takes it from there.`
      : detected.length === 0
        ? "Lazoo did not find an agent harness on this machine yet."
        : `Lazoo found ${list(harnessNames)} on this machine. These agents come pre-configured from what developers recommend for each kind of work. Change anything before creating them.`;

  return (
    <Dialog
      open={open}
      onClose={close}
      title={phase === "done" ? "Your team is ready" : "Set up your team"}
      label={phase === "done" ? "Your team is ready" : "Set up your team"}
      description={description}
      width={680}
      footer={
        phase === "done" ? (
          <>
            <span className="dialog__note">
              Edit agents any time in Settings, Employees.
            </span>
            <span className="spacer" />
            <Button variant="primary" onClick={close} data-autofocus>
              Start working
            </Button>
          </>
        ) : (
          <>
            <span className="dialog__note">
              {detected.length
                ? "Agents run on your own harness accounts. Nothing starts until you send work."
                : "Install Claude Code, Codex, OpenCode, or Antigravity, then check again."}
            </span>
            <span className="spacer" />
            <Button
              variant="ghost"
              onClick={close}
              disabled={phase === "creating"}
            >
              Not now
            </Button>
            {detected.length ? (
              <Button
                variant="primary"
                loading={phase === "creating"}
                disabled={!chosen.length}
                onClick={() => void create()}
              >
                {phase === "creating"
                  ? "Creating…"
                  : `Create ${chosen.length} ${chosen.length === 1 ? "agent" : "agents"}`}
              </Button>
            ) : (
              <Button
                variant="primary"
                onClick={() => void store.refresh("providers")}
              >
                Check again
              </Button>
            )}
          </>
        )
      }
    >
      <div className="harness-strip" aria-label="Detected harnesses">
        {HARNESSES.map((provider) => {
          const found = detected.some((entry) => entry.provider === provider);
          return (
            <span
              key={provider}
              className="harness-chip"
              data-found={found}
              title={found ? "Detected" : "Not found on this machine"}
            >
              {found ? (
                <Check size={12} aria-hidden="true" />
              ) : (
                <CircleSlash size={12} aria-hidden="true" />
              )}
              {providerLabel(provider)}
            </span>
          );
        })}
      </div>

      {phase === "done" ? (
        <motion.div
          className="team-ready"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={SPRING}
        >
          {drafts
            .filter((draft) => outcomes[draft.key] === "created")
            .map((draft, index) => (
              <motion.span
                key={draft.key}
                className="team-ready__member"
                initial={{ opacity: 0, scale: 0.8 }}
                animate={{ opacity: 1, scale: 1 }}
                transition={{ ...SPRING, delay: 0.05 * index }}
              >
                <Avatar name={draft.name} provider={draft.provider} size={36} />
                <span>{draft.name}</span>
                <span className="meta">{draft.title}</span>
              </motion.span>
            ))}
        </motion.div>
      ) : (
        <ul className="drafts" aria-label="Recommended agents">
          {drafts.map((draft, index) => (
            <motion.li
              key={draft.key}
              className="draft"
              data-included={draft.include && !draft.exists}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.26, ease: EASE, delay: 0.03 * index }}
            >
              <div className="draft__row">
                <input
                  type="checkbox"
                  aria-label={`Include ${draft.name || draft.title}`}
                  checked={draft.include && !draft.exists}
                  disabled={draft.exists || phase === "creating"}
                  onChange={(event) =>
                    update(draft.key, { include: event.target.checked })
                  }
                />
                <Avatar
                  name={draft.name || draft.title}
                  provider={draft.provider}
                  size={30}
                />
                <div className="draft__main">
                  <div className="draft__title">
                    <strong>{draft.name || "unnamed"}</strong>
                    <Badge tone={draft.key === "lead" ? "accent" : "neutral"}>
                      {draft.title}
                    </Badge>
                    {draft.exists ? (
                      <Badge tone="warning">name taken</Badge>
                    ) : null}
                  </div>
                  <span className="draft__summary">{draft.summary}</span>
                </div>
                <div className="draft__config">
                  <span>{providerLabel(draft.provider)}</span>
                  <span className="draft__model">
                    {draft.model || "default model"}
                    {draft.reasoningEffort ? ` · ${draft.reasoningEffort}` : ""}
                  </span>
                </div>
                <span className="draft__state" aria-live="polite">
                  <AnimatePresence mode="wait" initial={false}>
                    {outcomes[draft.key] === "creating" ? (
                      <motion.span key="spin" {...pop}>
                        <Spinner size={14} />
                      </motion.span>
                    ) : outcomes[draft.key] === "created" ? (
                      <motion.span
                        key="ok"
                        className="draft__ok"
                        aria-label="Created"
                        {...pop}
                      >
                        <Check size={14} aria-hidden="true" />
                      </motion.span>
                    ) : outcomes[draft.key] === "failed" ? (
                      <motion.span
                        key="fail"
                        className="draft__fail"
                        aria-label="Not created"
                        {...pop}
                      >
                        <X size={14} aria-hidden="true" />
                      </motion.span>
                    ) : (
                      <motion.span key="edit" {...pop}>
                        <Button
                          variant="ghost"
                          size="sm"
                          icon
                          aria-expanded={editing === draft.key}
                          aria-label={`Edit ${draft.name || draft.title}`}
                          disabled={phase === "creating"}
                          onClick={() =>
                            setEditing(editing === draft.key ? null : draft.key)
                          }
                        >
                          <ChevronDown
                            size={14}
                            aria-hidden="true"
                            className="draft__chevron"
                          />
                        </Button>
                      </motion.span>
                    )}
                  </AnimatePresence>
                </span>
              </div>
              <p className="draft__why">
                <Sparkles size={12} aria-hidden="true" />
                {draft.why}
              </p>
              <AnimatePresence initial={false}>
                {editing === draft.key ? (
                  <motion.div
                    key="edit"
                    className="draft__edit"
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: "auto", opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    transition={{ duration: 0.24, ease: EASE }}
                  >
                    <DraftEditor
                      draft={draft}
                      harnesses={detected.map((entry) => entry.provider)}
                      onChange={(patch) => update(draft.key, patch)}
                    />
                  </motion.div>
                ) : null}
              </AnimatePresence>
            </motion.li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}

const pop = {
  initial: { opacity: 0, scale: 0.6 },
  animate: { opacity: 1, scale: 1 },
  exit: { opacity: 0, scale: 0.6 },
  transition: { duration: 0.16, ease: EASE },
};

function DraftEditor({
  draft,
  harnesses,
  onChange,
}: {
  draft: AgentDraft;
  harnesses: string[];
  onChange: (patch: Partial<AgentDraft>) => void;
}) {
  const id = `draft-${draft.key}`;
  const efforts = EFFORTS[draft.provider] ?? [];
  const suggestions = MODEL_SUGGESTIONS[draft.provider] ?? [];
  return (
    <div className="draft__grid">
      <Field label="Name" htmlFor={`${id}-name`}>
        <input
          id={`${id}-name`}
          className="input input--sm"
          value={draft.name}
          maxLength={64}
          onChange={(event) => onChange({ name: event.target.value })}
        />
      </Field>
      <Field label="Harness" htmlFor={`${id}-harness`}>
        <select
          id={`${id}-harness`}
          className="select select--sm"
          value={draft.provider}
          onChange={(event) => onChange({ provider: event.target.value })}
        >
          {harnesses.map((provider) => (
            <option key={provider} value={provider}>
              {providerLabel(provider)}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Model"
        htmlFor={`${id}-model`}
        hint={
          draft.provider === "opencode"
            ? "provider/model, or empty for OpenCode's default"
            : "Empty uses the harness default"
        }
      >
        <input
          id={`${id}-model`}
          className="input input--sm mono-input"
          list={`${id}-models`}
          value={draft.model}
          onChange={(event) => onChange({ model: event.target.value })}
        />
        <datalist id={`${id}-models`}>
          {suggestions.map((model) => (
            <option key={model} value={model} />
          ))}
        </datalist>
      </Field>
      <Field label="Reasoning effort" htmlFor={`${id}-effort`}>
        <select
          id={`${id}-effort`}
          className="select select--sm"
          value={
            efforts.includes(draft.reasoningEffort) ? draft.reasoningEffort : ""
          }
          disabled={!efforts.length}
          onChange={(event) =>
            onChange({ reasoningEffort: event.target.value })
          }
        >
          <option value="">Harness default</option>
          {efforts.map((effort) => (
            <option key={effort} value={effort}>
              {effort}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Authority ceiling" htmlFor={`${id}-authority`}>
        <select
          id={`${id}-authority`}
          className="select select--sm"
          value={draft.authorityMode}
          onChange={(event) =>
            onChange({ authorityMode: event.target.value as AuthorityMode })
          }
        >
          {(["ask", "self-decision", "full-autonomy"] as AuthorityMode[]).map(
            (mode) => (
              <option key={mode} value={mode}>
                {authorityLabel(mode)}
              </option>
            ),
          )}
        </select>
      </Field>
      <Field label="Workspace" htmlFor={`${id}-checkout`}>
        <select
          id={`${id}-checkout`}
          className="select select--sm"
          value={draft.checkoutPolicy}
          onChange={(event) =>
            onChange({
              checkoutPolicy: event.target.value as "isolated" | "read-only",
            })
          }
        >
          <option value="isolated">Own worktree</option>
          <option value="read-only">Read-only</option>
        </select>
      </Field>
    </div>
  );
}

function list(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`;
}
