//! WCAG 2.x contrast over the same pairs as the approved design's contrast table. Text targets 4.5:1; control
//! boundaries and meaningful graphics 3:1; decorative scenery is informational (1:1).
//!
//! Each row pins the hex values and the ratio from the table, so a token change that moves a colour shows up
//! here, and asserts the target, so a change can never turn a pass into a fail.

use todo_design::{Palette, Rgb, Theme, agent_colors, hues, list_colors, palette, palette_scaled};

struct Row {
    label: &'static str,
    fg: Rgb,
    bg: Rgb,
    expect_fg: &'static str,
    expect_bg: &'static str,
    ratio: f64,
    target: f64,
}

fn row(
    label: &'static str,
    (fg, bg): (Rgb, Rgb),
    (expect_fg, expect_bg): (&'static str, &'static str),
    ratio: f64,
    target: f64,
) -> Row {
    Row {
        label,
        fg,
        bg,
        expect_fg,
        expect_bg,
        ratio,
        target,
    }
}

fn check(scenario: &str, rows: Vec<Row>) {
    for r in rows {
        let ctx = format!("{scenario}: {}", r.label);
        assert_eq!(r.fg.hex(), r.expect_fg, "{ctx}: foreground");
        assert_eq!(r.bg.hex(), r.expect_bg, "{ctx}: background");
        let ratio = r.fg.contrast(r.bg);
        assert!(
            (ratio - r.ratio).abs() < 0.006,
            "{ctx}: ratio {ratio:.3}, table says {:.2}",
            r.ratio
        );
        assert!(
            ratio >= r.target,
            "{ctx}: {ratio:.2}:1 is below {}:1",
            r.target
        );
    }
}

