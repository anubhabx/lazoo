// The only door to the Rust side. Every daemon read and action goes through
// `daemon_call`, which is allowlisted in Rust; streams come back over channels.

import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { EventFrame, RpcFailure, TerminalFrame } from "./protocol";

export interface TerminalStreamHandle {
  stop(): Promise<void>;
  focus(focused: boolean): Promise<void>;
}

export interface WindowControls {
  close(): Promise<void>;
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  startDragging(): Promise<void>;
  isMaximized(): Promise<boolean>;
}

export interface Bridge {
  call<T>(method: string, params?: unknown): Promise<T>;
  subscribeEvents(
    after: number,
    onFrame: (frame: EventFrame) => void,
  ): Promise<() => Promise<void>>;
  startTerminalStream(
    args: { terminalId: string; bootId: string; after: number },
    onFrame: (frame: TerminalFrame) => void,
  ): Promise<TerminalStreamHandle>;
  writeTerminal(
    terminalId: string,
    bootId: string,
    data: string,
  ): Promise<void>;
  window: WindowControls;
}

export function isRpcFailure(value: unknown): value is RpcFailure {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as RpcFailure).code === "string" &&
    typeof (value as RpcFailure).message === "string"
  );
}

export function asFailure(error: unknown): RpcFailure {
  if (isRpcFailure(error)) return error;
  if (error instanceof Error) {
    return { code: "LZ-DESKTOP-UNEXPECTED", message: error.message };
  }
  return { code: "LZ-DESKTOP-UNEXPECTED", message: String(error) };
}

export function runtimeAvailable(): boolean {
  return typeof globalThis === "object" && "__TAURI_INTERNALS__" in globalThis;
}

export function createTauriBridge(): Bridge {
  const win = () => getCurrentWindow();
  return {
    async call<T>(method: string, params: unknown = {}) {
      try {
        return (await invoke("daemon_call", { method, params })) as T;
      } catch (error) {
        throw asFailure(error);
      }
    },
    async subscribeEvents(after, onFrame) {
      const channel = new Channel<EventFrame>();
      channel.onmessage = onFrame;
      const id = (await invoke("events_subscribe", {
        after,
        onEvent: channel,
      })) as number;
      return async () => {
        await invoke("events_unsubscribe", { id });
      };
    },
    async startTerminalStream(args, onFrame) {
      const channel = new Channel<TerminalFrame>();
      channel.onmessage = onFrame;
      const id = (await invoke("terminal_stream_start", {
        terminalId: args.terminalId,
        bootId: args.bootId,
        after: args.after,
        onChunk: channel,
      })) as number;
      return {
        stop: async () => {
          await invoke("terminal_stream_stop", { id });
        },
        focus: async (focused: boolean) => {
          await invoke("terminal_stream_focus", { id, focused });
        },
      };
    },
    async writeTerminal(terminalId, bootId, data) {
      try {
        await invoke("terminal_write", { terminalId, bootId, data });
      } catch (error) {
        throw asFailure(error);
      }
    },
    window: {
      close: () => win().close(),
      minimize: () => win().minimize(),
      toggleMaximize: () => win().toggleMaximize(),
      startDragging: () => win().startDragging(),
      isMaximized: () => win().isMaximized(),
    },
  };
}

/** Used when the page is opened in a plain browser (vite dev without Tauri). */
export function createOfflineBridge(): Bridge {
  const offline: RpcFailure = {
    code: "LZ-DESKTOP-BRIDGE-MISSING",
    message: "The secure Tauri bridge is unavailable in this window.",
  };
  const noop = async () => undefined;
  return {
    call: () => Promise.reject(offline),
    subscribeEvents: () => Promise.reject(offline),
    startTerminalStream: () => Promise.reject(offline),
    writeTerminal: () => Promise.reject(offline),
    window: {
      close: noop,
      minimize: noop,
      toggleMaximize: noop,
      startDragging: noop,
      isMaximized: async () => false,
    },
  };
}
