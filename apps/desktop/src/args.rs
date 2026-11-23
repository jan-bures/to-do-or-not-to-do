//! Command-line parsing. Hand-rolled to keep startup lean; every path is unit-tested.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::theme::{Theme, parse_theme};

pub const USAGE: &str = "\
Usage:
  todo-desktop [--background]
  todo-desktop --screenshot <out.png> --fixture <name> [--theme light|dark] [--scale 1|1.25|1.5|2]
               [--overwrite]
  todo-desktop --screenshot-all <out-dir> [--fixture names] [--theme themes] [--scale scales] [--overwrite]
  todo-desktop --bench <startup|hotkey> --metrics <out.txt>

Options:
  --background      Start in the tray without showing the window (used by Start with Windows).
  --screenshot      Render one hidden fixture window, save its PNG and exit.
  --screenshot-all  Render all fixtures, both themes and scales 1 and 1.5 in one hidden run.
                    To select a subset, pass comma-separated fixture names, themes or scales.
  --fixture         Fixture to render: my-day, work, room-menu, account-menu, device-settings,
                    device-invite, device-approve, priority-menu,
                    reminder-menu, reminder-custom, reminder-focus, reminder-none,
                    repeat-menu, repeat-custom, repeat-focus, repeat-none, repeat-weekly, repeat-monthly, delete-tag-confirm,
                    task-context-menu, move-to-picker, merge-task-picker, merge-task-confirm,
                    sync-failures, delete-list-confirm,
                    new-collection, tab-new, tab-new-collection, sidebar-footer, sidebar-footer-min,
                    sidebar-new-menu,
                    collection-menu, list-context-menu,
                    delete-collection-confirm, move-list-picker, list-colour, tag-picker, tag-create, tag-edit, tag-colour,
                    tag-rename, task-links, add-link, attachments-empty, attachments-uploading,
                    attachments-error, attachments-list, collapsed-collection, long-sidebar, short-sidebar,
                    short-sidebar-new-list and short-sidebar-new-collection (960 x 600), long-names,
                    long-names-narrow and narrow-detail (960 px wide), long-task-list,
                    sidebar-wide, detail-narrow, detail-wide, detail-wide-1920 (1920 × 1080),
                    resize-handle-hover, caption-hover,
                    caption-close-hover, empty, finished, finished-short (700 px
                    tall), finished-nothing-tomorrow, handover-complete-50, quick-add, pairing,
                    pairing-code, open-failed or open-failed-confirm.
  --theme           Light or dark (default light).
  --scale           Scale factor to render at (default 1).
  --overwrite       Replace <out.png> if it exists (by default the screenshot refuses to).
  --bench           Measure cold start or hotkey-to-visible latency, write the numbers and exit.
  --metrics         Where --bench writes its numbers.
  -h, --help        Show this help.
  -V, --version     Show the version.
";

