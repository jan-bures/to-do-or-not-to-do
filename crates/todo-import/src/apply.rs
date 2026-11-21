//! `apply`: sends a bundle's commands to the server, in order, in batches, resumably.
//!
//! # Idempotency
//!
//! Every item carries a deterministic `command_id`. The server stores the result of each command under
//! its ID and replays it, without storing anything, when the same ID arrives again with the same
//! command. Entity IDs are deterministic too, so even a command under another ID could not create a
//! second copy (`already_exists`). Re-running `apply` is therefore always safe.
//!
//! # Resume journal
//!
//! To avoid re-sending thousands of commands after a crash, `apply` keeps a journal: a header naming
//! the bundle digest and the server, then one acknowledged `command_id` per line. Lines are written and
//! synced to disk at the end of every batch. Commands acknowledged after the last sync are simply sent
//! again on resume, and the server replays them. A journal for another bundle or another server is
//! refused.

use crate::bundle::{Bundle, BundleError, Item};
use crate::http::{Client, HttpError};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const JOURNAL_MAGIC: &str = "todo-import-journal 1";

/// Errors from `apply`.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The bundle could not be read or hashed.
    #[error(transparent)]
    Bundle(#[from] BundleError),
    /// A request failed.
    #[error("item {index} ({kind} {label}): {source}")]
    Http {
        /// Position in the bundle.
        index: usize,
        /// Source kind.
        kind: &'static str,
        /// Source label.
        label: String,
        /// The error.
        source: Box<HttpError>,
    },
    /// The journal is unusable.
    #[error("journal {path}: {reason}")]
    Journal {
        /// The journal file.
        path: String,
        /// What is wrong.
        reason: String,
    },
    /// Stopped on purpose after the given number of commands (tests simulate a crash with this).
    #[error("stopped after {0} commands")]
    Aborted(usize),
}

/// Options.
#[derive(Clone, Debug)]
pub struct ApplyOptions {
    /// Commands per batch: the journal is synced, and progress logged, after each batch.
    pub batch_size: usize,
    /// The journal file.
    pub journal: PathBuf,
    /// Stop right after sending this many commands, without syncing the journal: simulates a crash.
    #[doc(hidden)]
    pub abort_after: Option<usize>,
}

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplySummary {
    /// Items in the bundle.
    pub total: usize,
    /// Skipped because the journal records them as done.
    pub skipped: usize,
    /// Sent and newly stored.
    pub stored: usize,
    /// Sent and replayed by the server (already stored by an earlier run).
    pub replayed: usize,
    /// Batches sent.
    pub batches: usize,
}

/// The default journal path for a bundle and a server: next to the bundle, one per server.
#[must_use]
pub fn default_journal(bundle: &Path, server: &str) -> PathBuf {
    let mut name = bundle.as_os_str().to_owned();
    name.push(format!(".{}.journal", crate::label(server)));
    PathBuf::from(name)
}

fn header(digest: &str, server: &str) -> String {
    format!("{JOURNAL_MAGIC} {digest} {}", crate::label(server))
}

/// Reads the IDs a journal records, after checking its header. A missing file is an empty journal.
pub fn read_journal(path: &Path, digest: &str, server: &str) -> Result<BTreeSet<Uuid>, ApplyError> {
    let bad = |reason: String| ApplyError::Journal {
        path: path.display().to_string(),
        reason,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(bad(e.to_string())),
    };
    let mut lines = text.lines();
    match lines.next() {
        None => return Ok(BTreeSet::new()),
        Some(h) if h == header(digest, server) => {}
        Some(_) => {
            return Err(bad(
                "it belongs to another bundle or server; pass another --journal, or delete it (re-sending is safe)"
                    .into(),
            ));
        }
    }
    // A torn last line (crash mid-write) is not a valid UUID and is ignored.
    Ok(lines.filter_map(|l| l.trim().parse().ok()).collect())
}

