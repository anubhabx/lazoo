// The voice contract: docs/superpowers/specs/2026-11-04-voice-mode-design.md
// sections 4 and 5. Shapes here mirror the Rust commands exactly.

export type SttEngine = "off" | "whisper-local" | "endpoint";
export type TtsEngine = "off" | "system" | "endpoint";
export type Cleaning = "off" | "basic" | "model";
export type DictationTarget = "insert" | "clipboard" | "conversation";
export type WhisperModel =
  "tiny.en" | "base.en" | "small.en" | "base" | "small";

export interface VoiceSettings {
  stt: { engine: SttEngine; model: WhisperModel; language: string };
  tts: { engine: TtsEngine; voice: string; rate: number };
  endpoint: {
    baseUrl: string;
    transcriptionModel: string;
    speechModel: string;
    speechVoice: string;
    cleanupModel: string;
  };
  cleaning: Cleaning;
  allowExternal: boolean;
  reviewBeforeSend: boolean;
  readReplies: boolean;
  hotkey: string;
  dictationTarget: DictationTarget;
}

export interface WhisperCatalogEntry {
  model: WhisperModel;
  bytes: number;
  installed: boolean;
}

export interface VoiceState {
  revision: number;
  settings: VoiceSettings;
  keySet: boolean;
  endpointExternal: boolean;
  whisper: {
    binaryInstalled: boolean;
    installedModels: WhisperModel[];
    catalog: WhisperCatalogEntry[];
  };
  hotkeyRegistered: boolean;
  hotkeyError: string | null;
  /** Windows privacy settings deny microphone access (device, user, or desktop apps). */
  microphoneBlocked: boolean;
}

export interface WhisperProgress {
  stage: "binary" | "model" | "verify" | "done";
  received: number;
  total: number;
}

export type TranscribePurpose = "conversation" | "dictation";

export interface TranscribeResult {
  text: string;
  raw: string;
  cleaning: Cleaning;
  engine: SttEngine;
  external: boolean;
  audioMs: number;
  elapsedMs: number;
}

export type OverlayStateName =
  "hidden" | "recording" | "transcribing" | "done" | "error";

export interface OverlayUpdate {
  state: OverlayStateName;
  detail: string | null;
  level: number | null;
}

export type PttState = "pressed" | "released";

/** A finished capture: 16 kHz mono PCM16 WAV and how long it runs. */
export interface AudioClip {
  wav: Uint8Array;
  durationMs: number;
}

export const WHISPER_LABELS: Record<WhisperModel, string> = {
  "tiny.en": "Tiny, English",
  "base.en": "Base, English",
  "small.en": "Small, English",
  base: "Base, multilingual",
  small: "Small, multilingual",
};

export const DEFAULT_VOICE_SETTINGS: VoiceSettings = {
  stt: { engine: "whisper-local", model: "base.en", language: "en" },
  tts: { engine: "system", voice: "", rate: 1 },
  endpoint: {
    baseUrl: "https://api.openai.com/v1",
    transcriptionModel: "gpt-4o-mini-transcribe",
    speechModel: "gpt-4o-mini-tts",
    speechVoice: "alloy",
    cleanupModel: "gpt-4o-mini",
  },
  cleaning: "basic",
  allowExternal: false,
  reviewBeforeSend: true,
  readReplies: false,
  hotkey: "Ctrl+Alt+Space",
  dictationTarget: "insert",
};

/** The host of an endpoint URL, or null when it does not parse. */
export function endpointHost(baseUrl: string): string | null {
  try {
    return new URL(baseUrl).host || null;
  } catch {
    return null;
  }
}

/** Loopback hosts never leave the machine (spec section 3). */
export function isLoopbackUrl(baseUrl: string): boolean {
  try {
    const host = new URL(baseUrl).hostname.replace(/^\[|\]$/g, "");
    return (
      host === "localhost" || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host)
    );
  } catch {
    return false;
  }
}
