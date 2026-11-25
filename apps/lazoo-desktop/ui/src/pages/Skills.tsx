import { Ellipsis } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";

import {
  Badge,
  Button,
  Dialog,
  Field,
  Menu,
  Segmented,
  Empty,
  Page,
  PageHeader,
  Section,
  TableSkeleton,
} from "../components/primitives";
import { asFailure } from "../lib/bridge";
import { useAppState, useBridge } from "../lib/context";
import {
  EXTENSION_PROVIDERS,
  ErrorLine,
  cleanPath,
  displayPath,
  homePath,
  isExpired,
  isStale,
  parentPath,
  pendingPaths,
  sameTarget,
  useExtensionIntent,
  useExtensionSync,
} from "../lib/extensions";
import type {
  ExposureMode,
  LibrarySkill,
  RpcFailure,
  SkillInfo,
  SkillInventoryEntry,
  SkillListResult,
  SkillMutationResult,
  SkillOutcome,
  SkillPreview,
  SkillSource,
  SkillSourceKind,
} from "../lib/protocol";
import { Failure, useRead } from "../lib/useRead";
import {
  formatBytes,
  middleTruncate,
  providerLabel,
  untilTime,
} from "../lib/util";

type Call<T> = { ok: true; value: T } | { ok: false; error: RpcFailure };

async function attempt<T>(run: () => Promise<T>): Promise<Call<T>> {
  try {
    return { ok: true, value: await run() };
  } catch (error) {
    return { ok: false, error: asFailure(error) };
  }
}

const SHORT_LABEL: Record<string, string> = {
  "claude-code": "Claude",
  codex: "Codex",
  "github-copilot": "Copilot",
  opencode: "OpenCode",
  antigravity: "Antigravity",
};

const SOURCE_KINDS: {
  value: SkillSourceKind;
  label: string;
  placeholder: string;
}[] = [
  {
    value: "skills-sh",
    label: "skills.sh",
    placeholder: "https://skills.sh/owner/repo",
  },
  {
    value: "github",
    label: "GitHub",
    placeholder:
      "owner/repo or https://github.com/owner/repo/tree/main/skills/x",
  },
  {
    value: "git",
    label: "Git URL",
    placeholder: "https://host/org/repo.git",
  },
  {
    value: "local",
    label: "Folder",
    placeholder: "C:\\path\\to\\skill-or-repo",
  },
];

const OUTCOME_SUMMARY: Record<string, string> = {
  "skill.install": "Installed.",
  "skill.update": "Updated.",
  "skill.expose": "Added to the harness.",
  "skill.unexpose": "Removed from the harness.",
  "skill.remove": "Removed from the library.",
};

