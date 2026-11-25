// One voice controller for the main window: the voice state read model, the one
// microphone (composer and push-to-talk share it), the global push-to-talk
// hotkey, the overlay, and the read-aloud speech queue. Rust owns engines,
// secrets and the hotkey; this owns what the person sees and hears.

import type { Bridge } from "../bridge";
import { asFailure } from "../bridge";
import type { RpcFailure } from "../protocol";
import type { Store } from "../store";
import { createMicRecorder, type Recorder } from "./capture";
import {
  SpeechQueue,
  createBrowserSpeechDeps,
  type SpeechDeps,
} from "./speech";
import type {
  OverlayUpdate,
  PttState,
  TranscribePurpose,
  TranscribeResult,
  VoiceSettings,
  VoiceState,
  WhisperModel,
  WhisperProgress,
} from "./types";

export type CapturePhase = "idle" | "recording" | "transcribing";
export type CaptureOwner = "composer" | "ptt";

export interface VoiceSnapshot {
  state: VoiceState | null;
  stateError: RpcFailure | null;
  phase: CapturePhase;
  owner: CaptureOwner | null;
  /** `now()` at the moment recording began. */
  startedAt: number | null;
  speaking: boolean;
  canReplay: boolean;
  /** Dictated text the focused app refused; the composer takes it. */
  pendingInsert: { id: number; text: string } | null;
  /** Bumped when the recording length limit is reached. */
  limitHits: number;
}

export interface VoiceDeps {
  createRecorder: () => Recorder;
  speech: SpeechDeps;
  clipboard: (text: string) => Promise<void>;
  now: () => number;
}

/** Rust refuses audio longer than five minutes; stop a little before that. */
export const RECORDING_LIMIT_MS = 290_000;
const OVERLAY_LEVEL_MS = 66;
const DONE_FLASH_MS = 900;
const ERROR_FLASH_MS = 3000;

export type SaveOutcome = { ok: true } | { ok: false; failure: RpcFailure };

export class VoiceController {
  private snapshot: VoiceSnapshot = {
    state: null,
    stateError: null,
    phase: "idle",
    owner: null,
    startedAt: null,
    speaking: false,
    canReplay: false,
    pendingInsert: null,
    limitHits: 0,
  };
  private readonly listeners = new Set<() => void>();
  private readonly levelListeners = new Set<(level: number) => void>();
  private readonly deps: VoiceDeps;
  private readonly speech: SpeechQueue;
  private recorder: Recorder | null = null;
  private limitTimer: ReturnType<typeof setTimeout> | null = null;
  private overlayTimer: ReturnType<typeof setTimeout> | null = null;
  private lastOverlayLevelAt = 0;
  private pttHeld = false;
  private pttActive = false;
  private insertSeq = 0;
  private replay: (() => void) | null = null;
  private stopPtt: (() => void) | null = null;
  private stopSpeechWatch: (() => void) | null = null;
  private started = false;

  constructor(
    private readonly bridge: Bridge,
    private readonly store: Store,
    deps: Partial<VoiceDeps> = {},
  ) {
    this.deps = {
      createRecorder: createMicRecorder,
      speech: createBrowserSpeechDeps((text) => bridge.voice.speak(text)),
      clipboard: (text) => bridge.voice.copyText(text),
      now: () => Date.now(),
      ...deps,
    };
    this.speech = new SpeechQueue(this.deps.speech, (failure) => {
      this.store.toast({
        tone: "error",
        title: failure.code,
        detail: `${failure.message} The reply stays as text.`,
      });
    });
  }

  // ---- subscription ---------------------------------------------------

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;

