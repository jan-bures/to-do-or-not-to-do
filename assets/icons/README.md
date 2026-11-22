# Bundled icons

## `lucide/`: interface icons

These are 56 icons from [Lucide](https://lucide.dev) **v1.66.0** (npm package `lucide-static@1.66.0`), under the
ISC license. Some icons derive from Feather and are under the MIT license. Both licenses are in
[`lucide/LICENSE`](lucide/LICENSE).

Lucide is the one interface icon set. Its icons are drawn as round-capped strokes on a 24-unit grid, and they
match the line icons in the approved mockups. Two edits were made to each file:

- `stroke-width` is changed from 2 to 1.6, the weight the mockups use at 18 px;
- the `class` attribute is removed.

Only the icons that M1 needs are bundled. To add one, copy it from the same release, apply the same two edits, and
add a variant to `todo_design::Icon` (`crates/todo-design/src/icons.rs`). The `assets` test checks that every file
is listed there, and that every icon is a single-colour stroke SVG.

## `glyphs/`: hand-drawn list glyphs

These are the app's own drawings, under the repository's license. They are slightly irregular single-stroke
pictograms that lists can pick to give the sidebar and the phone room pills some character without colour blocks.
Each one is drawn in its list's `glyph` colour at 16 px. They are listed in `todo_design::Glyph`.

All the SVGs are single-colour: `stroke="currentColor"` and `fill="none"`, with no hard-coded colours. GPUI can
therefore tint them like text.
