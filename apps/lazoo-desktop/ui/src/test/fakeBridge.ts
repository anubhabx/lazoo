// A scripted daemon for tests: canned results per method, recorded calls,
// and hand-driven event and terminal channels.

import { vi } from "vitest";

import type { Bridge, TerminalStreamHandle } from "../lib/bridge";
import type {
  EventFrame,
  EventSummary,
  RpcFailure,
  TerminalFrame,
} from "../lib/protocol";

type Handler = (params: unknown) => unknown;

export interface FakeBridge extends Bridge {
  calls: { method: string; params: unknown }[];
  on(method: string, handler: Handler | unknown): void;
  fail(method: string, failure: RpcFailure): void;
  emit(event: EventSummary): void;
  closeEvents(reason: string): void;
  terminalFrames: ((frame: TerminalFrame) => void)[];
  subscribed: number[];
  unsubscribed: number;
  writes: { terminalId: string; data: string }[];
  callsTo(method: string): unknown[];
}

export function createFakeBridge(): FakeBridge {
  const handlers = new Map<string, Handler>();
  const listeners: ((frame: EventFrame) => void)[] = [];
  const bridge: FakeBridge = {
    calls: [],
    subscribed: [],
    unsubscribed: 0,
    writes: [],
    terminalFrames: [],
    on(method, handler) {
      handlers.set(
        method,
        typeof handler === "function" ? (handler as Handler) : () => handler,
      );
    },
    fail(method, failure) {
      handlers.set(method, () => {
        throw failure;
      });
    },
    emit(event) {
      for (const listener of listeners) listener({ kind: "event", event });
    },
    closeEvents(reason) {
      for (const listener of listeners.splice(0))
        listener({ kind: "closed", reason });
    },
    callsTo(method) {
      return bridge.calls
        .filter((call) => call.method === method)
        .map((call) => call.params);
    },
    async call<T>(method: string, params: unknown = {}) {
      bridge.calls.push({ method, params });
      const handler = handlers.get(method);
      if (!handler) {
        throw {
          code: "LZ-TEST-UNSCRIPTED",
          message: `no handler for ${method}`,
        } as RpcFailure;
      }
      return (await handler(params)) as T;
    },
    async subscribeEvents(after, onFrame) {
      bridge.subscribed.push(after);
      listeners.push(onFrame);
      return async () => {
        bridge.unsubscribed += 1;
        const index = listeners.indexOf(onFrame);
        if (index >= 0) listeners.splice(index, 1);
      };
    },
    async startTerminalStream(_args, onFrame): Promise<TerminalStreamHandle> {
      bridge.terminalFrames.push(onFrame);
      return {
        stop: vi.fn(async () => undefined),
        focus: vi.fn(async () => undefined),
      };
    },
    async writeTerminal(terminalId, _bootId, data) {
      bridge.writes.push({ terminalId, data });
    },
    window: {
      close: vi.fn(async () => undefined),
      minimize: vi.fn(async () => undefined),
      toggleMaximize: vi.fn(async () => undefined),
      startDragging: vi.fn(async () => undefined),
      isMaximized: vi.fn(async () => false),
    },
  };
  return bridge;
}
