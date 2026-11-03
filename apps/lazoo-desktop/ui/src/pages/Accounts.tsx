import { ArrowDown, ArrowUp, Ellipsis } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from "react";

import {
  Button,
  Dialog,
  Field,
  Menu,
  Segmented,
} from "../components/primitives";
import { useAppState, useStore, useTerminals } from "../lib/context";
import type {
  AccountUsageBucket,
  AccountUsageResult,
  ProviderAccountPolicy,
  ProviderAccountRecord,
  ProviderAccountSpec,
  ProviderAccountsSnapshot,
  TerminalRecord,
  UsageWindow,
  Uuid,
} from "../lib/protocol";
import { Failure, useRead } from "../lib/useRead";
import { providerLabel, untilTime } from "../lib/util";

export const ACCOUNT_PROVIDERS = [
  "codex",
  "claude-code",
  "github-copilot",
  "opencode",
  "antigravity",
];
const POOL_PROVIDERS = new Set(["codex", "claude-code", "opencode"]);
const RESERVES = [null, 10, 20, 30, 50, 75];
const uuid = () => crypto.randomUUID();

const MODE_HELP: Record<ProviderAccountPolicy["mode"], string> = {
  inherited:
    "New sessions use the harness's own login. Lazoo does not choose an account.",
  balanced: "New sessions go to the least-used eligible account.",
  priority:
    "New sessions use the first eligible account in order. The next one is used only when it is exhausted, cooling down or past its reserve.",
  pinned: "Every new session uses one account.",
  "api-pool":
    "Sessions route API requests through Lazoo's local proxy across eligible keys.",
};

export function windowLabel(window: UsageWindow): string {
  const minutes = window.windowMinutes;
  if (window.kind === "five-hour" || minutes === 300) return "5h";
  if (window.kind === "weekly" || minutes === 10080) return "Week";
  if (window.kind === "requests") return "Requests";
  if (window.kind === "tokens") return "Tokens";
  if (minutes && minutes % 1440 === 0) return `${minutes / 1440}d`;
  if (minutes && minutes % 60 === 0) return `${minutes / 60}h`;
  return window.kind;
}

export function windowPercent(window: UsageWindow): number | null {
  if (window.usedPercent !== null) return window.usedPercent;
  if (window.limit && window.remaining !== null)
    return Math.max(0, 100 - (window.remaining / window.limit) * 100);
  return null;
}

function policyFor(
  snapshot: ProviderAccountsSnapshot | null,
  provider: string,
): ProviderAccountPolicy {
  return (
    snapshot?.policies.find((policy) => policy.provider === provider) ?? {
      provider,
      mode: "inherited",
      accountId: null,
      revision: 0,
      order: [],
    }
  );
}

/** Accounts in routing order: the policy order first, then by creation. */
export function orderedAccounts(
  accounts: ProviderAccountRecord[],
  policy: ProviderAccountPolicy,
): ProviderAccountRecord[] {
  const rank = new Map((policy.order ?? []).map((id, index) => [id, index]));
  return [...accounts].sort((a, b) => {
    const ra = rank.get(a.spec.id) ?? Number.MAX_SAFE_INTEGER;
    const rb = rank.get(b.spec.id) ?? Number.MAX_SAFE_INTEGER;
    if (ra !== rb) return ra - rb;
    return (a.createdAt ?? a.updatedAt).localeCompare(
      b.createdAt ?? b.updatedAt,
    );
  });
}

function accountState(account: ProviderAccountRecord): {
  word: string;
  tone: string;
} {
  if (!account.spec.enabled) return { word: "Disabled", tone: "dormant" };
  if (account.exhaustedUntil && Date.parse(account.exhaustedUntil) > Date.now())
    return {
      word: `Exhausted until ${untilTime(account.exhaustedUntil)}`,
      tone: "failed",
    };
  if (account.cooldownUntil * 1000 > Date.now())
    return { word: "Cooling down", tone: "blocked" };
  if (account.spec.credentialKind === "api-key" && !account.hasSecret)
    return { word: "Key needed", tone: "blocked" };
  return { word: "Ready", tone: "healthy" };
}

