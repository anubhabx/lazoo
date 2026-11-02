# Lazoo desktop design system

Status: **v0.2 (Slate), implemented in the desktop** (`apps/lazoo-desktop/ui`). Rationale and the interaction inventory: `docs/superpowers/specs/2026-11-01-desktop-ui-refresh-design.md`.

## Files

- `tokens.css` is the single source of truth: slate and accent primitives, then the semantic roles components use (`--lz-color-*`, `--lz-status-*`, `--lz-provider-*`), type, space, radius, elevation, and motion. The desktop entry point imports it once.
- `tokens.json` is generated from `tokens.css` for tools. Never edit it by hand; run `node design-system/build-tokens.mjs` after changing the CSS.
- `apps/lazoo-desktop/ui/src/styles/components.css` and `apps/lazoo-desktop/ui/src/components/primitives.tsx` are the base components and their states; pages compose them and add layout only (`styles/app.css`).

## Usage rules

1. Components consume semantic roles, never slate or iris primitives, except when defining a new role.
2. Iris is the only accent: focus, current selection, "needs you", and progress. The primary action is quiet ink on slate, not a colour. No amber or ember anywhere.
3. Operational states use `--lz-status-*` and always travel with a word or icon; colour is never the only signal.
4. Each harness keeps one hue (`--lz-provider-*`) everywhere it appears: avatars, chips.
5. Radii: 6 to 10 px for controls, rows, and cards; 14 to 18 px for the composer, dialogs, and the command surface.
6. Depth: a 1 px top highlight plus a low shadow on raised surfaces (`--lz-shadow-raised`); overlays use `--lz-shadow-overlay` or `--lz-shadow-modal` with a backdrop blur. Panels separate by background role and one-pixel borders.
7. Motion: quick to respond, slow to settle, never bouncy. Opacity and transform only; selections move as shared-layout springs; presses scale to `--lz-press-scale`. Reduced motion removes ambient motion and press scale and keeps layout readable.
8. Buttons and selectable rows keep the desktop arrow cursor. Pointer cursors are reserved for links.
