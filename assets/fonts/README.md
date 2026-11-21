# Bundled fonts

Three families, all under the [SIL Open Font License 1.1](https://openfontlicense.org). None declares a
Reserved Font Name. Each family's license is next to its files.

| Family | Role | License |
|---|---|---|
| Fraunces | room titles, the italic day name, the empty-state line | [`OFL-Fraunces.txt`](OFL-Fraunces.txt) |
| Schibsted Grotesk | all interface text | [`OFL-SchibstedGrotesk.txt`](OFL-SchibstedGrotesk.txt) |
| Geist Mono | counts, due times, tags, keycaps, priority, caps labels | [`OFL-GeistMono.txt`](OFL-GeistMono.txt) |

## Files

These are static instances: every variation axis is pinned, so GPUI, Compose and browsers render the same
outlines without needing variable-font support. Each one is subset to Latin and Latin Extended, which gives full
Czech coverage. It also keeps Greek π (the Pi agent's letter mark), arrows, keyboard symbols and a few shapes.
All OpenType layout features are kept, including kerning, ligatures and tabular figures.

| File | Family name in the file | Weight | Axes pinned |
|---|---|---|---|
| `FrauncesDisplay-Book.ttf` | Fraunces Display | 360 | opsz 144, SOFT 100, WONK 0, wght 360 |
| `FrauncesDisplay-BookItalic.ttf` | Fraunces Display (italic) | 360 | opsz 144, SOFT 100, WONK 0, wght 360 |
| `FrauncesText-Italic.ttf` | Fraunces Text (italic) | 400 | opsz 36, SOFT 100, WONK 0, wght 400 |
| `SchibstedGrotesk-{400,500,550,600,650}.ttf` | Schibsted Grotesk | as named | wght |
| `GeistMono-{400,500,600,700}.ttf` | Geist Mono | as named | wght |

The family names and weights are the ones `todo_design::typography` and `todo_design::fonts::FACES` use. The
`assets` test in `crates/todo-design` checks them against the files, and checks that every file covers
`áčďéěíňóřšťúůýž` and their capitals.

The total is about 860 KB.

## Sources

Every file was taken from [google/fonts](https://github.com/google/fonts), which builds from each project's
upstream repository.

| Family | google/fonts commit | Upstream repository and commit | Version | Upstream file and SHA-256 |
|---|---|---|---|---|
| Fraunces | `4024282d9b0cffcdb8e3024560862746178d741f` | [undercasetype/Fraunces](https://github.com/undercasetype/Fraunces) `d6d385783609ceb11ac0f220f3abd9f1631c8a36` | 1.000 | `ofl/fraunces/Fraunces[SOFT,WONK,opsz,wght].ttf` `177ff6c0f14e5550a3c624247cd1189611d4eb65d000b14944c63d967958abbb`; `ofl/fraunces/Fraunces-Italic[SOFT,WONK,opsz,wght].ttf` `b24448c43702fac4ee856781d461a0dfba8d8e594b6e8e190234b75fed2c0e01` |
| Schibsted Grotesk | `8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5` | [schibsted/schibsted-grotesk](https://github.com/schibsted/schibsted-grotesk) `d485f61f105e1b3935f4d21dfb4d371359798603` | 1.100 | `ofl/schibstedgrotesk/SchibstedGrotesk[wght].ttf` `6ceeadf6be8e1fd7687011c7fa38ed0edd1abe967a0b73d97caec183552e823d` |
| Geist Mono | `121c8e30e20960139d24d1e93f350745205dd680` | [vercel/geist-font](https://github.com/vercel/geist-font) `77f0563c03009d6c15c6342183fa53b352255b22` | 1.701 | `ofl/geistmono/GeistMono[wght].ttf` `d00e590b8eb3a59acc329b2d044fd143ae935090b7da33199ebee27cc7de8196` |

## Rebuilding

1. Download the four upstream files above into an empty directory, keeping their names.
2. Run the script with fontTools installed (tested with 4.65):

   ```sh
   python build_fonts.py <that-directory>
   ```

3. Run `cargo test -p todo-design --test assets`.

To add a weight, add a row to `INSTANCES` in `build_fonts.py` and a matching entry to
`todo_design::fonts::FACES`.
