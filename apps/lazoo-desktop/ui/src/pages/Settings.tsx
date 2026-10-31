import { motion } from "motion/react";
import { Sparkles } from "lucide-react";
import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from "react";

import {
  Avatar,
  Badge,
  Button,
  Field,
  ProjectGlyph,
  SPRING,
  Segmented,
  Switch,
} from "../components/primitives";
import { useAppState, useBridge, useStore, useTerminals } from "../lib/context";
import type {
  AuthorityMode,
  EmployeeRecord,
  NativeTrustPolicy,
  ProviderAccountPolicy,
  ProviderAccountRecord,
  ProviderAccountSpec,
  ProviderAccountsSnapshot,
  ProviderListEntry,
  RpcFailure,
  TerminalRecord,
} from "../lib/protocol";
import { asFailure } from "../lib/bridge";
import type { SettingsSection } from "../lib/store";
import {
  authorityLabel,
  formatBytes,
  formatDuration,
  providerLabel,
  words,
} from "../lib/util";

const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "general", label: "General" },
  { id: "accounts", label: "Accounts" },
  { id: "employees", label: "Employees" },
  { id: "harnesses", label: "Harnesses" },
  { id: "projects", label: "Projects" },
  { id: "memory", label: "Memory" },
  { id: "resources", label: "Resources" },
  { id: "daemon", label: "Daemon" },
];

export function SettingsPage() {
  const store = useStore();
  const state = useAppState();
  const section: SettingsSection =
    state.route.kind === "settings" ? state.route.section : "general";
  return (
    <section className="settings" aria-label="Settings">
      <nav className="settings__nav" aria-label="Settings sections">
        {SECTIONS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            className="row"
            aria-current={section === entry.id ? "page" : undefined}
            onClick={() =>
              store.navigate({ kind: "settings", section: entry.id })
            }
          >
            {section === entry.id ? (
              <motion.span
                layoutId="settings-selection"
                className="rail-selection"
                transition={SPRING}
              />
            ) : null}
            <span className="row__label">{entry.label}</span>
          </button>
        ))}
      </nav>
      <div className="settings__content">
        {section === "general" ? <General /> : null}
        {section === "accounts" ? <Accounts /> : null}
        {section === "employees" ? <Employees /> : null}
        {section === "harnesses" ? <Harnesses /> : null}
        {section === "projects" ? <Projects /> : null}
        {section === "memory" ? <Memory /> : null}
        {section === "resources" ? <Resources /> : null}
        {section === "daemon" ? <Daemon /> : null}
      </div>
    </section>
  );
}

