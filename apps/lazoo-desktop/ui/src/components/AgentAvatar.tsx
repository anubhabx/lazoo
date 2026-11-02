import { useEffect, useRef } from "react";

import {
  avatarMotion,
  glyphModel,
  onTick,
  paintGlyph,
  parseColour,
  prefersReducedMotion,
  type Ink,
} from "../lib/dither";
import { providerLabel, words } from "../lib/util";

const DIM = new Set([
  "dormant",
  "suspended",
  "unknown",
  "complete",
  "stopping",
  "interrupted",
  "disabled",
]);

/**
 * An agent's mark: a dithered glyph whose shape is the agent (seeded from its
 * name) and whose colour is its harness. The state animates the dither field.
 */
export function AgentAvatar({
  name,
  provider,
  size = 20,
  status,
}: {
  name: string;
  provider: string | null | undefined;
  size?: number;
  status?: string;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const motion = prefersReducedMotion() ? "still" : avatarMotion(status);

  useEffect(() => {
    const element = canvas.current;
    const ctx = element?.getContext("2d");
    if (!element || !ctx) return;
    const scale = window.devicePixelRatio || 1;
    element.width = Math.round(size * scale);
    element.height = Math.round(size * scale);
    const styles = getComputedStyle(element);
    const read = (name: string, fallback: [number, number, number]) =>
      parseColour(styles.getPropertyValue(name), fallback);
    const surface = read("--lz-color-surface", [24, 27, 33]);
    const hue =
      status === "failed"
        ? read("--lz-status-failed", [236, 139, 145])
        : read(`--lz-provider-${provider ?? "none"}`, [139, 150, 167]);
    const ink: Ink = {
      hue,
      ground: [
        Math.round(surface[0] + (hue[0] - surface[0]) * 0.1),
        Math.round(surface[1] + (hue[1] - surface[1]) * 0.1),
        Math.round(surface[2] + (hue[2] - surface[2]) * 0.1),
      ],
      accent: read("--lz-color-accent", [141, 154, 244]),
    };
    const model = glyphModel(name);
    const dim = DIM.has(status ?? "");
    const paint = (t: number) => paintGlyph(ctx, model, ink, motion, t, dim);
    paint(0);
    if (motion === "still") return;
    // Off-screen avatars (a collapsed project, a scrolled list) cost nothing.
    let onScreen = true;
    const observer =
      typeof IntersectionObserver === "function"
        ? new IntersectionObserver((entries) => {
            onScreen = entries.some((entry) => entry.isIntersecting);
          })
        : null;
    observer?.observe(element);
    const stop = onTick((t) => {
      if (onScreen && element.isConnected) paint(t);
    });
    return () => {
      stop();
      observer?.disconnect();
    };
  }, [name, provider, size, status, motion]);

  return (
    <span
      className="avatar"
      data-motion={motion}
      style={{ width: size, height: size }}
      title={`${name} · ${providerLabel(provider)}${status ? ` · ${words(status)}` : ""}`}
      aria-hidden="true"
    >
      <canvas ref={canvas} style={{ width: size, height: size }} />
    </span>
  );
}
