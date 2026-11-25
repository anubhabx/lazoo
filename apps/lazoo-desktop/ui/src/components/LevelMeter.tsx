import { useEffect, useRef } from "react";

import { bayer, parseColour } from "../lib/dither";

/**
 * The live microphone level as a dithered scope: one column per reading, the
 * newest on the right, each column a stack of cells lit by the ordered-dither
 * threshold. It repaints on each level reading (about 16 a second) and holds
 * still between them, so it needs no animation loop.
 */
export function LevelMeter({
  source,
  columns = 32,
  rows = 6,
  cell = 3,
}: {
  /** Subscribes to level readings (0..1); returns the unsubscribe. */
  source: (listener: (level: number) => void) => () => void;
  columns?: number;
  rows?: number;
  cell?: number;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = canvas.current;
    const ctx = element?.getContext("2d");
    if (!element) return;
    const scale = Math.max(1, Math.round(window.devicePixelRatio || 1));
    const px = cell * scale;
    element.width = columns * px;
    element.height = rows * px;
    element.style.width = `${columns * cell}px`;
    element.style.height = `${rows * cell}px`;
    const ink = parseColour(
      getComputedStyle(element).getPropertyValue("--lz-iris-400"),
      [168, 179, 249],
    );
    const history = new Array<number>(columns).fill(0);

    function paint() {
      if (!ctx || !element) return;
      ctx.clearRect(0, 0, element.width, element.height);
      for (let x = 0; x < columns; x += 1) {
        const height = history[x]! * rows;
        for (let row = 0; row < rows; row += 1) {
          const lit = height - row > bayer(x, rows - 1 - row);
          if (!lit && row > 0) continue;
          ctx.fillStyle = `rgb(${ink[0]} ${ink[1]} ${ink[2]} / ${lit ? 1 : 0.22})`;
          ctx.fillRect(x * px, (rows - 1 - row) * px, px, px);
        }
      }
    }

    paint();
    return source((level) => {
      history.push(Math.max(0, Math.min(1, level)));
      history.shift();
      paint();
    });
  }, [source, columns, rows, cell]);

  return <canvas ref={canvas} className="level-meter" aria-hidden="true" />;
}
