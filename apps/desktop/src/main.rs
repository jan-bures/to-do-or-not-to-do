//! The to-do desktop app (GPUI). See `docs/desktop.md`.

// Release builds don't open a console window; `win::attach_parent_console` reconnects to a terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod args;
mod assets;
mod fixtures;
mod fonts;
mod geometry;
mod metrics;
mod theme;
mod ui;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod screenshot;

use std::process::ExitCode;

use args::Command;

#[cfg(windows)]
fn main() -> ExitCode {
    platform::win::attach_parent_console();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let command = match args::parse(std::env::args_os().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            print!("{}", args::USAGE);
            Ok(0)
        }
        Command::Version => {
            println!("todo-desktop {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Command::Screenshot(opts) => screenshot::run(opts),
        Command::Bench(opts) => {
            let run = args::RunOptions {
                background: false,
                fonts_dir: opts.fonts_dir.clone(),
            };
            app::run(run, Some(opts)).map(|()| 0)
        }
        Command::Run(opts) => app::run(opts, None).map(|()| 0),
    };
    match result {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("The desktop app supports Windows only for now (see docs/desktop.md).");
    ExitCode::FAILURE
}
