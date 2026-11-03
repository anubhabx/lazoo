import { describe, expect, test } from "vitest";

import { GRID, avatarMotion, bayer, glyphModel, hash } from "./dither";

describe("dithered avatars", () => {
  test("a name always yields the same glyph, and different names differ", () => {
    const a = glyphModel("atlas");
    expect(glyphModel("atlas")).toBe(a);
    expect(Array.from(glyphModel("Atlas ").mask)).toEqual(Array.from(a.mask));
    const names = ["atlas", "forge", "pixel", "sentinel", "scout", "dash"];
    const shapes = new Set(
      names.map((name) => Array.from(glyphModel(name).mask).join("")),
    );
    expect(shapes.size).toBe(names.length);
  });

  test("glyphs are mirror-symmetric and never empty", () => {
    for (const name of ["mira", "rune", "kit", "x", ""]) {
      const { mask } = glyphModel(name);
      let filled = 0;
      for (let y = 0; y < GRID; y++)
        for (let x = 0; x < GRID; x++) {
          expect(mask[y * GRID + x]).toBe(mask[y * GRID + (GRID - 1 - x)]);
          filled += mask[y * GRID + x]!;
        }
      expect(filled).toBeGreaterThanOrEqual(16);
    }
  });

  test("the Bayer matrix covers (0, 1) once per 4x4 tile", () => {
    const values = new Set<number>();
    for (let y = 0; y < 4; y++)
      for (let x = 0; x < 4; x++) values.add(bayer(x, y));
    expect(values.size).toBe(16);
    expect(Math.min(...values)).toBeGreaterThan(0);
    expect(Math.max(...values)).toBeLessThan(1);
    expect(bayer(5, 6)).toBe(bayer(1, 2));
    expect(hash("lazoo")).toBe(hash("lazoo"));
  });

  test("states map to the four motions", () => {
    expect(avatarMotion("running")).toBe("working");
    expect(avatarMotion("starting")).toBe("thinking");
    expect(avatarMotion("needs-approval")).toBe("attention");
    expect(avatarMotion("failed")).toBe("still");
    expect(avatarMotion(undefined)).toBe("still");
  });
});
