import { Ellipsis } from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";

import {
  Badge,
  Button,
  Dialog,
  Field,
  Menu,
  StatusWord,
  Switch,
} from "../components/primitives";
import { asFailure } from "../lib/bridge";
import { useAppState, useBridge, useStore } from "../lib/context";
import {
  EXTENSION_PROVIDERS,
  ErrorLine,
  commandLine,
  isExpired,
  isStale,
  pendingPaths,
  targetKey,
  targetLabel,
  targetParam,
  useExtensionIntent,
  useExtensionSync,
} from "../lib/extensions";
import type {
  ExtensionTarget,
  PluginAction,
  PluginCapability,
  PluginListResult,
  PluginMarketplaceRecord,
  PluginMarketplaceSelection,
  PluginMutationResult,
  PluginObservation,
  PluginPlannedTarget,
  PluginPreview,
  PluginRecord,
  PluginTargetOutcome,
  PluginTargetState,
  PluginTargetStatus,
  ProviderAccountsSnapshot,
  RpcFailure,
} from "../lib/protocol";
import { Failure, useRead } from "../lib/useRead";
import { providerLabel, words } from "../lib/util";

type Call<T> = { ok: true; value: T } | { ok: false; error: RpcFailure };

async function attempt<T>(run: () => Promise<T>): Promise<Call<T>> {
  try {
    return { ok: true, value: await run() };
  } catch (error) {
    return { ok: false, error: asFailure(error) };
  }
}

const MUTATION_METHOD: Record<PluginAction, string> = {
  install: "plugin.install",
  uninstall: "plugin.uninstall",
  enable: "plugin.enable",
  disable: "plugin.disable",
  "marketplace-add": "plugin.marketplace.add",
  "marketplace-remove": "plugin.marketplace.remove",
};

const STATUS_TONE: Record<PluginTargetStatus, string> = {
  applied: "healthy",
  "rolled-back": "blocked",
  "recovery-needed": "failed",
  refused: "failed",
  "unsupported-or-unavailable": "dormant",
};

interface PluginRequest {
  action: PluginAction;
  targets: ExtensionTarget[];
  name: string;
  marketplace?: PluginMarketplaceSelection;
}

interface Change {
  title: string;
  description: string;
  confirm: string;
  danger: boolean;
  request: PluginRequest;
}

function capabilityOf(
  list: PluginListResult | null,
  provider: string,
): PluginCapability | null {
  return (
    list?.capabilities.find((entry) => entry.provider === provider) ?? null
  );
}

const can = (capability: PluginCapability | null, action: string) =>
  Boolean(capability?.actions.includes(action));

const ACTION_WORDS: [string, string][] = [
  ["install", "install"],
  ["uninstall", "uninstall"],
  ["enable", "turn on and off"],
  ["marketplace.add", "manage marketplaces"],
];

function allowedWords(capability: PluginCapability): string {
  const found = ACTION_WORDS.filter(([action]) => can(capability, action)).map(
    ([, text]) => text,
  );
  return found.length ? `Allowed here: ${found.join(", ")}.` : "Read only.";
}

function toneFor(compatibility: string): "success" | "warning" | "danger" {
  if (compatibility === "Native") return "success";
  if (compatibility === "Unsupported") return "danger";
  return "warning";
}

/** `name@marketplace` split for display, whichever way the harness reports it. */
function splitPlugin(plugin: PluginRecord): { base: string; market: string } {
  const at = plugin.name.lastIndexOf("@");
  if (at > 0)
    return {
      base: plugin.name.slice(0, at),
      market: plugin.name.slice(at + 1),
    };
  return { base: plugin.name, market: plugin.marketplace ?? "" };
}

function sourceText(source: unknown): string {
  if (typeof source === "string") return source;
  if (source && typeof source === "object") {
    const record = source as Record<string, unknown>;
    for (const key of ["repo", "url", "path", "source"])
      if (typeof record[key] === "string") return record[key] as string;
  }
  return "";
}

function marketplacesOf(
  observed: PluginObservation | null,
): PluginMarketplaceRecord[] {
  return Array.isArray(observed?.marketplaces) ? observed.marketplaces : [];
}

