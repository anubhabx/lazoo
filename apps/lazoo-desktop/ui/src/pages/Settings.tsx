import { Sparkles } from "lucide-react";
import { Fragment, useState, type FormEvent } from "react";

import {
  Avatar,
  Badge,
  Button,
  Field,
  Kbd,
  ProjectGlyph,
  Switch,
} from "../components/primitives";
import { useAppState, useBridge, useStore } from "../lib/context";
import type {
  AuthorityMode,
  EmployeeRecord,
  NativeTrustPolicy,
  ProviderListEntry,
} from "../lib/protocol";
import { asFailure } from "../lib/bridge";
import { Failure, useRead } from "../lib/useRead";
import { AccountsPage } from "./Accounts";
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
            <span className="row__label">{entry.label}</span>
          </button>
        ))}
      </nav>
      <div className="settings__content">
        {section === "general" ? <General /> : null}
        {section === "accounts" ? <AccountsPage /> : null}
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

const SHORTCUTS: [string, string[]][] = [
  ["New work", ["Ctrl", "N"]],
  ["Search and commands", ["Ctrl", "K"]],
  ["Activity and details", ["Ctrl", "I"]],
  ["Toggle the sidebar", ["Ctrl", "B"]],
  ["Back, forward", ["Alt", "← →"]],
];

function General() {
  return (
    <>
      <h2>General</h2>
      <p className="settings__lede">
        Conversations open on each agent&apos;s own TUI. Lazoo&apos;s record of
        requests, replies and delegations is in Activity.
      </p>
      <section className="settings__group">
        <h3>Keyboard</h3>
        <dl className="facts facts--keys">
          {SHORTCUTS.map(([label, keys]) => (
            <Fragment key={label}>
              <dt>{label}</dt>
              <dd>
                <Kbd keys={keys} />
              </dd>
            </Fragment>
          ))}
        </dl>
      </section>
      <section className="settings__group">
        <h3>Motion</h3>
        <p className="meta">
          Follows the Windows &quot;Show animations&quot; setting. When it is
          off, avatars hold still and nonessential motion is removed.
        </p>
      </section>
    </>
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
