// The only door to the Rust side. Every daemon read and action goes through
// `daemon_call`, which is allowlisted in Rust; streams come back over channels.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import type { EventFrame, RpcFailure, TerminalFrame } from "./protocol";
import type {
  OverlayUpdate,
  PttState,
  TranscribePurpose,
  TranscribeResult,
  VoiceSettings,
  VoiceState,
  WhisperModel,
  WhisperProgress,
} from "./voice/types";

export interface TerminalStreamHandle {
  stop(): Promise<void>;
  focus(focused: boolean): Promise<void>;
}

export interface WindowControls {
  /** The Tauri window label: `main`, or `voice-overlay` for the dictation pill. */
  readonly label: string;
  close(): Promise<void>;
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  startDragging(): Promise<void>;
  isMaximized(): Promise<boolean>;
}

/** The desktop's voice commands and events (spec 2026-11-04, sections 4 and 5). */
export interface VoiceBridge {
  state(): Promise<VoiceState>;
  settingsSet(settings: VoiceSettings, revision: number): Promise<VoiceState>;
  keySet(key: string): Promise<VoiceState>;
  keyClear(): Promise<VoiceState>;
  whisperInstall(
    model: WhisperModel,
    onProgress: (progress: WhisperProgress) => void,
  ): Promise<VoiceState>;
  whisperRemove(model: WhisperModel | null): Promise<VoiceState>;
  /** Raw WAV bytes (16 kHz mono PCM16) in, the transcript out. Audio is never stored. */
  transcribe(
    wav: Uint8Array,
    purpose: TranscribePurpose,
  ): Promise<TranscribeResult>;
  /** Endpoint text to speech: the audio/mpeg bytes. */
  speak(text: string): Promise<ArrayBuffer>;
  insertText(text: string): Promise<void>;
  /** Written from Rust: WebView2 refuses clipboard writes while the window is unfocused. */
  copyText(text: string): Promise<void>;
  /** Opens Windows Settings at Privacy & security > Microphone. */
  openMicrophoneSettings(): Promise<void>;
  overlay(update: OverlayUpdate): Promise<void>;
  /** The global push-to-talk hotkey, pressed and released (main window). */
  onPtt(handler: (state: PttState) => void): Promise<() => void>;
  /** Overlay state pushed by the main window (overlay window). */
  onOverlay(handler: (update: OverlayUpdate) => void): Promise<() => void>;
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
  voice: VoiceBridge;
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

async function voiceInvoke<T>(
  command: string,
  args?: Record<string, unknown> | Uint8Array,
  options?: { headers: Record<string, string> },
): Promise<T> {
  try {
    return (await invoke(command, args, options)) as T;
  } catch (error) {
    throw asFailure(error);
  }
}

function createTauriVoice(): VoiceBridge {
  return {
    state: () => voiceInvoke("voice_state"),
    settingsSet: (settings, revision) =>
      voiceInvoke("voice_settings_set", { settings, revision }),
    keySet: (key) => voiceInvoke("voice_key_set", { key }),
    keyClear: () => voiceInvoke("voice_key_clear"),
    whisperInstall(model, onProgress) {
      const channel = new Channel<WhisperProgress>();
      channel.onmessage = onProgress;
      return voiceInvoke("voice_whisper_install", {
        model,
        onProgress: channel,
      });
    },
    whisperRemove: (model) => voiceInvoke("voice_whisper_remove", { model }),
    transcribe: (wav, purpose) =>
      voiceInvoke("voice_transcribe", wav, {
        headers: { "x-lazoo-purpose": purpose },
      }),
    speak: (text) => voiceInvoke<ArrayBuffer>("voice_speak", { text }),
    insertText: (text) => voiceInvoke<void>("voice_insert_text", { text }),
    copyText: (text) => voiceInvoke<void>("voice_copy_text", { text }),
    openMicrophoneSettings: () =>
      voiceInvoke<void>("voice_open_microphone_settings"),
    overlay: (update) =>
      voiceInvoke<void>("voice_overlay", {
        state: update.state,
        detail: update.detail,
        level: update.level,
      }),
    async onPtt(handler) {
      return listen<{ state: PttState }>("voice://ptt", (event) =>
        handler(event.payload.state),
      );
    },
    async onOverlay(handler) {
      return listen<OverlayUpdate>("voice://overlay", (event) =>
        handler(event.payload),
      );
    },
  };
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
    voice: createTauriVoice(),
    window: {
      get label() {
        return win().label;
      },
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
    voice: {
      state: () => Promise.reject(offline),
      settingsSet: () => Promise.reject(offline),
      keySet: () => Promise.reject(offline),
      keyClear: () => Promise.reject(offline),
      whisperInstall: () => Promise.reject(offline),
      whisperRemove: () => Promise.reject(offline),
      transcribe: () => Promise.reject(offline),
      speak: () => Promise.reject(offline),
      insertText: () => Promise.reject(offline),
      copyText: () => Promise.reject(offline),
      openMicrophoneSettings: () => Promise.reject(offline),
      overlay: noop,
      onPtt: async () => () => undefined,
      onOverlay: async () => () => undefined,
    },
    window: {
      label: "main",
      close: noop,
      minimize: noop,
      toggleMaximize: noop,
      startDragging: noop,
      isMaximized: async () => false,
    },
  };
}