/// The plan: what `apply` would send. Prints no task content.
pub fn plan(
    bundle: &Bundle,
    journal: &Path,
    server: &str,
    batch_size: usize,
) -> Result<String, ApplyError> {
    let done = read_journal(journal, &bundle.digest()?, server)?;
    let pending = bundle
        .items
        .iter()
        .filter(|i| !done.contains(&i.command_id))
        .count();
    let c = &bundle.counts;
    let mut s = String::new();
    let _ = writeln!(s, "server:        {server}");
    let _ = writeln!(s, "journal:       {}", journal.display());
    let _ = writeln!(
        s,
        "commands:      {} total, {} already acknowledged, {} to send in {} batch(es) of up to {}",
        bundle.items.len(),
        bundle.items.len() - pending,
        pending,
        pending.div_ceil(batch_size.max(1)),
        batch_size.max(1)
    );
    let _ = writeln!(
        s,
        "creates:       {} collections, {} lists, {} tags, {} tasks ({} open, {} completed), {} steps, {} links ({} email), {} attachments, {} reminders",
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
        c.reminders
    );
    let _ = writeln!(
        s,
        "fields:        {} due dates, {} high priority, {} with notes, {} recurring",
        c.due_dates, c.high_priority, c.notes, c.recurring
    );
    let _ = writeln!(
        s,
        "not imported:  attachment content ({} bytes, stays in the bundle), {} unmapped recurrence(s), {} stale My Day mark(s)",
        c.attachment_bytes,
        bundle.report.unmapped_recurrence.len(),
        bundle.report.my_day_marks_skipped
    );
    Ok(s)
}

#[derive(serde::Serialize)]
struct CommandRequest<'a> {
    command_id: Uuid,
    command: &'a todo_core::Command,
}

fn sync(
    file: &mut std::fs::File,
    pending: &mut Vec<String>,
    path: &Path,
) -> Result<(), ApplyError> {
    let bad = |e: std::io::Error| ApplyError::Journal {
        path: path.display().to_string(),
        reason: e.to_string(),
    };
    if pending.is_empty() {
        return Ok(());
    }
    let mut buf = pending.join("\n");
    buf.push('\n');
    file.write_all(buf.as_bytes()).map_err(bad)?;
    file.sync_data().map_err(bad)?;
    pending.clear();
    Ok(())
}

fn send(client: &Client, index: usize, item: &Item) -> Result<bool, ApplyError> {
    let body = CommandRequest {
        command_id: item.command_id,
        command: &item.command,
    };
    tracing::debug!(index, kind = item.source.kind.as_str(), "sending");
    match client.post("POST /api/v1/commands", "/api/v1/commands", &body) {
        Ok(reply) => Ok(reply.replayed),
        Err(source) => {
            if let HttpError::Status { message, .. } = &source {
                tracing::debug!(index, %message, "command rejected");
            }
            Err(ApplyError::Http {
                index,
                kind: item.source.kind.as_str(),
                label: crate::label(&item.source.id),
                source: Box::new(source),
            })
        }
    }
}

/// Sends every item the journal does not record yet.
pub fn apply(
    bundle: &Bundle,
    client: &Client,
    opts: &ApplyOptions,
) -> Result<ApplySummary, ApplyError> {
    let digest = bundle.digest()?;
    let server = client.base().to_owned();
    let done = read_journal(&opts.journal, &digest, &server)?;
    let jerr = |e: std::io::Error| ApplyError::Journal {
        path: opts.journal.display().to_string(),
        reason: e.to_string(),
    };
    if let Some(dir) = opts.journal.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(jerr)?;
    }
    let fresh =
        !opts.journal.exists() || std::fs::metadata(&opts.journal).map_err(jerr)?.len() == 0;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&opts.journal)
        .map_err(jerr)?;
    if fresh {
        writeln!(file, "{}", header(&digest, &server)).map_err(jerr)?;
        file.sync_data().map_err(jerr)?;
    }

    let batch = opts.batch_size.max(1);
    let mut sum = ApplySummary {
        total: bundle.items.len(),
        ..ApplySummary::default()
    };
    let mut pending: Vec<String> = Vec::new();
    let mut in_batch = 0usize;
    let mut sent = 0usize;
    for (index, item) in bundle.items.iter().enumerate() {
        if done.contains(&item.command_id) {
            sum.skipped += 1;
            continue;
        }
        let replayed = send(client, index, item)?;
        if replayed {
            sum.replayed += 1;
        } else {
            sum.stored += 1;
        }
        pending.push(item.command_id.to_string());
        sent += 1;
        if opts.abort_after == Some(sent) {
            // A crash: whatever was not synced is lost.
            return Err(ApplyError::Aborted(sent));
        }
        in_batch += 1;
        if in_batch == batch {
            sync(&mut file, &mut pending, &opts.journal)?;
            sum.batches += 1;
            in_batch = 0;
            tracing::info!(
                done = index + 1,
                total = sum.total,
                stored = sum.stored,
                replayed = sum.replayed,
                "batch acknowledged"
            );
        }
    }
    if in_batch > 0 {
        sync(&mut file, &mut pending, &opts.journal)?;
        sum.batches += 1;
    }
    tracing::info!(
        total = sum.total,
        stored = sum.stored,
        replayed = sum.replayed,
        skipped = sum.skipped,
        "apply finished"
    );
    Ok(sum)
}