/// The fixtures `--screenshot` can render. Invented sample data only.
pub const FIXTURES: &[&str] = &[
    "my-day",
    "toggle-states-on",
    "toggle-states-off",
    "work",
    "room-menu",
    "account-menu",
    "device-settings",
    "device-invite",
    "device-approve",
    "priority-menu",
    "reminder-menu",
    "reminder-custom",
    "reminder-focus",
    "reminder-none",
    "repeat-menu",
    "repeat-custom",
    "repeat-focus",
    "repeat-none",
    "repeat-weekly",
    "repeat-monthly",
    "delete-tag-confirm",
    "task-context-menu",
    "move-to-picker",
    "merge-task-picker",
    "merge-task-confirm",
    "sync-failures",
    "activity",
    "activity-agent",
    "activity-empty",
    "trash",
    "trash-empty",
    "activity-pending",
    "activity-refused",
    "activity-menu",
    "delete-list-confirm",
    "new-collection",
    "tab-new",
    "tab-new-collection",
    "sidebar-footer",
    "sidebar-footer-min",
    "sidebar-new-menu",
    "collection-menu",
    "list-context-menu",
    "delete-collection-confirm",
    "move-list-picker",
    "list-colour",
    "tag-picker",
    "tag-create",
    "tag-edit",
    "tag-colour",
    "tag-rename",
    "task-links",
    "add-link",
    "attachments-empty",
    "attachments-uploading",
    "attachments-error",
    "attachments-list",
    "task-activity",
    "collapsed-collection",
    "long-sidebar",
    "short-sidebar",
    "short-sidebar-new-list",
    "short-sidebar-new-collection",
    "long-names",
    "long-names-narrow",
    "narrow-detail",
    "long-task-list",
    "sidebar-wide",
    "detail-narrow",
    "detail-wide",
    "detail-wide-1920",
    "resize-handle-hover",
    "caption-hover",
    "caption-close-hover",
    "empty",
    "finished",
    "finished-short",
    "finished-nothing-tomorrow",
    "handover-complete-50",
    "quick-add",
    "pairing",
    "pairing-code",
    "open-failed",
    "open-failed-confirm",
    "completing-fill",
    "completing-hold",
    "completing-collapse-50",
    "sun-travel-50",
    "undo-toast",
    "narrow-undo-toast",
    "toast-in-50",
    "reopening-collapse-50",
    "long-list-collapse-80",
    "long-list-end-collapse-50",
    "dusk-50",
    "dusk-75",
    "dusk-one-task-50",
    "room-switch-50",
    "room-hue-50",
    "detail-open-50",
    "detail-dusk-50",
    "collection-collapse-50",
    "scrollbar-fade-50",
];

