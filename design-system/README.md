# Lazoo desktop design-system starter

Status: **Proposed — not imported by the application.**

This directory is the implementation-neutral starter for Lazoo's dark desktop interface. It combines:

- Zed's density-aware spacing, compact type hierarchy, semantic surface model, component sizing, restrained elevation, and short animation timing;
- Raycast's near-black color restraint, search-first workflow, contextual action model, keyboard teaching, and polished desktop behavior;
- Lazoo's own ember accent, provider truth states, agent states, and orchestration components.

## Files

- `tokens.json` is the canonical, tool-readable source.
- `tokens.css` is the generated-style CSS-variable starter for a future webview implementation.
- `../docs/superpowers/specs/2026-08-29-lazoo-desktop-design-system.md` contains the evidence, rationale, component contracts, and usage rules.

Neither token file is currently imported by `apps/lazoo-desktop/ui/styles.css`. The existing desktop remains unchanged until the design-system specification is reviewed and approved.

## Usage rules

1. Components consume semantic variables such as `--lz-color-surface` and `--lz-color-text-secondary`. They do not consume graphite primitives directly unless defining a new semantic token.
2. Ember is reserved for focus, current location, primary action, and Lazoo identity. It is not a generic decoration color.
3. Operational states use the dedicated `--lz-status-*` variables and always include a text label or icon; color is never the only signal.
4. Default density is the initial product density. Compact and comfortable values are already defined but should not become user settings until the default system is stable.
5. Use 4px and 6px radii for normal rows and controls. Use 12px or 16px only for overlays, command surfaces, and substantial floating panels.
6. Use shadows only for raised surfaces and modals. Panel separation comes from background roles and one-pixel borders.
7. Motion uses opacity and 4–8px translation. Layout must remain understandable with motion disabled.
8. Buttons and selectable rows keep the desktop arrow cursor. Pointer cursors are reserved for links.

## Intended future import

When the redesign is approved, the frontend entrypoint should import `tokens.css` once and place `data-theme="lazoo-dark"` and `data-density="default"` on the application root. This is guidance only; no import has been added yet.
