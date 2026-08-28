import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { DesktopSnapshot } from "./model";

type InvokeCommand = (command: string) => Promise<unknown>;

export interface DesktopWindowAdapter {
  close(): Promise<void>;
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  startDragging(): Promise<void>;
  isMaximized(): Promise<boolean>;
}

interface DesktopBridgeDependencies {
  invoke?: InvokeCommand;
  window?: DesktopWindowAdapter;
  runtimeAvailable?: () => boolean;
}

function defaultRuntimeAvailable() {
  return typeof globalThis === "object" && "__TAURI_INTERNALS__" in globalThis;
}

function defaultWindowAdapter(): DesktopWindowAdapter {
  return {
    close: () => getCurrentWindow().close(),
    minimize: () => getCurrentWindow().minimize(),
    toggleMaximize: () => getCurrentWindow().toggleMaximize(),
    startDragging: () => getCurrentWindow().startDragging(),
    isMaximized: () => getCurrentWindow().isMaximized(),
  };
}

export function createDesktopBridge(
  dependencies: DesktopBridgeDependencies = {},
) {
  const invoke = dependencies.invoke;
  const window = dependencies.window ?? defaultWindowAdapter();
  const runtimeAvailable =
    dependencies.runtimeAvailable ?? defaultRuntimeAvailable;

  return {
    async loadSnapshot() {
      if (!invoke && !runtimeAvailable()) {
        throw {
          code: "LZ-DESKTOP-BRIDGE-MISSING",
          message: "The secure Tauri bridge is unavailable in this window.",
        };
      }
      return (await (invoke ?? tauriInvoke)(
        "load_desktop_snapshot",
      )) as DesktopSnapshot;
    },
    close: () => window.close(),
    minimize: () => window.minimize(),
    toggleMaximize: () => window.toggleMaximize(),
    startDragging: () => window.startDragging(),
    isMaximized: () => window.isMaximized(),
  };
}

export type DesktopBridge = ReturnType<typeof createDesktopBridge>;

export const desktopBridge = createDesktopBridge();