export function AccountsPage() {
  const store = useStore();
  const accounts = useRead<ProviderAccountsSnapshot>("provider.accounts.list");
  const snapshot = accounts.data;
  const visible = useMemo(
    () => (snapshot?.accounts ?? []).filter((account) => !account.deleted),
    [snapshot],
  );
  const counts = (provider: string) =>
    visible.filter((account) => account.spec.provider === provider).length;
  const [provider, setProvider] = useState<string | null>(null);
  const current =
    provider ??
    ACCOUNT_PROVIDERS.find((entry) => counts(entry) > 0) ??
    ACCOUNT_PROVIDERS[0]!;
  const [adding, setAdding] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [login, setLogin] = useState<TerminalRecord | null>(null);

  const policy = policyFor(snapshot, current);
  const members = orderedAccounts(
    visible.filter((account) => account.spec.provider === current),
    policy,
  );

  async function setPolicy(
    mode: ProviderAccountPolicy["mode"],
    accountId: string | null,
    order: Uuid[] = policy.order ?? [],
  ) {
    const eligible = members.filter((account) => account.spec.enabled);
    const pinned =
      mode === "pinned" ? (accountId ?? eligible[0]?.spec.id ?? null) : null;
    if (mode === "pinned" && !pinned) {
      store.toast({
        tone: "error",
        title: "No enabled account",
        detail: "Add or enable an account before pinning new sessions.",
      });
      return;
    }
    const result = await store.mutate(
      "provider.accounts.policy",
      {
        provider: current,
        mode,
        accountId: pinned,
        order,
        revision: policy.revision,
      },
      `account.policy:${current}:${policy.revision}`,
    );
    if (result) accounts.reload();
  }

  function move(account: ProviderAccountRecord, offset: number) {
    const ids = members.map((member) => member.spec.id);
    const from = ids.indexOf(account.spec.id);
    const to = from + offset;
    if (to < 0 || to >= ids.length) return;
    [ids[from], ids[to]] = [ids[to]!, ids[from]!];
    void setPolicy(policy.mode, policy.accountId, ids);
  }

  const ordered = policy.mode === "priority";

  return (
    <>
      <div className="settings__title-row">
        <h2>Accounts</h2>
        <Button variant="primary" size="sm" onClick={() => setAdding(true)}>
          New account
        </Button>
      </div>
      <p className="settings__lede">
        Which account pays for each agent, and how close it is to its limit.
        Secrets are protected by Windows for this user and never shown again.
      </p>
      <Failure error={accounts.error} />

      <div
        className="harness-tabs"
        role="tablist"
        aria-label="Routing provider"
      >
        {ACCOUNT_PROVIDERS.map((entry) => (
          <button
            key={entry}
            type="button"
            role="tab"
            className="harness-tab"
            aria-selected={entry === current}
            onClick={() => setProvider(entry)}
          >
            <span
              className="harness-tab__swatch"
              style={{ background: `var(--lz-provider-${entry})` }}
              aria-hidden="true"
            />
            {providerLabel(entry)}
            {counts(entry) ? (
              <span className="harness-tab__count">{counts(entry)}</span>
            ) : null}
          </button>
        ))}
      </div>

      <section className="settings__group" aria-label="Routing">
        <div className="routing">
          <Field label="New sessions" htmlFor="routing-mode">
            <select
              id="routing-mode"
              className="select"
              value={policy.mode}
              onChange={(event) =>
                void setPolicy(
                  event.target.value as ProviderAccountPolicy["mode"],
                  event.target.value === "pinned"
                    ? (policy.accountId ?? null)
                    : null,
                )
              }
            >
              <option value="inherited">
                Use the harness&apos;s own login
              </option>
              <option value="priority" disabled={!members.length}>
                Use in order
              </option>
              <option value="balanced" disabled={!members.length}>
                Balance accounts
              </option>
              <option value="pinned" disabled={!members.length}>
                Pin one account
              </option>
              {POOL_PROVIDERS.has(current) ? (
                <option value="api-pool">Pool API keys</option>
              ) : null}
            </select>
          </Field>
          {policy.mode === "pinned" ? (
            <Field label="Pinned account" htmlFor="routing-account">
              <select
                id="routing-account"
                className="select"
                value={policy.accountId ?? ""}
                onChange={(event) =>
                  void setPolicy("pinned", event.target.value || null)
                }
              >
                {members
                  .filter((account) => account.spec.enabled)
                  .map((account) => (
                    <option key={account.spec.id} value={account.spec.id}>
                      {account.spec.label}
                    </option>
                  ))}
              </select>
            </Field>
          ) : null}
          <span className="spacer" />
          <Button size="sm" onClick={() => setVerifying(true)}>
            Verify routing
          </Button>
        </div>
        <p className="meta">
          {MODE_HELP[policy.mode]} Changes apply to new sessions; running and
          resumed sessions keep their account.
        </p>
      </section>

      <section className="settings__group" aria-label="Accounts list">
        <h3>
          {providerLabel(current)} accounts
          {ordered ? <span className="meta"> · in routing order</span> : null}
        </h3>
        {members.length ? (
          <div
            className="accounts"
            role="list"
            aria-label={`${providerLabel(current)} accounts`}
          >
            {members.map((account, index) => (
              <AccountRow
                key={account.spec.id}
                account={account}
                position={ordered ? index + 1 : null}
                first={index === 0}
                last={index === members.length - 1}
                onMove={(offset) => move(account, offset)}
                onLogin={setLogin}
                reload={accounts.reload}
              />
            ))}
          </div>
        ) : snapshot ? (
          <p className="meta">
            No {providerLabel(current)} accounts. Sessions use the
            harness&apos;s own login until you add one.
          </p>
        ) : null}
        <PersonalWindows provider={current} />
      </section>

      <UsageSection provider={current} accounts={members} />

      {login ? (
        <LoginTerminal terminal={login} onClose={() => setLogin(null)} />
      ) : null}
      <AddAccountDialog
        open={adding}
        provider={current}
        onClose={() => setAdding(false)}
        snapshot={snapshot}
        reload={accounts.reload}
      />
      <VerifyDialog
        open={verifying}
        provider={current}
        onClose={() => setVerifying(false)}
        reload={accounts.reload}
      />
    </>
  );
}

