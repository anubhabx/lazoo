import {
  AlertTriangle,
  CheckCircle2,
  Database,
  RefreshCw,
  ShieldCheck,
  WifiOff,
} from "lucide-react";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import { desktopBridge, type DesktopBridge } from "./bridge";
import { ActionBar, CommandPalette } from "./components/CommandPalette";
import { Inspector } from "./components/Inspector";
import { Navigation, viewLabels } from "./components/Navigation";
import { EventRows, ProjectRows } from "./components/OperationalRows";
import { Button } from "./components/primitives";
import { StatusBar, Titlebar } from "./components/ShellChrome";
import {
  createInitialDesktopState,
  describeSelection,
  normalizeFailure,
  reduceDesktopState,
  type CommandDefinition,
  type DesktopState,
  type DesktopViewModel,
  type Selection,
  type ViewId,
} from "./model";
import "./styles/app.css";

interface AppProps {
  bridge?: DesktopBridge;
  refreshIntervalMs?: number;
}

function LoadingWorkspace({ view }: { view: ViewId }) {
  return (
    <div className="loading-workspace">
      <section
        className="loading-overview"
        aria-label="Loading operational snapshot"
      >
        <div className="skeleton skeleton-kicker" />
        <div className="skeleton skeleton-title" />
        <div className="skeleton skeleton-copy" />
      </section>
      <section
        className="loading-ledger"
        aria-label={`${viewLabels[view]} loading`}
      >
        <div className="section-header">
          <div>
            <span className="section-kicker">Live snapshot</span>
            <h2>Reading project registry</h2>
          </div>
          <span className="section-count">Waiting for daemon</span>
        </div>
        <div className="skeleton-row" />
        <div className="skeleton-row" />
        <div className="skeleton-row" />
      </section>
    </div>
  );
}

function SectionHeader({
  kicker,
  title,
  count,
}: {
  kicker: string;
  title: string;
  count: string;
}) {
  return (
    <header className="section-header">
      <div>
        <span className="section-kicker">{kicker}</span>
        <h2>{title}</h2>
      </div>
      <span className="section-count">{count}</span>
    </header>
  );
}

interface OperationalSectionProps {
  kind: "projects" | "events";
  model: DesktopViewModel;
  selection: Selection | null;
  onSelect: (selection: Selection, origin?: HTMLElement) => void;
}

function OperationalSection({
  kind,
  model,
  selection,
  onSelect,
}: OperationalSectionProps) {
  const projects = kind === "projects";
  return (
    <section className="operational-section">
      <SectionHeader
        kicker={projects ? "Registry" : "Durable ledger"}
        title={projects ? "Registered projects" : "Recent activity"}
        count={
          projects
            ? `${model.projects.length} ${model.projects.length === 1 ? "project" : "projects"}`
            : model.events.length > 0
              ? "Newest first"
              : "Ledger quiet"
        }
      />
      {projects ? (
        <ProjectRows
          projects={model.projects}
          selection={selection}
          onSelect={onSelect}
        />
      ) : (
        <EventRows
          events={model.events}
          selection={selection}
          onSelect={onSelect}
        />
      )}
    </section>
  );
}

