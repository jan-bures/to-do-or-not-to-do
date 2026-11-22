//! Named hues, in OKLCH degrees. Pass them to [`crate::palette()`], [`crate::list_colors()`] and
//! [`crate::agent_colors()`].
//!
//! Smart lists have fixed hues. Collections and lists pick one of the [`SWATCHES`]. Agents have a hue each so
//! their letter marks stay apart.

/// A hue with a stable key (used in CSS selectors, JSON and settings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NamedHue {
    /// Stable kebab-case key.
    pub key: &'static str,
    /// OKLCH hue in degrees.
    pub degrees: f64,
}

const fn named(key: &'static str, degrees: f64) -> NamedHue {
    NamedHue { key, degrees }
}

/// My Day: warm apricot.
pub const MY_DAY: f64 = 65.0;
/// Important.
pub const IMPORTANT: f64 = 28.0;
/// Planned.
pub const PLANNED: f64 = 150.0;
/// All.
pub const ALL: f64 = 95.0;
/// Completed.
pub const COMPLETED: f64 = 135.0;
/// Agent activity.
pub const AGENT_ACTIVITY: f64 = 262.0;

/// The smart lists' fixed hues.
pub const SMART_LISTS: [NamedHue; 6] = [
    named("my-day", MY_DAY),
    named("important", IMPORTANT),
    named("planned", PLANNED),
    named("all", ALL),
    named("completed", COMPLETED),
    named("agent-activity", AGENT_ACTIVITY),
];

/// The hues a collection or list can take. Spread around the wheel so neighbouring lists stay apart; every one
/// passes the chip, pill and agent contrast checks in both themes (in fact every whole-degree hue does).
pub const SWATCHES: [NamedHue; 12] = [
    named("rose", 10.0),
    named("coral", 24.0),
    named("clay", 42.0),
    named("olive", 112.0),
    named("fern", 135.0),
    named("jade", 158.0),
    named("teal", 182.0),
    named("lagoon", 200.0),
    named("azure", 212.0),
    named("cornflower", 252.0),
    named("orchid", 330.0),
    named("pink", 352.0),
];

/// A stable default swatch from an entity's ID bytes (FNV-1a).
/// Reduce the 64-bit hash before converting to an index, including on wasm32.
pub fn swatch_for_seed(seed: &[u8]) -> NamedHue {
    let hash = seed.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    });
    SWATCHES[(hash % SWATCHES.len() as u64) as usize]
}

/// Default hues for the agents' letter marks.
pub const AGENTS: [NamedHue; 3] = [
    named("claude", 38.0),
    named("codex", 245.0),
    named("pi", 140.0),
];

/// Every room hue the exports precompute: the smart lists, then the swatches.
pub fn rooms() -> impl Iterator<Item = NamedHue> {
    SMART_LISTS.into_iter().chain(SWATCHES)
}

/// Looks a hue up by key among the smart lists, swatches and agents.
pub fn by_key(key: &str) -> Option<NamedHue> {
    rooms().chain(AGENTS).find(|h| h.key == key)
}
