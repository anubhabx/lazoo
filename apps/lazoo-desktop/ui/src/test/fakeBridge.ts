// A scripted daemon for tests: canned results per method, recorded calls,
// and hand-driven event and terminal channels.

import { vi, type Mock } from "vitest";

import type {
  Bridge,
  TerminalStreamHandle,
  VoiceBridge,
  WorkflowsBridge,
} from "../lib/bridge";
import type {
  EventFrame,
  EventSummary,
  RpcFailure,
  TerminalFrame,
} from "../lib/protocol";
import type { OverlayUpdate, PttState } from "../lib/voice/types";

type Handler = (params: unknown) => unknown;

export type FakeVoice = {
  [K in keyof VoiceBridge]: Mock<VoiceBridge[K]>;
};

export type FakeWorkflows = {
  [K in keyof WorkflowsBridge]: Mock<WorkflowsBridge[K]>;
} & {
  /** The stored document and revision, as the desktop would hold them. */
  stored: { revision: number; document: unknown };
};

export interface FakeBridge extends Bridge {
  voice: FakeVoice;
  workflows: FakeWorkflows;
  /** Drives the global push-to-talk hotkey. */
  firePtt(state: PttState): void;
  /** Drives the overlay window's event. */
  fireOverlay(update: OverlayUpdate): void;
  /** Sets the Tauri window label (`main` by default). */
  setWindowLabel(label: string): void;
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
  const pttHandlers = new Set<(state: PttState) => void>();
  const overlayHandlers = new Set<(update: OverlayUpdate) => void>();
  let windowLabel = "main";
  const stored: FakeWorkflows["stored"] = { revision: 0, document: null };
  const unscripted = (name: string) =>
    ({
      code: "LZ-TEST-UNSCRIPTED",
      message: `no handler for voice.${name}`,
    }) as RpcFailure;
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
    voice: {
      state: vi.fn<VoiceBridge["state"]>(async () => {
        throw unscripted("state");
      }),
      settingsSet: vi.fn<VoiceBridge["settingsSet"]>(async () => {
        throw unscripted("settingsSet");
      }),
      keySet: vi.fn<VoiceBridge["keySet"]>(async () => {
        throw unscripted("keySet");
      }),
      keyClear: vi.fn<VoiceBridge["keyClear"]>(async () => {
        throw unscripted("keyClear");
      }),
      whisperInstall: vi.fn<VoiceBridge["whisperInstall"]>(async () => {
        throw unscripted("whisperInstall");
      }),
      whisperRemove: vi.fn<VoiceBridge["whisperRemove"]>(async () => {
        throw unscripted("whisperRemove");
      }),
      transcribe: vi.fn<VoiceBridge["transcribe"]>(async () => {
        throw unscripted("transcribe");
      }),
      speak: vi.fn<VoiceBridge["speak"]>(async () => {
        throw unscripted("speak");
      }),
      insertText: vi.fn<VoiceBridge["insertText"]>(async () => undefined),
      copyText: vi.fn<VoiceBridge["copyText"]>(async () => undefined),
      openMicrophoneSettings: vi.fn<VoiceBridge["openMicrophoneSettings"]>(
        async () => undefined,
      ),
      overlay: vi.fn<VoiceBridge["overlay"]>(async () => undefined),
      onPtt: vi.fn<VoiceBridge["onPtt"]>(async (handler) => {
        pttHandlers.add(handler);
        return () => {
          pttHandlers.delete(handler);
        };
      }),
      onOverlay: vi.fn<VoiceBridge["onOverlay"]>(async (handler) => {
        overlayHandlers.add(handler);
        return () => {
          overlayHandlers.delete(handler);
        };
      }),
    },
    workflows: {
      stored,
      load: vi.fn<WorkflowsBridge["load"]>(async () => ({
        revision: stored.revision,
        document: structuredClone(stored.document),
      })),
      save: vi.fn<WorkflowsBridge["save"]>(async (document, revision) => {
        if (revision !== stored.revision) {
          throw {
            code: "LZ-WORKFLOWS-CONFLICT",
            message: "The workflows changed elsewhere.",
          } as RpcFailure;
        }
        stored.document = structuredClone(document);
        stored.revision += 1;
        return { revision: stored.revision };
      }),
    },
    firePtt(state) {
      for (const handler of [...pttHandlers]) handler(state);
    },
    fireOverlay(update) {
      for (const handler of [...overlayHandlers]) handler(update);
    },
    setWindowLabel(label) {
      windowLabel = label;
    },
    window: {
      get label() {
        return windowLabel;
      },
      close: vi.fn(async () => undefined),
      minimize: vi.fn(async () => undefined),
      toggleMaximize: vi.fn(async () => undefined),
      startDragging: vi.fn(async () => undefined),
      isMaximized: vi.fn(async () => false),
    },
  };
  return bridge;
}
