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
//! the bundle digest, the server and the server's log epoch, then one acknowledged `command_id` per
//! line. Lines are written and synced to disk at the end of every batch, and before `apply` returns an
//! error. Commands acknowledged after the last sync are simply sent again on resume, and the server
//! replays them. A journal for another bundle or another server is refused.
//!
//! The header is written to a temporary file, synced, and renamed into place before any command is
//! sent, so a crash can never leave a half-written header behind.
//!
//! A command counts as acknowledged only when the server answers 2xx with a command result for that
//! `command_id`: the result names the command, lists its events (an import always stores one), and
//! names the log epoch. Anything else (an empty body, another ID, no events) is an error, and nothing
//! is journaled for it.
//!
//! # Log epoch
//!
//! The server rotates its log epoch when it is restored from a backup, and a restore can lose
//! commands it had acknowledged. So the journal records the epoch it was written in, and `apply`
//! refuses a journal from another epoch, and stops if the epoch changes during a run. Deleting the
//! journal and running again is always safe.

use crate::bundle::{Bundle, BundleError, Item};
use crate::http::{Client, HttpError};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const JOURNAL_MAGIC: &str = "todo-import-journal 2";

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
    /// The server answered 2xx without acknowledging the command.
    #[error("item {index} ({kind} {label}): not acknowledged: {reason}")]
    NotAcknowledged {
        /// Position in the bundle.
        index: usize,
        /// Source kind.
        kind: &'static str,
        /// Source label.
        label: String,
        /// What was wrong with the response.
        reason: String,
    },
    /// The server's log epoch is not the one the journal was written in (it was restored from a
    /// backup, which may have lost acknowledged commands).
    #[error(
        "the server's log epoch changed (restored from a backup?): journal {journal} has {had}, the server {now}; delete the journal and run again (re-sending is safe)"
    )]
    EpochChanged {
        /// The journal file.
        journal: String,
        /// The journal's epoch.
        had: Uuid,
        /// The server's.
        now: Uuid,
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

fn header_prefix(digest: &str, server: &str) -> String {
    format!("{JOURNAL_MAGIC} {digest} {} ", crate::label(server))
}

fn header(digest: &str, server: &str, epoch: Uuid) -> String {
    format!("{}{epoch}", header_prefix(digest, server))
}

/// What a journal records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    /// The server's log epoch when it was written (`None` for a new journal).
    pub epoch: Option<Uuid>,
    /// Acknowledged command IDs.
    pub done: BTreeSet<Uuid>,
}

/// The temporary file a journal header is written to before it is renamed into place.
fn journal_tmp(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Reads a journal, after checking its header. A missing or empty file is a new journal.
pub fn read_journal(path: &Path, digest: &str, server: &str) -> Result<Journal, ApplyError> {
    let bad = |reason: String| ApplyError::Journal {
        path: path.display().to_string(),
        reason,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Journal::default()),
        Err(e) => return Err(bad(e.to_string())),
    };
    let mut lines = text.lines();
    let epoch = match lines.next() {
        None => return Ok(Journal::default()),
        Some(h) => h
            .strip_prefix(&header_prefix(digest, server))
            .and_then(|e| e.parse::<Uuid>().ok())
            .ok_or_else(|| {
                bad("it belongs to another bundle or server; pass another --journal, or delete it (re-sending is safe)".into())
            })?,
    };
    // A torn last line (crash mid-write) is not a valid UUID and is ignored.
    Ok(Journal {
        epoch: Some(epoch),
        done: lines.filter_map(|l| l.trim().parse().ok()).collect(),
    })
}

