import { useState } from "react";

import { desktopBridge, type DesktopBridge } from "./bridge";
import { Navigation, viewLabels } from "./components/Navigation";
import { StatusBar, Titlebar } from "./components/ShellChrome";
import { createInitialDesktopState, type ViewId } from "./model";
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

export function App({
  bridge = desktopBridge,
  refreshIntervalMs = 10_000,
}: AppProps) {
  const [activeView, setActiveView] = useState<ViewId>("control");
  const [desktopState] = useState(createInitialDesktopState);

  void refreshIntervalMs;

  return (
    <div className="app-shell" data-connection={desktopState.connection}>
      <Titlebar
        bridge={bridge}
        connection={desktopState.connection}
        currentView={viewLabels[activeView]}
        onOpenCommand={() => undefined}
      />

      <div className="shell-body">
        <Navigation activeView={activeView} onNavigate={setActiveView} />
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
            <span className="read-only-label">Snapshot view</span>
          </header>
          <main className="workspace" aria-busy={desktopState.isRefreshing}>
            <div className="workspace-heading">
              <div>
                <span className="section-kicker">Local control plane</span>
                <h1>{viewLabels[activeView]}</h1>
              </div>
              <span className="workspace-context">Windows local</span>
            </div>
            <LoadingWorkspace view={activeView} />
          </main>
        </section>
      </div>

      <StatusBar
        connection={desktopState.connection}
        model={desktopState.model}
      />
    </div>
  );
}
