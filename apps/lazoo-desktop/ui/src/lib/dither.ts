// Ordered (Bayer) dithering: the app's one decorative device. Agent avatars are
// 10×10 symmetric glyphs seeded from the agent's name and shaded by a dither
// ramp; states animate the threshold field, never the layout.

export const GRID = 10;

/** 4×4 Bayer matrix, normalised to (0, 1). */
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5].map(
  (value) => (value + 0.5) / 16,
);

export function bayer(x: number, y: number): number {
  return BAYER[(y & 3) * 4 + (x & 3)]!;
}

/** FNV-1a: stable across sessions and platforms. */
export function hash(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

function prng(seed: number) {
  let state = seed || 1;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface GlyphModel {
  /** 1 inside the glyph, 0 outside, row-major GRID×GRID. */
  mask: Uint8Array;
  /** Base tone per cell in [0, 1] before animation. */
  tone: Float32Array;
}

const models = new Map<string, GlyphModel>();

/** The agent's shape: mirrored, blobby, never empty, cached per name. */
export function glyphModel(name: string): GlyphModel {
  const cached = models.get(name);
  if (cached) return cached;
  const random = prng(hash(name.trim().toLowerCase() || "?"));
  const half = Math.ceil(GRID / 2);
  let mask = new Uint8Array(GRID * GRID);
  // Seeded fill, denser toward the centre so shapes read as one figure.
  for (let y = 1; y < GRID - 1; y++) {
    for (let x = 1; x < half; x++) {
      const centre = 1 - Math.abs(y - (GRID - 1) / 2) / (GRID / 2);
      const inward = x / half;
      const p = 0.18 + 0.45 * centre + 0.32 * inward;
      const on = random() < p ? 1 : 0;
      mask[y * GRID + x] = on;
      mask[y * GRID + (GRID - 1 - x)] = on;
    }
  }
  // One smoothing pass (cellular automaton) removes salt-and-pepper noise.
  const smoothed = new Uint8Array(GRID * GRID);
  for (let y = 0; y < GRID; y++) {
    for (let x = 0; x < GRID; x++) {
      let around = 0;
      for (let dy = -1; dy <= 1; dy++)
        for (let dx = -1; dx <= 1; dx++) {
          if (!dx && !dy) continue;
          const nx = x + dx;
          const ny = y + dy;
          if (nx >= 0 && ny >= 0 && nx < GRID && ny < GRID)
            around += mask[ny * GRID + nx]!;
        }
      const self = mask[y * GRID + x]!;
      smoothed[y * GRID + x] = around >= 5 || (self && around >= 2) ? 1 : 0;
    }
  }
  mask = smoothed;
  let filled = 0;
  for (const cell of mask) filled += cell;
  // A sparse result falls back to a solid core so no agent is invisible.
  if (filled < 24) {
    for (let y = 3; y < 7; y++)
      for (let x = 3; x < 7; x++) mask[y * GRID + x] = 1;
  }
  // Light comes from a seeded corner; inside cells ramp from lit to shaded.
  const angle = random() * Math.PI * 2;
  const dx = Math.cos(angle);
  const dy = Math.sin(angle);
  const tone = new Float32Array(GRID * GRID);
  const c = (GRID - 1) / 2;
  for (let y = 0; y < GRID; y++) {
    for (let x = 0; x < GRID; x++) {
      const along = ((x - c) * dx + (y - c) * dy) / (GRID * 0.7);
      // Lit side solid, shaded side dithered; the ground stays clean.
      const value = mask[y * GRID + x]
        ? 0.8 + 0.3 * Math.max(-1, Math.min(1, along))
        : 0;
      tone[y * GRID + x] = value;
    }
  }
  const model = { mask, tone };
  models.set(name, model);
  return model;
}

/** What an agent is doing, as far as the avatar is concerned. */
export type AvatarMotion = "still" | "working" | "thinking" | "attention";

export function avatarMotion(tone: string | undefined): AvatarMotion {
  switch (tone) {
    case "running":
    case "busy":
      return "working";
    case "starting":
    case "waiting":
      return "thinking";
    case "attention":
    case "needs-approval":
    case "needs-input":
      return "attention";
    default:
      return "still";
  }
}

export interface Ink {
  /** RGB of the harness hue. */
  hue: [number, number, number];
  /** RGB of the surface behind the avatar. */
  ground: [number, number, number];
  /** RGB of the accent, used by "attention". */
  accent: [number, number, number];
}

/**
 * Paint one frame. `t` is seconds; cells are drawn at integer device-pixel
 * edges so the glyph stays crisp at any display scale.
 */
export function paintGlyph(
  ctx: CanvasRenderingContext2D,
  model: GlyphModel,
  ink: Ink,
  motion: AvatarMotion,
  t: number,
  dim = false,
) {
  const width = ctx.canvas.width;
  const height = ctx.canvas.height;
  const [gr, gg, gb] = ink.ground;
  ctx.fillStyle = `rgb(${gr} ${gg} ${gb})`;
  ctx.fillRect(0, 0, width, height);
  const blink = motion === "attention" && Math.floor(t * 1.6) % 2 === 0;
  for (let y = 0; y < GRID; y++) {
    const y0 = Math.floor((y * height) / GRID);
    const y1 = Math.floor(((y + 1) * height) / GRID);
    for (let x = 0; x < GRID; x++) {
      const i = y * GRID + x;
      let value = model.tone[i]!;
      const inside = model.mask[i] === 1;
      if (motion === "working") {
        // A diagonal band sweeps across the glyph.
        value += 0.3 * Math.sin(t * 5.2 - (x + y) * 0.55);
      } else if (motion === "thinking") {
        // A slow, scattered shimmer.
        value += 0.22 * Math.sin(t * 2.4 + bayer(x + 1, y + 2) * 6.283);
      }
      if (dim) value *= 0.7;
      // "Needs you" lights a dithered accent frame; the glyph keeps its hue.
      const frame =
        blink &&
        !inside &&
        (x === 0 || y === 0 || x === GRID - 1 || y === GRID - 1);
      if (frame) value = 0.6;
      if (value <= bayer(x, y)) continue;
      const strong = inside && value > 0.78;
      const colour = frame ? ink.accent : ink.hue;
      const mix = frame ? 0.9 : inside ? (strong ? 1 : 0.82) : 0.32;
      const r = Math.round(gr + (colour[0] - gr) * mix);
      const g = Math.round(gg + (colour[1] - gg) * mix);
      const b = Math.round(gb + (colour[2] - gb) * mix);
      ctx.fillStyle = `rgb(${r} ${g} ${b})`;
      const x0 = Math.floor((x * width) / GRID);
      const x1 = Math.floor(((x + 1) * width) / GRID);
      ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
    }
  }
}

/** Parse `#rrggbb` or `rgb(r g b)` / `rgb(r, g, b)` into RGB. */
export function parseColour(
  value: string,
  fallback: [number, number, number],
): [number, number, number] {
  const text = value.trim();
  const hex = /^#([0-9a-f]{6})$/i.exec(text);
  if (hex) {
    const n = parseInt(hex[1]!, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const rgb = /rgba?\(\s*(\d+)[ ,]+(\d+)[ ,]+(\d+)/i.exec(text);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  return fallback;
}

// ---- One ticker for every animated avatar --------------------------------

type Frame = (t: number) => void;
const subscribers = new Set<Frame>();
let handle: number | null = null;
let last = 0;
const FRAME_MS = 1000 / 12;

function loop(now: number) {
  handle = requestAnimationFrame(loop);
  if (now - last < FRAME_MS) return;
  last = now;
  if (typeof document !== "undefined" && document.hidden) return;
  const t = now / 1000;
  for (const frame of subscribers) frame(t);
}

/** Subscribe a painter to the shared 12 fps ticker; returns the unsubscribe. */
export function onTick(frame: Frame): () => void {
  subscribers.add(frame);
  if (handle === null && typeof requestAnimationFrame === "function")
    handle = requestAnimationFrame(loop);
  return () => {
    subscribers.delete(frame);
    if (!subscribers.size && handle !== null) {
      cancelAnimationFrame(handle);
      handle = null;
    }
  };
}

export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

// ---- Dithered fields for CSS backgrounds -----------------------------------

/**
 * Draw the app's dithered ramp once and publish it as `--lz-dither-ramp`, so
 * CSS can use it like any token (the launcher's top edge, empty terminal fields).
 */
export function installDitherFields(
  root: HTMLElement = document.documentElement,
) {
  const ink = parseColour(
    getComputedStyle(root).getPropertyValue("--lz-slate-800"),
    [175, 184, 197],
  );
  const url = ditherRamp(ink, 0.09, 160, 2);
  if (url) root.style.setProperty("--lz-dither-ramp", `url("${url}")`);
}

const fields = new Map<string, string>();

/**
 * A dithered vertical ramp from `ink` (top) to transparent (bottom), as a data
 * URL for a CSS background. `cell` is the dither cell in CSS pixels.
 */
export function ditherRamp(
  ink: [number, number, number],
  alpha: number,
  height: number,
  cell = 2,
): string {
  const key = `${ink.join(",")}:${alpha}:${height}:${cell}`;
  const cached = fields.get(key);
  if (cached) return cached;
  if (typeof document === "undefined") return "";
  const canvas = document.createElement("canvas");
  const rows = Math.max(4, Math.round(height / cell));
  canvas.width = 4 * cell;
  canvas.height = rows * cell;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  ctx.fillStyle = `rgb(${ink[0]} ${ink[1]} ${ink[2]} / ${alpha})`;
  for (let y = 0; y < rows; y++) {
    const level = 1 - y / rows;
    for (let x = 0; x < 4; x++) {
      if (level * level > bayer(x, y))
        ctx.fillRect(x * cell, y * cell, cell, cell);
    }
  }
  const url = canvas.toDataURL("image/png");
  fields.set(key, url);
  return url;
}
