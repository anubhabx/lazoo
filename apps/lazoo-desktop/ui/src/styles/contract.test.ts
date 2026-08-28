import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, test } from "vitest";

const sourceRoot = join(process.cwd(), "src");
const appStylesPath = join(sourceRoot, "styles", "app.css");

function appStyles() {
  expect(existsSync(appStylesPath)).toBe(true);
  return existsSync(appStylesPath) ? readFileSync(appStylesPath, "utf8") : "";
}

describe("desktop visual contract", () => {
  test("uses canonical semantic tokens without literal product colors", () => {
    const css = appStyles();

    expect(css).toContain("var(--lz-color-window)");
    expect(css).toContain("var(--lz-color-text-primary)");
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
    expect(css).not.toMatch(/\b(?:rgb|rgba|hsl|hsla|oklch)\(/iu);
  });

  test("keeps desktop interaction and motion constraints explicit", () => {
    const css = appStyles();

    expect(css).toContain("height: var(--lz-titlebar-h)");
    expect(css).toContain("width: var(--lz-navigation-w)");
    expect(css).toContain("height: var(--lz-statusbar-h)");
    expect(css).not.toMatch(/transition\s*:\s*all/iu);
    expect(css).not.toContain("cursor: pointer");
  });

  test("ships no remote resources or emoji icon substitutes in source", () => {
    const sourceFiles = [
      join(sourceRoot, "App.tsx"),
      join(sourceRoot, "components", "primitives.tsx"),
      join(sourceRoot, "components", "ShellChrome.tsx"),
      join(sourceRoot, "components", "Navigation.tsx"),
      join(sourceRoot, "components", "OperationalRows.tsx"),
      join(sourceRoot, "components", "Inspector.tsx"),
      join(sourceRoot, "components", "CommandPalette.tsx"),
    ];
    const source = sourceFiles
      .filter((file) => existsSync(file))
      .map((file) => readFileSync(file, "utf8"))
      .join("\n");
    const main = readFileSync(join(sourceRoot, "main.tsx"), "utf8");

    expect(sourceRoot).toContain("src");
    expect(`${source}\n${appStyles()}`).not.toMatch(/https?:\/\//iu);
    expect(source).not.toMatch(/[\u{1F300}-\u{1FAFF}]/u);
    expect(main).toContain('design-system/tokens.css"');
  });
});