function MetricStrip({ model }: { model: DesktopViewModel }) {
  const metrics = [
    ["Projects", model.metrics.projects],
    ["Event waterline", model.metrics.events],
    ["Outbox waiting", model.metrics.pendingOutbox],
    ["Recovered incidents", model.metrics.incidents],
  ];
  return (
    <dl className="metric-strip" aria-label="Control-plane totals">
      {metrics.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function HealthOverview({ model }: { model: DesktopViewModel }) {
  const healthy = model.health.tone === "healthy";
  const Icon = healthy ? CheckCircle2 : AlertTriangle;
  return (
    <section className="health-overview" data-tone={model.health.tone}>
      <div className="health-icon" aria-hidden>
        <Icon size={18} strokeWidth={1.8} />
      </div>
      <div className="health-copy">
        <span className="section-kicker">Current state</span>
        <h2>{model.health.label}</h2>
        <p>{model.health.detail}</p>
      </div>
      <div className="health-contract">
        <ShieldCheck aria-hidden size={14} strokeWidth={1.8} />
        <span>Protocol {model.system.protocolVersion}</span>
      </div>
    </section>
  );
}

function SystemView({ model }: { model: DesktopViewModel }) {
  const facts = [
    ["Snapshot", model.capturedAt],
    ["Daemon", `v${model.system.daemonVersion}`],
    ["Uptime", model.system.uptime],
    [
      "Protocol / schema",
      `${model.system.protocolVersion} / ${model.system.schemaVersion}`,
    ],
    ["Journal", model.system.journalMode],
    ["Keeper", model.system.keeper],
    ["Boot identity", model.system.bootId],
    ["Data root", model.system.dataRoot],
  ];
  return (
    <section className="operational-section system-section">
      <SectionHeader
        kicker="Local runtime"
        title="System pulse"
        count="Read only"
      />
      <dl className="system-facts">
        {facts.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd
              className={
                label === "Boot identity" || label === "Data root"
                  ? "technical"
                  : ""
              }
            >
              {value}
            </dd>
          </div>
        ))}
      </dl>
      <div className="system-boundary">
        <Database aria-hidden size={16} strokeWidth={1.7} />
        <div>
          <strong>Snapshot authority only</strong>
          <p>
            This desktop cannot register projects, stop the daemon, or run
            employees.
          </p>
        </div>
      </div>
    </section>
  );
}

function WorkspaceView({
  view,
  model,
  selection,
  onSelect,
}: {
  view: ViewId;
  model: DesktopViewModel;
  selection: Selection | null;
  onSelect: (selection: Selection, origin?: HTMLElement) => void;
}) {
  if (view === "projects") {
    return (
      <OperationalSection
        kind="projects"
        model={model}
        selection={selection}
        onSelect={onSelect}
      />
    );
  }
  if (view === "activity") {
    return (
      <OperationalSection
        kind="events"
        model={model}
        selection={selection}
        onSelect={onSelect}
      />
    );
  }
  if (view === "system") return <SystemView model={model} />;
  return (
    <>
      <HealthOverview model={model} />
      <MetricStrip model={model} />
      <div className="control-grid">
        <OperationalSection
          kind="projects"
          model={model}
          selection={selection}
          onSelect={onSelect}
        />
        <OperationalSection
          kind="events"
          model={model}
          selection={selection}
          onSelect={onSelect}
        />
      </div>
    </>
  );
}

function FailureBanner({
  state,
  onRetry,
}: {
  state: DesktopState;
  onRetry: () => void;
}) {
  if (state.connection !== "offline" || !state.failure || !state.model)
    return null;
  return (
    <div className="failure-banner" role="alert">
      <WifiOff aria-hidden size={16} strokeWidth={1.8} />
      <div>
        <strong>Showing last known snapshot</strong>
        <span>
          {state.failure.code} · {state.failure.message}
        </span>
      </div>
      <Button onPress={onRetry} isDisabled={state.isRefreshing}>
        Retry
      </Button>
    </div>
  );
}

function DisconnectedView({
  state,
  onRetry,
}: {
  state: DesktopState;
  onRetry: () => void;
}) {
  return (
    <section className="disconnected-state">
      <WifiOff aria-hidden size={24} strokeWidth={1.6} />
      <span className="section-kicker">Local control channel</span>
      <h2>Control room disconnected</h2>
      <p>{state.failure?.message}</p>
      <code>{state.failure?.code}</code>
      <Button
        className="primary-button"
        onPress={onRetry}
        isDisabled={state.isRefreshing}
        aria-busy={state.isRefreshing}
      >
        Retry connection
      </Button>
    </section>
  );
}

export function App({
  bridge = desktopBridge,
  refreshIntervalMs = 10_000,
}: AppProps) {
  const [activeView, setActiveView] = useState<ViewId>("control");
  const [selection, setSelection] = useState<Selection | null>(null);
  const [commandOpen, setCommandOpen] = useState(false);
  const [desktopState, dispatch] = useReducer(
    reduceDesktopState,
    undefined,
    createInitialDesktopState,
  );
  const refreshInFlight = useRef(false);
  const selectionOrigin = useRef<HTMLElement | null>(null);
  const commandOrigin = useRef<HTMLElement | null>(null);
  const commandTriggerRef = useRef<HTMLButtonElement | null>(null);

  const refresh = useCallback(async () => {
    if (refreshInFlight.current) return;
    refreshInFlight.current = true;
    dispatch({ type: "loadStarted" });
    try {
      const snapshot = await bridge.loadSnapshot();
      dispatch({ type: "loadSucceeded", snapshot });
    } catch (error) {
      dispatch({ type: "loadFailed", failure: normalizeFailure(error) });
    } finally {
      refreshInFlight.current = false;
    }
  }, [bridge]);

  useEffect(() => {
    void refresh();
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    const interval =
      refreshIntervalMs > 0
        ? window.setInterval(() => {
            if (document.visibilityState === "visible") void refresh();
          }, refreshIntervalMs)
        : null;
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      if (interval !== null) window.clearInterval(interval);
    };
  }, [refresh, refreshIntervalMs]);

  const closeInspector = useCallback(() => {
    const origin = selectionOrigin.current;
    setSelection(null);
    queueMicrotask(() => origin?.focus());
  }, []);

  const openCommand = useCallback(() => {
    const activeElement = document.activeElement;
    commandOrigin.current =
      activeElement instanceof HTMLElement && activeElement !== document.body
        ? activeElement
        : commandTriggerRef.current;
    setCommandOpen(true);
  }, []);

  const changeCommandOpen = useCallback(
    (nextOpen: boolean) => {
      if (nextOpen) {
        openCommand();
        return;
      }
      const origin = commandOrigin.current ?? commandTriggerRef.current;
      setCommandOpen(false);
      queueMicrotask(() => origin?.focus());
    },
    [openCommand],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (!commandOpen) openCommand();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [commandOpen, openCommand]);

  useEffect(() => {
    if (!selection || commandOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeInspector();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [closeInspector, commandOpen, selection]);

  const navigate = (view: ViewId) => {
    setActiveView(view);
    setSelection(null);
  };

  const select = (nextSelection: Selection, origin?: HTMLElement) => {
    if (origin?.getAttribute("role") === "option")
      selectionOrigin.current = origin;
    setSelection(nextSelection);
  };

  const executeCommand = (command: CommandDefinition) => {
    if (command.id === "refresh") {
      void refresh();
      return;
    }
    const destination = command.id.replace("navigate-", "") as ViewId;
    if (command.id.startsWith("navigate-") && destination in viewLabels) {
      navigate(destination);
    }
  };

  const model = desktopState.model;
  const selectionDescription =
    model && selection ? describeSelection(model, selection) : null;
  return (
    <>
      <div className="app-shell" data-connection={desktopState.connection}>
        <Titlebar
          bridge={bridge}
          connection={desktopState.connection}
          currentView={viewLabels[activeView]}
          onOpenCommand={openCommand}
          commandTriggerRef={commandTriggerRef}
        />

        <div className="shell-body">
          <Navigation activeView={activeView} onNavigate={navigate} />
          <div
            className="workspace-stage"
            data-inspector-open={selection ? "true" : "false"}
          >
            <section
              className="workspace-frame"
              aria-label={`${viewLabels[activeView]} workspace`}
            >
              <header className="workspace-toolbar">
                <div className="workspace-location">
                  <span>Lazoo</span>
                  <span aria-hidden>/</span>
                  <strong>{viewLabels[activeView]}</strong>
                </div>
                <Button
                  className="refresh-button"
                  aria-label="Refresh snapshot"
                  aria-busy={desktopState.isRefreshing}
                  isDisabled={desktopState.isRefreshing}
                  onPress={() => void refresh()}
                >
                  <RefreshCw aria-hidden size={14} strokeWidth={1.8} />
                  <span>Refresh</span>
                </Button>
              </header>
              <FailureBanner
                state={desktopState}
                onRetry={() => void refresh()}
              />
              <main className="workspace" aria-busy={desktopState.isRefreshing}>
                <div className="workspace-heading">
                  <div>
                    <span className="section-kicker">Local control plane</span>
                    <h1>{viewLabels[activeView]}</h1>
                  </div>
                  <span className="workspace-context">Windows local</span>
                </div>
                {!model && desktopState.connection === "loading" ? (
                  <LoadingWorkspace view={activeView} />
                ) : null}
                {!model && desktopState.connection === "offline" ? (
                  <DisconnectedView
                    state={desktopState}
                    onRetry={() => void refresh()}
                  />
                ) : null}
                {model ? (
                  <WorkspaceView
                    view={activeView}
                    model={model}
                    selection={selection}
                    onSelect={select}
                  />
                ) : null}
              </main>
              <ActionBar
                viewLabel={viewLabels[activeView]}
                selectionLabel={selectionDescription?.label}
                selectionKind={selectionDescription?.kindLabel}
                onOpenCommand={openCommand}
                onCloseInspector={closeInspector}
              />
            </section>
            {model && selection ? (
              <Inspector
                model={model}
                selection={selection}
                onClose={closeInspector}
              />
            ) : null}
          </div>
        </div>

        <StatusBar
          connection={desktopState.connection}
          model={desktopState.model}
        />
        <p className="sr-only" aria-live="polite">
          {desktopState.connection === "loading"
            ? "Loading control-plane snapshot"
            : desktopState.connection === "offline"
              ? `${desktopState.failure?.code}. ${desktopState.failure?.message}`
              : `${desktopState.model?.health.label}. Snapshot updated.`}
        </p>
      </div>
      <CommandPalette
        isOpen={commandOpen}
        context={{ view: activeView, selection }}
        onOpenChange={changeCommandOpen}
        onExecute={executeCommand}
        onInspectSelection={() => undefined}
      />
    </>
  );
}