  private set(patch: Partial<VoiceSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch };
    for (const listener of this.listeners) listener();
  }

  /** The live microphone level, 0..1, about 16 times a second while recording. */
  onLevel = (listener: (level: number) => void) => {
    this.levelListeners.add(listener);
    return () => {
      this.levelListeners.delete(listener);
    };
  };

  // ---- lifecycle ------------------------------------------------------

  async start() {
    if (this.started) return;
    this.started = true;
    window.addEventListener("keydown", this.onKeyDown, true);
    this.stopSpeechWatch = this.speech.subscribe(() =>
      this.set({ speaking: this.speech.getSnapshot().speaking }),
    );
    void this.bridge.voice
      .onPtt((state) => void this.onPtt(state))
      .then((stop) => {
        if (this.started) this.stopPtt = stop;
        else stop();
      })
      .catch(() => undefined);
    await this.refresh();
  }

  dispose() {
    this.started = false;
    window.removeEventListener("keydown", this.onKeyDown, true);
    this.stopPtt?.();
    this.stopPtt = null;
    this.stopSpeechWatch?.();
    this.stopSpeechWatch = null;
    this.speech.stop();
    this.recorder?.cancel();
    this.recorder = null;
    if (this.limitTimer) clearTimeout(this.limitTimer);
    if (this.overlayTimer) clearTimeout(this.overlayTimer);
    this.pttHeld = false;
    this.pttActive = false;
    this.set({ phase: "idle", owner: null, startedAt: null, speaking: false });
  }

  private onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && this.speech.getSnapshot().speaking)
      this.speech.stop();
  };

  // ---- state and settings ----------------------------------------------

  async refresh(): Promise<void> {
    try {
      const state = await this.bridge.voice.state();
      this.set({ state, stateError: null });
    } catch (error) {
      this.set({ stateError: asFailure(error) });
    }
  }

  private async mutate(
    run: () => Promise<VoiceState>,
  ): Promise<RpcFailure | null> {
    try {
      this.set({ state: await run(), stateError: null });
      return null;
    } catch (error) {
      const failure = asFailure(error);
      // A stale revision: the state is re-read so the page shows what is true.
      if (failure.code === "LZ-VOICE-REVISION") await this.refresh();
      return failure;
    }
  }

  /** Revision-checked: pass the revision the settings were edited from. */
  async saveSettings(
    settings: VoiceSettings,
    revision?: number,
  ): Promise<SaveOutcome> {
    const base = revision ?? this.snapshot.state?.revision ?? 0;
    const failure = await this.mutate(() =>
      this.bridge.voice.settingsSet(settings, base),
    );
    return failure ? { ok: false, failure } : { ok: true };
  }

  setKey(key: string) {
    return this.mutate(() => this.bridge.voice.keySet(key));
  }

  clearKey() {
    return this.mutate(() => this.bridge.voice.keyClear());
  }

  installModel(model: WhisperModel, onProgress: (p: WhisperProgress) => void) {
    return this.mutate(() =>
      this.bridge.voice.whisperInstall(model, onProgress),
    );
  }

  removeModel(model: WhisperModel | null) {
    return this.mutate(() => this.bridge.voice.whisperRemove(model));
  }

  /** Flip one setting from outside the Voice page (agent bar, command palette). */
  async patchSettings(
    patch: (settings: VoiceSettings) => VoiceSettings,
  ): Promise<boolean> {
    const state = this.snapshot.state;
    if (!state) return false;
    const outcome = await this.saveSettings(
      patch(state.settings),
      state.revision,
    );
    if (!outcome.ok)
      this.store.toast({
        tone: "error",
        title: outcome.failure.code,
        detail: outcome.failure.message,
      });
    return outcome.ok;
  }

  async setReadReplies(on: boolean) {
    if (!on) this.speech.stop();
    return this.patchSettings((settings) => ({ ...settings, readReplies: on }));
  }

  // ---- the microphone -------------------------------------------------

  /** Opens the microphone. Returns a failure, or null once recording. */
  async beginRecording(owner: CaptureOwner): Promise<RpcFailure | null> {
    if (this.snapshot.phase !== "idle")
      return { code: "LZ-VOICE-BUSY", message: "Voice is already in use." };
    const recorder = this.deps.createRecorder();
    this.recorder = recorder;
    this.set({ phase: "recording", owner, startedAt: this.deps.now() });
    try {
      await recorder.start((level) => this.emitLevel(level));
    } catch (error) {
      if (this.recorder === recorder) this.recorder = null;
      this.set({ phase: "idle", owner: null, startedAt: null });
      return asFailure(error);
    }
    if (this.recorder !== recorder) {
      // Cancelled while the microphone was still opening.
      recorder.cancel();
      return null;
    }
    this.limitTimer = setTimeout(() => this.onLimit(), RECORDING_LIMIT_MS);
    return null;
  }

  private onLimit() {
    this.limitTimer = null;
    if (this.snapshot.phase !== "recording") return;
    if (this.snapshot.owner === "ptt") void this.releasePtt();
    else this.set({ limitHits: this.snapshot.limitHits + 1 });
  }

  cancelRecording() {
    if (this.snapshot.phase !== "recording") return;
    const recorder = this.recorder;
    this.recorder = null;
    if (this.limitTimer) clearTimeout(this.limitTimer);
    this.limitTimer = null;
    recorder?.cancel();
    this.set({ phase: "idle", owner: null, startedAt: null });
  }

  /**
   * Ends recording and transcribes it. Null when the clip was under 300 ms
   * (discarded without a word). Throws an `RpcFailure` when transcription fails.
   */
  async finishRecording(
    purpose: TranscribePurpose,
  ): Promise<TranscribeResult | null> {
    const recorder = this.recorder;
    if (this.snapshot.phase !== "recording" || !recorder) return null;
    this.recorder = null;
    if (this.limitTimer) clearTimeout(this.limitTimer);
    this.limitTimer = null;
    let clip;
    try {
      clip = await recorder.stop();
    } catch (error) {
      this.set({ phase: "idle", owner: null, startedAt: null });
      throw asFailure(error);
    }
    if (!clip) {
      this.set({ phase: "idle", owner: null, startedAt: null });
      return null;
    }
    this.set({ phase: "transcribing", startedAt: null });
    try {
      return await this.bridge.voice.transcribe(clip.wav, purpose);
    } catch (error) {
      throw asFailure(error);
    } finally {
      this.set({ phase: "idle", owner: null });
    }
  }

  private emitLevel(level: number) {
    for (const listener of this.levelListeners) listener(level);
    if (this.snapshot.owner !== "ptt") return;
    const now = this.deps.now();
    if (now - this.lastOverlayLevelAt < OVERLAY_LEVEL_MS) return;
    this.lastOverlayLevelAt = now;
    void this.overlay({ state: "recording", detail: null, level });
  }

  // ---- composer hand-off ---------------------------------------------

  takeInsert(id: number) {
    if (this.snapshot.pendingInsert?.id === id)
      this.set({ pendingInsert: null });
  }

  private offerInsert(text: string) {
    this.insertSeq += 1;
    this.set({ pendingInsert: { id: this.insertSeq, text } });
    this.store.focusTab("lead");
  }

  // ---- global push-to-talk ---------------------------------------------

  private async overlay(update: OverlayUpdate) {
    try {
      await this.bridge.voice.overlay(update);
    } catch {
      // The pill is a courtesy; dictation does not depend on it.
    }
  }

  private flash(state: "done" | "error", detail: string) {
    if (this.overlayTimer) clearTimeout(this.overlayTimer);
    void this.overlay({ state, detail, level: null });
    this.overlayTimer = setTimeout(
      () => {
        this.overlayTimer = null;
        void this.overlay({ state: "hidden", detail: null, level: null });
      },
      state === "done" ? DONE_FLASH_MS : ERROR_FLASH_MS,
    );
  }

  async onPtt(state: PttState) {
    if (state === "released") {
      this.pttHeld = false;
      if (this.pttActive) await this.releasePtt();
      return;
    }
    if (this.pttHeld) return;
    const settings = this.snapshot.state?.settings;
    if (this.snapshot.phase !== "idle") return;
    if (!settings || settings.stt.engine === "off") {
      this.flash("error", "Speech to text is off");
      return;
    }
    this.pttHeld = true;
    if (this.overlayTimer) clearTimeout(this.overlayTimer);
    this.overlayTimer = null;
    this.lastOverlayLevelAt = 0;
    void this.overlay({ state: "recording", detail: null, level: 0 });
    const failure = await this.beginRecording("ptt");
    if (failure) {
      this.pttHeld = false;
      this.flash("error", failure.message);
      return;
    }
    this.pttActive = true;
    // Released while the microphone was still opening.
    if (!this.pttHeld) await this.releasePtt();
  }

  private async releasePtt() {
    this.pttActive = false;
    void this.overlay({ state: "transcribing", detail: null, level: null });
    try {
      const result = await this.finishRecording("dictation");
      if (!result) {
        void this.overlay({ state: "hidden", detail: null, level: null });
        return;
      }
      const text = result.text.trim();
      if (!text) {
        // Windows hands a denied desktop app silent audio; say which it is.
        this.flash(
          "error",
          this.snapshot.state?.microphoneBlocked
            ? "Windows is blocking the microphone"
            : "Nothing heard",
        );
        return;
      }
      await this.deliver(text);
    } catch (error) {
      this.flash("error", asFailure(error).message);
    }
  }

  private conversationOpen(): boolean {
    const snapshot = this.store.getSnapshot();
    return snapshot.route.kind === "conversation" && !!snapshot.conversation;
  }

  private async copy(text: string): Promise<boolean> {
    try {
      await this.deps.clipboard(text);
      return true;
    } catch {
      return false;
    }
  }

  private async deliver(text: string) {
    const target = this.snapshot.state?.settings.dictationTarget ?? "insert";
    if (target === "clipboard") {
      if (await this.copy(text)) this.flash("done", "Copied");
      else this.flash("error", "Could not copy to the clipboard");
      return;
    }
    if (target === "conversation") {
      if (!this.conversationOpen()) {
        this.store.toast({
          tone: "info",
          title: "No conversation is open",
          detail: "The dictation was copied instead.",
        });
        if (await this.copy(text)) this.flash("done", "Copied");
        else this.flash("error", "Could not copy to the clipboard");
        return;
      }
      const lead = this.store.getSnapshot().conversation?.lead;
      const ok = await this.store.submitTurn(text, lead?.authority ?? "ask");
      if (ok) this.flash("done", "Sent");
      else {
        const failure = this.store.getSnapshot().composerError;
        this.flash("error", failure?.message ?? "Could not send");
      }
      return;
    }
    try {
      await this.bridge.voice.insertText(text);
      this.flash("done", "Inserted");
    } catch (error) {
      const failure = asFailure(error);
      if (failure.code !== "LZ-VOICE-INSERT-REFUSED") {
        this.flash("error", failure.message);
        return;
      }
      if (this.conversationOpen()) {
        this.offerInsert(text);
        this.flash("done", "Inserted");
      } else if (await this.copy(text)) {
        this.flash("done", "Copied");
      } else {
        this.flash("error", failure.message);
      }
    }
  }

  // ---- speech -----------------------------------------------------------

  /** Queues text for the configured voice. Nothing happens when speech is off. */
  speak(text: string, override?: Partial<VoiceSettings["tts"]>) {
    const tts = this.snapshot.state?.settings.tts;
    if (!tts) return;
    const merged = { ...tts, ...override };
    if (merged.engine === "off") return;
    this.speech.configure({
      engine: merged.engine,
      voice: merged.voice,
      rate: merged.rate,
    });
    this.speech.enqueue(text);
  }

  stopSpeaking() {
    this.speech.stop();
  }

  /** The conversation page registers how to speak its latest reply again. */
  registerReplay(replay: (() => void) | null) {
    this.replay = replay;
    this.set({ canReplay: replay !== null });
  }

  replayLast() {
    this.replay?.();
  }
}
