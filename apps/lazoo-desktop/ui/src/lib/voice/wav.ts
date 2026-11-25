// WAV encoding for the capture path: RIFF, PCM 16-bit, mono. The Rust side
// accepts only this format (`LZ-VOICE-AUDIO` otherwise), at 16 kHz.

export const SAMPLE_RATE = 16_000;
export const MIN_CLIP_MS = 300;
const HEADER_BYTES = 44;

/** Float samples in [-1, 1] to signed 16-bit, clamped and rounded. */
export function floatToPcm16(samples: Float32Array): Int16Array {
  const out = new Int16Array(samples.length);
  for (let index = 0; index < samples.length; index += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[index]!));
    out[index] = Math.round(clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff);
  }
  return out;
}

export function encodeWav(
  samples: Float32Array,
  sampleRate: number = SAMPLE_RATE,
): Uint8Array {
  const pcm = floatToPcm16(samples);
  const dataBytes = pcm.length * 2;
  const buffer = new ArrayBuffer(HEADER_BYTES + dataBytes);
  const view = new DataView(buffer);
  const ascii = (offset: number, text: string) => {
    for (let index = 0; index < text.length; index += 1)
      view.setUint8(offset + index, text.charCodeAt(index));
  };
  ascii(0, "RIFF");
  view.setUint32(4, 36 + dataBytes, true);
  ascii(8, "WAVE");
  ascii(12, "fmt ");
  view.setUint32(16, 16, true); // fmt chunk size
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  ascii(36, "data");
  view.setUint32(40, dataBytes, true);
  for (let index = 0; index < pcm.length; index += 1)
    view.setInt16(HEADER_BYTES + index * 2, pcm[index]!, true);
  return new Uint8Array(buffer);
}

export function concatFloat32(parts: Float32Array[]): Float32Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Float32Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** Linear resampling, for a context that would not open at 16 kHz. */
export function resample(
  samples: Float32Array,
  from: number,
  to: number = SAMPLE_RATE,
): Float32Array {
  if (from === to || samples.length === 0) return samples;
  const length = Math.max(1, Math.round((samples.length * to) / from));
  const out = new Float32Array(length);
  const ratio = from / to;
  for (let index = 0; index < length; index += 1) {
    const position = index * ratio;
    const base = Math.min(Math.floor(position), samples.length - 1);
    const next = Math.min(base + 1, samples.length - 1);
    const fraction = position - base;
    out[index] = samples[base]! * (1 - fraction) + samples[next]! * fraction;
  }
  return out;
}

/** Root mean square of a block. */
export function rms(samples: Float32Array): number {
  if (!samples.length) return 0;
  let sum = 0;
  for (const sample of samples) sum += sample * sample;
  return Math.sqrt(sum / samples.length);
}

/** RMS to a 0..1 meter position: -60 dBFS is empty, -12 dBFS is full. */
export function levelFromRms(value: number): number {
  if (value <= 0) return 0;
  const db = 20 * Math.log10(value);
  return Math.max(0, Math.min(1, (db + 60) / 48));
}
