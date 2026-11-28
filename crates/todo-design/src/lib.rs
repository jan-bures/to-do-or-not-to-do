//! Design tokens for every client: the desktop app (GPUI), Android (Compose) and the web (wasm32 and CSS).
//!
//! The design language is described in `docs/design-language.md`. This crate is its single source of truth:
//! colours, type, spacing, radii, elevation, motion, row and column metrics, the header landscape, and the
//! bundled fonts and icons. It is pure Rust with no I/O, so it builds for every client target.
//!
//! # Quick start
//!
//! ```
//! use todo_design::{Theme, hues, palette, list_colors, typography, layout::space};
//!
//! let p = palette(Theme::Light, hues::MY_DAY);
//! assert_eq!(p.pane.hex(), "#fefbf9");
//! let gpui_rgb: u32 = p.ink.to_u32(); // 0x241c15, for gpui::rgb()
//! # let _ = gpui_rgb;
//!
//! let chip = list_colors(Theme::Dark, 200.0);
//! let title = typography::ROOM_TITLE; // Geist 600, 42/48
//! let gap = space::S3; // 12
//! # let _ = (chip, title, gap);
//! ```
//!
//! # How colour works
//!
//! Every colour is authored in OKLCH and derived from a hue ([`palette`] module). A room (My Day, a collection,
//! a list's own page) tints its whole palette with its hue: canvas, sky, hills, accent. Lists tint only their chip,
//! swatch and glyph. The sun is the only fixed amber. [`color::Oklch::to_rgb`] maps out-of-gamut colours by
//! reducing chroma, exactly as the design generator did, so the hex values match the approved mockups.
//!
//! | Module | Contents |
//! |---|---|
//! | [`chrome`] | Windows caption and desktop scrollbar colours |
//! | [`color`] | [`Rgb`], [`Oklch`], WCAG contrast |
//! | [`palette`] | [`Theme`], [`Palette`], [`ListColors`], [`AgentColors`], [`Dusk`] and their derivation |
//! | [`hues`] | smart-list hues, the list swatches, agent hues |
//! | [`typography`] | families and the type scale |
//! | [`layout`] | spacing, radii, elevation, desktop / phone / component metrics |
//! | [`motion`] | durations, easings, reduced motion |
//! | [`landscape`] | header landscape geometry: hills, the sun's arc, stars, moon |
//! | [`icons`] | the Lucide icon subset and the hand-drawn list glyphs |
//! | [`fonts`] | the bundled font files |
//! | [`export`] | `tokens.css` and `tokens.json` generators |

pub mod chrome;
pub mod color;
pub mod export;
pub mod fonts;
pub mod hues;
pub mod icons;
pub mod landscape;
pub mod layout;
pub mod motion;
pub mod palette;
pub mod typography;

pub use color::{Oklch, Rgb, Rgba, oklch};
pub use icons::{Glyph, Icon};
pub use palette::{
    AgentColors, Dusk, ListColors, Palette, Theme, agent_colors, dusk, list_colors, palette,
    palette_acrylic, palette_scaled,
};