function Windows({ windows }: { windows: UsageWindow[] }) {
  if (!windows.length) return <span className="meta">not observed</span>;
  return (
    <span className="windows">
      {windows.slice(0, 2).map((window) => (
        <WindowMeter key={`${window.kind}:${window.source}`} window={window} />
      ))}
    </span>
  );
}

function WindowMeter({
  window,
  reserve = null,
}: {
  window: UsageWindow;
  reserve?: number | null;
}) {
  const percent = windowPercent(window);
  const level =
    percent === null
      ? "low"
      : percent >= 95
        ? "full"
        : percent >= 75
          ? "high"
          : "low";
  return (
    <span
      className="window"
      title={`${windowLabel(window)} window · ${window.source}${
        window.resetsAt ? ` · resets ${untilTime(window.resetsAt)}` : ""
      }`}
    >
      <span className="window__label">{windowLabel(window)}</span>
      <span className="meter" aria-hidden="true">
        <span
          className="meter__fill"
          data-level={level}
          style={{ width: `${Math.min(100, percent ?? 0)}%` }}
        />
        {reserve ? (
          <span className="meter__reserve" style={{ width: `${reserve}%` }} />
        ) : null}
      </span>
      <span className="window__value">
        {percent === null ? "—" : `${Math.round(percent)}%`}
      </span>
    </span>
  );
}

