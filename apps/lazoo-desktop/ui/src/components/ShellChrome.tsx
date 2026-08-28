import { Maximize2, Minus, Search, X } from "lucide-react";

import type { DesktopBridge } from "../bridge";
import type { ConnectionState, DesktopViewModel } from "../model";
import { Button, TooltipButton } from "./primitives";

const connectionLabels: Record<ConnectionState, string> = {
  loading: "Connecting",
  connected: "Connected",
  degraded: "Needs attention",
  offline: "Disconnected",
};

interface TitlebarProps {
  bridge: DesktopBridge;
  connection: ConnectionState;
  currentView: string;
  onOpenCommand: () => void;
}

export function Titlebar({
  bridge,
  connection,
  currentView,
  onOpenCommand,
}: TitlebarProps) {
  return (
    <header className="titlebar" aria-label="Application titlebar">
      <div className="titlebar-identity" data-tauri-drag-region>
        <span className="brand-mark" aria-hidden>
          L
        </span>
        <span className="brand-name" data-tauri-drag-region>
          Lazoo
        </span>
        <span className="titlebar-divider" aria-hidden />
        <span className="titlebar-view" data-tauri-drag-region>
          {currentView}
        </span>
      </div>

      <Button
        className="command-trigger"
        aria-label="Open command palette"
        onPress={onOpenCommand}
      >
        <Search aria-hidden size={14} strokeWidth={1.9} />
        <span>Search or run a command</span>
        <kbd>Ctrl K</kbd>
      </Button>

      <div className="titlebar-end">
        <div className="connection-summary" data-connection={connection}>
          <span className="status-dot" aria-hidden />
          <span>{connectionLabels[connection]}</span>
        </div>
        <div className="window-controls" aria-label="Window controls">
          <TooltipButton
            className="window-control"
            aria-label="Minimize window"
            tooltip="Minimize"
            onPress={() => void bridge.minimize()}
          >
            <Minus aria-hidden size={16} strokeWidth={1.7} />
          </TooltipButton>
          <TooltipButton
            className="window-control"
            aria-label="Maximize or restore window"
            tooltip="Maximize or restore"
            onPress={() => void bridge.toggleMaximize()}
          >
            <Maximize2 aria-hidden size={13} strokeWidth={1.7} />
          </TooltipButton>
          <TooltipButton
            className="window-control window-control-close"
            aria-label="Close window"
            tooltip="Close"
            onPress={() => void bridge.close()}
          >
            <X aria-hidden size={16} strokeWidth={1.7} />
          </TooltipButton>
        </div>
      </div>
    </header>
  );
}

interface StatusBarProps {
  connection: ConnectionState;
  model: DesktopViewModel | null;
}

export function StatusBar({ connection, model }: StatusBarProps) {
  return (
    <footer className="statusbar" role="status" aria-label="Desktop status">
      <div className="statusbar-group">
        <span className="status-dot" data-connection={connection} aria-hidden />
        <span>{connectionLabels[connection]}</span>
        <span className="statusbar-separator" aria-hidden />
        <span>Local pipe</span>
      </div>
      <div className="statusbar-group statusbar-summary">
        <span>
          {model ? `${model.metrics.projects} projects` : "Projects —"}
        </span>
        <span>{model ? `${model.metrics.events} events` : "Events —"}</span>
        <span>
          {model ? `Updated ${model.lastUpdated}` : "Snapshot pending"}
        </span>
      </div>
    </footer>
  );
}