/// What to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Run(RunOptions),
    Screenshot(Vec<ScreenshotOptions>),
    Bench(BenchOptions),
    Help,
    Version,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct RunOptions {
    /// Start hidden in the tray.
    pub background: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotOptions {
    pub out: PathBuf,
    pub fixture: String,
    pub theme: Theme,
    pub scale: f32,
    /// Replace an existing file at `out`.
    pub overwrite: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchKind {
    /// Process start to the first presented frame of the main window.
    Startup,
    /// Hotkey handler to the quick-add window's first presented frame.
    Hotkey,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchOptions {
    pub kind: BenchKind,
    pub metrics: PathBuf,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ArgError {
    #[error("unknown argument `{0}`")]
    Unknown(String),
    #[error("`{0}` needs a value")]
    MissingValue(&'static str),
    #[error("`{0}` was given more than once")]
    Duplicate(&'static str),
    #[error("unknown fixture `{0}` (expected one of: {list})", list = FIXTURES.join(", "))]
    UnknownFixture(String),
    #[error("unknown theme `{0}` (expected light or dark)")]
    UnknownTheme(String),
    #[error("unsupported scale `{0}` (expected 1, 1.25, 1.5 or 2)")]
    UnsupportedScale(String),
    #[error("unknown benchmark `{0}` (expected startup or hotkey)")]
    UnknownBench(String),
    #[error("`--screenshot` needs `--fixture`")]
    ScreenshotNeedsFixture,
    #[error("`--bench` needs `--metrics`")]
    BenchNeedsMetrics,
    #[error("`{0}` only applies together with `{1}`")]
    Orphan(&'static str, &'static str),
    #[error("`{0}` and `{1}` can't be combined")]
    Conflict(&'static str, &'static str),
    #[error("argument is not valid Unicode: {0:?}")]
    NotUnicode(OsString),
}

const SCALES: &[(&str, f32)] = &[
    ("1", 1.0),
    ("1.0", 1.0),
    ("1.25", 1.25),
    ("1.5", 1.5),
    ("2", 2.0),
    ("2.0", 2.0),
];

#[derive(Default)]
struct Raw {
    background: bool,
    overwrite: bool,
    screenshot: Option<PathBuf>,
    gallery: Option<PathBuf>,
    fixture: Option<String>,
    theme: Option<String>,
    scale: Option<String>,
    bench: Option<String>,
    metrics: Option<PathBuf>,
    help: bool,
    version: bool,
}

fn set<T>(slot: &mut Option<T>, flag: &'static str, value: T) -> Result<(), ArgError> {
    if slot.is_some() {
        return Err(ArgError::Duplicate(flag));
    }
    *slot = Some(value);
    Ok(())
}

/// Parses the arguments after the program name.
pub fn parse<I>(args: I) -> Result<Command, ArgError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut raw = Raw::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let arg = arg.into_string().map_err(ArgError::NotUnicode)?;
        // Accept `--flag=value` as well as `--flag value`.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &'static str| -> Result<String, ArgError> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it
                    .next()
                    .ok_or(ArgError::MissingValue(name))?
                    .into_string()
                    .map_err(ArgError::NotUnicode),
            }
        };
        match flag.as_str() {
            "--background" => raw.background = true,
            "--overwrite" => raw.overwrite = true,
            "--screenshot" => set(
                &mut raw.screenshot,
                "--screenshot",
                value("--screenshot")?.into(),
            )?,
            "--screenshot-all" => set(
                &mut raw.gallery,
                "--screenshot-all",
                value("--screenshot-all")?.into(),
            )?,
            "--fixture" => set(&mut raw.fixture, "--fixture", value("--fixture")?)?,
            "--theme" => set(&mut raw.theme, "--theme", value("--theme")?)?,
            "--scale" => set(&mut raw.scale, "--scale", value("--scale")?)?,
            "--bench" => set(&mut raw.bench, "--bench", value("--bench")?)?,
            "--metrics" => set(&mut raw.metrics, "--metrics", value("--metrics")?.into())?,
            "-h" | "--help" => raw.help = true,
            "-V" | "--version" => raw.version = true,
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    build(raw)
}

/// Validate each axis before expanding the gallery. Aliases (e.g. 1 and 1.0) and repeated
/// selections must not render the same file twice, particularly without --overwrite.
fn selection<T: PartialEq>(
    arg: Option<&str>,
    defaults: &[&str],
    gallery: bool,
    parse: impl Fn(&str) -> Result<T, ArgError>,
) -> Result<Vec<T>, ArgError> {
    let mut values = Vec::new();
    let mut add = |s: &str| -> Result<(), ArgError> {
        let v = parse(s)?;
        if !values.contains(&v) {
            values.push(v);
        }
        Ok(())
    };
    match arg {
        Some(s) if gallery => s.split(',').try_for_each(&mut add)?,
        Some(s) => add(s)?,
        None => defaults.iter().copied().try_for_each(add)?,
    }
    Ok(values)
}

fn build(raw: Raw) -> Result<Command, ArgError> {
    if raw.help {
        return Ok(Command::Help);
    }
    if raw.version {
        return Ok(Command::Version);
    }
    if raw.screenshot.is_some() && raw.gallery.is_some() {
        return Err(ArgError::Conflict("--screenshot", "--screenshot-all"));
    }
    let gallery = raw.gallery.is_some();
    if let Some(out) = raw.screenshot.or(raw.gallery) {
        let mode = if gallery {
            "--screenshot-all"
        } else {
            "--screenshot"
        };
        if raw.bench.is_some() {
            return Err(ArgError::Conflict(mode, "--bench"));
        }
        if raw.background {
            return Err(ArgError::Conflict(mode, "--background"));
        }
        if raw.metrics.is_some() {
            return Err(ArgError::Orphan("--metrics", "--bench"));
        }
        if !gallery && raw.fixture.is_none() {
            return Err(ArgError::ScreenshotNeedsFixture);
        }
        let fixtures = selection(raw.fixture.as_deref(), FIXTURES, gallery, |s| {
            FIXTURES
                .iter()
                .copied()
                .find(|&f| f == s)
                .ok_or_else(|| ArgError::UnknownFixture(s.into()))
        })?;
        let themes = selection(
            raw.theme.as_deref(),
            if gallery {
                &["light", "dark"]
            } else {
                &["light"]
            },
            gallery,
            |s| parse_theme(s).ok_or_else(|| ArgError::UnknownTheme(s.into())),
        )?;
        let scales = selection(
            raw.scale.as_deref(),
            if gallery { &["1", "1.5"] } else { &["1"] },
            gallery,
            |s| {
                SCALES
                    .iter()
                    .find(|&&(name, _)| name == s)
                    .map(|&(_, v)| v)
                    .ok_or_else(|| ArgError::UnsupportedScale(s.into()))
            },
        )?;
        let mut shots = Vec::new();
        for fixture in fixtures {
            for &theme in &themes {
                for &scale in &scales {
                    let path = if gallery {
                        out.join(format!(
                            "{fixture}-{}-{}.png",
                            theme.key(),
                            (scale * 100.) as u32
                        ))
                    } else {
                        out.clone()
                    };
                    shots.push(ScreenshotOptions {
                        out: path,
                        fixture: fixture.into(),
                        theme,
                        scale,
                        overwrite: raw.overwrite,
                    });
                }
            }
        }
        return Ok(Command::Screenshot(shots));
    }
    for (present, flag) in [
        (raw.fixture.is_some(), "--fixture"),
        (raw.theme.is_some(), "--theme"),
        (raw.scale.is_some(), "--scale"),
        (raw.overwrite, "--overwrite"),
    ] {
        if present {
            return Err(ArgError::Orphan(flag, "--screenshot"));
        }
    }
    if let Some(kind) = raw.bench {
        if raw.background {
            return Err(ArgError::Conflict("--bench", "--background"));
        }
        let kind = match kind.as_str() {
            "startup" => BenchKind::Startup,
            "hotkey" => BenchKind::Hotkey,
            _ => return Err(ArgError::UnknownBench(kind)),
        };
        let metrics = raw.metrics.ok_or(ArgError::BenchNeedsMetrics)?;
        return Ok(Command::Bench(BenchOptions { kind, metrics }));
    }
    if raw.metrics.is_some() {
        return Err(ArgError::Orphan("--metrics", "--bench"));
    }
    Ok(Command::Run(RunOptions {
        background: raw.background,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, ArgError> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_runs_the_app_in_the_foreground() {
        assert_eq!(p(&[]), Ok(Command::Run(RunOptions::default())));
    }

    #[test]
    fn background() {
        assert_eq!(
            p(&["--background"]),
            Ok(Command::Run(RunOptions { background: true }))
        );
        // The fonts are embedded now; the old option is gone.
        assert_eq!(
            p(&["--fonts-dir", "x"]),
            Err(ArgError::Unknown("--fonts-dir".into()))
        );
    }

    #[test]
    fn screenshot_with_defaults() {
        assert_eq!(
            p(&["--screenshot", "out.png", "--fixture", "my-day"]),
            Ok(Command::Screenshot(vec![ScreenshotOptions {
                out: "out.png".into(),
                fixture: "my-day".into(),
                theme: Theme::Light,
                scale: 1.0,
                overwrite: false,
            }]))
        );
    }

    #[test]
    fn screenshot_with_everything_in_any_order_and_equals_syntax() {
        assert_eq!(
            p(&[
                "--scale=1.5",
                "--theme",
                "dark",
                "--fixture=work",
                "--screenshot",
                "g/work.png",
                "--overwrite"
            ]),
            Ok(Command::Screenshot(vec![ScreenshotOptions {
                out: "g/work.png".into(),
                fixture: "work".into(),
                theme: Theme::Dark,
                scale: 1.5,
                overwrite: true,
            }]))
        );
    }

    #[test]
    fn gallery_uses_only_the_fixture_runner() {
        let Ok(Command::Screenshot(shots)) = p(&["--screenshot-all", "g"]) else {
            panic!("gallery must not enter interactive startup")
        };
        assert_eq!(shots.len(), FIXTURES.len() * 4);
        assert_eq!(
            shots
                .iter()
                .map(|s| &s.out)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            shots.len()
        );
        let Ok(Command::Screenshot(shots)) = p(&[
            "--screenshot-all",
            "g",
            "--theme",
            "dark",
            "--fixture",
            "work,quick-add,work",
            "--scale",
            "1.25,1.25",
        ]) else {
            panic!("valid selection")
        };
        assert_eq!(shots.len(), 2);
        assert_eq!(
            shots[1].out,
            PathBuf::from("g").join("quick-add-dark-125.png")
        );
        for extra in [
            ["--screenshot", "x"],
            ["--bench", "hotkey"],
            ["--fixture", "../x"],
            ["--theme", ""],
            ["--scale", "3"],
        ] {
            assert!(p(&["--screenshot-all", "g", extra[0], extra[1]]).is_err());
        }
        assert!(p(&["--screenshot-all", "g", "--background"]).is_err());
    }

    #[test]
    fn screenshot_rejects_bad_values() {
        assert_eq!(
            p(&["--screenshot", "o.png"]),
            Err(ArgError::ScreenshotNeedsFixture)
        );
        assert_eq!(
            p(&["--screenshot", "o.png", "--fixture", "inbox"]),
            Err(ArgError::UnknownFixture("inbox".into()))
        );
        assert_eq!(
            p(&[
                "--screenshot",
                "o.png",
                "--fixture",
                "work",
                "--theme",
                "sepia"
            ]),
            Err(ArgError::UnknownTheme("sepia".into()))
        );
        assert_eq!(
            p(&["--screenshot", "o.png", "--fixture", "work", "--scale", "3"]),
            Err(ArgError::UnsupportedScale("3".into()))
        );
        assert_eq!(
            p(&["--screenshot"]),
            Err(ArgError::MissingValue("--screenshot"))
        );
        assert_eq!(
            p(&[
                "--screenshot",
                "a.png",
                "--screenshot",
                "b.png",
                "--fixture",
                "work"
            ]),
            Err(ArgError::Duplicate("--screenshot"))
        );
    }

    #[test]
    fn screenshot_only_options_need_screenshot() {
        assert_eq!(
            p(&["--fixture", "work"]),
            Err(ArgError::Orphan("--fixture", "--screenshot"))
        );
        assert_eq!(
            p(&["--theme", "dark"]),
            Err(ArgError::Orphan("--theme", "--screenshot"))
        );
        assert_eq!(
            p(&["--scale", "1.5"]),
            Err(ArgError::Orphan("--scale", "--screenshot"))
        );
        assert_eq!(
            p(&["--overwrite"]),
            Err(ArgError::Orphan("--overwrite", "--screenshot"))
        );
    }

    #[test]
    fn conflicting_modes_are_rejected() {
        assert_eq!(
            p(&["--screenshot", "o.png", "--fixture", "work", "--background"]),
            Err(ArgError::Conflict("--screenshot", "--background"))
        );
        assert_eq!(
            p(&[
                "--screenshot",
                "o.png",
                "--fixture",
                "work",
                "--bench",
                "startup"
            ]),
            Err(ArgError::Conflict("--screenshot", "--bench"))
        );
    }

    #[test]
    fn bench_modes() {
        assert_eq!(
            p(&["--bench", "hotkey", "--metrics", "m.txt"]),
            Ok(Command::Bench(BenchOptions {
                kind: BenchKind::Hotkey,
                metrics: "m.txt".into(),
            }))
        );
        assert_eq!(p(&["--bench", "startup"]), Err(ArgError::BenchNeedsMetrics));
        assert_eq!(
            p(&["--bench", "warm", "--metrics", "m.txt"]),
            Err(ArgError::UnknownBench("warm".into()))
        );
        assert_eq!(
            p(&["--metrics", "m.txt"]),
            Err(ArgError::Orphan("--metrics", "--bench"))
        );
    }

    #[test]
    fn help_version_and_unknown() {
        assert_eq!(p(&["--help"]), Ok(Command::Help));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
        assert_eq!(
            p(&["--frobnicate"]),
            Err(ArgError::Unknown("--frobnicate".into()))
        );
        assert_eq!(p(&["stray"]), Err(ArgError::Unknown("stray".into())));
    }

    #[test]
    fn every_listed_fixture_parses() {
        for f in FIXTURES {
            assert!(p(&["--screenshot", "o.png", "--fixture", f]).is_ok(), "{f}");
        }
    }
}