function AccountRow({
  account,
  position,
  first,
  last,
  onMove,
  onLogin,
  reload,
}: {
  account: ProviderAccountRecord;
  position: number | null;
  first: boolean;
  last: boolean;
  onMove: (offset: number) => void;
  onLogin: (terminal: TerminalRecord) => void;
  reload: () => void;
}) {
  const store = useStore();
  useAppState();
  const [rotating, setRotating] = useState(false);
  const [secret, setSecret] = useState("");
  const state = accountState(account);
  const label = account.spec.label;

  async function mutate(
    method: string,
    params: Record<string, unknown>,
    key: string,
  ) {
    const result = await store.mutate(method, params, key);
    if (result) reload();
    return result;
  }
  async function signIn() {
    const terminal = (await store.mutate(
      "provider.accounts.login",
      { id: account.spec.id, expectedRevision: account.revision },
      `account.login:${account.spec.id}:${account.revision}`,
    )) as TerminalRecord | null;
    if (terminal) onLogin(terminal);
  }
  async function rotate() {
    if (!secret) return;
    const result = await mutate(
      "provider.accounts.put",
      { spec: account.spec, expectedRevision: account.revision, secret },
      `account.rotate:${account.spec.id}:${account.revision}`,
    );
    setSecret("");
    if (result) setRotating(false);
  }

  const native = account.spec.credentialKind === "native-profile";
  return (
    <div
      className="account"
      role="listitem"
      data-enabled={account.spec.enabled}
    >
      <span className="account__order">
        {position !== null ? (
          <>
            <span className="account__rank">{position}</span>
            <Button
              variant="ghost"
              size="sm"
              icon
              aria-label={`Move ${label} up`}
              disabled={first}
              onClick={() => onMove(-1)}
            >
              <ArrowUp size={12} aria-hidden="true" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon
              aria-label={`Move ${label} down`}
              disabled={last}
              onClick={() => onMove(1)}
            >
              <ArrowDown size={12} aria-hidden="true" />
            </Button>
          </>
        ) : null}
      </span>
      <span className="account__who">
        <span className="account__label">{label}</span>
        <span className="account__kind">
          {native ? "native profile" : "API key"}
          {account.spec.models.length
            ? ` · ${account.spec.models.join(", ")}`
            : ""}
        </span>
      </span>
      <span className="account__state" data-tone={state.tone}>
        {state.word}
      </span>
      <span className="account__windows">
        {account.windows?.length ? (
          account.windows
            .slice(0, 2)
            .map((window) => (
              <WindowMeter
                key={`${window.kind}:${window.source}`}
                window={window}
                reserve={account.reservePercent}
              />
            ))
        ) : (
          <span className="meta">usage not observed</span>
        )}
      </span>
      <span className="account__reserve">
        <select
          className="select select--bare"
          aria-label={`Reserve for ${label}`}
          title={
            account.reserveEnforceable
              ? "Keep this share of the account's window for yourself"
              : (account.reserveReason ??
                "This harness does not report usage windows, so a reserve cannot be enforced.")
          }
          disabled={!account.reserveEnforceable}
          value={account.reservePercent ?? ""}
          onChange={(event) =>
            void mutate(
              "provider.accounts.preferences",
              {
                id: account.spec.id,
                expectedRevision: account.revision,
                reservePercent: event.target.value
                  ? Number(event.target.value)
                  : null,
              },
              `account.reserve:${account.spec.id}:${account.revision}`,
            )
          }
        >
          {RESERVES.map((value) => (
            <option key={value ?? "none"} value={value ?? ""}>
              {value === null ? "No reserve" : `Keep ${value}%`}
            </option>
          ))}
        </select>
      </span>
      <span className="account__actions">
        {native && account.spec.enabled ? (
          <Button size="sm" variant="ghost" onClick={() => void signIn()}>
            Sign in
          </Button>
        ) : null}
        {!native ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setRotating(true);
              setSecret("");
            }}
          >
            Rotate key
          </Button>
        ) : null}
        <Menu
          label={`More for ${label}`}
          items={[
            {
              label: account.spec.enabled ? "Disable" : "Enable",
              onSelect: () =>
                void mutate(
                  "provider.accounts.put",
                  {
                    spec: { ...account.spec, enabled: !account.spec.enabled },
                    expectedRevision: account.revision,
                  },
                  `account.toggle:${account.spec.id}:${account.revision}`,
                ),
            },
            {
              label: "Remove",
              danger: true,
              onSelect: () =>
                void mutate(
                  "provider.accounts.remove",
                  { id: account.spec.id, expectedRevision: account.revision },
                  `account.remove:${account.spec.id}:${account.revision}`,
                ),
            },
          ]}
        >
          <Ellipsis size={14} aria-hidden="true" />
        </Menu>
      </span>
      {rotating ? (
        <form
          className="account__rotate"
          onSubmit={(event) => {
            event.preventDefault();
            void rotate();
          }}
        >
          <input
            className="input input--sm"
            aria-label={`New secret for ${label}`}
            type="password"
            autoComplete="new-password"
            placeholder="New secret"
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
          />
          <Button size="sm" type="submit" disabled={!secret}>
            Save new key
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setRotating(false);
              setSecret("");
            }}
          >
            Cancel
          </Button>
        </form>
      ) : null}
    </div>
  );
}

