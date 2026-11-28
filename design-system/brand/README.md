# Lazoo brand assets

The mark is an L and a cursor block on a 16-unit grid. The L is ink, the cursor is iris.

| File | Use |
| --- | --- |
| `lazoo-mark.svg` | The full mark on its `#171c24` tile (16 x 16 units, 3-unit corners). App icon, favicon, avatars, store listings. `apps/lazoo-desktop/build.rs` draws the same geometry into the app's `.ico`. |
| `lazoo-glyph-on-dark.svg` | The L and cursor with no tile, ink `#e8ecf1` and iris `#8d9af4`. For dark surfaces. |
| `lazoo-glyph-on-light.svg` | The same glyph in `#171c24` and `#5a69d6`. For light surfaces. |
| `lazoo-lockup-on-dark.svg` | Glyph plus the "lazoo" wordmark, for dark surfaces. |
| `lazoo-lockup-on-light.svg` | The same lockup for light surfaces. |

## Rules

- Keep clear space of 2 grid units (20% of the glyph's side) on every edge. Minimum size is 16 px.
- Do not recolour, rotate, stretch, outline, shadow or animate the files. The in-app `LogoMark` component is the only place the cursor blinks, and only while an agent is working.
- Pick the dark or light file for the surface it sits on; never place the dark glyph on a light surface or the reverse.
- Use `shape-rendering="crispEdges"` (already set) and whole-pixel sizes so the edges stay sharp.

## Wordmark

The wordmark outlines were generated once from IBM Plex Mono SemiBold (SIL Open Font License 1.1) and are committed as plain SVG paths. The lockups need no font at runtime. To change the wordmark, regenerate the paths from the font and replace both lockup files together.
