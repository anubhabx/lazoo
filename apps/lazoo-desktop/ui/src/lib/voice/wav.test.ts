import { describe, expect, test } from "vitest";

import {
  concatFloat32,
  encodeWav,
  floatToPcm16,
  levelFromRms,
  resample,
  rms,
} from "./wav";

const text = (bytes: Uint8Array, start: number, end: number) =>
  String.fromCharCode(...bytes.slice(start, end));

describe("encodeWav", () => {
  test("writes a byte-exact 44 byte RIFF header for 16 kHz mono PCM16", () => {
    const wav = encodeWav(new Float32Array([0, 0.5, -0.5, 1, -1]));
    const view = new DataView(wav.buffer);
    expect(wav.byteLength).toBe(44 + 5 * 2);
    expect(text(wav, 0, 4)).toBe("RIFF");
    expect(view.getUint32(4, true)).toBe(36 + 10);
    expect(text(wav, 8, 12)).toBe("WAVE");
    expect(text(wav, 12, 16)).toBe("fmt ");
    expect(view.getUint32(16, true)).toBe(16);
    expect(view.getUint16(20, true)).toBe(1); // PCM
    expect(view.getUint16(22, true)).toBe(1); // mono
    expect(view.getUint32(24, true)).toBe(16_000);
    expect(view.getUint32(28, true)).toBe(32_000); // byte rate
    expect(view.getUint16(32, true)).toBe(2); // block align
    expect(view.getUint16(34, true)).toBe(16); // bits
    expect(text(wav, 36, 40)).toBe("data");
    expect(view.getUint32(40, true)).toBe(10);
  });

  test("writes little-endian samples, clamped to the 16-bit range", () => {
    const wav = encodeWav(new Float32Array([0, 0.5, -0.5, 1, -1, 2, -3]));
    const view = new DataView(wav.buffer);
    const samples = Array.from({ length: 7 }, (_, index) =>
      view.getInt16(44 + index * 2, true),
    );
    expect(samples).toEqual([0, 16384, -16384, 32767, -32768, 32767, -32768]);
  });

  test("an empty clip is a valid header with no data", () => {
    const wav = encodeWav(new Float32Array(0));
    expect(wav.byteLength).toBe(44);
    expect(new DataView(wav.buffer).getUint32(40, true)).toBe(0);
  });
});

describe("audio helpers", () => {
  test("floatToPcm16 rounds to the nearest step", () => {
    expect(Array.from(floatToPcm16(new Float32Array([0.25])))).toEqual([8192]);
  });

  test("concatFloat32 joins blocks in order", () => {
    const joined = concatFloat32([
      new Float32Array([1, 2]),
      new Float32Array([3]),
    ]);
    expect(Array.from(joined)).toEqual([1, 2, 3]);
  });

  test("resample converts the length by the rate ratio", () => {
    const input = new Float32Array(480).fill(0.5);
    const out = resample(input, 48_000, 16_000);
    expect(out.length).toBe(160);
    expect(out[10]).toBeCloseTo(0.5);
    expect(resample(input, 16_000, 16_000)).toBe(input);
  });

  test("rms and the meter level", () => {
    expect(rms(new Float32Array([0.5, -0.5, 0.5, -0.5]))).toBeCloseTo(0.5);
    expect(levelFromRms(0)).toBe(0);
    expect(levelFromRms(0.0001)).toBe(0);
    expect(levelFromRms(1)).toBe(1);
    const quiet = levelFromRms(0.01);
    const loud = levelFromRms(0.1);
    expect(loud).toBeGreaterThan(quiet);
    expect(quiet).toBeGreaterThan(0);
    expect(loud).toBeLessThan(1);
  });
});