/** Read one daemon method; the result is shown as it is, never invented. */
function useRead<T>(
  method: string,
  params: unknown = {},
  deps: unknown[] = [],
) {
  const bridge = useBridge();
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<RpcFailure | null>(null);
  const [version, setVersion] = useState(0);
  const reload = useCallback(() => setVersion((value) => value + 1), []);
  useEffect(() => {
    let cancelled = false;
    setError(null);
    bridge
      .call<T>(method, params)
      .then((value) => {
        if (!cancelled) setData(value);
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(asFailure(failure));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [method, version, ...deps]);
  return { data, error, reload };
}

function Failure({ error }: { error: RpcFailure | null }) {
  if (!error) return null;
  return (
    <p className="meta" role="alert">
      {error.code}: {error.message}
    </p>
  );
}

/** Flat facts from any object; arrays of objects become tables. */
function JsonFacts({ value }: { value: unknown }) {
  if (value === null || value === undefined)
    return <span className="meta">Nothing yet.</span>;
  if (Array.isArray(value)) {
    if (!value.length) return <span className="meta">Empty.</span>;
    const first = value[0];
    if (typeof first === "object" && first !== null) {
      const columns = Object.keys(first).filter(
        (key) => typeof (first as Record<string, unknown>)[key] !== "object",
      );
      return (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                {columns.map((column) => (
                  <th key={column}>{words(column)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {value.map((row: Record<string, unknown>, index) => (
                <tr key={index}>
                  {columns.map((column) => (
                    <td key={column} className="mono">
                      {String(row[column] ?? "")}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    return <span className="mono">{value.map(String).join(", ")}</span>;
  }
  if (typeof value !== "object")
    return <span className="mono">{String(value)}</span>;
  const entries = Object.entries(value as Record<string, unknown>);
  return (
    <>
      <dl className="facts">
        {entries
          .filter(([, item]) => typeof item !== "object" || item === null)
          .map(([key, item]) => (
            <FactPair key={key} label={words(key)} value={item} />
          ))}
      </dl>
      {entries
        .filter(([, item]) => typeof item === "object" && item !== null)
        .map(([key, item]) => (
          <div key={key}>
            <p className="section-title">{words(key)}</p>
            <JsonFacts value={item} />
          </div>
        ))}
    </>
  );
}

function FactPair({ label, value }: { label: string; value: unknown }) {
  return (
    <>
      <dt>{label}</dt>
      <dd className="mono">{value === null ? "—" : String(value)}</dd>
    </>
  );
}

function General() {
  const store = useStore();
  const { defaultPaneMode } = useAppState();
  return (
    <>
      <h2>General</h2>
      <p className="settings__lede">
        How agents are shown when a conversation opens.
      </p>
      <div className="card">
        <h3>Default pane</h3>
        <Segmented
          name="default-pane"
          label="Default pane"
          value={defaultPaneMode}
          options={[
            { value: "native", label: "Native terminal" },
            { value: "transcript", label: "Transcript" },
          ]}
          onChange={(value) => store.setDefaultPaneMode(value)}
        />
        <p className="meta">
          Native shows the harness&apos;s own TUI through a read-only view.
          Transcript shows Lazoo&apos;s rendering of the durable turn history
          and costs no process. Each conversation remembers its own choice.
        </p>
      </div>
      <div className="card">
        <h3>Motion</h3>
        <p className="meta">
          Transitions follow the Windows &quot;Show animations&quot; setting.
          When it is off, nonessential motion is removed.
        </p>
      </div>
    </>
  );
}

const ACCOUNT_PROVIDERS = [
  "codex",
  "claude-code",
  "github-copilot",
  "opencode",
  "antigravity",
];
const poolProviders = new Set(["codex", "claude-code", "opencode"]);
const uuid = () => crypto.randomUUID();

function Accounts() {
  const store = useStore();
  const accounts = useRead<ProviderAccountsSnapshot>("provider.accounts.list");
  const [form, setForm] = useState({
    provider: "codex",
    label: "",
    kind: "native-profile" as "native-profile" | "api-key",
    secret: "",
    backendProvider: "",
    baseUrl: "",
    models: "",
  });
  const [login, setLogin] = useState<TerminalRecord | null>(null);
  const [draftId, setDraftId] = useState(uuid);
  const [rotating, setRotating] = useState<string | null>(null);
  const [rotationSecret, setRotationSecret] = useState("");
  const [routingProvider, setRoutingProvider] = useState("codex");
  const [verificationModel, setVerificationModel] = useState("");
  const [verifying, setVerifying] = useState(false);
  const snapshot = accounts.data;
  const visible = (snapshot?.accounts ?? []).filter(
    (account) => !account.deleted,
  );
  const policyFor = (provider: string): ProviderAccountPolicy =>
    snapshot?.policies.find((policy) => policy.provider === provider) ?? {
      provider,
      mode: "inherited",
      accountId: null,
      revision: 0,
    };
  const api = form.kind === "api-key";
  useEffect(() => {
    if (!snapshot?.accounts.some((account) => account.spec.id === draftId))
      return;
    // A lost response may still have committed. Reconcile its durable ID before
    // the next submission so it cannot create a second copy of the draft.
    setDraftId(uuid());
    setForm((value) => ({ ...value, label: "", secret: "", models: "" }));
  }, [draftId, snapshot]);
  const defaults = (provider: string) =>
    provider === "codex"
      ? { env: "OPENAI_API_KEY", protocol: "responses" }
      : provider === "claude-code"
        ? { env: "ANTHROPIC_API_KEY", protocol: "anthropic" }
        : provider === "github-copilot"
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
    if (result) setDraftId(uuid());
    accounts.reload();
  }
  async function setPolicy(
    provider: string,
    mode: ProviderAccountPolicy["mode"],
    accountId: string | null,
  ) {
    const eligible = visible.filter(
      (account) => account.spec.provider === provider && account.spec.enabled,
    );
    const normalizedAccountId =
      mode === "pinned" ? (accountId ?? eligible[0]?.spec.id ?? null) : null;
    if (mode === "pinned" && !normalizedAccountId) {
      store.toast({
        tone: "error",
        title: "No enabled account",
        detail: "Add or enable an account before pinning new sessions.",
      });
      return;
    }
    const current = policyFor(provider);
    const result = await store.mutate(
      "provider.accounts.policy",
      {
        provider,
        mode,
        accountId: normalizedAccountId,
        revision: current.revision,
      },
      `account.policy:${provider}:${current.revision}`,
    );
    if (result) accounts.reload();
  }
  async function verifyRouting() {
    setVerifying(true);
    const result = (await store.mutate(
      "provider.accounts.verify",
      {
        provider: routingProvider,
        model: verificationModel.trim() || null,
      },
      `account.verify:${routingProvider}`,
    )) as { modelAnswered: boolean; routeVerified: boolean } | null;
    setVerifying(false);
    if (result)
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
    accounts.reload();
  }
  async function toggle(account: ProviderAccountRecord) {
    const result = await store.mutate(
      "provider.accounts.put",
      {
        spec: { ...account.spec, enabled: !account.spec.enabled },
        expectedRevision: account.revision,
      },
      `account.toggle:${account.spec.id}:${account.revision}`,
    );
    if (result) accounts.reload();
  }
  async function remove(account: ProviderAccountRecord) {
    const result = await store.mutate(
      "provider.accounts.remove",
      { id: account.spec.id, expectedRevision: account.revision },
      `account.remove:${account.spec.id}:${account.revision}`,
    );
    if (result) accounts.reload();
  }
  async function rotateSecret(account: ProviderAccountRecord) {
    if (!rotationSecret) return;
    const result = await store.mutate(
      "provider.accounts.put",
      {
        spec: account.spec,
        expectedRevision: account.revision,
        secret: rotationSecret,
      },
      `account.rotate:${account.spec.id}:${account.revision}`,
    );
    setRotationSecret("");
    if (result) setRotating(null);
    accounts.reload();
  }
  async function signIn(account: ProviderAccountRecord) {
    const terminal = (await store.mutate(
      "provider.accounts.login",
      { id: account.spec.id, expectedRevision: account.revision },
      `account.login:${account.spec.id}:${account.revision}`,
    )) as TerminalRecord | null;
    if (terminal) setLogin(terminal);
  }
  const routingPolicy = policyFor(routingProvider);
  const routingChoices = visible.filter(
    (account) =>
      account.spec.provider === routingProvider && account.spec.enabled,
  );
  return (
    <>
      <h2>Accounts</h2>
      <p className="settings__lede">
        Separate native profiles and API credentials. Secrets are protected by
        Windows for this user and never shown again. Secret fields clear after
        submission and are not saved in browser storage.
      </p>
      <Failure error={accounts.error} />
      <div className="card">
        <h3>Routing</h3>
        <div className="form-grid">
          <Field label="Routing provider" htmlFor="routing-provider">
            <select
              id="routing-provider"
              className="select"
              value={routingProvider}
              onChange={(event) => setRoutingProvider(event.target.value)}
            >
              {ACCOUNT_PROVIDERS.map((provider) => (
                <option key={provider} value={provider}>
                  {providerLabel(provider)}
                </option>
              ))}
            </select>
          </Field>
          <Field label="New sessions" htmlFor="routing-mode">
            <select
              id="routing-mode"
              className="select"
              value={routingPolicy.mode}
              onChange={(event) =>
                void setPolicy(
                  routingProvider,
                  event.target.value as ProviderAccountPolicy["mode"],
                  event.target.value === "pinned"
                    ? (routingPolicy.accountId ??
                        routingChoices[0]?.spec.id ??
                        null)
                    : null,
                )
              }
            >
              <option value="inherited">Use existing behavior</option>
              <option value="balanced">Balance accounts</option>
              <option value="pinned" disabled={!routingChoices.length}>
                Pin an account
                {routingChoices.length ? "" : " (no enabled account)"}
              </option>
              {poolProviders.has(routingProvider) ? (
                <option value="api-pool">Pool API credentials</option>
              ) : null}
            </select>
          </Field>
          {routingPolicy.mode === "pinned" ? (
            <Field label="Pinned account" htmlFor="routing-account">
              <select
                id="routing-account"
                className="select"
                value={routingPolicy.accountId ?? ""}
                onChange={(event) =>
                  void setPolicy(
                    routingProvider,
                    "pinned",
                    event.target.value || null,
                  )
                }
              >
                <option value="">Choose an account</option>
                {routingChoices.map((account) => (
                  <option key={account.spec.id} value={account.spec.id}>
                    {account.spec.label}
                  </option>
                ))}
              </select>
            </Field>
          ) : null}
        </div>
        <p className="meta">
          {routingProvider === "github-copilot" ||
          routingProvider === "antigravity"
            ? `Native sessions stay pinned. ${providerLabel(routingProvider)} does not use Lazoo's API proxy pool.`
            : "Changes apply to new sessions. Existing and resumed native sessions retain their account."}
        </p>
        <div className="form-grid">
          <Field
            label="Verification model"
            htmlFor="account-verification-model"
          >
            <input
              id="account-verification-model"
              className="input"
              value={verificationModel}
              onChange={(event) => setVerificationModel(event.target.value)}
              placeholder={
                routingProvider === "opencode"
                  ? "backend/model (required for API accounts)"
                  : "Optional"
              }
            />
          </Field>
          <Button
            style={{ alignSelf: "end" }}
            disabled={verifying}
            onClick={() => void verifyRouting()}
          >
            {verifying ? "Verifying…" : "Verify routing"}
          </Button>
        </div>
        <p className="meta">
          Sends one short prompt through the current route and uses model quota.
          Run after sign-in or changing credentials.
          {routingProvider === "opencode" &&
            " API accounts require a matching backend/model selection."}
        </p>
      </div>
      {visible.length ? (
        <table className="table">
          <thead>
            <tr>
              <th>Account</th>
              <th>Provider</th>
              <th>Credential</th>
              <th>State</th>
              <th>Calls</th>
              <th>Last result</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {visible.map((account) => (
              <Fragment key={account.spec.id}>
                <tr>
                  <td>
                    {account.spec.label}
                    {account.hasSecret
                      ? ""
                      : account.spec.credentialKind === "api-key"
                        ? " · key needed"
                        : ""}
                  </td>
                  <td>{providerLabel(account.spec.provider)}</td>
                  <td>
                    {account.spec.credentialKind === "native-profile"
                      ? "Native profile"
                      : "API key"}
                  </td>
                  <td>
                    {!account.spec.enabled
                      ? "disabled"
                      : account.cooldownUntil * 1000 > Date.now()
                        ? "cooldown"
                        : "enabled"}
                  </td>
                  <td>
                    {account.requests} requests · {account.failures} failed
                  </td>
                  <td>
                    {account.lastStatus ?? "—"}
                    {account.lastLatencyMillis !== null
                      ? ` · ${account.lastLatencyMillis}ms`
                      : ""}
                  </td>
                  <td>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => void toggle(account)}
                    >
                      {account.spec.enabled ? "Disable" : "Enable"}
                    </Button>
                    {account.spec.credentialKind === "native-profile" &&
                    account.spec.enabled ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void signIn(account)}
                      >
                        Sign in
                      </Button>
                    ) : null}
                    {account.spec.credentialKind === "api-key" ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => {
                          setRotating(account.spec.id);
                          setRotationSecret("");
                        }}
                      >
                        Rotate key
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => void remove(account)}
                    >
                      Remove
                    </Button>
                  </td>
                </tr>
                {rotating === account.spec.id ? (
                  <tr key={`${account.spec.id}:rotate`}>
                    <td colSpan={7}>
                      <div className="form-actions">
                        <label className="field">
                          <span className="field__label">New secret</span>
                          <input
                            className="input"
                            aria-label={`New secret for ${account.spec.label}`}
                            type="password"
                            autoComplete="new-password"
                            value={rotationSecret}
                            onChange={(event) =>
                              setRotationSecret(event.target.value)
                            }
                          />
                        </label>
                        <Button
                          size="sm"
                          onClick={() => void rotateSecret(account)}
                          disabled={!rotationSecret}
                        >
                          Save new key
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setRotating(null);
                            setRotationSecret("");
                          }}
                        >
                          Cancel
                        </Button>
                      </div>
                    </td>
                  </tr>
                ) : null}
              </Fragment>
            ))}
          </tbody>
        </table>
      ) : snapshot ? (
        <p className="meta">
          No accounts configured. Add an account before selecting balanced,
          pinned, or API pool routing.
        </p>
      ) : null}
      <form className="card" onSubmit={(event) => void save(event)}>
        <h3>Add account</h3>
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
              {ACCOUNT_PROVIDERS.map((provider) => (
                <option key={provider} value={provider}>
                  {providerLabel(provider)}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Label" htmlFor="account-label">
            <input
              id="account-label"
              required
              className="input"
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
      {login ? (
        <LoginTerminal terminal={login} onClose={() => setLogin(null)} />
      ) : null}
    </>
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
    <div className="card">
      <h3>Provider sign-in</h3>
      <p className="meta">
        Complete the provider&apos;s own sign-in here. This terminal accepts
        input and is separate from agent sessions.
      </p>
      <div
        ref={host}
        className="terminal-host"
        style={{ height: 360 }}
        aria-label="Provider sign-in terminal"
      />
      <div className="form-actions">
        <Button variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

function Employees() {
  const store = useStore();
  useAppState();
  const employees = useRead<EmployeeRecord[]>("employee.list");
  const providers = useRead<ProviderListEntry[]>("provider.list");
  const [form, setForm] = useState({
    name: "",
    provider: "codex",
    role: "",
    model: "",
    reasoningEffort: "",
    authorityMode: "ask" as AuthorityMode,
    checkoutPolicy: "isolated" as "isolated" | "read-only",
  });

  async function create(event: FormEvent) {
    event.preventDefault();
    if (!form.name.trim()) return;
    const result = await store.mutate(
      "employee.create",
      {
        name: form.name.trim(),
        provider: form.provider,
        role: form.role.trim(),
        instructions: "",
        model: form.model.trim() || null,
        reasoningEffort: form.reasoningEffort.trim() || null,
        nativeSettings: {},
        authorityMode: form.authorityMode,
        checkoutPolicy: form.checkoutPolicy,
        activationPolicy: "manual",
        idleTimeoutSeconds: 0,
        enabled: true,
      },
      `employee.create:${form.name.trim()}:${form.provider}`,
    );
    if (result) {
      setForm((value) => ({
        ...value,
        name: "",
        role: "",
        model: "",
        reasoningEffort: "",
      }));
      employees.reload();
      store.toast({
        tone: "info",
        title: "Employee created",
        detail: form.name.trim(),
      });
    }
  }

  async function disable(employee: EmployeeRecord) {
    const ok = await store.mutate(
      "employee.disable",
      { id: employee.id, expectedRevision: employee.configurationRevision },
      `employee.disable:${employee.id}:${employee.configurationRevision}`,
    );
    if (ok) employees.reload();
  }

  const list = Array.isArray(employees.data) ? employees.data : [];
  const detected = (providers.data ?? []).filter((entry) => entry.detected);

  return (
    <>
      <h2>Employees</h2>
      <p className="settings__lede">
        Named identities on a harness. Creating one runs a bounded readiness
        probe, never a model.
      </p>
      <div className="settings__callout">
        <Sparkles size={16} aria-hidden="true" />
        <span>
          Lazoo ships a recommended team for the harnesses on this machine: a
          lead, builders, and a reviewer, each on the model developers prefer
          for that work.
        </span>
        <Button variant="accent" size="sm" onClick={() => store.openSetup()}>
          Set up agents
        </Button>
      </div>
      <Failure error={employees.error} />
      {list.length ? (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Harness</th>
                <th>Role</th>
                <th>Model</th>
                <th>Authority</th>
                <th>Activation</th>
                <th>Enabled</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {list.map((employee) => (
                <tr key={employee.id}>
                  <td>
                    <span className="cell-identity">
                      <Avatar
                        name={employee.name}
                        provider={employee.provider}
                        size={20}
                      />
                      {employee.name}
                    </span>
                  </td>
                  <td>{providerLabel(employee.provider)}</td>
                  <td>
                    {employee.role ? <Badge>{employee.role}</Badge> : "—"}
                  </td>
                  <td className="mono">{employee.model ?? "—"}</td>
                  <td>{authorityLabel(employee.authorityMode)}</td>
                  <td>{words(employee.activationPolicy)}</td>
                  <td>
                    {employee.enabled ? (
                      <Badge tone="success">enabled</Badge>
                    ) : (
                      <Badge>disabled</Badge>
                    )}
                  </td>
                  <td>
                    {employee.enabled ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void disable(employee)}
                      >
                        Disable
                      </Button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : employees.data ? (
        <p className="meta">No employees yet.</p>
      ) : null}
      <form className="card" onSubmit={(event) => void create(event)}>
        <h3>New employee</h3>
        <div className="form-grid">
          <Field label="Name" htmlFor="emp-name">
            <input
              id="emp-name"
              className="input"
              required
              value={form.name}
              onChange={(event) =>
                setForm({ ...form, name: event.target.value })
              }
            />
          </Field>
          <Field
            label="Harness"
            htmlFor="emp-provider"
            hint={
              detected.length
                ? undefined
                : "No harness detected on this machine yet."
            }
          >
            <select
              id="emp-provider"
              className="select"
              value={form.provider}
              onChange={(event) =>
                setForm({ ...form, provider: event.target.value })
              }
            >
              {[
                "codex",
                "claude-code",
                "github-copilot",
                "opencode",
                "antigravity",
              ].map((provider) => (
                <option key={provider} value={provider}>
                  {providerLabel(provider)}
                  {detected.some((entry) => entry.provider === provider)
                    ? ""
                    : " (not detected)"}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Role" htmlFor="emp-role">
            <input
              id="emp-role"
              className="input"
              placeholder="e.g. backend"
              value={form.role}
              onChange={(event) =>
                setForm({ ...form, role: event.target.value })
              }
            />
          </Field>
          <Field
            label="Model"
            htmlFor="emp-model"
            hint="Leave empty for the harness default."
          >
            <input
              id="emp-model"
              className="input"
              value={form.model}
              onChange={(event) =>
                setForm({ ...form, model: event.target.value })
              }
            />
          </Field>
          <Field label="Reasoning effort" htmlFor="emp-effort">
            <input
              id="emp-effort"
              className="input"
              placeholder="e.g. high"
              value={form.reasoningEffort}
              onChange={(event) =>
                setForm({ ...form, reasoningEffort: event.target.value })
              }
            />
          </Field>
          <Field label="Authority ceiling" htmlFor="emp-authority">
            <select
              id="emp-authority"
              className="select"
              value={form.authorityMode}
              onChange={(event) =>
                setForm({
                  ...form,
                  authorityMode: event.target.value as AuthorityMode,
                })
              }
            >
              <option value="ask">Ask</option>
              <option value="self-decision">Self-decision</option>
              <option value="full-autonomy">Full autonomy</option>
            </select>
          </Field>
          <Field label="Checkout policy" htmlFor="emp-checkout">
            <select
              id="emp-checkout"
              className="select"
              value={form.checkoutPolicy}
              onChange={(event) =>
                setForm({
                  ...form,
                  checkoutPolicy: event.target.value as
                    "isolated" | "read-only",
                })
              }
            >
              <option value="isolated">Isolated worktree</option>
              <option value="read-only">Read-only</option>
            </select>
          </Field>
        </div>
        <div className="form-actions">
          <Button type="submit" variant="primary" disabled={!form.name.trim()}>
            Create employee
          </Button>
        </div>
      </form>
    </>
  );
}

function Harnesses() {
  const providers = useRead<ProviderListEntry[]>("provider.list");
  const roster = useRead<unknown>("roster.get");
  return (
    <>
      <h2>Harnesses</h2>
      <p className="settings__lede">
        What Lazoo detected on this machine, and the native trust policy per
        harness.
      </p>
      <Failure error={providers.error} />
      {providers.data ? (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Harness</th>
                <th>Detected</th>
                <th>Executable</th>
                <th>Source</th>
                <th>Auto-accept managed trust</th>
              </tr>
            </thead>
            <tbody>
              {providers.data.map((entry) => (
                <tr key={entry.provider}>
                  <td>{providerLabel(entry.provider)}</td>
                  <td>
                    {entry.detected ? (
                      <Badge tone="success">detected</Badge>
                    ) : (
                      <Badge>not found</Badge>
                    )}
                  </td>
                  <td className="mono" title={entry.executable ?? undefined}>
                    {entry.executable ?? "—"}
                  </td>
                  <td>{entry.discoverySource ?? "—"}</td>
                  <td>
                    {entry.provider === "claude-code" ||
                    entry.provider === "antigravity" ? (
                      <TrustPolicy provider={entry.provider} />
                    ) : (
                      <span className="meta">not applicable</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
      <div className="card">
        <h3>Roster</h3>
        <Failure error={roster.error} />
        <JsonFacts value={roster.data} />
      </div>
    </>
  );
}

function TrustPolicy({ provider }: { provider: string }) {
  const bridge = useBridge();
  const store = useStore();
  const policy = useRead<NativeTrustPolicy>("native.trust.policy.show", {
    provider,
  });
  if (policy.error) return <span className="meta">{policy.error.code}</span>;
  if (!policy.data) return <span className="meta">…</span>;
  const current = policy.data;
  async function toggle() {
    try {
      if (current.autoAcceptManagedNextTime) {
        await bridge.call("native.trust.policy.reset", {
          provider,
          expectedRevision: current.revision,
        });
      } else {
        await bridge.call("native.trust.policy.set", {
          provider,
          expectedRevision: current.revision,
          autoAcceptManagedNextTime: true,
        });
      }
    } catch (error) {
      const failure = asFailure(error);
      store.toast({
        tone: "error",
        title: failure.code,
        detail: failure.message,
      });
    }
    policy.reload();
  }
  return (
    <Switch
      checked={current.autoAcceptManagedNextTime}
      label={`Auto-accept managed workspace trust for ${providerLabel(provider)}`}
      onChange={() => void toggle()}
    />
  );
}

function Projects() {
  const store = useStore();
  const { projects } = useAppState();
  const [form, setForm] = useState({
    path: "",
    name: "",
    checkoutMode: "isolated" as "isolated" | "lead-main",
  });

  async function register(event: FormEvent) {
    event.preventDefault();
    if (!form.path.trim()) return;
    const result = await store.mutate(
      "project.register",
      {
        path: form.path.trim(),
        displayName: form.name.trim() || null,
        checkoutMode: form.checkoutMode,
      },
      `project.register:${form.path.trim().toLowerCase()}`,
    );
    if (result) {
      setForm({ path: "", name: "", checkoutMode: "isolated" });
      store.toast({ tone: "info", title: "Project registered" });
    }
  }

  return (
    <>
      <h2>Projects</h2>
      <p className="settings__lede">
        Registered local repositories. Each is a space with its own
        conversations.
      </p>
      {projects.length ? (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Path</th>
                <th>Checkout</th>
                <th>Registered</th>
              </tr>
            </thead>
            <tbody>
              {projects.map((project) => (
                <tr key={project.id}>
                  <td>
                    <span className="cell-identity">
                      <ProjectGlyph name={project.displayName} size={18} />
                      {project.displayName}
                    </span>
                  </td>
                  <td className="mono">{project.canonicalPath}</td>
                  <td>{words(project.checkoutMode)}</td>
                  <td>{new Date(project.createdAt).toLocaleDateString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="meta">
          No projects registered. Home holds projectless conversations.
        </p>
      )}
      <form className="card" onSubmit={(event) => void register(event)}>
        <h3>Register a project</h3>
        <div className="form-grid">
          <Field
            label="Path"
            htmlFor="proj-path"
            hint="A local Git repository."
          >
            <input
              id="proj-path"
              className="input"
              required
              placeholder="C:\\workspace\\my-repo"
              value={form.path}
              onChange={(event) =>
                setForm({ ...form, path: event.target.value })
              }
            />
          </Field>
          <Field label="Display name" htmlFor="proj-name">
            <input
              id="proj-name"
              className="input"
              value={form.name}
              onChange={(event) =>
                setForm({ ...form, name: event.target.value })
              }
            />
          </Field>
          <Field label="Checkout mode" htmlFor="proj-checkout">
            <select
              id="proj-checkout"
              className="select"
              value={form.checkoutMode}
              onChange={(event) =>
                setForm({
                  ...form,
                  checkoutMode: event.target.value as "isolated" | "lead-main",
                })
              }
            >
              <option value="isolated">Isolated worktrees</option>
              <option value="lead-main">Lead on main</option>
            </select>
          </Field>
        </div>
        <div className="form-actions">
          <Button type="submit" variant="primary" disabled={!form.path.trim()}>
            Register
          </Button>
        </div>
      </form>
    </>
  );
}

function Memory() {
  const policy = useRead<unknown>("memory.policy.show");
  const queue = useRead<unknown>("memory.queue.status");
  const worker = useRead<unknown>("memory.worker.show");
  return (
    <>
      <h2>Memory</h2>
      <p className="settings__lede">
        Retention policy, the archive queue, and the opt-in summary worker.
      </p>
      <div className="card">
        <h3>Policy</h3>
        <Failure error={policy.error} />
        <JsonFacts value={policy.data} />
      </div>
      <div className="card">
        <h3>Queue</h3>
        <Failure error={queue.error} />
        <JsonFacts value={queue.data} />
      </div>
      <div className="card">
        <h3>Summary worker</h3>
        <Failure error={worker.error} />
        <JsonFacts value={worker.data} />
      </div>
    </>
  );
}

function Resources() {
  const resources = useRead<Record<string, unknown>>("system.resources");
  const budget = useRead<unknown>("system.budget");
  const daemon = resources.data?.daemon as
    | {
        cpuMillis: number;
        workingSetBytes: number;
        peakWorkingSetBytes: number;
        handleCount: number;
        liveSessions: number;
        uptimeMillis: number;
      }
    | undefined;
  return (
    <>
      <h2>Resources</h2>
      <p className="settings__lede">
        Point samples of what Lazoo itself uses. Provider trees are reported
        beside it, never counted against it.
      </p>
      <div className="card">
        <h3>Daemon</h3>
        <Failure error={resources.error} />
        {daemon ? (
          <dl className="facts">
            <FactPair
              label="Working set"
              value={formatBytes(daemon.workingSetBytes)}
            />
            <FactPair
              label="Peak"
              value={formatBytes(daemon.peakWorkingSetBytes)}
            />
            <FactPair
              label="CPU time"
              value={formatDuration(daemon.cpuMillis)}
            />
            <FactPair
              label="Uptime"
              value={formatDuration(daemon.uptimeMillis)}
            />
            <FactPair label="Handles" value={daemon.handleCount} />
            <FactPair label="Live sessions" value={daemon.liveSessions} />
          </dl>
        ) : (
          <JsonFacts value={resources.data} />
        )}
        <div className="form-actions">
          <Button size="sm" variant="ghost" onClick={resources.reload}>
            Sample again
          </Button>
        </div>
      </div>
      <div className="card">
        <h3>Budget</h3>
        <Failure error={budget.error} />
        <JsonFacts value={budget.data} />
      </div>
    </>
  );
}

function Daemon() {
  const { status, connectionError } = useAppState();
  return (
    <>
      <h2>Daemon</h2>
      <p className="settings__lede">
        The local control plane this window is connected to.
      </p>
      <div className="card">
        <Failure error={connectionError} />
        {status ? (
          <dl className="facts">
            <FactPair label="Version" value={status.daemonVersion} />
            <FactPair label="Protocol" value={status.protocolVersion} />
            <FactPair label="Schema" value={status.schemaVersion} />
            <FactPair label="Journal" value={status.journalMode} />
            <FactPair label="Data root" value={status.dataRoot} />
            <FactPair label="Boot" value={status.bootId} />
            <FactPair
              label="Uptime"
              value={formatDuration(status.uptimeMillis)}
            />
            <FactPair label="Projects" value={status.projectCount} />
            <FactPair label="Event high-water" value={status.eventHighWater} />
            <FactPair label="Pending outbox" value={status.pendingOutbox} />
            <FactPair
              label="Emergency incidents"
              value={status.emergencyIncidentCount}
            />
            <FactPair
              label="Degraded"
              value={
                status.degraded.length ? status.degraded.join(", ") : "none"
              }
            />
          </dl>
        ) : (
          <span className="meta">Not connected.</span>
        )}
      </div>
    </>
  );
}
