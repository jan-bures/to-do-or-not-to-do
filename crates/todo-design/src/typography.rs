//! The type scale. Three families, each with one job:
//!
//! - **Fraunces** (display serif) for room titles and the italic day name. It is bundled as static instances
//!   with the optical size, softness and weight baked in ([`DISPLAY`], [`DISPLAY_TEXT`]).
//! - **Schibsted Grotesk** ([`UI`]) for all interface text.
//! - **Geist Mono** ([`MONO`]) for counts, due times, tags, keycaps, priority marks and caps labels.
//!
//! Sizes and line heights are in logical pixels (dp on Android). Letter spacing is in em.

/// Fraunces at optical size 144, SOFT 100, WONK 0, weight 360 (roman and italic). Room titles and the
/// empty-state line.
pub const DISPLAY: &str = "Fraunces Display";
/// Fraunces at optical size 36, SOFT 100, WONK 0, weight 400, italic only. The day name above the title.
pub const DISPLAY_TEXT: &str = "Fraunces Text";
/// Schibsted Grotesk. All interface text.
pub const UI: &str = "Schibsted Grotesk";
/// Geist Mono. Numbers, times, tags, keycaps, labels.
pub const MONO: &str = "Geist Mono";

/// CSS fallback stacks for each family.
pub const FALLBACKS: [(&str, &str); 4] = [
    (DISPLAY, "Georgia, serif"),
    (DISPLAY_TEXT, "Georgia, serif"),
    (UI, "system-ui, sans-serif"),
    (MONO, "ui-monospace, monospace"),
];

/// One text style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Family name, one of [`DISPLAY`], [`DISPLAY_TEXT`], [`UI`], [`MONO`].
    pub family: &'static str,
    /// Font size in logical pixels.
    pub size: f32,
    /// Line height in logical pixels.
    pub line_height: f32,
    /// Weight, 100–900. Only the weights in [`crate::fonts::FACES`] are bundled.
    pub weight: u16,
    /// Italic.
    pub italic: bool,
    /// Letter spacing in em (negative tightens).
    pub tracking_em: f32,
    /// Rendered in capitals.
    pub uppercase: bool,
    /// Tabular figures (`tnum`), so digits line up in columns.
    pub tabular: bool,
}

impl TextStyle {
    const fn new(family: &'static str, weight: u16, size: f32, line_height: f32) -> TextStyle {
        TextStyle {
            family,
            size,
            line_height,
            weight,
            italic: false,
            tracking_em: 0.0,
            uppercase: false,
            tabular: family_is_mono(family),
        }
    }

    const fn italic(mut self) -> TextStyle {
        self.italic = true;
        self
    }

    const fn tracking(mut self, em: f32) -> TextStyle {
        self.tracking_em = em;
        self
    }

    const fn caps(mut self) -> TextStyle {
        self.uppercase = true;
        self
    }

    /// Letter spacing in logical pixels.
    pub fn tracking_px(&self) -> f32 {
        self.tracking_em * self.size
    }

    /// The same style at another weight (for example due times that turn semibold when due soon).
    pub const fn with_weight(mut self, weight: u16) -> TextStyle {
        self.weight = weight;
        self
    }
}

