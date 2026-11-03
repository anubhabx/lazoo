// The speech queue for read-aloud. Chunks play strictly in order. `system`
// speaks through WebView2's `speechSynthesis`; `endpoint` asks Rust for audio
// (`voice_speak`) and plays the bytes from a blob URL. For the endpoint, the
// chunk being played plus at most two more are fetched ahead (backpressure), so
// a long reply never queues dozens of paid requests it may never play.

import { asFailure } from "../bridge";
import type { RpcFailure } from "../protocol";

export const FETCH_AHEAD = 2;

export interface Playback {
  done: Promise<void>;
  cancel(): void;
}

export interface SpeechDeps {
  speakSystem(text: string, options: { voice: string; rate: number }): Playback;
  fetchAudio(text: string): Promise<ArrayBuffer>;
  playAudio(audio: ArrayBuffer, rate: number): Playback;
}

export interface SpeechConfig {
  engine: "system" | "endpoint";
  voice: string;
  rate: number;
}

export interface SpeechSnapshot {
  speaking: boolean;
  pending: number;
}

interface Item {
  text: string;
  audio: Promise<ArrayBuffer> | null;
}

export class SpeechQueue {
  private items: Item[] = [];
  private playing: Playback | null = null;
  private running = false;
  private generation = 0;
  private config: SpeechConfig = { engine: "system", voice: "", rate: 1 };
  private readonly listeners = new Set<() => void>();
  private snapshot: SpeechSnapshot = { speaking: false, pending: 0 };

  constructor(
    private readonly deps: SpeechDeps,
    private readonly onError: (failure: RpcFailure) => void = () => {},
  ) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;

  configure(config: SpeechConfig) {
    this.config = config;
  }

  enqueue(text: string) {
    if (!text.trim()) return;
    this.items.push({ text, audio: null });
    this.publish();
    this.prefetch();
    void this.run();
  }

  /** Stops what is playing and drops everything queued. */
  stop() {
    this.generation += 1;
    this.items = [];
    const playing = this.playing;
    this.playing = null;
    playing?.cancel();
    this.running = false;
    this.publish();
  }

  private publish() {
    const speaking = this.items.length > 0 || this.playing !== null;
    const pending = this.items.length;
    if (
      speaking === this.snapshot.speaking &&
      pending === this.snapshot.pending
    )
      return;
    this.snapshot = { speaking, pending };
    for (const listener of this.listeners) listener();
  }

  /** Endpoint only: start fetching the head and the next FETCH_AHEAD chunks. */
  private prefetch() {
    if (this.config.engine !== "endpoint") return;
    for (const item of this.items.slice(0, FETCH_AHEAD + 1)) {
      if (item.audio) continue;
      const request = this.deps.fetchAudio(item.text);
      // A failure is reported when the chunk's turn comes, not as an
      // unhandled rejection while it waits.
      request.catch(() => undefined);
      item.audio = request;
    }
  }

  private async run() {
    if (this.running) return;
    this.running = true;
    const generation = this.generation;
    try {
      while (this.items.length && generation === this.generation) {
        const item = this.items[0]!;
        this.prefetch();
        let playback: Playback;
        if (this.config.engine === "endpoint") {
          const audio = await item.audio!;
          if (generation !== this.generation) return;
          playback = this.deps.playAudio(audio, this.config.rate);
        } else {
          playback = this.deps.speakSystem(item.text, {
            voice: this.config.voice,
            rate: this.config.rate,
          });
        }
        this.playing = playback;
        this.publish();
        await playback.done;
        if (generation !== this.generation) return;
        this.playing = null;
        this.items.shift();
        this.publish();
      }
    } catch (error) {
      if (generation === this.generation) {
        this.stop();
        this.onError(asFailure(error));
      }
    } finally {
      if (generation === this.generation) this.running = false;
    }
  }
}

// ---- Real playback ---------------------------------------------------------

export function createBrowserSpeechDeps(
  fetchAudio: (text: string) => Promise<ArrayBuffer>,
): SpeechDeps {
  return {
    fetchAudio,
    speakSystem(text, { voice, rate }) {
      const synth = window.speechSynthesis;
      if (!synth) throw unavailable("This window has no system speech.");
      const utterance = new SpeechSynthesisUtterance(text);
      utterance.rate = rate;
      if (voice) {
        const match = synth.getVoices().find((item) => item.name === voice);
        if (match) utterance.voice = match;
      }
      const done = new Promise<void>((resolve, reject) => {
        utterance.onend = () => resolve();
        utterance.onerror = (event) => {
          // Cancelling is how Stop works; it is not a failure.
          if (event.error === "canceled" || event.error === "interrupted")
            resolve();
          else reject(unavailable(`System speech failed (${event.error}).`));
        };
      });
      synth.speak(utterance);
      return { done, cancel: () => synth.cancel() };
    },
    playAudio(audio, rate) {
      const url = URL.createObjectURL(
        new Blob([audio], { type: "audio/mpeg" }),
      );
      const element = new Audio(url);
      element.playbackRate = rate;
      let settle: { resolve: () => void; reject: (reason: RpcFailure) => void };
      const done = new Promise<void>((resolve, reject) => {
        settle = { resolve, reject };
        element.onended = () => resolve();
        element.onerror = () =>
          reject(unavailable("The audio could not play."));
      }).finally(() => URL.revokeObjectURL(url));
      element.play().catch(() => {
        settle.reject(unavailable("The audio could not play."));
      });
      return {
        done,
        cancel: () => {
          element.pause();
          settle.resolve();
        },
      };
    },
  };
}

function unavailable(message: string): RpcFailure {
  return { code: "LZ-VOICE-PLAYBACK", message };
}

// ---- Text preparation -------------------------------------------------------

/** Markdown noise out, so the voice reads sentences and not syntax. */
export function toSpeakable(chunk: string): string {
  return chunk
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/`+([^`]*)`+/g, "$1")
    .replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*[-*+]\s+/gm, "")
    .replace(/(\*\*|__|\*|_)(\S(?:[^*_]*\S)?)\1/g, "$2")
    .replace(/\s+/g, " ")
    .trim();
}