/** The unmanaged personal login's windows, when the harness reports them. */
function PersonalWindows({ provider }: { provider: string }) {
  const usage = useRead<AccountUsageResult>(
    "provider.accounts.usage",
    { provider, accountId: null, bucket: "day" },
    [provider],
  );
  const windows = (usage.data?.windows ?? []).filter(
    (window) => window.accountId === null && window.provider === provider,
  );
  if (!windows.length) return null;
  return (
    <div className="account account--personal" role="note">
      <span className="account__order" />
      <span className="account__who">
        <span className="account__label">Personal login</span>
        <span className="account__kind">not managed by Lazoo</span>
      </span>
      <span className="account__state" />
      <span className="account__windows">
        <Windows windows={windows} />
      </span>
      <span className="account__reserve" />
      <span className="account__actions" />
    </div>
  );
}

type Bucket = "day" | "hour";

function UsageSection({
  provider,
  accounts,
}: {
  provider: string;
  accounts: ProviderAccountRecord[];
}) {
  const [bucket, setBucket] = useState<Bucket>("day");
  const usage = useRead<AccountUsageResult>(
    "provider.accounts.usage",
    bucket === "day"
      ? { provider, bucket }
      : {
          provider,
          bucket,
          from: new Date(Date.now() - 24 * 3600 * 1000).toISOString(),
        },
    [provider, bucket],
  );
  const observed = usage.data?.observableProviders?.[provider] ?? null;
  const rows = useMemo(() => {
    const byAccount = new Map<string, AccountUsageBucket[]>();
    for (const row of usage.data?.buckets ?? []) {
      if (row.provider !== provider) continue;
      const key = row.accountId ?? "personal";
      byAccount.set(key, [...(byAccount.get(key) ?? []), row]);
    }
    return [...byAccount.entries()].map(([key, buckets]) => ({
      key,
      label:
        key === "personal"
          ? "Personal login"
          : (accounts.find((account) => account.spec.id === key)?.spec.label ??
            "Removed account"),
      buckets: buckets.sort((a, b) => a.start.localeCompare(b.start)),
    }));
  }, [usage.data, provider, accounts]);
  const span = bucket === "day" ? 7 : 24;

  return (
    <section className="settings__group" aria-label="Usage">
      <div className="settings__title-row">
        <h3>
          Usage{" "}
          <span className="meta">
            · {bucket === "day" ? "last 7 days" : "last 24 hours"}
          </span>
        </h3>
        <Segmented
          name="usage-bucket"
          label="Usage range"
          value={bucket}
          options={[
            { value: "day", label: "7 days" },
            { value: "hour", label: "24 hours" },
          ]}
          onChange={setBucket}
        />
      </div>
      <Failure error={usage.error} />
      {rows.length ? (
        <div className="table-wrap">
          <table className="table usage">
            <thead>
              <tr>
                <th>Account</th>
                <th>Activity</th>
                <th>Sessions</th>
                <th>Turns</th>
                <th>Failed</th>
                <th>Requests</th>
                <th>Tokens in / out</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => {
                const sum = (pick: (bucket: AccountUsageBucket) => number) =>
                  row.buckets.reduce((total, entry) => total + pick(entry), 0);
                const reported = sum((entry) => entry.tokensReported);
                return (
                  <tr key={row.key}>
                    <td>{row.label}</td>
                    <td>
                      <Spark
                        buckets={row.buckets}
                        span={span}
                        bucket={bucket}
                      />
                    </td>
                    <td className="mono">{sum((entry) => entry.sessions)}</td>
                    <td className="mono">
                      {sum((entry) => entry.turnsCompleted)}
                    </td>
                    <td className="mono">
                      {sum((entry) => entry.turnsFailed)}
                    </td>
                    <td className="mono">{sum((entry) => entry.requests)}</td>
                    <td className="mono">
                      {reported
                        ? `${compact(sum((entry) => entry.inputTokens ?? 0))} / ${compact(sum((entry) => entry.outputTokens ?? 0))}`
                        : "not reported"}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : usage.data ? (
        <p className="meta">
          No {providerLabel(provider)} sessions in this range.
        </p>
      ) : null}
      {observed ? (
        <p className="meta">
          {observed.windows
            ? "Quota windows are reported by the harness."
            : `Quota windows are not observable${observed.reason ? `: ${observed.reason}` : "."}`}{" "}
          {observed.tokens
            ? "Token counts come from the harness's own usage events."
            : "This harness does not report token counts."}
        </p>
      ) : null}
    </section>
  );
}

/** Turns per bucket as dithered bars: shape over precision. */
function Spark({
  buckets,
  span,
  bucket,
}: {
  buckets: AccountUsageBucket[];
  span: number;
  bucket: Bucket;
}) {
  const size = bucket === "day" ? 24 * 3600 * 1000 : 3600 * 1000;
  const end = Math.floor(Date.now() / size) * size;
  const values = Array.from({ length: span }, (_, index) => {
    const start = end - (span - 1 - index) * size;
    return buckets
      .filter((entry) => {
        const at = Date.parse(entry.start);
        return at >= start && at < start + size;
      })
      .reduce(
        (total, entry) => total + entry.turnsCompleted + entry.turnsFailed,
        0,
      );
  });
  const max = Math.max(1, ...values);
  return (
    <span className="spark" aria-label={`${values.join(", ")} turns`}>
      {values.map((value, index) => (
        <span
          key={index}
          className="spark__bar"
          style={{
            height: `${Math.max(value ? 12 : 0, (value / max) * 100)}%`,
          }}
        />
      ))}
    </span>
  );
}

function compact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

function VerifyDialog({
  open,
  provider,
  onClose,
  reload,
}: {
  open: boolean;
  provider: string;
  onClose: () => void;
  reload: () => void;
}) {
  const store = useStore();
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState(false);
  async function verify(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    const result = (await store.mutate(
      "provider.accounts.verify",
      { provider, model: model.trim() || null },
      `account.verify:${provider}`,
    )) as { modelAnswered: boolean; routeVerified: boolean } | null;
    setBusy(false);
    if (result) {
      store.toast({
        tone: result.routeVerified ? "info" : "error",
        title: result.routeVerified
          ? "Routing verified"
          : "Routing not verified",
        detail: result.routeVerified
          ? "The selected route returned a model response."
          : result.modelAnswered
            ? "The provider answered, but routing changed or readiness could not be recorded. Refresh and verify again."
            : "The provider did not return a completed answer. Check sign-in and model access.",
      });
      onClose();
    }
    reload();
  }
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`Verify ${providerLabel(provider)} routing`}
      description="Sends one short prompt through the current route. It uses model quota. Run it after signing in or changing credentials."
      width={440}
    >
      <form className="stack" onSubmit={(event) => void verify(event)}>
        <Field
          label="Verification model"
          htmlFor="account-verification-model"
          hint={
            provider === "opencode"
              ? "Required for API accounts, as backend/model."
              : "Optional. Leave empty for the harness default."
          }
        >
          <input
            id="account-verification-model"
            className="input mono-input"
            value={model}
            data-autofocus
            onChange={(event) => setModel(event.target.value)}
          />
        </Field>
        <div className="form-actions">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" loading={busy}>
            Send one prompt
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function AddAccountDialog({
  open,
  provider,
  onClose,
  snapshot,
  reload,
}: {
  open: boolean;
  provider: string;
  onClose: () => void;
  snapshot: ProviderAccountsSnapshot | null;
  reload: () => void;
}) {
  const store = useStore();
  const [form, setForm] = useState({
    provider,
    label: "",
    kind: "native-profile" as "native-profile" | "api-key",
    secret: "",
    backendProvider: "",
    baseUrl: "",
    models: "",
  });
  const [draftId, setDraftId] = useState(uuid);
  const api = form.kind === "api-key";
  useEffect(() => {
    if (open) setForm((value) => ({ ...value, provider }));
  }, [open, provider]);
  useEffect(() => {
    if (!snapshot?.accounts.some((account) => account.spec.id === draftId))
      return;
    // A lost response may still have committed. Reconcile its durable ID before
    // the next submission so it cannot create a second copy of the draft.
    setDraftId(uuid());
    setForm((value) => ({ ...value, label: "", secret: "", models: "" }));
  }, [draftId, snapshot]);

  const defaults = (name: string) =>
    name === "codex"
      ? { env: "OPENAI_API_KEY", protocol: "responses" }
      : name === "claude-code"
        ? { env: "ANTHROPIC_API_KEY", protocol: "anthropic" }
        : name === "github-copilot"
          ? { env: "COPILOT_GITHUB_TOKEN", protocol: null }
          : form.backendProvider.trim() === "anthropic"
            ? { env: "ANTHROPIC_API_KEY", protocol: "anthropic" }
            : form.backendProvider.trim() === "openai"
              ? { env: "OPENAI_API_KEY", protocol: "responses" }
              : form.backendProvider.trim() === "google"
                ? { env: "GOOGLE_GENERATIVE_AI_API_KEY", protocol: null }
                : form.backendProvider.trim() === "openrouter"
                  ? { env: "OPENROUTER_API_KEY", protocol: null }
                  : { env: "OPENAI_API_KEY", protocol: null };

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!form.label.trim()) return;
    const defaulted = defaults(form.provider);
    const spec: ProviderAccountSpec = {
      id: draftId,
      provider: form.provider,
      label: form.label.trim(),
      credentialKind: form.kind,
      profileDirectory: "",
      credentialEnvironment: api ? defaulted.env : null,
      baseUrl:
        api && form.provider !== "github-copilot"
          ? form.baseUrl.trim() || null
          : null,
      apiProtocol: api
        ? (defaulted.protocol as ProviderAccountSpec["apiProtocol"])
        : null,
      backendProvider:
        form.provider === "opencode" && api
          ? form.backendProvider.trim() || null
          : null,
      models: form.models
        .split(",")
        .map((model) => model.trim())
        .filter(Boolean),
      enabled: true,
    };
    const result = await store.mutate(
      "provider.accounts.put",
      {
        spec,
        expectedRevision: 0,
        ...(api && form.secret ? { secret: form.secret } : {}),
      },
      `account.put:${spec.id}`,
    );
    // A secret never leaves component state and is cleared immediately after the mutation settles.
    setForm((value) => ({
      ...value,
      label: result ? "" : value.label,
      secret: "",
      models: result ? "" : value.models,
    }));
    if (result) {
      setDraftId(uuid());
      onClose();
    }
    reload();
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New account"
      description="A native profile gets its own configuration directory under Lazoo's data root; sign in to it afterwards. An API key is stored once and never shown again."
      width={520}
    >
      <form className="stack" onSubmit={(event) => void save(event)}>
        <div className="form-grid">
          <Field label="Provider" htmlFor="account-provider">
            <select
              id="account-provider"
              className="select"
              value={form.provider}
              onChange={(event) =>
                setForm({
                  ...form,
                  provider: event.target.value,
                  kind:
                    event.target.value === "antigravity"
                      ? "native-profile"
                      : form.kind,
                  baseUrl:
                    event.target.value === "github-copilot" ? "" : form.baseUrl,
                })
              }
            >
              {ACCOUNT_PROVIDERS.map((entry) => (
                <option key={entry} value={entry}>
                  {providerLabel(entry)}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Label" htmlFor="account-label">
            <input
              id="account-label"
              required
              className="input"
              data-autofocus
              value={form.label}
              onChange={(event) =>
                setForm({ ...form, label: event.target.value })
              }
            />
          </Field>
          {form.provider !== "antigravity" ? (
            <Field label="Credential" htmlFor="account-kind">
              <select
                id="account-kind"
                className="select"
                value={form.kind}
                onChange={(event) =>
                  setForm({
                    ...form,
                    kind: event.target.value as typeof form.kind,
                  })
                }
              >
                <option value="native-profile">Native profile</option>
                <option value="api-key">API key</option>
              </select>
            </Field>
          ) : null}
          {api ? (
            <Field
              label="Secret"
              htmlFor="account-secret"
              hint="Saved once; it cannot be displayed later."
            >
              <input
                id="account-secret"
                type="password"
                autoComplete="new-password"
                className="input"
                value={form.secret}
                onChange={(event) =>
                  setForm({ ...form, secret: event.target.value })
                }
              />
            </Field>
          ) : null}
          {api && form.provider === "opencode" ? (
            <Field
              label="OpenCode backend"
              htmlFor="account-backend"
              hint="OpenAI and Anthropic can use API pooling; Google and OpenRouter stay direct to a session."
            >
              <select
                id="account-backend"
                required
                className="select"
                value={form.backendProvider}
                onChange={(event) =>
                  setForm({ ...form, backendProvider: event.target.value })
                }
              >
                <option value="">Choose a backend</option>
                <option value="openai">OpenAI (API pool)</option>
                <option value="anthropic">Anthropic (API pool)</option>
                <option value="google">Google (direct session)</option>
                <option value="openrouter">OpenRouter (direct session)</option>
              </select>
            </Field>
          ) : null}
          {api && form.provider !== "github-copilot" ? (
            <Field label="Base URL" htmlFor="account-base">
              <input
                id="account-base"
                className="input"
                placeholder="Optional"
                value={form.baseUrl}
                onChange={(event) =>
                  setForm({ ...form, baseUrl: event.target.value })
                }
              />
            </Field>
          ) : null}
          <Field
            label="Models"
            htmlFor="account-models"
            hint="Optional, comma separated."
          >
            <input
              id="account-models"
              className="input"
              value={form.models}
              onChange={(event) =>
                setForm({ ...form, models: event.target.value })
              }
            />
          </Field>
        </div>
        <div className="form-actions">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            disabled={
              !form.label.trim() ||
              (api &&
                form.provider === "opencode" &&
                !form.backendProvider.trim())
            }
          >
            Add account
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function LoginTerminal({
  terminal,
  onClose,
}: {
  terminal: TerminalRecord;
  onClose: () => void;
}) {
  const terminals = useTerminals();
  const host = useRef<HTMLDivElement>(null);
  const key = `provider-login:${terminal.id}`;
  useSyncExternalStore(
    terminals.subscribe,
    terminals.getVersion,
    terminals.getVersion,
  );
  useLayoutEffect(() => {
    if (!host.current) return;
    terminals.mount(key, host.current);
    void terminals.adoptRecord(key, terminal);
    return () => {
      void terminals.close(key);
    };
  }, [key, terminal, terminals]);
  return (
    <section className="settings__group">
      <h3>Provider sign-in</h3>
      <p className="meta">
        Complete the provider&apos;s own sign-in here. This terminal accepts
        input and is separate from agent sessions.
      </p>
      <div
        ref={host}
        className="terminal-host login-terminal"
        data-visible="true"
        aria-label="Provider sign-in terminal"
      />
      <div className="form-actions">
        <Button variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
    </section>
  );
}
