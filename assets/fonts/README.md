# Bundled fonts

Two families, both under the [SIL Open Font License 1.1](https://openfontlicense.org). Neither declares a
Reserved Font Name. Each family's license is next to its files.

| Family | Role | License |
|---|---|---|
| Geist | room titles, eyebrows, empty states and all interface text | [`OFL-Geist.txt`](OFL-Geist.txt) |
| Geist Mono | counts, due times, tags, keycaps, priority, caps labels | [`OFL-GeistMono.txt`](OFL-GeistMono.txt) |

## Files

These are static instances: the weight axis is pinned, so GPUI, Compose and browsers render the same
outlines without needing variable-font support. Each one is subset to Latin and Latin Extended for full
Czech coverage. The subset also includes Greek π (the Pi agent's letter mark), arrows, keyboard symbols
and a few shapes where the source font provides them. All OpenType layout features are kept, including
kerning, ligatures and tabular figures.

| File | Family name in the file | Weight |
|---|---|---|
| `Geist-{400,500,600}.ttf` | Geist | as named |
| `GeistMono-{400,500,600,700}.ttf` | Geist Mono | as named |

The UI scale uses Regular (400), Medium (500) and Semibold (600). The former intermediate weights 550
and 650 map to 500 and 600, respectively. The `assets` test in `crates/todo-design` checks the families
and weights against `todo_design::fonts::FACES` and verifies that every file covers `áčďéěíňóřšťúůýž`
and their capitals.

The fonts total about 400 KiB.

## Sources

| Family | Source | Variable font and SHA-256 |
|---|---|---|
| Geist | [Vercel Geist v1.9.2 release](https://github.com/vercel/geist-font/releases/tag/v1.9.2), commit `ef2597cfc42f4797676df105517ccc0a1cfa6569`. `OFL-Geist.txt` is `geist-font/OFL.txt` from the same archive. | `geist-font/Geist/variable/Geist[wght].ttf` · `1863756b2189613e653b676c0871b03b5e8b71362ab61d7977668c3eb09ab239` |
| Geist Mono | [google/fonts](https://github.com/google/fonts) commit `121c8e30e20960139d24d1e93f350745205dd680`, built from [vercel/geist-font](https://github.com/vercel/geist-font) commit `77f0563c03009d6c15c6342183fa53b352255b22` (1.701). | `ofl/geistmono/GeistMono[wght].ttf` · `d00e590b8eb3a59acc329b2d044fd143ae935090b7da33199ebee27cc7de8196` |

## Rebuilding

1. Download or extract the two variable fonts above into an empty directory, keeping their file names.
2. Run the script with fontTools installed (tested with 4.65):

   ```sh
   python build_fonts.py <that-directory>
   ```

3. Run `cargo test -p todo-design --test assets`.

To add a weight, add a row to `INSTANCES` in `build_fonts.py` and a matching entry to
`todo_design::fonts::FACES`.
