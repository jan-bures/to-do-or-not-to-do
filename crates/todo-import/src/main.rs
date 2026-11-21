//! `todo-import`: Microsoft To Do importer. See the library documentation for the design.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use todo_import::apply::{ApplyOptions, apply, default_journal, plan};
use todo_import::bundle::{BuildOptions, Bundle, build};
use todo_import::cache::Cache;
use todo_import::graph::Export;
use todo_import::http::Client;

/// Environment variable that may hold the API token (instead of `--token-file`).
const TOKEN_ENV: &str = "TODO_IMPORT_TOKEN";

#[derive(Parser)]
#[command(
    version,
    about = "Imports Microsoft To Do data (Graph export + cache copy) into the to-do server"
)]
struct Cli {
    /// Log at debug level (may include task content; never the token).
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Builds an import bundle from a Graph export and a copy of the To Do cache.
    Bundle {
        /// The Graph export directory (with lists.json).
        #[arg(long)]
        export: PathBuf,
        /// A copy of todosqlite.db (never the live file).
        #[arg(long)]
        cache: PathBuf,
        /// Where to write the bundle. Must be outside any git work tree.
        #[arg(long)]
        out: PathBuf,
        /// The owner's IANA time zone for due dates (default: this machine's).
        #[arg(long)]
        time_zone: Option<String>,
    },
    /// Compares a bundle with the raw Graph export. Exits 1 on any mismatch.
    Verify {
        /// The Graph export directory.
        #[arg(long)]
        export: PathBuf,
        /// The bundle.
        #[arg(long)]
        bundle: PathBuf,
        /// Show list names in the report (personal data: keep the output private).
        #[arg(long)]
        show_names: bool,
    },
    /// Sends a bundle to the server. Safe to re-run; resumes after a crash.
    Apply {
        /// The bundle.
        #[arg(long)]
        bundle: PathBuf,
        /// Server base URL, for example https://todo.example.com.
        #[arg(long)]
        server: String,
        /// File holding an owner API token (or set TODO_IMPORT_TOKEN).
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Print the plan and counts; send nothing.
        #[arg(long)]
        dry_run: bool,
        /// Commands per batch (the resume journal is synced after each).
        #[arg(long, default_value_t = 50)]
        batch_size: usize,
        /// Resume journal (default: next to the bundle, one per server).
        #[arg(long)]
        journal: Option<PathBuf>,
    },
    /// Reads everything back from the server and checks it against the bundle. Exits 1 on mismatch.
    VerifyServer {
        /// The bundle.
        #[arg(long)]
        bundle: PathBuf,
        /// Server base URL.
        #[arg(long)]
        server: String,
        /// File holding an owner API token (or set TODO_IMPORT_TOKEN).
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
}

fn token(file: Option<&Path>) -> Result<String> {
    let t = match file {
        Some(p) => std::fs::read_to_string(p)
            .with_context(|| format!("reading the token file {}", p.display()))?,
        None => std::env::var(TOKEN_ENV)
            .with_context(|| format!("pass --token-file or set {TOKEN_ENV}"))?,
    };
    let t = t.trim().to_owned();
    if t.is_empty() {
        bail!("the token is empty");
    }
    Ok(t)
}

/// The git work tree containing `path`, if any.
fn git_work_tree(path: &Path) -> Option<PathBuf> {
    let abs = std::path::absolute(path).ok()?;
    abs.ancestors()
        .skip(1)
        .find(|d| d.join(".git").exists())
        .map(Path::to_owned)
}

fn time_zone(name: Option<&str>) -> Result<(jiff::tz::TimeZone, String)> {
    match name {
        Some(n) => {
            let tz =
                jiff::tz::TimeZone::get(n).with_context(|| format!("unknown time zone `{n}`"))?;
            Ok((tz, n.to_owned()))
        }
        None => {
            let tz = jiff::tz::TimeZone::system();
            let name = tz
                .iana_name()
                .map(str::to_owned)
                .context("this machine's time zone has no IANA name; pass --time-zone")?;
            Ok((tz, name))
        }
    }
}

fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Cmd::Bundle {
            export,
            cache,
            out,
            time_zone: tz,
        } => {
            if let Some(repo) = git_work_tree(&out) {
                bail!(
                    "refusing to write personal data into the git work tree at {}; choose an --out outside it",
                    repo.display()
                );
            }
            let (tz, tz_name) = time_zone(tz.as_deref())?;
            let export = Export::load(&export).context("reading the export")?;
            let cache = Cache::load(&cache).context("reading the cache copy")?;
            let bundle = build(
                &export,
                &cache,
                &BuildOptions {
                    time_zone: tz,
                    time_zone_name: tz_name,
                },
            )?;
            bundle.write(&out)?;
            let c = &bundle.counts;
            let r = &bundle.report;
            println!(
                "bundle: {} commands -> {}\n  {} collections, {} lists, {} tags, {} tasks ({} open, {} completed), {} steps, {} links ({} email), {} attachments ({} bytes), {} reminders\n  {} due dates, {} high priority, {} with notes, {} recurring\n  ignored: {} orphan cache steps, {} cache-only lists, {} stale My Day marks; {} tasks and {} steps without cache order; {} due-date mismatches with the cache\n  unmapped recurrence: {}; warnings: {}",
                bundle.items.len(),
                out.display(),
                c.collections,
                c.lists,
                c.tags,
                c.tasks(),
                c.tasks_open,
                c.tasks_completed,
                c.steps,
                c.links,
                c.email_links,
                c.attachments,
                c.attachment_bytes,
                c.reminders,
                c.due_dates,
                c.high_priority,
                c.notes,
                c.recurring,
                r.orphan_cache_steps,
                r.cache_only_lists,
                r.my_day_marks_skipped,
                r.tasks_without_cache_order,
                r.steps_without_cache_order,
                r.due_date_mismatches,
                r.unmapped_recurrence.len(),
                r.warnings.len()
            );
            for w in &r.warnings {
                println!("  warning: {w}");
            }
            for u in &r.unmapped_recurrence {
                println!(
                    "  unmapped recurrence: task {} ({})",
                    todo_import::label(&u.task),
                    u.reason
                );
            }
            Ok(true)
        }
        Cmd::Verify {
            export,
            bundle,
            show_names,
        } => {
            let export = Export::load(&export).context("reading the export")?;
            let bundle = Bundle::read(&bundle)?;
            let rep = todo_import::verify::verify(&export, &bundle, show_names);
            print!("{}", rep.render());
            Ok(rep.ok())
        }
        Cmd::Apply {
            bundle: path,
            server,
            token_file,
            dry_run,
            batch_size,
            journal,
        } => {
            let bundle = Bundle::read(&path)?;
            let base = todo_import::http::check_base_url(&server)?;
            let journal = journal.unwrap_or_else(|| default_journal(&path, &base));
            print!("{}", plan(&bundle, &journal, &base, batch_size)?);
            if dry_run {
                println!("dry run: nothing sent");
                return Ok(true);
            }
            let client = Client::new(&base, token(token_file.as_deref())?)?;
            let sum = apply(
                &bundle,
                &client,
                &ApplyOptions {
                    batch_size,
                    journal,
                    abort_after: None,
                },
            )?;
            println!(
                "apply: {} commands: {} stored, {} replayed (already there), {} skipped (journal), {} batches",
                sum.total, sum.stored, sum.replayed, sum.skipped, sum.batches
            );
            Ok(true)
        }
        Cmd::VerifyServer {
            bundle,
            server,
            token_file,
        } => {
            let bundle = Bundle::read(&bundle)?;
            let client = Client::new(&server, token(token_file.as_deref())?)?;
            let rep = todo_import::verify_server::verify_server(&bundle, &client)?;
            print!("{}", rep.render());
            Ok(rep.ok())
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .init();
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