/// Creates a journal holding only its header: written to a temporary file, synced, then renamed into
/// place, so the journal is never half-written.
fn create_journal(path: &Path, header: &str) -> std::io::Result<()> {
    let tmp = journal_tmp(path);
    {
        let mut f = std::fs::File::create(&tmp)?;
        writeln!(f, "{header}")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    // Make the rename itself durable where the platform allows syncing a directory.
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// The plan: what `apply` would send. Prints no task content.
pub fn plan(
    bundle: &Bundle,
    journal: &Path,
    server: &str,
    batch_size: usize,
) -> Result<String, ApplyError> {
    let done = read_journal(journal, &bundle.digest()?, server)?.done;
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

/// The part of the server's command result that acknowledges a command.
#[derive(serde::Deserialize)]
struct CommandAck {
    command_id: Uuid,
    events: Vec<EventAck>,
    epoch: Uuid,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)] // Parsed to check the shape; only the count is used.
struct EventAck {
    seq: u64,
    id: Uuid,
}

/// The part of an event page that names the log epoch.
#[derive(serde::Deserialize)]
struct EpochPage {
    epoch: Uuid,
}

/// The server's current log epoch.
pub fn server_epoch(client: &Client) -> Result<Uuid, HttpError> {
    let page: EpochPage = client.get("GET /api/v1/events", "/api/v1/events?limit=1")?;
    Ok(page.epoch)
}

/// Sends one command. Returns whether the server replayed it, after checking that the response
/// acknowledges this command in `epoch`.
fn send(
    client: &Client,
    index: usize,
    item: &Item,
    epoch: Uuid,
    journal: &Path,
) -> Result<bool, ApplyError> {
    let body = CommandRequest {
        command_id: item.command_id,
        command: &item.command,
    };
    tracing::debug!(index, kind = item.source.kind.as_str(), "sending");
    let not_acked = |reason: String| ApplyError::NotAcknowledged {
        index,
        kind: item.source.kind.as_str(),
        label: crate::label(&item.source.id),
        reason,
    };
    match client.post("POST /api/v1/commands", "/api/v1/commands", &body) {
        Ok(reply) => {
            let ack: CommandAck = serde_json::from_value(reply.body)
                .map_err(|e| not_acked(format!("the response is not a command result: {e}")))?;
            if ack.command_id != item.command_id {
                return Err(not_acked("the result is for another command_id".to_owned()));
            }
            if ack.events.is_empty() {
                // An import always stores exactly one event; an empty list would mean a no-op.
                return Err(not_acked("the result has no events".to_owned()));
            }
            if ack.epoch != epoch {
                return Err(ApplyError::EpochChanged {
                    journal: journal.display().to_string(),
                    had: epoch,
                    now: ack.epoch,
                });
            }
            Ok(reply.replayed)
        }
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
    let journal = read_journal(&opts.journal, &digest, &server)?;
    let jerr = |e: std::io::Error| ApplyError::Journal {
        path: opts.journal.display().to_string(),
        reason: e.to_string(),
    };
    let epoch = server_epoch(client).map_err(|source| ApplyError::Http {
        index: 0,
        kind: "epoch",
        label: "-".to_owned(),
        source: Box::new(source),
    })?;
    match journal.epoch {
        Some(had) if had != epoch => {
            return Err(ApplyError::EpochChanged {
                journal: opts.journal.display().to_string(),
                had,
                now: epoch,
            });
        }
        Some(_) => {}
        None => {
            if let Some(dir) = opts.journal.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(jerr)?;
            }
            create_journal(&opts.journal, &header(&digest, &server, epoch)).map_err(jerr)?;
        }
    }
    let done = journal.done;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&opts.journal)
        .map_err(jerr)?;

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
        let replayed = match send(client, index, item, epoch, &opts.journal) {
            Ok(r) => r,
            Err(e) => {
                // Keep what was acknowledged before the failure, unless the epoch moved under us
                // (then those acknowledgements may be gone, and the journal is refused anyway).
                if !matches!(e, ApplyError::EpochChanged { .. }) {
                    sync(&mut file, &mut pending, &opts.journal)?;
                }
                return Err(e);
            }
        };
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
