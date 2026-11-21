//! `todo-backup`: encrypted SQLite backups to S3-compatible storage. See the crate docs and
//! `deploy/windows/README.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use todo_backup::budget::SystemClock;
use todo_backup::config::Config;
use todo_backup::ops::{self, RunOutcome, Target};
use todo_backup::store::S3Store;
use todo_backup::{Error, Kind, crypto};

/// Exit code when the monthly operations budget refuses the run.
const EXIT_BUDGET: u8 = 3;
/// Exit code when a restore drill runs but finds a mismatch.
const EXIT_DRILL_FAILED: u8 = 4;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Encrypted SQLite backups to S3-compatible storage (Cloudflare R2)"
)]
struct Cli {
    /// Path to backup.toml.
    #[arg(long, global = true, env = "TODO_BACKUP_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Snapshot the database and upload it, unless nothing changed since the last backup of this kind.
    Run {
        #[arg(long, value_parser = parse_kind)]
        kind: Kind,
        /// Upload even if nothing changed.
        #[arg(long)]
        force: bool,
    },
    /// Download, decrypt and check a backup, writing it to a new file.
    ///
    /// To put it into service: stop the server, move the file into place, and run
    /// `todo-server rotate-log-epoch` before starting the server again, so that clients that followed
    /// the replaced log resync instead of mixing the two histories.
    Restore {
        /// The object key, for example frequent/20261121T060000Z-42.sqlite3.zst.age.
        #[arg(long, conflicts_with_all = ["latest", "kind"], required_unless_present = "latest")]
        object: Option<String>,
        /// Restore the newest backup of --kind. Costs one LIST.
        #[arg(long, requires = "kind")]
        latest: bool,
        #[arg(long, value_parser = parse_kind)]
        kind: Option<Kind>,
        /// The age identity file (from `keygen`).
        #[arg(long)]
        identity: PathBuf,
        /// Where to write the database. Must not exist.
        #[arg(long)]
        out: PathBuf,
    },
    /// Restore the latest backup to a scratch directory and verify it against the upload record.
    Drill {
        #[arg(long)]
        identity: PathBuf,
        /// Only consider this kind; by default the newest of either.
        #[arg(long, value_parser = parse_kind)]
        kind: Option<Kind>,
    },
    /// Generate an age X25519 identity and print its public recipient.
    Keygen {
        /// Where to write the identity. Must not exist.
        #[arg(long)]
        out: PathBuf,
    },
    /// Show this month's operation counts and the last uploads. Sends no requests.
    Status,
}

fn parse_kind(s: &str) -> Result<Kind, String> {
    s.parse().map_err(|e: Error| e.to_string())
}

fn main() -> ExitCode {
    let mut filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // The HTTP stack can log request URLs, and presigned URLs carry the credential scope and signature.
    // Keep it quiet whatever RUST_LOG says.
    for directive in ["ureq=warn", "ureq_proto=warn", "rustls=warn"] {
        if let Ok(d) = directive.parse() {
            filter = filter.add_directive(d);
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("todo-backup: {e:#}");
            if matches!(
                e.downcast_ref::<Error>(),
                Some(Error::BudgetExceeded { .. })
            ) {
                ExitCode::from(EXIT_BUDGET)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn load_config(path: Option<PathBuf>) -> Result<Config> {
    let path = path.context("no config given: pass --config or set TODO_BACKUP_CONFIG")?;
    Ok(Config::load(&path)?)
}

fn store(cfg: &Config) -> Result<S3Store> {
    Ok(S3Store::new(&cfg.credentials()?)?)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let now = jiff::Timestamp::now();
    match cli.command {
        Command::Keygen { out } => {
            let recipient = crypto::keygen(&out, now)?;
            println!("{recipient}");
            eprintln!(
                "Wrote the identity to {}.\n\
                 1. Store that file's contents in your password manager.\n\
                 2. Delete the file from this machine; backups never need it here.\n\
                 3. Put only the public recipient above in backup.toml as `recipient`.",
                out.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Run { kind, force } => {
            let cfg = load_config(cli.config)?;
            let store = store(&cfg)?;
            match ops::run(&cfg, &store, kind, force, SystemClock)? {
                RunOutcome::Uploaded(r) => {
                    println!("uploaded {} ({} bytes)", r.key, r.object_bytes)
                }
                RunOutcome::Unchanged(r) => println!("unchanged since {}; nothing uploaded", r.key),
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Restore {
            object,
            latest,
            kind,
            identity,
            out,
        } => {
            let cfg = load_config(cli.config)?;
            let identities = crypto::load_identities(&identity)?;
            let target = match (object, latest, kind) {
                (Some(key), _, _) => Target::Key(key),
                (None, true, Some(kind)) => Target::Latest(kind),
                _ => anyhow::bail!("pass --object <key>, or --latest --kind <kind>"),
            };
            let store = store(&cfg)?;
            let r = ops::restore(&cfg, &store, &target, &identities, &out, SystemClock)?;
            println!(
                "restored {} to {} ({} bytes, integrity ok, max(seq) {:?}, {:?} events)",
                r.key,
                out.display(),
                r.bytes,
                r.info.max_seq,
                r.info.events_count
            );
            println!(
                "to use it: stop the server, move it into place, then run `todo-server rotate-log-epoch` before starting the server"
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Drill { identity, kind } => {
            let cfg = load_config(cli.config)?;
            let identities = crypto::load_identities(&identity)?;
            let store = store(&cfg)?;
            let report = ops::drill(&cfg, &store, &identities, kind, SystemClock)
                .context("restore drill FAILED")?;
            print!("{report}");
            Ok(if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_DRILL_FAILED)
            })
        }
        Command::Status => {
            let cfg = load_config(cli.config)?;
            print!("{}", ops::status(&cfg, now)?);
            Ok(ExitCode::SUCCESS)
        }
    }
}
