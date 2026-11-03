// Microphone capture to a 16 kHz mono PCM16 WAV, held in memory only.
//
// Under the app's CSP (`script-src 'self'`) an AudioWorklet cannot load from a
// Blob, so the processor is a real file (capture.worklet.js) that Vite emits
// next to the bundle and `audioWorklet.addModule` loads from the app origin.
// (`&no-inline` matters: Vite would otherwise inline this small file as a data:
// URL, which the CSP blocks.) If that load fails anyway, a ScriptProcessorNode (deprecated, but needs no
// script file) does the same job.

import type { RpcFailure } from "../protocol";
import workletUrl from "./capture.worklet.js?url&no-inline";
import type { AudioClip } from "./types";
import {
  MIN_CLIP_MS,
  SAMPLE_RATE,
  concatFloat32,
  encodeWav,
  levelFromRms,
  resample,
} from "./wav";

export interface Recorder {
  /** Opens the microphone. `onLevel` gets a 0..1 meter level about 16 times a second. */
  start(onLevel: (level: number) => void): Promise<void>;
  /** Ends capture, releases the microphone, and returns the clip (null under 300 ms). */
  stop(): Promise<AudioClip | null>;
  /** Ends capture and discards the audio. */
  cancel(): void;
}

const LEVEL_INTERVAL_MS = 60;

export function micFailure(error: unknown): RpcFailure {
  const name = (error as { name?: string } | null)?.name;
  if (name === "NotAllowedError" || name === "SecurityError")
    return {
      code: "LZ-VOICE-MIC",
      message:
        "Microphone access is blocked. Allow it in Windows privacy settings.",
    };
  if (name === "NotFoundError" || name === "OverconstrainedError")
    return { code: "LZ-VOICE-MIC", message: "No microphone was found." };
  // WebView2 reports a Windows privacy block as a device that cannot start.
  if (name === "NotReadableError")
    return {
      code: "LZ-VOICE-MIC",
      message:
        "The microphone could not start. Another app may hold it, or Windows may be blocking it (Settings > Voice says which).",
    };
  return {
    code: "LZ-VOICE-MIC",
    message:
      error instanceof Error && error.message
        ? error.message
        : "The microphone could not be opened.",
  };
}

export function createMicRecorder(): Recorder {
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let node: AudioNode | null = null;
  let sink: GainNode | null = null;
  let worklet: AudioWorkletNode | null = null;
  let parts: Float32Array[] = [];
  let sumSquares = 0;
  let counted = 0;
  let lastLevelAt = 0;
  let onLevel: (level: number) => void = () => {};
  let flushed: (() => void) | null = null;

  function push(block: Float32Array) {
    parts.push(block);
    for (const sample of block) sumSquares += sample * sample;
    counted += block.length;
    const now = performance.now();
    if (now - lastLevelAt >= LEVEL_INTERVAL_MS && counted > 0) {
      onLevel(levelFromRms(Math.sqrt(sumSquares / counted)));
      sumSquares = 0;
      counted = 0;
      lastLevelAt = now;
    }
  }

  function release() {
    for (const track of stream?.getTracks() ?? []) track.stop();
    stream = null;
  }

  async function teardown() {
    release();
    try {
      node?.disconnect();
      sink?.disconnect();
    } catch {
      // Already disconnected.
    }
    const closing = context;
    node = null;
    sink = null;
    worklet = null;
    context = null;
    await closing?.close().catch(() => undefined);
  }

  return {
    async start(levelHandler) {
      onLevel = levelHandler;
      parts = [];
      sumSquares = 0;
      counted = 0;
      lastLevelAt = 0;
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: {
            channelCount: 1,
            echoCancellation: true,
            noiseSuppression: true,
          },
        });
        try {
          context = new AudioContext({ sampleRate: SAMPLE_RATE });
        } catch {
          // A device that will not open at 16 kHz: resampled on stop.
          context = new AudioContext();
        }
        if (context.state === "suspended") await context.resume();
        const source = context.createMediaStreamSource(stream);
        sink = context.createGain();
        sink.gain.value = 0;
        sink.connect(context.destination);
        try {
          await context.audioWorklet.addModule(workletUrl);
          worklet = new AudioWorkletNode(context, "lazoo-capture", {
            numberOfInputs: 1,
            numberOfOutputs: 1,
            channelCount: 1,
          });
          worklet.port.onmessage = (event: MessageEvent) => {
            if (event.data instanceof Float32Array) push(event.data);
            else if (event.data === "flushed") flushed?.();
          };
          node = worklet;
        } catch {
          const processor = context.createScriptProcessor(2048, 1, 1);
          processor.onaudioprocess = (event) =>
            push(new Float32Array(event.inputBuffer.getChannelData(0)));
          node = processor;
        }
        source.connect(node);
        node.connect(sink);
      } catch (error) {
        await teardown();
        throw micFailure(error);
      }
    },

    async stop() {
      const rate = context?.sampleRate ?? SAMPLE_RATE;
      // The microphone is released first: the light goes off when recording ends.
      release();
      if (worklet) {
        const waiting = new Promise<void>((resolve) => {
          flushed = resolve;
          setTimeout(resolve, 250);
        });
        worklet.port.postMessage("flush");
        await waiting;
        flushed = null;
      }
      await teardown();
      const samples = resample(concatFloat32(parts), rate, SAMPLE_RATE);
      parts = [];
      const durationMs = (samples.length / SAMPLE_RATE) * 1000;
      if (durationMs < MIN_CLIP_MS) return null;
      return { wav: encodeWav(samples, SAMPLE_RATE), durationMs };
    },

    cancel() {
      parts = [];
      void teardown();
    },
  };
}