/// The pairs every scenario shares, with the expected values for one scenario.
#[rustfmt::skip]
fn base_rows(t: &Palette, e: [(&'static str, &'static str, f64); 16]) -> Vec<Row> {
    let pairs: [(&'static str, Rgb, Rgb, f64); 16] = [
        ("Body text: ink on pane", t.ink, t.pane, 4.5),
        ("Secondary text: ink-2 on pane", t.ink_2, t.pane, 4.5),
        ("Metadata: ink-3 on pane", t.ink_3, t.pane, 4.5),
        ("Metadata: ink-3 on canvas (sidebar)", t.ink_3, t.canvas, 4.5),
        ("Metadata: ink-3 on selected row (wash)", t.ink_3, t.wash, 4.5),
        ("Due soon: accent-strong on pane", t.accent_strong, t.pane, 4.5),
        ("Overdue badge text on badge", t.danger_ink, t.danger_bg, 4.5),
        ("Toast text on toast", t.toast_ink, t.toast, 4.5),
        ("Toast \"just now\" on toast", t.toast_ink_2, t.toast, 4.5),
        ("Toast Undo label on toast", t.toast_accent, t.toast, 4.5),
        ("Primary button label on accent-fill", t.on_accent, t.accent_fill, 4.5),
        ("Checked box (accent) on pane", t.accent, t.pane, 3.0),
        ("Checkbox ring (control) on pane", t.control, t.pane, 3.0),
        ("Checkbox ring on selected row", t.control, t.wash, 3.0),
        ("Quick-add border (control) on pane", t.control, t.pane, 3.0),
        ("Overdue ring (danger) on pane", t.danger, t.pane, 3.0),
    ];
    pairs
        .into_iter()
        .zip(e)
        .map(|((label, fg, bg, target), (ef, eb, ratio))| row(label, (fg, bg), (ef, eb), ratio, target))
        .collect()
}

/// The landscape pairs specific to the Playful header.
#[rustfmt::skip]
fn landscape_rows(t: &Palette, e: [(&'static str, &'static str, f64); 5]) -> Vec<Row> {
    let pairs: [(&'static str, Rgb, Rgb, f64); 5] = [
        ("Header summary: ink-2 on sky", t.ink_2, t.sky, 4.5),
        ("Header estimate: accent-strong on sky", t.accent_strong, t.sky, 4.5),
        ("Sun path, remaining (arc) on sky", t.arc, t.sky, 3.0),
        ("Sun disc vs sky (non-text)", t.sun, t.sky, 1.0),
        ("Near hill vs pane edge (dark silhouette)", t.hill_near, t.pane, 1.0),
    ];
    pairs
        .into_iter()
        .zip(e)
        .map(|((label, fg, bg, target), (ef, eb, ratio))| row(label, (fg, bg), (ef, eb), ratio, target))
        .collect()
}

#[test]
fn android_panorama_keeps_text_and_progress_legible_at_each_gradient_stop() {
    use todo_design::palette::{dusk, phone_arc};

    for theme in Theme::ALL {
        for hue in (0..360).map(f64::from) {
            let t = palette(theme, hue);
            let arc = phone_arc(theme, hue);
            for bg in [t.canvas, t.sky, t.horizon] {
                for fg in [t.ink, t.ink_2, t.ink_3, t.accent_strong, t.danger_ink] {
                    assert!(fg.contrast(bg) >= 4.5, "{theme:?}, {hue}, {fg:?} on {bg:?}");
                }
                assert!(arc.contrast(bg) >= 3.0, "{theme:?}, {hue}, arc on {bg:?}");
                assert!(
                    t.sun_ink.contrast(bg) >= 3.0,
                    "{theme:?}, {hue}, sun-ink on {bg:?}"
                );
            }
            for fg in [t.ink, t.ink_2, t.danger_ink] {
                assert!(fg.contrast(t.selection_wash) >= 4.5);
            }
            let night = palette(Theme::Dark, hue);
            let zenith = dusk(hue).top;
            for fg in [night.ink, night.ink_2, night.ink_3, night.accent_strong] {
                assert!(fg.contrast(zenith) >= 4.5);
            }
        }
    }
}

const PLANNED: f64 = 150.0;
const HEALTH: f64 = 352.0;
const CLAUDE: f64 = 38.0;
const PI_AGENT: f64 = 140.0;

#[test]
#[rustfmt::skip]
fn light_my_day() {
    let t = palette(Theme::Light, hues::MY_DAY);
    let mut rows = base_rows(&t, [
        ("#241c15", "#fefbf9", 16.28), ("#4f463d", "#fefbf9", 8.96), ("#6c6158", "#fefbf9", 5.84),
        ("#6c6158", "#f7f1ec", 5.37), ("#6c6158", "#fdecdc", 5.22), ("#824c00", "#fefbf9", 6.83),
        ("#9a2925", "#ffe7e6", 6.56), ("#f6f3ef", "#28211b", 14.34), ("#bcb6b1", "#28211b", 7.91),
        ("#f3ad66", "#28211b", 8.30), ("#ffffff", "#9e5d00", 5.22), ("#cc7d1b", "#fefbf9", 3.13),
        ("#8f7c6b", "#fefbf9", 3.87), ("#8f7c6b", "#fdecdc", 3.46), ("#8f7c6b", "#fefbf9", 3.87),
        ("#ba3630", "#fefbf9", 5.57),
    ]);
    let planned = list_colors(Theme::Light, PLANNED);
    let health = list_colors(Theme::Light, HEALTH);
    let pi = agent_colors(Theme::Light, PI_AGENT);
    rows.push(row("List chip text, worst hue (planned)", (planned.ink, planned.tint), ("#15592b", "#d7f2db"), 7.06, 4.5));
    rows.push(row("Active room pill text, worst hue (health)", (health.on_strong, health.strong), ("#311422", "#d97ca7"), 5.90, 4.5));
    rows.push(row("Agent letter on badge, worst (Pi)", (pi.ink, pi.bg), ("#225c13", "#daf1d5"), 6.72, 4.5));
    rows.extend(landscape_rows(&t, [
        ("#4f463d", "#ffefe1", 8.21), ("#824c00", "#ffefe1", 6.26), ("#a2866c", "#ffefe1", 3.03),
        ("#f5ae39", "#ffefe1", 1.70), ("#f8c799", "#fefbf9", 1.50),
    ]));
    check("Light · My Day (hue 65)", rows);
}

#[test]
#[rustfmt::skip]
fn dark_work() {
    let t = palette(Theme::Dark, 200.0);
    let mut rows = base_rows(&t, [
        ("#e7efee", "#0e1918", 15.35), ("#b9c3c2", "#0e1918", 9.95), ("#919f9d", "#0e1918", 6.53),
        ("#919f9d", "#071110", 6.98), ("#919f9d", "#112928", 5.58), ("#7ddad4", "#0e1918", 10.98),
        ("#ffbfb6", "#4c221f", 8.60), ("#eff5f4", "#253432", 11.78), ("#b1b9b9", "#253432", 6.50),
        ("#74e0d9", "#253432", 8.30), ("#001b1a", "#4ac0b9", 8.13), ("#4ac0b9", "#0e1918", 8.14),
        ("#6c8684", "#0e1918", 4.60), ("#6c8684", "#112928", 3.92), ("#6c8684", "#0e1918", 4.60),
        ("#f47c6e", "#0e1918", 6.79),
    ]);
    let health = list_colors(Theme::Dark, HEALTH);
    let claude = agent_colors(Theme::Dark, CLAUDE);
    rows.push(row("List chip text, worst hue (health)", (health.ink, health.tint), ("#fdc4e2", "#402333"), 9.41, 4.5));
    rows.push(row("Active room pill text, worst hue (health)", (health.on_strong, health.strong), ("#301524", "#d489b2"), 6.37, 4.5));
    rows.push(row("Agent letter on badge, worst (Claude)", (claude.ink, claude.bg), ("#ffc9b9", "#4b281e"), 8.82, 4.5));
    rows.extend(landscape_rows(&t, [
        ("#b9c3c2", "#0c2220", 9.21), ("#7ddad4", "#0c2220", 10.16), ("#517471", "#0c2220", 3.23),
        ("#f8b14e", "#0c2220", 9.00), ("#061413", "#0e1918", 1.05),
    ]));
    check("Dark · Work (hue 200)", rows);
}

#[test]
#[rustfmt::skip]
#[allow(clippy::approx_constant, reason = "3.14 here is a contrast ratio from the table")]
fn dark_personal_inner_screen() {
    let t = palette_scaled(Theme::Dark, 10.0, 0.85);
    let mut rows = base_rows(&t, [
        ("#f1ebec", "#1c1416", 15.36), ("#c7bec0", "#1c1416", 9.95), ("#a3989b", "#1c1416", 6.48),
        ("#a3989b", "#140d0f", 6.87), ("#a3989b", "#2f1f24", 5.60), ("#f2b3c5", "#1c1416", 10.36),
        ("#ffbfb6", "#4c221f", 8.60), ("#f7f2f3", "#382d30", 11.95), ("#bcb5b7", "#382d30", 6.58),
        ("#fab4c8", "#382d30", 7.83), ("#220f15", "#da93a8", 7.60), ("#da93a8", "#1c1416", 7.51),
        ("#8e7a7f", "#1c1416", 4.51), ("#8e7a7f", "#2f1f24", 3.90), ("#8e7a7f", "#1c1416", 4.51),
        ("#f47c6e", "#1c1416", 6.85),
    ]);
    rows.extend(landscape_rows(&t, [
        ("#c7bec0", "#27191d", 9.30), ("#f2b3c5", "#27191d", 9.68), ("#7d646b", "#27191d", 3.14),
        ("#f8b14e", "#27191d", 9.16), ("#180e11", "#1c1416", 1.05),
    ]));
    check("Dark · Personal on the inner screen (hue 10, k .85)", rows);
}

/// Stronger than the table: every whole-degree hue, not just the ones in the mockups, must pass every text and
/// control pair, for rooms (at both chroma scales), list chips, active room pills and agent marks.
#[test]
fn every_hue_passes() {
    let mut failures = Vec::new();
    for theme in Theme::ALL {
        for h in 0..360 {
            let h = f64::from(h);
            for k in [1.0, 0.85] {
                let t = palette_scaled(theme, h, k);
                assert!(t.selection_wash.contrast(t.pane) > t.wash.contrast(t.pane));
                let pairs = [
                    ("ink/pane", t.ink, t.pane, 4.5),
                    ("ink-2/pane", t.ink_2, t.pane, 4.5),
                    ("ink-3/pane", t.ink_3, t.pane, 4.5),
                    ("ink-3/canvas", t.ink_3, t.canvas, 4.5),
                    ("ink-3/wash", t.ink_3, t.wash, 4.5),
                    ("accent-strong/pane", t.accent_strong, t.pane, 4.5),
                    ("danger-ink/danger-bg", t.danger_ink, t.danger_bg, 4.5),
                    ("toast-ink/toast", t.toast_ink, t.toast, 4.5),
                    ("toast-ink-2/toast", t.toast_ink_2, t.toast, 4.5),
                    ("toast-accent/toast", t.toast_accent, t.toast, 4.5),
                    ("on-accent/accent-fill", t.on_accent, t.accent_fill, 4.5),
                    ("accent/pane", t.accent, t.pane, 3.0),
                    ("control/pane", t.control, t.pane, 3.0),
                    ("control/wash", t.control, t.wash, 3.0),
                    ("danger/pane", t.danger, t.pane, 3.0),
                    ("ink-2/sky", t.ink_2, t.sky, 4.5),
                    ("accent-strong/sky", t.accent_strong, t.sky, 4.5),
                    ("arc/sky", t.arc, t.sky, 3.0),
                    ("ink-3/hover", t.ink_3, t.hover, 3.0),
                    ("sun-ink/pane", t.sun_ink, t.pane, 3.0),
                    ("sun-ink/hover", t.sun_ink, t.hover, 3.0),
                    ("important-fill/pane", t.important_fill, t.pane, 3.0),
                    ("important-fill/wash", t.important_fill, t.wash, 3.0),
                    ("important-fill/hover", t.important_fill, t.hover, 3.0),
                    ("accent-strong/wash", t.accent_strong, t.wash, 4.5),
                    ("accent-strong/tint", t.accent_strong, t.tint, 4.5),
                    ("accent-strong/canvas", t.accent_strong, t.canvas, 3.0),
                    (
                        "selection-wash/pane",
                        t.selection_wash,
                        t.pane,
                        if theme == Theme::Dark { 1.30 } else { 1.15 },
                    ),
                    ("ink/selection-wash", t.ink, t.selection_wash, 4.5),
                    ("ink-2/selection-wash", t.ink_2, t.selection_wash, 4.5),
                    (
                        "accent-strong/selection-wash",
                        t.accent_strong,
                        t.selection_wash,
                        4.5,
                    ),
                    ("control/selection-wash", t.control, t.selection_wash, 3.0),
                    ("danger/selection-wash", t.danger, t.selection_wash, 3.0),
                    (
                        "important-fill/selection-wash",
                        t.important_fill,
                        t.selection_wash,
                        3.0,
                    ),
                    (
                        "selection-check/selection-wash",
                        t.selection_check,
                        t.selection_wash,
                        3.0,
                    ),
                ];
                for (name, fg, bg, target) in pairs {
                    let r = fg.contrast(bg);
                    if r < target {
                        failures.push(format!("{} room hue {h} k {k}: {name} {r:.2}", theme.key()));
                    }
                }
            }
            let l = list_colors(theme, h);
            let a = agent_colors(theme, h);
            for (name, fg, bg) in [
                ("chip", l.ink, l.tint),
                ("room pill", l.on_strong, l.strong),
                ("agent", a.ink, a.bg),
            ] {
                let r = fg.contrast(bg);
                if r < 4.5 {
                    failures.push(format!("{} hue {h}: {name} {r:.2}", theme.key()));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
