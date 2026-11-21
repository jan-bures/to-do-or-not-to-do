//! Command-line parsing. Hand-rolled to keep startup lean; every path is unit-tested.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::theme::{Theme, parse_theme};

pub const USAGE: &str = "\
Usage:
  todo-desktop [--background]
  todo-desktop --screenshot <out.png> --fixture <name> [--theme light|dark] [--scale 1|1.25|1.5|2]
               [--overwrite]
  todo-desktop --bench <startup|hotkey> --metrics <out.txt>

Options:
  --background      Start in the tray without showing the window (used by Start with Windows).
  --screenshot      Render a fixture into its own window, save that window to a PNG and exit.
  --fixture         Fixture to render: my-day, work, empty, finished, quick-add,
                    pairing, pairing-code, open-failed or open-failed-confirm.
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
    "work",
    "empty",
    "finished",
    "quick-add",
    "pairing",
    "pairing-code",
    "open-failed",
    "open-failed-confirm",
];

/// What to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Run(RunOptions),
    Screenshot(ScreenshotOptions),
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

fn build(raw: Raw) -> Result<Command, ArgError> {
    if raw.help {
        return Ok(Command::Help);
    }
    if raw.version {
        return Ok(Command::Version);
    }
    if raw.screenshot.is_some() && raw.bench.is_some() {
        return Err(ArgError::Conflict("--screenshot", "--bench"));
    }
    if let Some(out) = raw.screenshot {
        if raw.background {
            return Err(ArgError::Conflict("--screenshot", "--background"));
        }
        if raw.metrics.is_some() {
            return Err(ArgError::Orphan("--metrics", "--bench"));
        }
        let fixture = raw.fixture.ok_or(ArgError::ScreenshotNeedsFixture)?;
        if !FIXTURES.contains(&fixture.as_str()) {
            return Err(ArgError::UnknownFixture(fixture));
        }
        let theme = match raw.theme {
            None => Theme::Light,
            Some(t) => parse_theme(&t).ok_or(ArgError::UnknownTheme(t))?,
        };
        let scale = match raw.scale {
            None => 1.0,
            Some(s) => SCALES
                .iter()
                .find(|(name, _)| *name == s)
                .map(|(_, v)| *v)
                .ok_or(ArgError::UnsupportedScale(s))?,
        };
        return Ok(Command::Screenshot(ScreenshotOptions {
            out,
            fixture,
            theme,
            scale,
            overwrite: raw.overwrite,
        }));
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
            Ok(Command::Screenshot(ScreenshotOptions {
                out: "out.png".into(),
                fixture: "my-day".into(),
                theme: Theme::Light,
                scale: 1.0,
                overwrite: false,
            }))
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
            Ok(Command::Screenshot(ScreenshotOptions {
                out: "g/work.png".into(),
                fixture: "work".into(),
                theme: Theme::Dark,
                scale: 1.5,
                overwrite: true,
            }))
        );
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