/** `owner/repo` for GitHub-shaped sources, the path or URL otherwise. */
export function sourceLabel(source: SkillSource): string {
  const text = source.source.trim();
  const github = text.match(
    /^(?:https?:\/\/)?(?:www\.)?(?:github\.com|skills\.sh)\/([^/\s]+)\/([^/\s#?]+)/i,
  );
  if (github) return `${github[1]}/${github[2]!.replace(/\.git$/i, "")}`;
  if (source.sourceKind === "github") {
    const short = text.match(/^([\w.-]+)\/([\w.-]+)/);
    if (short) return `${short[1]}/${short[2]}`;
  }
  if (source.sourceKind === "local") return displayPath(text, 44);
  return middleTruncate(
    text.replace(/^https?:\/\//i, "").replace(/\.git$/i, ""),
    44,
  );
}

function modeFor(name: string, list: SkillListResult): ExposureMode {
  return list.exposures.find((entry) => entry.name === name)?.mode ?? "link";
}

/** Harnesses that reach a managed skill through a shared folder, not through Lazoo. */
export function alsoVisible(
  skill: LibrarySkill,
  list: SkillListResult,
): string | null {
  const exposed = new Set(
    list.exposures
      .filter((entry) => entry.name === skill.name)
      .map((entry) => entry.target.provider),
  );
  const found = new Map<string, string | null>();
  for (const entry of list.inventory) {
    if (!entry.managed || entry.name !== skill.name) continue;
    for (const target of entry.alsoVisibleTo) {
      if (exposed.has(target.provider) || found.has(target.provider)) continue;
      const seen = list.inventory.find(
        (other) =>
          other.name === skill.name &&
          sameTarget(other.target, target) &&
          other.canonicalPath === entry.canonicalPath,
      );
      found.set(
        target.provider,
        seen ? homePath(parentPath(seen.path)).replaceAll("\\", "/") : null,
      );
    }
  }
  if (!found.size) return null;
  const parts = [...found].map(
    ([provider, path]) =>
      `${providerLabel(provider)}${path ? ` through ${path}` : ""}`,
  );
  return `Also visible to ${parts.join(", ")}`;
}

function OutcomeNote({ outcome }: { outcome: SkillOutcome }) {
  if (outcome.status === "applied") {
    return (
      <p className="meta" role="status">
        {OUTCOME_SUMMARY[outcome.method] ?? "Done."} Running sessions pick this
        up after they restart.
      </p>
    );
  }
  return (
    <div className="ext-note" role="alert" data-tone="warning">
      <strong>
        {outcome.status === "rolled-back"
          ? "Rolled back. Nothing was changed."
          : "Changes are paused."}
      </strong>
      {outcome.status === "recovery-needed" ? (
        <span>
          The change could not be undone cleanly. Nothing more can change until
          this folder is checked: <code>{outcome.backupDirectory}</code>
        </span>
      ) : null}
      <ErrorLine error={outcome.error} quiet />
    </div>
  );
}

export function SkillsPage() {
  const bridge = useBridge();
  const { extensionBusy } = useAppState();
  const read = useRead<SkillListResult>("skill.list");
  useExtensionSync(read.reload);
  const list = read.data;
  const [working, setWorking] = useState(false);
  const [notice, setNotice] = useState<RpcFailure | null>(null);
  const [outcome, setOutcome] = useState<SkillOutcome | null>(null);
  const [adding, setAdding] = useState(false);
  const [prefill, setPrefill] = useState<string | null>(null);
  const [updating, setUpdating] = useState<LibrarySkill | null>(null);
  const [removing, setRemoving] = useState<LibrarySkill | null>(null);

  useExtensionIntent("add-skills", () => {
    setPrefill(null);
    setAdding(true);
  });

  const paused = Boolean(list?.pending) && !extensionBusy;
  const blocked = extensionBusy || working || Boolean(list?.pending);

  function onApplied(result: SkillOutcome) {
    setOutcome(result);
    setNotice(null);
    read.reload();
  }

  function onFailure(error: RpcFailure) {
    setOutcome(null);
    if (isStale(error)) {
      read.reload();
      setNotice({
        code: error.code,
        message:
          "Skills changed since this page loaded. The list is current again; try once more.",
      });
    } else {
      setNotice(error);
    }
  }

  async function toggle(skill: LibrarySkill, provider: string, on: boolean) {
    if (!list) return;
    setWorking(true);
    const base = { name: skill.name, expectedRevision: list.revision };
    const result = await attempt(() =>
      on
        ? bridge.call<SkillMutationResult>("skill.expose", {
            ...base,
            providers: [provider],
            mode: modeFor(skill.name, list),
          })
        : bridge.call<SkillMutationResult>("skill.unexpose", {
            ...base,
            providers: [provider],
          }),
    );
    setWorking(false);
    if (result.ok) onApplied(result.value.outcome);
    else onFailure(result.error);
  }

  const managedPaths = useMemo(
    () =>
      new Set(
        (list?.inventory ?? [])
          .filter((entry) => entry.managed)
          .map((entry) => entry.canonicalPath),
      ),
    [list],
  );
  const unmanaged = useMemo(() => {
    const groups = new Map<string, SkillInventoryEntry[]>();
    for (const entry of list?.inventory ?? []) {
      if (entry.managed || managedPaths.has(entry.canonicalPath)) continue;
      groups.set(entry.name, [...(groups.get(entry.name) ?? []), entry]);
    }
    return [...groups].sort(([a], [b]) => a.localeCompare(b));
  }, [list, managedPaths]);

  return (
    <Page>
      <PageHeader
        title="Skills"
        description="Skills work the same in every harness. Lazoo keeps one copy and links it into the harnesses you choose."
        actions={
          <Button variant="primary" onClick={() => setAdding(true)}>
            Add skills
          </Button>
        }
      />
      <Failure error={read.error} />

      {paused && list ? (
        <div className="ext-note" role="alert" data-tone="warning">
          <strong>Changes are paused.</strong>
          <span>
            An earlier change did not finish. Skills cannot change until these
            paths are checked:
          </span>
          {pendingPaths(list.pending).map((path) => (
            <code key={path}>{path}</code>
          ))}
        </div>
      ) : null}
      {extensionBusy ? (
        <p className="meta" role="status">
          A change is in progress. Controls return when it finishes.
        </p>
      ) : null}
      <ErrorLine error={notice} />
      {outcome ? <OutcomeNote outcome={outcome} /> : null}

      <Section label="Library" title="Library">
        {list?.library.length ? (
          <div className="ext-list" role="list" aria-label="Library skills">
            {list.library.map((skill) => (
              <LibraryRow
                key={skill.name}
                skill={skill}
                list={list}
                blocked={blocked}
                onToggle={(provider, on) => void toggle(skill, provider, on)}
                onUpdate={() => setUpdating(skill)}
                onRemove={() => setRemoving(skill)}
              />
            ))}
          </div>
        ) : list ? (
          <Empty
            compact
            title="No skills in the library yet"
            detail="Add skills from skills.sh, GitHub, a Git URL or a folder."
          />
        ) : read.error ? null : (
          <TableSkeleton rows={2} />
        )}
        {list?.problems.map((problem) => (
          <p
            className="meta"
            key={`${problem.path}:${problem.reason}`}
            title={problem.path}
          >
            {problem.reason} ·{" "}
            <span className="mono">{displayPath(problem.path)}</span>
          </p>
        ))}
      </Section>

      {unmanaged.length ? (
        <Section
          label="On this computer"
          title="On this computer"
          description="Skills the harnesses already see that Lazoo does not manage."
        >
          <div className="ext-list" role="list" aria-label="Unmanaged skills">
            {unmanaged.map(([name, entries]) => (
              <UnmanagedRow
                key={name}
                name={name}
                entries={entries}
                onAdd={() => {
                  setPrefill(cleanPath(entries[0]!.canonicalPath));
                  setAdding(true);
                }}
              />
            ))}
          </div>
        </Section>
      ) : null}

      <AddSkillsDialog
        open={adding}
        list={list}
        prefill={prefill}
        blocked={blocked}
        onClose={() => setAdding(false)}
        onReload={read.reload}
      />
      <UpdateDialog
        skill={updating}
        list={list}
        blocked={blocked}
        onClose={() => setUpdating(null)}
        onApplied={onApplied}
        onReload={read.reload}
      />
      <RemoveDialog
        skill={removing}
        list={list}
        blocked={blocked}
        onClose={() => setRemoving(null)}
        onApplied={onApplied}
        onFailure={onFailure}
      />
    </Page>
  );
}

function LibraryRow({
  skill,
  list,
  blocked,
  onToggle,
  onUpdate,
  onRemove,
}: {
  skill: LibrarySkill;
  list: SkillListResult;
  blocked: boolean;
  onToggle: (provider: string, on: boolean) => void;
  onUpdate: () => void;
  onRemove: () => void;
}) {
  const also = alsoVisible(skill, list);
  const revision = skill.resolvedRevision?.slice(0, 7);
  return (
    <div className="ext-row" role="listitem">
      <div className="ext-row__main">
        <span className="ext-row__name mono" title={skill.name}>
          {skill.name}
        </span>
        <span className="ext-row__desc">{skill.description}</span>
        <span className="meta">
          {sourceLabel(skill.source)}
          {revision ? (
            <>
              {" "}
              · <span className="mono">{revision}</span>
            </>
          ) : null}
        </span>
      </div>
      <div
        className="ext-row__toggles"
        role="group"
        aria-label={`Harnesses for ${skill.name}`}
      >
        {EXTENSION_PROVIDERS.map((provider) => {
          const exposure = list.exposures.find(
            (entry) =>
              entry.name === skill.name && entry.target.provider === provider,
          );
          const native = list.targets.some(
            (target) =>
              target.target.provider === provider &&
              target.compatibility === "Native",
          );
          return (
            <button
              key={provider}
              type="button"
              className="harness-toggle"
              aria-pressed={Boolean(exposure)}
              aria-label={`${providerLabel(provider)} for ${skill.name}`}
              title={
                !native
                  ? `${providerLabel(provider)} has no skills folder Lazoo can use`
                  : exposure
                    ? `${exposure.mode === "link" ? "Linked" : "Copied"} into ${providerLabel(provider)}`
                    : `Not in ${providerLabel(provider)}`
              }
              disabled={blocked || !native}
              onClick={() => onToggle(provider, !exposure)}
            >
              <span
                className="harness-toggle__swatch"
                style={{ background: `var(--lz-provider-${provider})` }}
                aria-hidden="true"
              />
              {SHORT_LABEL[provider]}
              {exposure ? (
                <span className="harness-toggle__mode">{exposure.mode}</span>
              ) : null}
            </button>
          );
        })}
      </div>
      <span className="ext-row__menu">
        <Menu
          label={`More for ${skill.name}`}
          items={[
            { label: "Update", onSelect: onUpdate, disabled: blocked },
            {
              label: "Remove",
              danger: true,
              onSelect: onRemove,
              disabled: blocked,
            },
          ]}
        >
          <Ellipsis size={14} aria-hidden="true" />
        </Menu>
      </span>
      {also ? <p className="meta ext-row__note">{also}</p> : null}
      {skill.problems.map((problem) => (
        <p className="meta ext-row__note" key={problem}>
          {problem}
        </p>
      ))}
    </div>
  );
}

function UnmanagedRow({
  name,
  entries,
  onAdd,
}: {
  name: string;
  entries: SkillInventoryEntry[];
  onAdd: () => void;
}) {
  const places = useMemo(() => {
    const byPath = new Map<string, SkillInventoryEntry[]>();
    for (const entry of entries)
      byPath.set(entry.path, [...(byPath.get(entry.path) ?? []), entry]);
    return [...byPath.values()];
  }, [entries]);
  return (
    <div className="ext-row ext-row--plain" role="listitem">
      <div className="ext-row__main">
        <span className="ext-row__name mono" title={name}>
          {name}
        </span>
        <span className="ext-row__desc">{entries[0]!.description}</span>
      </div>
      <span className="ext-row__menu">
        <Button
          variant="ghost"
          aria-label={`Add ${name} to library`}
          onClick={onAdd}
        >
          Add to library
        </Button>
      </span>
      <ul className="ext-places">
        {places.map((group) => {
          const first = group[0]!;
          const harnesses = [
            ...new Set(group.map((entry) => providerLabel(entry.provider))),
          ].join(", ");
          return (
            <li key={first.path}>
              <span>{harnesses}</span>
              <span className="meta">{first.kind}</span>
              <span className="mono ext-path" title={cleanPath(first.path)}>
                {displayPath(first.path, 52)}
              </span>
              {group.some((entry) => entry.duplicate) ? (
                <Badge
                  tone="warning"
                  title="This harness sees the same skill name twice"
                >
                  duplicate
                </Badge>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ---- update and remove ---------------------------------------------------

function short(value: string | null | undefined, length: number): string {
  return value ? value.replace(/^sha256:/, "").slice(0, length) : "none";
}

function UpdateDialog({
  skill,
  list,
  blocked,
  onClose,
  onApplied,
  onReload,
}: {
  skill: LibrarySkill | null;
  list: SkillListResult | null;
  blocked: boolean;
  onClose: () => void;
  onApplied: (outcome: SkillOutcome) => void;
  onReload: () => void;
}) {
  const bridge = useBridge();
  const [preview, setPreview] = useState<SkillPreview | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [applying, setApplying] = useState(false);
  const [attemptCount, setAttemptCount] = useState(0);
  const name = skill?.name ?? null;

  useEffect(() => {
    if (!name) return;
    let cancelled = false;
    setPreview(null);
    setError(null);
    void attempt(() =>
      bridge.call<SkillPreview>("skill.update.preview", { name }),
    ).then((result) => {
      if (cancelled) return;
      if (result.ok) setPreview(result.value);
      else setError(result.error);
    });
    return () => {
      cancelled = true;
    };
  }, [bridge, name, attemptCount]);

  const next = preview?.skills.find((entry) => entry.name === name) ?? null;
  const unchanged = Boolean(skill && next && next.checksum === skill.checksum);

  async function apply() {
    if (!skill || !preview || !next || !list) return;
    setApplying(true);
    const result = await attempt(() =>
      bridge.call<SkillMutationResult>("skill.update", {
        previewId: preview.previewId,
        names: [skill.name],
        providers: [],
        mode: modeFor(skill.name, list),
        replace: true,
        expectedRevision: preview.revision,
      }),
    );
    setApplying(false);
    if (result.ok) {
      onApplied(result.value.outcome);
      onClose();
    } else {
      if (isStale(result.error) || isExpired(result.error)) {
        onReload();
        setPreview(null);
      }
      setError(result.error);
    }
  }

  return (
    <Dialog
      open={Boolean(skill)}
      onClose={onClose}
      title={skill ? `Update ${skill.name}` : "Update skill"}
      description="Fetches the recorded source again and shows what would change."
      width={520}
      footer={
        <>
          <span className="spacer" />
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          {error && !preview ? (
            <Button onClick={() => setAttemptCount((value) => value + 1)}>
              Check again
            </Button>
          ) : (
            <Button
              variant="primary"
              loading={applying}
              disabled={!next || unchanged || blocked}
              onClick={() => void apply()}
            >
              Update
            </Button>
          )}
        </>
      }
    >
      <div className="stack">
        {!preview && !error ? (
          <p className="meta" role="status">
            Fetching the source. A Git source can take up to a minute.
          </p>
        ) : null}
        <ErrorLine error={error} />
        {preview && skill && !next ? (
          <p className="meta">
            The source no longer contains <code>{skill.name}</code>.
          </p>
        ) : null}
        {preview && skill && next ? (
          <>
            <table className="ext-compare">
              <thead>
                <tr>
                  <th scope="col">
                    <span className="sr-only">Property</span>
                  </th>
                  <th scope="col">Installed</th>
                  <th scope="col">Available</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <th scope="row">Revision</th>
                  <td className="mono">{short(skill.resolvedRevision, 7)}</td>
                  <td className="mono">{short(preview.resolvedRevision, 7)}</td>
                </tr>
                <tr>
                  <th scope="row">Checksum</th>
                  <td className="mono">{short(skill.checksum, 12)}</td>
                  <td className="mono">{short(next.checksum, 12)}</td>
                </tr>
                <tr>
                  <th scope="row">Files</th>
                  <td>{skill.fileCount}</td>
                  <td>{next.fileCount}</td>
                </tr>
                <tr>
                  <th scope="row">Size</th>
                  <td>{formatBytes(skill.bytes)}</td>
                  <td>{formatBytes(next.bytes)}</td>
                </tr>
              </tbody>
            </table>
            {unchanged ? (
              <p className="meta">Already up to date.</p>
            ) : (
              <p className="meta">
                Updating replaces Lazoo&apos;s copy and every link or copy of
                it. Running sessions pick it up after they restart.
              </p>
            )}
            {next.problems.map((problem) => (
              <p className="ext-error" key={problem}>
                {problem}
              </p>
            ))}
          </>
        ) : null}
      </div>
    </Dialog>
  );
}

function RemoveDialog({
  skill,
  list,
  blocked,
  onClose,
  onApplied,
  onFailure,
}: {
  skill: LibrarySkill | null;
  list: SkillListResult | null;
  blocked: boolean;
  onClose: () => void;
  onApplied: (outcome: SkillOutcome) => void;
  onFailure: (error: RpcFailure) => void;
}) {
  const bridge = useBridge();
  const [busy, setBusy] = useState(false);
  const exposures = list?.exposures.filter((e) => e.name === skill?.name) ?? [];
  const libraryPath = exposures[0]?.libraryPath;

  async function remove() {
    if (!skill || !list) return;
    setBusy(true);
    const result = await attempt(() =>
      bridge.call<SkillMutationResult>("skill.remove", {
        name: skill.name,
        expectedRevision: list.revision,
      }),
    );
    setBusy(false);
    onClose();
    if (result.ok) onApplied(result.value.outcome);
    else onFailure(result.error);
  }

  return (
    <Dialog
      open={Boolean(skill)}
      onClose={onClose}
      title={skill ? `Remove ${skill.name}` : "Remove skill"}
      description="Skills you added yourself outside Lazoo are not touched."
      width={520}
      footer={
        <>
          <span className="spacer" />
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="danger"
            loading={busy}
            disabled={blocked}
            onClick={() => void remove()}
          >
            Remove skill
          </Button>
        </>
      }
    >
      <div className="stack">
        <p>This deletes:</p>
        <ul className="ext-paths">
          <li>
            Lazoo&apos;s library copy
            {libraryPath ? (
              <>
                {" "}
                <code>{homePath(libraryPath)}</code>
              </>
            ) : null}
          </li>
          {exposures.map((exposure) => (
            <li key={exposure.path}>
              The {exposure.mode} in {providerLabel(exposure.target.provider)}{" "}
              <code>{homePath(exposure.path)}</code>
            </li>
          ))}
        </ul>
      </div>
    </Dialog>
  );
}

// ---- add skills ------------------------------------------------------------

function AddSkillsDialog({
  open,
  list,
  prefill,
  blocked,
  onClose,
  onReload,
}: {
  open: boolean;
  list: SkillListResult | null;
  prefill: string | null;
  blocked: boolean;
  onClose: () => void;
  onReload: () => void;
}) {
  const bridge = useBridge();
  const [kind, setKind] = useState<SkillSourceKind>("github");
  const [source, setSource] = useState("");
  const [gitRef, setGitRef] = useState("");
  const [fetching, setFetching] = useState(false);
  const [preview, setPreview] = useState<SkillPreview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [replace, setReplace] = useState<Set<string>>(new Set());
  const [providers, setProviders] = useState<Set<string>>(new Set());
  const [mode, setMode] = useState<ExposureMode>("link");
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [result, setResult] = useState<SkillMutationResult | null>(null);
  const [installed, setInstalled] = useState(0);

  useEffect(() => {
    if (!open) return;
    setKind(prefill ? "local" : "github");
    setSource(prefill ?? "");
    setGitRef("");
    setPreview(null);
    setSelected(new Set());
    setReplace(new Set());
    setProviders(new Set());
    setMode("link");
    setError(null);
    setResult(null);
    setFetching(false);
  }, [open, prefill]);

  const kindInfo = SOURCE_KINDS.find((entry) => entry.value === kind)!;
  const refKind = kind === "github" || kind === "git";
  const nativeProviders = EXTENSION_PROVIDERS.filter((provider) =>
    list?.targets.some(
      (target) =>
        target.target.provider === provider &&
        target.compatibility === "Native",
    ),
  );

  async function find(event: FormEvent) {
    event.preventDefault();
    const text = source.trim();
    if (!text) return;
    setFetching(true);
    setError(null);
    const result = await attempt(() =>
      bridge.call<SkillPreview>("skill.source.preview", {
        source: text,
        sourceKind: kind,
        ...(refKind && gitRef.trim() ? { gitRef: gitRef.trim() } : {}),
      }),
    );
    setFetching(false);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    const found = result.value;
    setPreview(found);
    // A collection repository can hold dozens of skills: nothing is chosen for the user unless
    // the source has exactly one installable skill.
    const installableNames = found.skills
      .filter((skill) => installable(found, skill))
      .map((skill) => skill.name);
    setSelected(new Set(installableNames.length === 1 ? installableNames : []));
    setReplace(new Set());
  }

  function backToSource(message: RpcFailure | null) {
    setPreview(null);
    setSelected(new Set());
    setReplace(new Set());
    setError(message);
  }

  const blockers = useMemo(() => {
    if (!preview) return [];
    const found: { name: string; provider: string; path: string }[] = [];
    for (const name of selected) {
      for (const conflict of preview.conflicts) {
        if (
          conflict.name === name &&
          conflict.scope === "harness" &&
          conflict.managed !== true &&
          conflict.kind === "conflict" &&
          conflict.target &&
          providers.has(conflict.target.provider)
        )
          found.push({
            name,
            provider: conflict.target.provider,
            path: conflict.path ?? "",
          });
      }
    }
    return found;
  }, [preview, selected, providers]);

  const needsReplace = preview
    ? [...selected].filter(
        (name) =>
          preview.conflicts.some(
            (conflict) =>
              conflict.name === name &&
              conflict.scope === "library" &&
              conflict.kind === "conflict",
          ) && !replace.has(name),
      )
    : [];

  async function install() {
    if (!preview) return;
    const names = preview.skills
      .map((skill) => skill.name)
      .filter((name) => selected.has(name));
    setInstalling(true);
    setError(null);
    const outcome = await attempt(() =>
      bridge.call<SkillMutationResult>("skill.install", {
        previewId: preview.previewId,
        names,
        providers: EXTENSION_PROVIDERS.filter((provider) =>
          providers.has(provider),
        ),
        mode,
        replace: names.some((name) => replace.has(name)),
        expectedRevision: preview.revision,
      }),
    );
    setInstalling(false);
    if (outcome.ok) {
      setResult(outcome.value);
      setInstalled(names.length);
      onReload();
      return;
    }
    if (isStale(outcome.error)) {
      onReload();
      backToSource({
        code: outcome.error.code,
        message:
          "Skills changed while you were choosing. Find the skills again.",
      });
    } else if (isExpired(outcome.error)) {
      backToSource({
        code: outcome.error.code,
        message: "The preview expired. Find the skills again.",
      });
    } else {
      setError(outcome.error);
    }
  }

  const count = selected.size;
  const footer = result ? (
    <>
      <span className="spacer" />
      <Button variant="primary" onClick={onClose}>
        Done
      </Button>
    </>
  ) : preview ? (
    <>
      {preview ? (
        <span className="meta">
          Preview expires {untilTime(preview.expiresAt)}
        </span>
      ) : null}
      <span className="spacer" />
      <Button variant="ghost" onClick={() => backToSource(null)}>
        Back
      </Button>
      <Button
        variant="primary"
        loading={installing}
        disabled={
          !count || blocked || blockers.length > 0 || needsReplace.length > 0
        }
        onClick={() => void install()}
      >
        {`Install ${count} skill${count === 1 ? "" : "s"}`}
      </Button>
    </>
  ) : (
    <>
      <span className="spacer" />
      <Button variant="ghost" onClick={onClose}>
        Cancel
      </Button>
      <Button
        variant="primary"
        type="submit"
        form="add-skills-source"
        loading={fetching}
        disabled={!source.trim()}
      >
        Find skills
      </Button>
    </>
  );

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Add skills"
      description={
        result
          ? undefined
          : preview
            ? "Choose what to install and where it should appear."
            : "Find skills in a source. Nothing is installed until you confirm."
      }
      width={620}
      footer={footer}
    >
      {result ? (
        <div className="stack">
          {result.outcome.status === "applied" ? (
            <p>
              Installed {installed} skill{installed === 1 ? "" : "s"}.
            </p>
          ) : null}
          <OutcomeNote outcome={result.outcome} />
        </div>
      ) : preview ? (
        <div className="stack">
          <p className="meta">
            From <span className="mono">{sourceLabel(preview.source)}</span>
            {preview.resolvedRevision ? (
              <>
                {" "}
                at{" "}
                <span className="mono">
                  {preview.resolvedRevision.slice(0, 7)}
                </span>
              </>
            ) : null}
          </p>
          <ErrorLine error={error} />
          <fieldset className="ext-fieldset">
            <legend>Skills found</legend>
            {preview.skills.filter((skill) => installable(preview, skill))
              .length > 1 ? (
              <div className="ext-select-all">
                <Button
                  variant="ghost"
                  onClick={() => {
                    const names = preview.skills
                      .filter((skill) => installable(preview, skill))
                      .map((skill) => skill.name);
                    const all = names.every((name) => selected.has(name));
                    setSelected(new Set(all ? [] : names));
                  }}
                >
                  {preview.skills
                    .filter((skill) => installable(preview, skill))
                    .every((skill) => selected.has(skill.name))
                    ? "Select none"
                    : "Select all"}
                </Button>
              </div>
            ) : null}
            {preview.skills.length ? (
              preview.skills.map((skill) => (
                <FoundSkill
                  key={skill.name}
                  skill={skill}
                  preview={preview}
                  checked={selected.has(skill.name)}
                  replace={replace.has(skill.name)}
                  providers={providers}
                  onCheck={(on) =>
                    setSelected((current) => toggled(current, skill.name, on))
                  }
                  onReplace={(on) => {
                    setReplace((current) => toggled(current, skill.name, on));
                    if (on)
                      setSelected((current) =>
                        toggled(current, skill.name, true),
                      );
                  }}
                />
              ))
            ) : (
              <p className="meta">This source has no skills.</p>
            )}
          </fieldset>
          <fieldset className="ext-fieldset">
            <legend>Show them in</legend>
            {nativeProviders.map((provider) => {
              const limits =
                list?.targets
                  .filter((target) => target.target.provider === provider)
                  .flatMap((target) => target.limitations) ?? [];
              return (
                <label className="check-row" key={provider}>
                  <input
                    type="checkbox"
                    checked={providers.has(provider)}
                    onChange={(event) =>
                      setProviders((current) =>
                        toggled(current, provider, event.target.checked),
                      )
                    }
                  />
                  <span>
                    <strong>{providerLabel(provider)}</strong>
                    {providers.has(provider)
                      ? [...new Set(limits)].map((limit) => (
                          <span className="meta" key={limit}>
                            {provider === "antigravity"
                              ? `Unverified: ${limit}.`
                              : `${limit}.`}
                          </span>
                        ))
                      : null}
                  </span>
                </label>
              );
            })}
            {!providers.size ? (
              <p className="meta">
                With no harness chosen, the skills are added to the library
                only.
              </p>
            ) : null}
          </fieldset>
          <div className="ext-mode">
            <Segmented
              label="Install mode"
              name="skill-mode"
              value={mode}
              onChange={setMode}
              options={[
                { value: "link", label: "Link (recommended)" },
                { value: "copy", label: "Copy" },
              ]}
            />
            <p className="meta">
              {mode === "link"
                ? "Every harness points at the one copy Lazoo keeps, so updates reach all of them."
                : "Each harness gets its own copy. Later updates replace each copy separately."}
            </p>
          </div>
          {blockers.map((blocker) => (
            <p
              className="ext-error"
              key={`${blocker.name}:${blocker.provider}`}
            >
              A different {blocker.name} is already visible to{" "}
              {providerLabel(blocker.provider)}
              {blocker.path ? (
                <>
                  {" "}
                  at <code>{homePath(blocker.path)}</code>
                </>
              ) : null}
              . It blocks the install. Remove it or clear{" "}
              {providerLabel(blocker.provider)}.
            </p>
          ))}
          {needsReplace.length ? (
            <p className="meta">
              Tick Replace for {needsReplace.join(", ")} to overwrite the
              library copy.
            </p>
          ) : null}
        </div>
      ) : (
        <form
          id="add-skills-source"
          className="stack"
          onSubmit={(event) => void find(event)}
        >
          <Segmented
            label="Source type"
            name="skill-source-kind"
            value={kind}
            onChange={setKind}
            options={SOURCE_KINDS.map(({ value, label }) => ({
              value,
              label,
            }))}
          />
          <Field label="Source" htmlFor="skill-source">
            <input
              id="skill-source"
              className="input mono-input"
              data-autofocus
              spellCheck={false}
              placeholder={kindInfo.placeholder}
              value={source}
              onChange={(event) => setSource(event.target.value)}
            />
          </Field>
          {refKind ? (
            <Field
              label="Branch, tag or commit"
              htmlFor="skill-ref"
              hint="Optional. Leave empty for the default branch."
            >
              <input
                id="skill-ref"
                className="input mono-input"
                spellCheck={false}
                value={gitRef}
                onChange={(event) => setGitRef(event.target.value)}
              />
            </Field>
          ) : null}
          {fetching ? (
            <p className="meta" role="status">
              Fetching and checking the source. A Git source can take up to a
              minute.
            </p>
          ) : null}
          <ErrorLine error={error} />
        </form>
      )}
    </Dialog>
  );
}

function toggled(current: Set<string>, value: string, on: boolean) {
  const next = new Set(current);
  if (on) next.add(value);
  else next.delete(value);
  return next;
}

/** A skill the install can take as it is: valid and not yet in the library. */
function installable(preview: SkillPreview, skill: SkillInfo): boolean {
  if (skill.problems.length) return false;
  return !preview.conflicts.some(
    (conflict) => conflict.name === skill.name && conflict.scope === "library",
  );
}

function FoundSkill({
  skill,
  preview,
  checked,
  replace,
  providers,
  onCheck,
  onReplace,
}: {
  skill: SkillInfo;
  preview: SkillPreview;
  checked: boolean;
  replace: boolean;
  providers: Set<string>;
  onCheck: (on: boolean) => void;
  onReplace: (on: boolean) => void;
}) {
  const conflicts = preview.conflicts.filter(
    (conflict) => conflict.name === skill.name,
  );
  const library = conflicts.find((conflict) => conflict.scope === "library");
  const identical = library?.kind === "duplicate";
  const differs = library?.kind === "conflict";
  const invalid = skill.problems.length > 0;
  const harness = conflicts.filter(
    (conflict) => conflict.scope === "harness" && conflict.target,
  );
  return (
    <div className="ext-found" data-disabled={invalid || identical}>
      <label className="check-row">
        <input
          type="checkbox"
          checked={checked}
          disabled={invalid || identical}
          onChange={(event) => onCheck(event.target.checked)}
        />
        <span>
          <strong className="mono">{skill.name}</strong>
          <span className="meta ext-clamp" title={skill.description}>
            {skill.description}
          </span>
          <span className="meta">
            {skill.fileCount} file{skill.fileCount === 1 ? "" : "s"} ·{" "}
            {formatBytes(skill.bytes)}
          </span>
        </span>
      </label>
      {skill.problems.map((problem) => (
        <p className="ext-error ext-found__line" key={problem}>
          {problem}
        </p>
      ))}
      {identical ? (
        <p className="meta ext-found__line">
          Already in your library and identical. Skipped.
        </p>
      ) : null}
      {differs && library?.replaceAllowed ? (
        <label className="check-row ext-found__line">
          <input
            type="checkbox"
            checked={replace}
            onChange={(event) => onReplace(event.target.checked)}
          />
          <span>
            <strong>Replace</strong>
            <span className="meta">
              A different version is in your library. Replacing overwrites it.
            </span>
          </span>
        </label>
      ) : null}
      {harness.map((conflict) => {
        const provider = conflict.target!.provider;
        const label = providerLabel(provider);
        if (conflict.managed)
          return (
            <p
              className="meta ext-found__line"
              key={`${provider}:${conflict.path}`}
            >
              Lazoo already manages this for {label}; it is updated in place.
            </p>
          );
        return (
          <p
            className={
              conflict.kind === "conflict" && providers.has(provider)
                ? "ext-error ext-found__line"
                : "meta ext-found__line"
            }
            key={`${provider}:${conflict.path}`}
          >
            {conflict.kind === "conflict"
              ? `A different copy is already visible to ${label} and not managed by Lazoo.`
              : `An identical copy is already visible to ${label}.`}
          </p>
        );
      })}
    </div>
  );
}
