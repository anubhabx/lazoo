// Regenerates tokens.json from tokens.css, the single source of truth.
// Usage: node design-system/build-tokens.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "tokens.css"), "utf8");
// Only the first block (the theme root); the reduced-motion block overrides it.
const root = css.slice(css.indexOf("{") + 1, css.indexOf("\n}"));
const tokens = {};
for (const [, name, value] of root.matchAll(/--lz-([a-z0-9-]+):\s*([^;]+);/g)) {
  const [group, ...rest] = name.split("-");
  const key = rest.join("-") || group;
  (tokens[group] ??= {})[key] = value.replace(/\s+/g, " ").trim();
}
const json = {
  meta: {
    name: "Lazoo Desktop Design System",
    version: "0.2.0",
    theme: "dark",
    direction: "Slate surfaces, one iris accent, soft depth, quick motion",
    generatedFrom: "tokens.css",
  },
  tokens,
};
writeFileSync(join(here, "tokens.json"), `${JSON.stringify(json, null, 2)}\n`);
console.log(`tokens.json: ${Object.values(tokens).reduce((n, g) => n + Object.keys(g).length, 0)} tokens`);