const fn family_is_mono(family: &str) -> bool {
    let (a, b) = (family.as_bytes(), MONO.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Desktop room title: "My Day", "Work".
pub const ROOM_TITLE: TextStyle = TextStyle::new(DISPLAY, 360, 56.0, 60.0).tracking(-0.028);
/// Room title in narrow panes (the empty and quick-add states) and on the inner phone screen.
pub const ROOM_TITLE_COMPACT: TextStyle = TextStyle::new(DISPLAY, 360, 48.0, 52.0).tracking(-0.028);
/// Phone room title.
pub const ROOM_TITLE_PHONE: TextStyle = TextStyle::new(DISPLAY, 360, 44.0, 48.0).tracking(-0.026);
/// Phone room title in the collapsed header bar: one line, while scrolling or typing.
pub const ROOM_TITLE_BAR: TextStyle = TextStyle::new(DISPLAY, 360, 24.0, 28.0).tracking(-0.016);
/// The italic day name above the title ("Saturday").
pub const EYEBROW_DAY: TextStyle = TextStyle::new(DISPLAY_TEXT, 400, 18.0, 22.0).italic();
/// Phone day name.
pub const EYEBROW_DAY_PHONE: TextStyle = TextStyle::new(DISPLAY_TEXT, 400, 17.0, 20.0).italic();
/// The date and week next to the day name, in mono caps.
pub const EYEBROW_META: TextStyle = TextStyle::new(MONO, 400, 11.0, 22.0).tracking(0.1).caps();
/// Phone date and week.
pub const EYEBROW_META_PHONE: TextStyle =
    TextStyle::new(MONO, 400, 10.5, 20.0).tracking(0.1).caps();
/// The one italic line of an empty list ("Nothing on the shelf yet.").
pub const EMPTY_LINE: TextStyle = TextStyle::new(DISPLAY, 360, 32.0, 38.0)
    .italic()
    .tracking(-0.02);
/// Detail pane title. Sans, so it sits well next to dense steps.
pub const DETAIL_TITLE: TextStyle = TextStyle::new(UI, 600, 21.0, 28.0).tracking(-0.012);
/// Detail title on the inner phone screen.
pub const DETAIL_TITLE_PHONE: TextStyle = TextStyle::new(UI, 600, 23.0, 30.0).tracking(-0.012);
/// The next task in the finished state ("When the sun comes up").
pub const NEXT_TITLE: TextStyle = TextStyle::new(UI, 600, 19.0, 26.0).tracking(-0.01);
/// Task title in a desktop row.
pub const TASK_TITLE: TextStyle = TextStyle::new(UI, 500, 15.0, 20.0);
/// Task title in a phone row.
pub const TASK_TITLE_PHONE: TextStyle = TextStyle::new(UI, 500, 16.0, 22.0);
/// Default body text.
pub const BODY: TextStyle = TextStyle::new(UI, 400, 14.0, 20.0);
/// Notes in the detail pane.
pub const NOTES: TextStyle = TextStyle::new(UI, 400, 14.0, 22.0);
/// Quick-add input text.
pub const QUICK_ADD: TextStyle = TextStyle::new(UI, 400, 14.5, 20.0);
/// Phone composer text.
pub const COMPOSER: TextStyle = TextStyle::new(UI, 400, 15.5, 22.0);
/// Smart lists in the sidebar.
pub const NAV: TextStyle = TextStyle::new(UI, 500, 14.0, 20.0);
/// Lists inside a collection in the sidebar.
pub const NAV_SUB: TextStyle = TextStyle::new(UI, 400, 13.5, 20.0);
/// Collection headers in the sidebar.
pub const NAV_COLLECTION: TextStyle = TextStyle::new(UI, 600, 13.0, 16.0);
/// Step rows in the detail pane.
pub const STEP: TextStyle = TextStyle::new(UI, 400, 13.5, 20.0);
/// Row metadata line (desktop).
pub const META: TextStyle = TextStyle::new(UI, 400, 12.0, 16.0);
/// Row metadata line (phone).
pub const META_PHONE: TextStyle = TextStyle::new(UI, 400, 12.5, 16.0);
/// The header summary ("3 of 12 done · 9 open · est. 5h 40m"); numbers use [`SUMMARY_FIGURES`].
pub const SUMMARY: TextStyle = TextStyle::new(UI, 400, 12.5, 16.0);
/// Numbers inside the header summary.
pub const SUMMARY_FIGURES: TextStyle = TextStyle::new(MONO, 400, 12.0, 16.0);
/// List chips and the overdue badge's neighbour text.
pub const CHIP: TextStyle = TextStyle::new(UI, 550, 12.0, 20.0);
/// Phone room pills.
pub const ROOM_PILL: TextStyle = TextStyle::new(UI, 550, 13.5, 20.0);
/// The overdue badge ("2d overdue").
pub const OVERDUE_BADGE: TextStyle = TextStyle::new(MONO, 600, 12.0, 20.0);
/// Toast message.
pub const TOAST: TextStyle = TextStyle::new(UI, 550, 13.5, 20.0);
/// The toast's Undo action.
pub const TOAST_ACTION: TextStyle = TextStyle::new(UI, 650, 13.5, 20.0);
/// Buttons.
pub const BUTTON: TextStyle = TextStyle::new(UI, 600, 14.0, 20.0);
/// Small buttons and the suggestion chip.
pub const BUTTON_SMALL: TextStyle = TextStyle::new(UI, 600, 12.5, 16.0);
/// Section labels in mono caps ("STEPS", "AGENTS", "WHEN THE SUN COMES UP").
pub const CAPS_LABEL: TextStyle = TextStyle::new(MONO, 600, 10.5, 16.0).tracking(0.12).caps();
/// Due dates and times in rows. Due-soon uses weight 600.
pub const DUE: TextStyle = TextStyle::new(MONO, 500, 12.5, 20.0);
/// `#tags`.
pub const TAG: TextStyle = TextStyle::new(MONO, 500, 12.0, 16.0);
/// Keycaps.
pub const KEYCAP: TextStyle = TextStyle::new(MONO, 500, 10.5, 16.0);
/// Priority marks `!` / `!!` / `!!!`. Low priority uses weight 600.
pub const PRIORITY: TextStyle = TextStyle::new(MONO, 700, 13.0, 16.0).tracking(-0.115);
/// Sidebar counts, the status bar and timestamps.
pub const COUNT: TextStyle = TextStyle::new(MONO, 400, 11.5, 16.0);
/// The letter in a 20 px agent mark. Scale the size with the mark: half its diameter.
pub const AGENT_LETTER: TextStyle = TextStyle::new(MONO, 600, 10.0, 10.0);

/// The code a new device shows while it waits to be approved: "KQ4M-7XWD". Mono, so it reads character by
/// character.
pub const PAIRING_CODE: TextStyle = TextStyle::new(MONO, 500, 40.0, 48.0).tracking(0.06);

/// Every named style, for the exports and for a type specimen.
pub const ALL: [(&str, TextStyle); 41] = [
    ("room-title", ROOM_TITLE),
    ("room-title-compact", ROOM_TITLE_COMPACT),
    ("room-title-phone", ROOM_TITLE_PHONE),
    ("room-title-bar", ROOM_TITLE_BAR),
    ("eyebrow-day", EYEBROW_DAY),
    ("eyebrow-day-phone", EYEBROW_DAY_PHONE),
    ("eyebrow-meta", EYEBROW_META),
    ("eyebrow-meta-phone", EYEBROW_META_PHONE),
    ("empty-line", EMPTY_LINE),
    ("detail-title", DETAIL_TITLE),
    ("detail-title-phone", DETAIL_TITLE_PHONE),
    ("next-title", NEXT_TITLE),
    ("task-title", TASK_TITLE),
    ("task-title-phone", TASK_TITLE_PHONE),
    ("body", BODY),
    ("notes", NOTES),
    ("quick-add", QUICK_ADD),
    ("composer", COMPOSER),
    ("nav", NAV),
    ("nav-sub", NAV_SUB),
    ("nav-collection", NAV_COLLECTION),
    ("step", STEP),
    ("meta", META),
    ("meta-phone", META_PHONE),
    ("summary", SUMMARY),
    ("summary-figures", SUMMARY_FIGURES),
    ("chip", CHIP),
    ("room-pill", ROOM_PILL),
    ("overdue-badge", OVERDUE_BADGE),
    ("toast", TOAST),
    ("toast-action", TOAST_ACTION),
    ("button", BUTTON),
    ("button-small", BUTTON_SMALL),
    ("caps-label", CAPS_LABEL),
    ("due", DUE),
    ("tag", TAG),
    ("keycap", KEYCAP),
    ("priority", PRIORITY),
    ("count", COUNT),
    ("agent-letter", AGENT_LETTER),
    ("pairing-code", PAIRING_CODE),
];
