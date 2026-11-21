# App icon: Daybreak

The amber sun, which in this app means "done", rises over the My Day hills and carries the tick. It sits on a
light sky tile. See "App icon" in `docs/design-language.md`.

## Colours

Every colour is a `todo-design` token of the light My Day room (`palette(Theme::Light, hues::MY_DAY)`, hue 65).
The SVGs are flat and hold the hex values the tokens resolve to; if a token changes, change the SVG and regenerate.

| Part | Token | Hex |
|---|---|---|
| Sky (tile) | `sky` | `#ffefe1` |
| Far hill | `hill-far` | `#fcdbbc` |
| Near hill | `hill-near` | `#f8c799` |
| Sun, and the halo at 22 % opacity | `sun` | `#f5ae39` |
| Tick | `ink` | `#241c15` |

## Sources (edit these)

| File | Used for |
|---|---|
| `master.svg` | The icon at 32 px and up: a rounded tile, viewBox `18 18 72 72` of the 108-unit canvas. |
| `small-16.svg` | The icon at 16 and 24 px: no halo, a bigger disc and a heavier tick, so it survives the pixel grid. |
| `adaptive-background.svg` | Android adaptive icon, background layer (108-unit canvas, 66-unit safe circle). |
| `adaptive-foreground.svg` | Android adaptive icon, foreground layer. |
| `adaptive-monochrome.svg` | Android themed icon, as drawn. It uses a mask and a clip path, which VectorDrawable cannot import: use `generated/adaptive-monochrome-flat.svg` instead. |

## Generated (do not edit)

Everything in `generated/` is produced by `generate.mjs` from the sources above.

| File | Used for |
|---|---|
| `todo.ico` | Windows: the desktop exe's icon resource, the window and the tray icon (16, 24, 32, 48, 64, 256 px, PNG images). Also the web favicon. |
| `icon-16.png`, `icon-24.png` | From `small-16.svg`. Favicons, small UI. |
| `icon-32.png` to `icon-512.png` | From `master.svg`: 32, 48, 64, 128, 192, 256, 512. `icon-192.png` and `icon-512.png` are the PWA manifest icons. |
| `apple-touch-icon.png` | 180 px, for iOS. It is full-bleed (a square, no rounded tile), because iOS rounds it itself and paints transparent corners black. |
| `maskable-512.png` | The PWA `maskable` icon: the adaptive background and foreground stacked, cropped so the 66-unit key-art circle is the 80 % safe circle. |
| `adaptive-monochrome-flat.svg` | The monochrome layer with the mask and the clip path computed away: two plain filled paths on the 108-unit canvas. The sun is a clipped circle with the tick's outline (round caps and join) cut out of it; the hill line is a stadium. Convert this to the Android VectorDrawable. |

## Regenerating

```sh
cd assets/brand
npm ci
npm run generate   # write generated/
npm run check      # exit 1 if generated/ is missing or stale (CI runs this)
```

The only dependency is the pure-Node rasteriser `@resvg/resvg-js`, pinned in `package-lock.json`. It is not part
of the Rust workspace. The generator also renders the flattened monochrome layer against the masked original and
fails if they differ.

Never edit a file in `generated/` by hand: change a source, run the generator and commit both.