/** An npm module name: plain or scoped, with no `@marketplace` suffix. */
export function looksLikeNpm(name: string): boolean {
  return /^(@[\w.-]+\/)?[\w.-]+$/.test(name.trim());
}

function marketplaceName(name: string): string {
  const at = name.lastIndexOf("@");
  return at > 0 ? name.slice(at + 1) : "";
}

export function PluginsPage() {
  const store = useStore();
  const bridge = useBridge();
  const { extensionBusy } = useAppState();
  const read = useRead<PluginListResult>("plugin.list", {});
  useExtensionSync(read.reload);
  const accounts = useRead<ProviderAccountsSnapshot>("provider.accounts.list");
  const list = read.data;
  const [provider, setProvider] = useState(EXTENSION_PROVIDERS[0]!);
  const [profile, setProfile] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [installing, setInstalling] = useState(false);
  const [working, setWorking] = useState(false);

  useExtensionIntent("install-plugin", () => setInstalling(true));

  const profiles = useMemo(
    () =>
      new Map(
        (accounts.data?.accounts ?? []).map((account) => [
          account.spec.id,
          account.spec.label,
        ]),
      ),
    [accounts.data],
  );
  const capability = capabilityOf(list, provider);
  const targets = (list?.targets ?? []).filter(
    (entry) => entry.target.provider === provider,
  );
  const state: PluginTargetState | null =
    targets.find((entry) => (entry.target.accountId ?? null) === profile) ??
    targets.find((entry) => !entry.target.accountId) ??
    null;
  const target: ExtensionTarget = state?.target ?? { provider };
  const observed = state?.observed ?? null;
  const blocked = extensionBusy || working || Boolean(list?.pending);
  const paused = Boolean(list?.pending) && !extensionBusy;

  function countFor(entry: string) {
    const personal = (list?.targets ?? []).find(
      (item) => item.target.provider === entry && !item.target.accountId,
    );
    return personal?.observed?.plugins.length ?? 0;
  }

  async function toggle(plugin: PluginRecord) {
    if (!list) return;
    const action: PluginAction = plugin.enabled ? "disable" : "enable";
    setWorking(true);
    const preview = await attempt(() =>
      bridge.call<PluginPreview>("plugin.preview", {
        action,
        targets: [targetParam(target)],
        name: plugin.name,
      }),
    );
    if (!preview.ok) {
      setWorking(false);
      store.toast({
        tone: "error",
        title: preview.error.code,
        detail: preview.error.message,
      });
      return;
    }
    const planned = preview.value.targets[0];
    if (planned?.error) {
      setWorking(false);
      store.toast({
        tone: "error",
        title: planned.error.code,
        detail: planned.error.message,
      });
      return;
    }
    const applied = await attempt(() =>
      bridge.call<PluginMutationResult>(MUTATION_METHOD[action], {
        previewId: preview.value.previewId,
        expectedRevision: preview.value.revision,
      }),
    );
    setWorking(false);
    if (!applied.ok) {
      if (isStale(applied.error) || isExpired(applied.error)) read.reload();
      store.toast({
        tone: "error",
        title: applied.error.code,
        detail: applied.error.message,
      });
      return;
    }
    const result = applied.value.outcome.targets[0];
    const done = result?.status === "applied";
    store.toast({
      tone: done ? "info" : "error",
      title: done
        ? `${action === "enable" ? "Enabled" : "Disabled"} ${plugin.name}`
        : `${plugin.name} was not ${action === "enable" ? "enabled" : "disabled"}`,
      detail: done
        ? "New plugins load when a session restarts."
        : (result?.error?.message ?? words(result?.status ?? "refused")),
    });
    read.reload();
  }

  function ask(next: Change) {
    setChange(next);
  }

  const marketplaces = marketplacesOf(observed);
  const manageMarketplaces =
    can(capability, "marketplace.add") || can(capability, "marketplace.remove");
  const canToggle = can(capability, "enable") && can(capability, "disable");

  return (
    <>
      <div className="settings__title-row">
        <h2>Plugins</h2>
        <Button variant="primary" size="sm" onClick={() => setInstalling(true)}>
          Install plugin
        </Button>
      </div>
      <p className="settings__lede">
        Each harness manages its own plugins. Lazoo runs the harness&apos;s own
        commands and shows what it reports.
      </p>
      <Failure error={read.error} />

      {paused && list ? (
        <div className="ext-note" role="alert" data-tone="warning">
          <strong>Changes are paused.</strong>
          <span>
            An earlier change did not finish. Plugins cannot change until these
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

      <div className="harness-tabs" role="tablist" aria-label="Plugin harness">
        {EXTENSION_PROVIDERS.map((entry) => (
          <button
            key={entry}
            type="button"
            role="tab"
            className="harness-tab"
            aria-selected={entry === provider}
            onClick={() => {
              setProvider(entry);
              setProfile(null);
            }}
          >
            <span
              className="harness-tab__swatch"
              style={{ background: `var(--lz-provider-${entry})` }}
              aria-hidden="true"
            />
            {providerLabel(entry)}
            {countFor(entry) ? (
              <span className="harness-tab__count">{countFor(entry)}</span>
            ) : null}
          </button>
        ))}
      </div>

      {!list && !read.error ? (
        <p className="meta" role="status">
          Asking each harness what is installed. This can take a few seconds.
        </p>
      ) : null}

      {capability ? (
        <section className="ext-capability" aria-label="Capabilities">
          <div className="ext-capability__line">
            <Badge tone={toneFor(capability.compatibility)}>
              {capability.compatibility}
            </Badge>
            <span className="meta">{allowedWords(capability)}</span>
          </div>
          {capability.limitations.map((limit) => (
            <p className="meta" key={limit}>
              {limit}
            </p>
          ))}
          <p className="meta">New plugins load when a session restarts.</p>
        </section>
      ) : null}

      {targets.length > 1 ? (
        <div className="ext-target">
          <Field label="Profile" htmlFor="plugin-target">
            <select
              id="plugin-target"
              className="select"
              value={profile ?? ""}
              onChange={(event) => setProfile(event.target.value || null)}
            >
              <option value="">Personal</option>
              {targets
                .filter((entry) => entry.target.accountId)
                .map((entry) => (
                  <option
                    key={entry.target.accountId}
                    value={entry.target.accountId!}
                  >
                    {profiles.get(entry.target.accountId!) ??
                      `Profile ${entry.target.accountId!.slice(0, 8)}`}
                  </option>
                ))}
            </select>
          </Field>
        </div>
      ) : null}

      {state?.error ? (
        <section className="settings__group" aria-label="Harness status">
          {state.error.code === "LZ-EXTENSION-NOT-DETECTED" ? (
            <p className="meta">
              {providerLabel(provider)} was not detected on this computer.
            </p>
          ) : null}
          <ErrorLine error={state.error} />
        </section>
      ) : null}

      {observed ? (
        <section className="settings__group" aria-label="Installed">
          <h3>Installed</h3>
          {observed.plugins.length ? (
            <div
              className="ext-list"
              role="list"
              aria-label="Installed plugins"
            >
              {observed.plugins.map((plugin) => (
                <PluginRow
                  key={plugin.name}
                  plugin={plugin}
                  canToggle={canToggle}
                  canUninstall={can(capability, "uninstall")}
                  limitation={capability?.limitations[0]}
                  blocked={blocked}
                  onToggle={() => void toggle(plugin)}
                  onUninstall={() =>
                    ask({
                      title: `Uninstall ${plugin.name}`,
                      description:
                        "Lazoo will run these commands. Review them before you continue.",
                      confirm: "Uninstall",
                      danger: true,
                      request: {
                        action: "uninstall",
                        targets: [targetParam(target)],
                        name: plugin.name,
                      },
                    })
                  }
                />
              ))}
            </div>
          ) : (
            <p className="meta">
              No plugins installed in {providerLabel(provider)}.
            </p>
          )}
        </section>
      ) : null}

      {observed && manageMarketplaces ? (
        <section className="settings__group" aria-label="Marketplaces">
          <h3>Marketplaces</h3>
          {marketplaces.length ? (
            <div className="ext-list" role="list" aria-label="Marketplaces">
              {marketplaces.map((market) => (
                <div
                  className="ext-row ext-row--plain"
                  role="listitem"
                  key={market.name}
                >
                  <div className="ext-row__main">
                    <span className="ext-row__name mono">{market.name}</span>
                    <span className="meta mono">
                      {sourceText(market.source)}
                    </span>
                  </div>
                  <span className="ext-row__menu">
                    <Menu
                      label={`More for ${market.name}`}
                      items={
                        can(capability, "marketplace.remove")
                          ? [
                              {
                                label: "Remove",
                                danger: true,
                                disabled: blocked,
                                onSelect: () =>
                                  ask({
                                    title: `Remove marketplace ${market.name}`,
                                    description:
                                      "Lazoo will run these commands. Review them before you continue.",
                                    confirm: "Remove marketplace",
                                    danger: true,
                                    request: {
                                      action: "marketplace-remove",
                                      targets: [targetParam(target)],
                                      name: market.name,
                                    },
                                  }),
                              },
                            ]
                          : []
                      }
                    >
                      <Ellipsis size={14} aria-hidden="true" />
                    </Menu>
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <p className="meta">No marketplaces registered.</p>
          )}
          {can(capability, "marketplace.add") ? (
            <AddMarketplace
              disabled={blocked}
              onSubmit={(name, source) =>
                ask({
                  title: `Add marketplace ${name}`,
                  description:
                    "Lazoo will run these commands. Review them before you continue.",
                  confirm: "Add marketplace",
                  danger: false,
                  request: {
                    action: "marketplace-add",
                    targets: [targetParam(target)],
                    name,
                    marketplace: { name, source },
                  },
                })
              }
            />
          ) : null}
        </section>
      ) : null}

      <ChangeDialog
        change={change}
        profiles={profiles}
        blocked={blocked}
        onClose={() => setChange(null)}
        onReload={read.reload}
      />
      <InstallDialog
        open={installing}
        list={list}
        profiles={profiles}
        initial={target}
        blocked={blocked}
        onClose={() => setInstalling(false)}
        onReload={read.reload}
      />
    </>
  );
}

function PluginRow({
  plugin,
  canToggle,
  canUninstall,
  limitation,
  blocked,
  onToggle,
  onUninstall,
}: {
  plugin: PluginRecord;
  canToggle: boolean;
  canUninstall: boolean;
  limitation: string | undefined;
  blocked: boolean;
  onToggle: () => void;
  onUninstall: () => void;
}) {
  const { base, market } = splitPlugin(plugin);
  const state =
    plugin.enabled === true
      ? "Enabled"
      : plugin.enabled === false
        ? "Disabled"
        : "State not reported";
  return (
    <div className="ext-row ext-row--plugin" role="listitem">
      <div className="ext-row__main">
        <span className="ext-row__name mono">
          {base}
          {market ? <span className="ext-row__market">@{market}</span> : null}
        </span>
        {plugin.version ? (
          <span className="meta mono">{plugin.version}</span>
        ) : null}
      </div>
      <span className="ext-row__state">
        {canToggle && typeof plugin.enabled === "boolean" ? (
          <Switch
            checked={plugin.enabled}
            label={`${plugin.enabled ? "Disable" : "Enable"} ${plugin.name}`}
            disabled={blocked}
            onChange={onToggle}
          />
        ) : (
          <span className="meta" title={limitation}>
            {state}
          </span>
        )}
      </span>
      <span className="ext-row__menu">
        <Menu
          label={`More for ${plugin.name}`}
          items={
            canUninstall
              ? [
                  {
                    label: "Uninstall",
                    danger: true,
                    disabled: blocked,
                    onSelect: onUninstall,
                  },
                ]
              : []
          }
        >
          <Ellipsis size={14} aria-hidden="true" />
        </Menu>
      </span>
    </div>
  );
}

function AddMarketplace({
  disabled,
  onSubmit,
}: {
  disabled: boolean;
  onSubmit: (name: string, source: string) => void;
}) {
  const [name, setName] = useState("");
  const [source, setSource] = useState("");
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || !source.trim()) return;
    onSubmit(name.trim(), source.trim());
    setName("");
    setSource("");
  }
  return (
    <form className="ext-inline-form" onSubmit={submit}>
      <Field label="Marketplace name" htmlFor="marketplace-name">
        <input
          id="marketplace-name"
          className="input mono-input"
          spellCheck={false}
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </Field>
      <Field label="Marketplace source" htmlFor="marketplace-source">
        <input
          id="marketplace-source"
          className="input mono-input"
          spellCheck={false}
          placeholder="owner/repo"
          value={source}
          onChange={(event) => setSource(event.target.value)}
        />
      </Field>
      <Button
        type="submit"
        size="sm"
        disabled={disabled || !name.trim() || !source.trim()}
      >
        Add marketplace
      </Button>
    </form>
  );
}

// ---- plans and outcomes -------------------------------------------------------

function beforeSummary(before: PluginObservation | null): string {
  if (!before) return "";
  const plugins = Array.isArray(before.plugins) ? before.plugins.length : 0;
  const markets = marketplacesOf(before).length;
  return `${plugins} plugin${plugins === 1 ? "" : "s"} · ${markets} marketplace${markets === 1 ? "" : "s"} · v${before.providerVersion}`;
}

function PlanList({
  targets,
  profiles,
}: {
  targets: PluginPlannedTarget[];
  profiles: ReadonlyMap<string, string>;
}) {
  return (
    <ul className="ext-plan" aria-label="Planned changes">
      {targets.map((planned) => {
        const label = targetLabel(planned.target, profiles);
        return (
          <li key={targetKey(planned.target)}>
            <div className="ext-plan__head">
              <strong>{label}</strong>
              <span className="meta">{beforeSummary(planned.before)}</span>
            </div>
            <ErrorLine error={planned.error} />
            {planned.commands.length ? (
              <div role="group" aria-label={`Commands for ${label}`}>
                <pre className="ext-commands">
                  {planned.commands.map(commandLine).join("\n")}
                </pre>
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

function OutcomeList({
  outcomes,
  profiles,
}: {
  outcomes: PluginTargetOutcome[];
  profiles: ReadonlyMap<string, string>;
}) {
  const applied = outcomes.filter((entry) => entry.status === "applied").length;
  return (
    <div className="stack">
      {applied === outcomes.length ? (
        <p role="status">
          Applied on {applied} target{applied === 1 ? "" : "s"}. New plugins
          load when a session restarts.
        </p>
      ) : (
        <p className="ext-error" role="alert">
          Not every target succeeded: {applied} of {outcomes.length} applied.
          Check each result below.
        </p>
      )}
      <ul className="ext-plan" aria-label="Results">
        {outcomes.map((entry) => (
          <li key={targetKey(entry.target)}>
            <div className="ext-plan__head">
              <strong>{targetLabel(entry.target, profiles)}</strong>
              <StatusWord
                tone={STATUS_TONE[entry.status] ?? "unknown"}
                label={words(entry.status)}
              />
            </div>
            <ErrorLine error={entry.error} />
            {entry.status === "recovery-needed" && entry.backupDirectory ? (
              <p className="meta">
                Changes are paused until this folder is checked:{" "}
                <code>{entry.backupDirectory}</code>
              </p>
            ) : null}
            {entry.status === "rolled-back" ? (
              <p className="meta">
                Rolled back: {words(entry.rollbackStatus)}.
              </p>
            ) : null}
            {(entry.limitations ?? []).map((limit) => (
              <p className="meta" key={limit}>
                {limit}
              </p>
            ))}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Preview, show the exact commands, then apply. Used where a change is destructive. */
function ChangeDialog({
  change,
  profiles,
  blocked,
  onClose,
  onReload,
}: {
  change: Change | null;
  profiles: ReadonlyMap<string, string>;
  blocked: boolean;
  onClose: () => void;
  onReload: () => void;
}) {
  const bridge = useBridge();
  const [preview, setPreview] = useState<PluginPreview | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [result, setResult] = useState<PluginMutationResult | null>(null);
  const [applying, setApplying] = useState(false);
  const [round, setRound] = useState(0);

  useEffect(() => {
    if (!change) return;
    let cancelled = false;
    setPreview(null);
    setError(null);
    setResult(null);
    void attempt(() =>
      bridge.call<PluginPreview>("plugin.preview", change.request),
    ).then((outcome) => {
      if (cancelled) return;
      if (outcome.ok) setPreview(outcome.value);
      else setError(outcome.error);
    });
    return () => {
      cancelled = true;
    };
  }, [bridge, change, round]);

  const usable = preview?.targets.some((planned) => !planned.error) ?? false;

  async function apply() {
    if (!change || !preview) return;
    setApplying(true);
    const outcome = await attempt(() =>
      bridge.call<PluginMutationResult>(
        MUTATION_METHOD[change.request.action],
        {
          previewId: preview.previewId,
          expectedRevision: preview.revision,
        },
      ),
    );
    setApplying(false);
    onReload();
    if (outcome.ok) {
      setResult(outcome.value);
    } else {
      setPreview(null);
      setError(outcome.error);
    }
  }

  const footer = result ? (
    <>
      <span className="spacer" />
      <Button variant="primary" onClick={onClose}>
        Done
      </Button>
    </>
  ) : (
    <>
      <span className="spacer" />
      <Button variant="ghost" onClick={onClose}>
        Cancel
      </Button>
      {error && !preview ? (
        <Button onClick={() => setRound((value) => value + 1)}>
          Preview again
        </Button>
      ) : (
        <Button
          variant={change?.danger ? "danger" : "primary"}
          loading={applying}
          disabled={!usable || blocked}
          onClick={() => void apply()}
        >
          {change?.confirm}
        </Button>
      )}
    </>
  );

  return (
    <Dialog
      open={Boolean(change)}
      onClose={onClose}
      title={change?.title ?? "Change plugins"}
      description={result ? undefined : change?.description}
      width={580}
      footer={footer}
    >
      {result ? (
        <OutcomeList outcomes={result.outcome.targets} profiles={profiles} />
      ) : (
        <div className="stack">
          {!preview && !error ? (
            <p className="meta" role="status">
              Asking the harness what would change.
            </p>
          ) : null}
          {error ? <ErrorNote error={error} /> : null}
          {preview ? (
            <PlanList targets={preview.targets} profiles={profiles} />
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

function ErrorNote({ error }: { error: RpcFailure }) {
  return (
    <>
      <ErrorLine error={error} />
      {isStale(error) ||
      isExpired(error) ||
      error.code === "LZ-INVALID-PARAMS" ? (
        <p className="meta">
          Plugins changed since this preview. Preview again.
        </p>
      ) : null}
    </>
  );
}

// ---- install ------------------------------------------------------------------

function InstallDialog({
  open,
  list,
  profiles,
  initial,
  blocked,
  onClose,
  onReload,
}: {
  open: boolean;
  list: PluginListResult | null;
  profiles: ReadonlyMap<string, string>;
  initial: ExtensionTarget;
  blocked: boolean;
  onClose: () => void;
  onReload: () => void;
}) {
  const bridge = useBridge();
  const [name, setName] = useState("");
  const [source, setSource] = useState("");
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState<PluginPreview | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [installing, setInstalling] = useState(false);
  const [result, setResult] = useState<PluginMutationResult | null>(null);

  useEffect(() => {
    if (!open) return;
    setName("");
    setSource("");
    setChosen(new Set([targetKey(initial)]));
    setPreview(null);
    setError(null);
    setResult(null);
    // The dialog resets when it opens, not when the tab behind it changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const npm = looksLikeNpm(name);
  const options = (list?.targets ?? []).filter(
    (entry) =>
      EXTENSION_PROVIDERS.includes(entry.target.provider) &&
      can(capabilityOf(list, entry.target.provider), "install") &&
      (entry.target.provider !== "opencode" || npm),
  );
  const selected = options.filter((entry) =>
    chosen.has(targetKey(entry.target)),
  );
  const market = marketplaceName(name.trim());
  const sourceNeedsName = Boolean(source.trim()) && !market;

  async function showPlan(event: FormEvent) {
    event.preventDefault();
    if (!name.trim() || !selected.length || sourceNeedsName) return;
    setPreviewing(true);
    setError(null);
    const outcome = await attempt(() =>
      bridge.call<PluginPreview>("plugin.preview", {
        action: "install",
        targets: selected.map((entry) => targetParam(entry.target)),
        name: name.trim(),
        ...(source.trim()
          ? { marketplace: { name: market, source: source.trim() } }
          : {}),
      }),
    );
    setPreviewing(false);
    if (outcome.ok) setPreview(outcome.value);
    else setError(outcome.error);
  }

  async function install() {
    if (!preview) return;
    setInstalling(true);
    const outcome = await attempt(() =>
      bridge.call<PluginMutationResult>("plugin.install", {
        previewId: preview.previewId,
        expectedRevision: preview.revision,
      }),
    );
    setInstalling(false);
    onReload();
    if (outcome.ok) {
      setResult(outcome.value);
    } else {
      setPreview(null);
      setError(outcome.error);
    }
  }

  const ready =
    preview?.targets.filter((planned) => !planned.error).length ?? 0;
  const footer = result ? (
    <>
      <span className="spacer" />
      <Button variant="primary" onClick={onClose}>
        Done
      </Button>
    </>
  ) : preview ? (
    <>
      <span className="spacer" />
      <Button variant="ghost" onClick={() => setPreview(null)}>
        Back
      </Button>
      <Button
        variant="primary"
        loading={installing}
        disabled={!ready || blocked}
        onClick={() => void install()}
      >
        {`Install on ${ready} target${ready === 1 ? "" : "s"}`}
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
        form="install-plugin-form"
        loading={previewing}
        disabled={!name.trim() || !selected.length || sourceNeedsName}
      >
        Preview
      </Button>
    </>
  );

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Install plugin"
      description={
        result
          ? undefined
          : preview
            ? "Lazoo will run these commands in each harness."
            : "Choose a plugin and the harnesses to install it in. Nothing changes until you confirm."
      }
      width={600}
      footer={footer}
    >
      {result ? (
        <OutcomeList outcomes={result.outcome.targets} profiles={profiles} />
      ) : preview ? (
        <div className="stack">
          <ErrorLine error={error} />
          <PlanList targets={preview.targets} profiles={profiles} />
        </div>
      ) : (
        <form
          id="install-plugin-form"
          className="stack"
          onSubmit={(event) => void showPlan(event)}
        >
          <Field
            label="Plugin"
            htmlFor="plugin-name"
            hint="plugin@marketplace, or an npm module for OpenCode."
          >
            <input
              id="plugin-name"
              className="input mono-input"
              data-autofocus
              spellCheck={false}
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field
            label="Marketplace source"
            htmlFor="plugin-market-source"
            hint={
              sourceNeedsName
                ? "Name the plugin as plugin@marketplace so Lazoo knows what to call this marketplace."
                : "Optional. For example owner/repo. Adds the marketplace first when it is missing."
            }
          >
            <input
              id="plugin-market-source"
              className="input mono-input"
              spellCheck={false}
              placeholder="owner/repo"
              value={source}
              onChange={(event) => setSource(event.target.value)}
            />
          </Field>
          <fieldset className="ext-fieldset">
            <legend>Install in</legend>
            {options.length ? (
              options.map((entry) => {
                const key = targetKey(entry.target);
                return (
                  <label className="check-row" key={key}>
                    <input
                      type="checkbox"
                      checked={chosen.has(key)}
                      onChange={(event) =>
                        setChosen((current) => {
                          const next = new Set(current);
                          if (event.target.checked) next.add(key);
                          else next.delete(key);
                          return next;
                        })
                      }
                    />
                    <span>
                      <strong>{targetLabel(entry.target, profiles)}</strong>
                    </span>
                  </label>
                );
              })
            ) : (
              <p className="meta">No harness can install plugins yet.</p>
            )}
            {!npm && list ? (
              <p className="meta">
                OpenCode appears when the name is an npm module.
              </p>
            ) : null}
          </fieldset>
          <ErrorLine error={error} />
        </form>
      )}
    </Dialog>
  );
}
