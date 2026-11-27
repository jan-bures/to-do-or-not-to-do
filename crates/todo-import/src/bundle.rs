//! The import bundle: a self-contained, typed and versioned file with everything to import.
//!
//! # Format (version 2)
//!
//! A JSON object:
//!
//! | Field | Contents |
//! |---|---|
//! | `format` | always `"todo-import-bundle"` |
//! | `version` | `2` |
//! | `time_zone` | IANA name of the owner's time zone, used for due dates |
//! | `counts` | aggregate counts of what the items create ([`Counts`]) |
//! | `items` | the commands, in the order they must be applied ([`Item`]) |
//! | `blobs` | attachment content, base64, keyed by attachment ID ([`Blob`]) |
//! | `report` | what was skipped or could not be mapped ([`Report`]), including every date that could not
//! be read (`report.lossy`) |
//!
//! # Dates
//!
//! Every date in the export is read strictly: instants as ISO 8601 with an offset, and Graph
//! `dateTimeTimeZone` values in `UTC`, an IANA zone or a Windows zone (mapped with the CLDR table in
//! [`crate::windows_zones`]). A date that is missing where one is required, or that cannot be read, is
//! never replaced silently: [`build`] fails unless [`BuildOptions::allow_lossy`] is set, and then each
//! one is listed in [`Report::lossy`] with what was used instead.
//!
//! Every item is one `todo-core` [`Command::ImportEntity`] with a deterministic `command_id` (see
//! [`crate::ids`]) and the source object it came from. Items are ordered so that parents come before
//! children: tags, collections, lists, then each list's tasks, each followed by its steps, links,
//! attachments and reminders. Position keys are assigned in display order.

use crate::cache::{Cache, CachePos};
use crate::graph::{Export, ExportList, GraphTask, LinkedResource};
use crate::ids::{command_id, entity_uuid};
use crate::mapping;
use base64::Engine;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use todo_core::command::Command;
use todo_core::decide::MAX_URL;
use todo_core::ids::{
    AttachmentId, CollectionId, LinkId, ListId, ReminderId, StepId, TagId, TaskId,
};
use todo_core::model::{
    AttachmentRef, Collection, EmailLink, Entity, Link, LinkKind, List, Priority, Reminder, Stamp,
    Step, Tag, Task,
};
use todo_core::ordering::PosKey;
use todo_core::{Actor, PROTOCOL_VERSION};
use uuid::Uuid;

/// Value of the `format` field.
pub const FORMAT: &str = "todo-import-bundle";
/// Current bundle format version.
pub const VERSION: u32 = 2;

/// Errors while building, reading or writing a bundle.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// I/O failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The error.
        source: std::io::Error,
    },
    /// The file is not a bundle of a supported version.
    #[error("{0}")]
    Format(String),
    /// JSON failed.
    #[error("bundle JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The export could not be read.
    #[error(transparent)]
    Export(#[from] crate::graph::ExportError),
    /// Key generation failed (too many siblings).
    #[error(transparent)]
    Order(#[from] todo_core::ordering::OrderError),
    /// Dates could not be read, and lossy building was not allowed.
    #[error(
        "{} date(s) in the export could not be read, for example {first}; fix the export or pass --allow-lossy to import with the substitutions listed in the report",
        lossy.len()
    )]
    Lossy {
        /// The first one, as text.
        first: String,
        /// All of them.
        lossy: Vec<Lossy>,
    },
}

/// The kind of source object an item came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A list group from the cache (ID: its server ID).
    Group,
    /// A category (ID: its lowercase name).
    Category,
    /// A Graph list.
    List,
    /// A Graph task.
    Task,
    /// A Graph checklist item.
    Step,
    /// A Graph linked resource.
    Link,
    /// A Graph attachment.
    Attachment,
    /// The reminder of a Graph task (ID: the task's).
    Reminder,
}

impl SourceKind {
    /// Stable name, also part of the ID hashes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Category => "category",
            Self::List => "list",
            Self::Task => "task",
            Self::Step => "step",
            Self::Link => "link",
            Self::Attachment => "attachment",
            Self::Reminder => "reminder",
        }
    }
}

/// Where an item came from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Source {
    /// The kind.
    pub kind: SourceKind,
    /// The source ID.
    pub id: String,
}

/// One command of the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Deterministic `command_id`.
    pub command_id: Uuid,
    /// The source object.
    pub source: Source,
    /// The command (always [`Command::ImportEntity`] in version 1).
    pub command: Command,
}

impl Item {
    /// The imported entity.
    #[must_use]
    pub fn entity(&self) -> Option<&Entity> {
        match &self.command {
            Command::ImportEntity { entity } => Some(entity),
            _ => None,
        }
    }
}

/// Content of one attachment, sent by `apply --attachments` (see [`crate::apply::apply_attachments`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// The attachment.
    pub attachment: AttachmentId,
    /// The storage key its reference carries (`import/sha256/<hex>`).
    pub storage_key: String,
    /// Hex SHA-256 of the content.
    pub sha256: String,
    /// Length of the content.
    pub size_bytes: u64,
    /// The content, base64.
    pub content_base64: String,
}

/// Aggregate counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Collections (list groups).
    pub collections: usize,
    /// Lists.
    pub lists: usize,
    /// Open tasks.
    pub tasks_open: usize,
    /// Completed tasks.
    pub tasks_completed: usize,
    /// Steps.
    pub steps: usize,
    /// Links (all kinds).
    pub links: usize,
    /// Of which email links.
    pub email_links: usize,
    /// Attachment references.
    pub attachments: usize,
    /// Bytes of attachment content in the bundle.
    pub attachment_bytes: u64,
    /// Reminders.
    pub reminders: usize,
    /// Tags (categories).
    pub tags: usize,
    /// Tasks with a due date.
    pub due_dates: usize,
    /// Tasks with high priority.
    pub high_priority: usize,
    /// Tasks with notes.
    pub notes: usize,
    /// Tasks with a recurrence.
    pub recurring: usize,
}

impl Counts {
    /// Counts what `items` create.
    #[must_use]
    pub fn of(items: &[Item], blobs: &[Blob]) -> Self {
        let mut c = Self::default();
        for e in items.iter().filter_map(Item::entity) {
            match e {
                Entity::Collection(_) => c.collections += 1,
                Entity::List(_) => c.lists += 1,
                Entity::Task(t) => {
                    if t.completed_at.is_some() {
                        c.tasks_completed += 1;
                    } else {
                        c.tasks_open += 1;
                    }
                    c.due_dates += usize::from(t.due.is_some());
                    c.high_priority += usize::from(t.priority == Priority::High);
                    c.notes += usize::from(!t.notes.is_empty());
                    c.recurring += usize::from(t.recurrence.is_some());
                }
                Entity::Step(_) => c.steps += 1,
                Entity::Link(l) => {
                    c.links += 1;
                    c.email_links += usize::from(matches!(l.kind, LinkKind::Email(_)));
                }
                Entity::Attachment(_) => c.attachments += 1,
                Entity::Reminder(_) => c.reminders += 1,
                Entity::Tag(_) => c.tags += 1,
            }
        }
        c.attachment_bytes = blobs.iter().map(|b| b.size_bytes).sum();
        c
    }

    /// Total tasks.
    #[must_use]
    pub fn tasks(&self) -> usize {
        self.tasks_open + self.tasks_completed
    }
}

/// A recurrence that could not be mapped. The task is imported without it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unmapped {
    /// Graph ID of the task.
    pub task: String,
    /// Why it does not map.
    pub reason: String,
    /// The raw Graph `patternedRecurrence`.
    pub pattern: serde_json::Value,
}

/// A date that could not be read, and what the bundle uses instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lossy {
    /// The kind of source object the date belongs to.
    pub kind: SourceKind,
    /// Its source ID.
    pub id: String,
    /// The Graph (or cache) field name, for example `createdDateTime`.
    pub field: String,
    /// What is wrong with it.
    pub problem: String,
    /// What the bundle does instead: drops the value, or uses another time.
    pub instead: String,
}

impl std::fmt::Display for Lossy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {}: {} ({})",
            self.kind.as_str(),
            crate::label(&self.id),
            self.field,
            self.problem,
            self.instead
        )
    }
}

/// What the builder skipped, ignored or could not map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Free-form warnings. They name objects by source-ID label, never by content.
    pub warnings: Vec<String>,
    /// Recurrences that do not map to the supported subset.
    pub unmapped_recurrence: Vec<Unmapped>,
    /// Steps in the cache whose task does not exist (ignored).
    pub orphan_cache_steps: usize,
    /// Lists in the cache that the export does not have (not imported).
    pub cache_only_lists: usize,
    /// Tasks with no position in the cache (placed last, in export order).
    pub tasks_without_cache_order: usize,
    /// Steps with no position in the cache (placed last, in export order).
    pub steps_without_cache_order: usize,
    /// My Day marks in the cache (not imported: they are stale by definition).
    pub my_day_marks_skipped: usize,
    /// Due dates where the Graph date (in `time_zone`) and the cache date differ.
    pub due_date_mismatches: usize,
    /// Dates that could not be read (only with [`BuildOptions::allow_lossy`]).
    pub lossy: Vec<Lossy>,
}

/// The bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    /// [`FORMAT`].
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// `todo-core` protocol version the commands are written in.
    pub protocol: u32,
    /// IANA time zone for due dates.
    pub time_zone: String,
    /// Aggregate counts.
    pub counts: Counts,
    /// The commands, in order.
    pub items: Vec<Item>,
    /// Attachment content.
    pub blobs: Vec<Blob>,
    /// The build report.
    pub report: Report,
}

impl Bundle {
    /// Reads and checks a bundle file.
    pub fn read(path: &Path) -> Result<Self, BundleError> {
        let text = std::fs::read_to_string(path).map_err(|source| BundleError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let head: serde_json::Value = serde_json::from_str(&text)?;
        if head["format"] != FORMAT {
            return Err(BundleError::Format(format!(
                "{} is not a todo-import bundle",
                path.display()
            )));
        }
        if head["version"] != VERSION {
            return Err(BundleError::Format(format!(
                "bundle version {} is not supported (expected {VERSION})",
                head["version"]
            )));
        }
        let b: Self = serde_json::from_value(head)?;
        if Counts::of(&b.items, &b.blobs) != b.counts {
            return Err(BundleError::Format(
                "bundle counts do not match its items (edited or corrupt?)".into(),
            ));
        }
        Ok(b)
    }

    /// Writes the bundle.
    pub fn write(&self, path: &Path) -> Result<(), BundleError> {
        let io = |source| BundleError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json).map_err(io)
    }

    /// Hex SHA-256 of the items: identifies the bundle's commands for the resume journal.
    pub fn digest(&self) -> Result<String, BundleError> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(
            &self.items,
        )?)))
    }

    /// The blob of an attachment.
    #[must_use]
    pub fn blob(&self, id: AttachmentId) -> Option<&Blob> {
        self.blobs.iter().find(|b| b.attachment == id)
    }
}

/// Builder options.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// The owner's time zone, for due dates.
    pub time_zone: TimeZone,
    /// Its IANA name.
    pub time_zone_name: String,
    /// Build even if some dates cannot be read, listing each in [`Report::lossy`].
    pub allow_lossy: bool,
}

/// The ID of the entity made from a source object.
#[must_use]
pub fn uuid_for(kind: SourceKind, id: &str, at: Timestamp) -> Uuid {
    entity_uuid(kind.as_str(), id, at)
}

/// The import stamp: the actor is replaced by the server with the caller.
fn stamp(at: Timestamp) -> Stamp {
    Stamp {
        at,
        by: Actor::Importer,
    }
}

/// `n` increasing position keys.
fn keys(n: usize) -> Result<Vec<PosKey>, BundleError> {
    let mut out: Vec<PosKey> = Vec::with_capacity(n);
    for _ in 0..n {
        let k = PosKey::between(out.last(), None)?;
        out.push(k);
    }
    Ok(out)
}

/// The creation time of a Graph task, for ordering and for derived times. An unreadable one falls
/// back to `fallback`; [`Builder::task`] records it in [`Report::lossy`].
fn task_created(t: &GraphTask, fallback: Timestamp) -> Timestamp {
    mapping::instant(&t.created_date_time).unwrap_or(fallback)
}

/// Task notes as imported, and whether they had to be cut to the domain's size limit.
#[must_use]
pub fn task_notes(t: &GraphTask) -> (String, bool) {
    let body = t.body.clone().unwrap_or_default();
    let mut notes = mapping::notes(&body.content, &body.content_type);
    if mapping::notes_fit(&notes) {
        return (notes, false);
    }
    let mut cut = todo_core::decide::MAX_NOTES;
    while !notes.is_char_boundary(cut) {
        cut -= 1;
    }
    notes.truncate(cut);
    (notes, true)
}

/// Mapped scalar fields of one Graph task.
struct TaskFields {
    title: String,
    notes: String,
    due: Option<jiff::civil::Date>,
    priority: Priority,
    completed_at: Option<Timestamp>,
    created: Timestamp,
    updated: Timestamp,
}

/// The link of a linked resource, if it is an `http` or `https` URL within the domain's limit.
#[must_use]
pub fn web_url(lr: &LinkedResource) -> Option<String> {
    lr.web_url.clone().filter(|u| {
        (u.starts_with("https://") || u.starts_with("http://"))
            && u.len() <= MAX_URL
            && !u.contains(char::is_whitespace)
    })
}

/// Cuts an email metadata field so that it fits the domain's byte limit.
#[must_use]
pub fn clip_field(s: &str) -> String {
    s.chars().take(MAX_URL / 4).collect()
}

/// Maps a linked resource to a link kind, using the cache's email metadata when there is some.
///
/// For an email link the received time comes from the cache: `Ok(at)`, or `Err(problem)` when the cache
/// has none or it cannot be read. The caller decides what to use instead.
pub fn link_kind(
    lr: &LinkedResource,
    cache: &Cache,
) -> Option<(LinkKind, Option<Result<Timestamp, String>>)> {
    let web = web_url(lr);
    let subject = lr.display_name.clone().unwrap_or_default();
    let meta = lr.external_id.as_ref().and_then(|x| cache.emails.get(x));
    let outlook = lr.application_name.as_deref() == Some("Outlook");
    if let Some(m) = meta.or(outlook.then_some(&crate::cache::EmailMeta::default())) {
        let sender = m
            .sender_address
            .clone()
            .or_else(|| m.sender_name.clone())
            .unwrap_or_else(|| "unknown sender".to_owned());
        let received = match (meta, m.received.as_deref()) {
            (None, _) => Err("no email metadata in the cache".to_owned()),
            (Some(_), None) => Err("no received time in the cache".to_owned()),
            (Some(_), Some(r)) => mapping::instant(r).map_err(|e| e.0),
        };
        let message_id = lr.external_id.clone().unwrap_or_else(|| lr.id.clone());
        let kind = LinkKind::Email(EmailLink {
            subject: clip_field(&subject),
            sender: clip_field(&sender),
            // Set by the caller.
            received_at: Timestamp::UNIX_EPOCH,
            message_id: clip_field(&message_id),
            web_link: web,
        });
        return Some((kind, Some(received)));
    }
    web.map(|url| {
        let kind = LinkKind::Web {
            url,
            title: Some(subject.trim().to_owned()).filter(|s| !s.is_empty() && s.len() <= MAX_URL),
        };
        (kind, None)
    })
}

struct Builder<'a> {
    export: &'a Export,
    cache: &'a Cache,
    opts: &'a BuildOptions,
    items: Vec<Item>,
    blobs: Vec<Blob>,
    report: Report,
    tags: BTreeMap<String, TagId>,
}

impl Builder<'_> {
    /// Records a date that could not be read.
    fn lossy(
        &mut self,
        kind: SourceKind,
        id: &str,
        field: &str,
        problem: impl Into<String>,
        instead: &str,
    ) {
        self.report.lossy.push(Lossy {
            kind,
            id: id.to_owned(),
            field: field.to_owned(),
            problem: problem.into(),
            instead: instead.to_owned(),
        });
    }

    /// Maps the scalar fields of a Graph task. Unreadable dates go to [`Report::lossy`].
    fn task_fields(&mut self, t: &GraphTask, fallback: Timestamp) -> TaskFields {
        const TASK: SourceKind = SourceKind::Task;
        let label = crate::label(&t.id);
        let created = match mapping::instant(&t.created_date_time) {
            Ok(at) => at,
            Err(e) => {
                self.lossy(
                    TASK,
                    &t.id,
                    "createdDateTime",
                    e.0,
                    "used the earliest readable creation time in the export",
                );
                fallback
            }
        };
        let updated = match t.last_modified_date_time.as_deref().map(mapping::instant) {
            Some(Ok(at)) => at,
            Some(Err(e)) => {
                self.lossy(
                    TASK,
                    &t.id,
                    "lastModifiedDateTime",
                    e.0,
                    "used the creation time",
                );
                created
            }
            None => {
                self.lossy(
                    TASK,
                    &t.id,
                    "lastModifiedDateTime",
                    "missing",
                    "used the creation time",
                );
                created
            }
        };
        let (notes, cut) = task_notes(t);
        if cut {
            self.report
                .warnings
                .push(format!("task {label}: notes cut to the size limit"));
        }
        let due = match &t.due_date_time {
            None => None,
            Some(d) => match mapping::due_date(d, &self.opts.time_zone) {
                Ok(d) => Some(d),
                Err(e) => {
                    self.lossy(
                        TASK,
                        &t.id,
                        "dueDateTime",
                        e.0,
                        "imported without a due date",
                    );
                    None
                }
            },
        };
        let completed_at = if t.is_completed() {
            Some(
                match t.completed_date_time.as_ref().map(mapping::date_time) {
                    Some(Ok(at)) => at,
                    Some(Err(e)) => {
                        self.lossy(
                            TASK,
                            &t.id,
                            "completedDateTime",
                            e.0,
                            "used the last modification time",
                        );
                        updated
                    }
                    None => {
                        self.lossy(
                            TASK,
                            &t.id,
                            "completedDateTime",
                            "missing on a completed task",
                            "used the last modification time",
                        );
                        updated
                    }
                },
            )
        } else {
            None
        };
        TaskFields {
            title: mapping::title(t.title.as_deref()),
            notes,
            due,
            priority: mapping::priority(&t.importance),
            completed_at,
            created,
            updated,
        }
    }

    fn push(&mut self, kind: SourceKind, id: &str, entity: Entity) {
        self.items.push(Item {
            command_id: command_id(kind.as_str(), id, "entity"),
            source: Source {
                kind,
                id: id.to_owned(),
            },
            command: Command::ImportEntity {
                entity: Box::new(entity),
            },
        });
    }

    fn earliest(&self) -> Timestamp {
        self.export
            .lists
            .iter()
            .flat_map(|l| &l.tasks)
            .filter_map(|t| mapping::instant(&t.created_date_time).ok())
            .min()
            .unwrap_or(Timestamp::UNIX_EPOCH)
    }

    fn list_created(&self, l: &ExportList, fallback: Timestamp) -> Timestamp {
        l.tasks
            .iter()
            .filter_map(|t| mapping::instant(&t.created_date_time).ok())
            .min()
            .unwrap_or(fallback)
    }

    fn tags(&mut self, fallback: Timestamp) {
        // Lowercase name -> (display name of first use, earliest task using it).
        let mut seen: BTreeMap<String, (String, Timestamp)> = BTreeMap::new();
        for t in self.export.lists.iter().flat_map(|l| &l.tasks) {
            let at = task_created(t, fallback);
            for c in &t.categories {
                let name = mapping::name(c);
                let e = seen.entry(name.to_lowercase()).or_insert((name, at));
                e.1 = e.1.min(at);
            }
        }
        for (key, (name, at)) in seen {
            let id = TagId::from_uuid(uuid_for(SourceKind::Category, &key, at));
            self.tags.insert(key.clone(), id);
            self.push(
                SourceKind::Category,
                &key,
                Entity::Tag(Tag {
                    id,
                    name,
                    color: None,
                    created: stamp(at),
                    deleted: None,
                }),
            );
        }
    }

    /// Collections and lists, in display order. Returns the lists in the order their tasks are
    /// emitted.
    fn structure(&mut self, fallback: Timestamp) -> Result<Vec<(usize, ListId)>, BundleError> {
        let export = self.export;
        let cache = self.cache;
        let created: Vec<Timestamp> = export
            .lists
            .iter()
            .map(|l| self.list_created(l, fallback))
            .collect();

        // Groups in ascending position order.
        let mut groups: Vec<_> = cache.groups.iter().collect();
        groups.sort_by(|a, b| {
            (a.position.is_none(), &a.position, &a.local_id).cmp(&(
                b.position.is_none(),
                &b.position,
                &b.local_id,
            ))
        });
        let gkeys = keys(groups.len())?;
        let mut collection_of: BTreeMap<&str, CollectionId> = BTreeMap::new();
        let mut collection_order: Vec<CollectionId> = Vec::new();
        for (g, key) in groups.iter().zip(gkeys) {
            let at = export
                .lists
                .iter()
                .zip(&created)
                .filter(|(l, _)| {
                    cache
                        .folders
                        .get(&l.list.id)
                        .and_then(|f| f.group_local_id.as_deref())
                        == Some(g.local_id.as_str())
                })
                .map(|(_, at)| *at)
                .min()
                .unwrap_or(fallback);
            let id = CollectionId::from_uuid(uuid_for(SourceKind::Group, &g.online_id, at));
            collection_of.insert(&g.local_id, id);
            collection_order.push(id);
            self.push(
                SourceKind::Group,
                &g.online_id,
                Entity::Collection(Collection {
                    task_sort: None,
                    id,
                    name: mapping::name(&g.name),
                    color: None,
                    position: key,
                    created: stamp(at),
                    deleted: None,
                }),
            );
        }

        self.report.cache_only_lists = cache
            .folders
            .keys()
            .filter(|id| !export.lists.iter().any(|l| &l.list.id == *id))
            .count();

        // Lists grouped by scope; within a scope the default list first, then by cache position,
        // then lists the cache does not know, in export order.
        let mut scopes: BTreeMap<Option<CollectionId>, Vec<usize>> = BTreeMap::new();
        for (i, l) in export.lists.iter().enumerate() {
            let folder = cache.folders.get(&l.list.id);
            if folder.is_none() {
                self.report.warnings.push(format!(
                    "list {}: not in the cache; placed last at the top level",
                    crate::label(&l.list.id)
                ));
            }
            let col = folder
                .and_then(|f| f.group_local_id.as_deref())
                .and_then(|g| collection_of.get(g).copied());
            scopes.entry(col).or_default().push(i);
        }
        let sort_key = |i: &usize| {
            let l = &export.lists[*i];
            let f = cache.folders.get(&l.list.id);
            let default = l.list.is_default() || f.is_some_and(|f| f.is_default);
            (
                !default,
                f.is_none(),
                f.and_then(|f| f.position.clone()).is_none(),
                f.and_then(|f| f.position.clone()),
                *i,
            )
        };
        let mut emitted = Vec::new();
        let order: Vec<Option<CollectionId>> = std::iter::once(None)
            .chain(collection_order.iter().copied().map(Some))
            .collect();
        for col in order {
            let Some(mut members) = scopes.remove(&col) else {
                continue;
            };
            members.sort_by_key(sort_key);
            for (i, key) in members.iter().copied().zip(keys(members.len())?) {
                let l = &export.lists[i];
                let at = created[i];
                let id = ListId::from_uuid(uuid_for(SourceKind::List, &l.list.id, at));
                let color = cache
                    .folders
                    .get(&l.list.id)
                    .and_then(|f| f.theme_color.as_deref())
                    .and_then(mapping::theme_color);
                self.push(
                    SourceKind::List,
                    &l.list.id,
                    Entity::List(List {
                        task_sort: None,
                        id,
                        collection: col,
                        name: mapping::name(&l.list.display_name),
                        color,
                        features: Default::default(),
                        position: key,
                        created: stamp(at),
                        deleted: None,
                    }),
                );
                emitted.push((i, id));
            }
        }
        Ok(emitted)
    }

    /// Task indexes of a list in display order.
    fn task_order(&mut self, l: &ExportList, fallback: Timestamp) -> Vec<usize> {
        let ascending = self
            .cache
            .folders
            .get(&l.list.id)
            .is_some_and(crate::cache::CacheFolder::tasks_ascending);
        let (mut known, unknown): (Vec<usize>, Vec<usize>) =
            (0..l.tasks.len()).partition(|i| self.cache.task_pos.contains_key(&l.tasks[*i].id));
        let key = |i: &usize| {
            let t = &l.tasks[*i];
            (
                self.cache.task_pos.get(&t.id).cloned(),
                task_created(t, fallback),
                t.id.clone(),
            )
        };
        known.sort_by_key(key);
        if !ascending {
            known.reverse();
        }
        self.report.tasks_without_cache_order += unknown.len();
        known.extend(unknown);
        known
    }

    fn task(
        &mut self,
        l: &ExportList,
        list: ListId,
        t: &GraphTask,
        position: PosKey,
        fallback: Timestamp,
    ) -> Result<(), BundleError> {
        let label = crate::label(&t.id);
        let f = self.task_fields(t, fallback);
        if let (Some(due), Some(cached)) = (f.due, self.cache.task_due.get(&t.id))
            && cached.parse::<jiff::civil::Date>().ok() != Some(due)
        {
            self.report.due_date_mismatches += 1;
            self.report.warnings.push(format!(
                "task {label}: due date differs from the cache's; check --time-zone"
            ));
        }
        let recurrence = match &t.recurrence {
            None => None,
            Some(raw) if raw.is_null() => None,
            Some(raw) => match mapping::recurrence(raw, f.due) {
                Ok(r) => Some(r),
                Err(e) => {
                    self.report.unmapped_recurrence.push(Unmapped {
                        task: t.id.clone(),
                        reason: e.0,
                        pattern: raw.clone(),
                    });
                    None
                }
            },
        };
        let tags = t
            .categories
            .iter()
            .filter_map(|c| self.tags.get(&mapping::name(c).to_lowercase()).copied())
            .collect();
        let id = TaskId::from_uuid(uuid_for(SourceKind::Task, &t.id, f.created));
        self.push(
            SourceKind::Task,
            &t.id,
            Entity::Task(Task {
                id,
                list,
                title: f.title,
                notes: f.notes,
                due: f.due,
                start_date: None,
                status: Default::default(),
                priority: f.priority,
                my_day: None,
                completed_at: f.completed_at,
                recurrence,
                tags,
                position,
                created: stamp(f.created),
                updated: stamp(f.updated),
                deleted: None,
                merged_into: None,
            }),
        );

        let ch = l.children_of(&t.id);
        // Steps: descending cache position; steps the cache does not place under this task go last.
        let (mut known, unknown): (Vec<usize>, Vec<usize>) = (0..ch.checklist_items.len())
            .partition(|i| {
                self.cache
                    .step_pos
                    .get(&ch.checklist_items[*i].id)
                    .is_some_and(|(task, _)| *task == t.id)
            });
        let step_pos = |i: &usize| -> (Option<CachePos>, String) {
            let s = &ch.checklist_items[*i];
            (
                self.cache.step_pos.get(&s.id).map(|(_, p)| p.clone()),
                s.created_date_time.clone(),
            )
        };
        known.sort_by_key(step_pos);
        known.reverse();
        self.report.steps_without_cache_order += unknown.len();
        known.extend(unknown);
        for (i, key) in known.into_iter().zip(keys(ch.checklist_items.len())?) {
            let s = &ch.checklist_items[i];
            const STEP: SourceKind = SourceKind::Step;
            let created = match mapping::instant(&s.created_date_time) {
                Ok(at) => at,
                Err(e) => {
                    self.lossy(
                        STEP,
                        &s.id,
                        "createdDateTime",
                        e.0,
                        "used the task's creation time",
                    );
                    f.created
                }
            };
            let completed_at = if s.is_checked {
                Some(match s.checked_date_time.as_deref().map(mapping::instant) {
                    Some(Ok(at)) => at,
                    Some(Err(e)) => {
                        self.lossy(
                            STEP,
                            &s.id,
                            "checkedDateTime",
                            e.0,
                            "used the step's creation time",
                        );
                        created
                    }
                    None => {
                        self.lossy(
                            STEP,
                            &s.id,
                            "checkedDateTime",
                            "missing on a checked step",
                            "used the step's creation time",
                        );
                        created
                    }
                })
            } else {
                None
            };
            self.push(
                SourceKind::Step,
                &s.id,
                Entity::Step(Step {
                    id: StepId::from_uuid(uuid_for(SourceKind::Step, &s.id, created)),
                    task: id,
                    title: mapping::title(Some(&s.display_name)),
                    completed_at,
                    position: key,
                    created: stamp(created),
                    deleted: None,
                }),
            );
        }

        for lr in &ch.linked_resources {
            let kind = link_kind(lr, self.cache).map(|(mut kind, received)| {
                if let (LinkKind::Email(m), Some(received)) = (&mut kind, received) {
                    m.received_at = match received {
                        Ok(at) => at,
                        Err(problem) => {
                            self.lossy(
                                SourceKind::Link,
                                &lr.id,
                                "receivedDateTime",
                                problem,
                                "used the task's creation time",
                            );
                            f.created
                        }
                    };
                }
                kind
            });
            match kind {
                Some(kind) => self.push(
                    SourceKind::Link,
                    &lr.id,
                    Entity::Link(Link {
                        id: LinkId::from_uuid(uuid_for(SourceKind::Link, &lr.id, f.created)),
                        task: id,
                        kind,
                        created: stamp(f.created),
                        deleted: None,
                    }),
                ),
                None => self.report.warnings.push(format!(
                    "link {}: neither an email nor a web link; skipped",
                    crate::label(&lr.id)
                )),
            }
        }

        for a in &ch.attachments {
            let created = match a.last_modified_date_time.as_deref().map(mapping::instant) {
                Some(Ok(at)) => at,
                Some(Err(e)) => {
                    self.lossy(
                        SourceKind::Attachment,
                        &a.id,
                        "lastModifiedDateTime",
                        e.0,
                        "used the task's creation time",
                    );
                    f.created
                }
                None => {
                    self.lossy(
                        SourceKind::Attachment,
                        &a.id,
                        "lastModifiedDateTime",
                        "missing",
                        "used the task's creation time",
                    );
                    f.created
                }
            };
            let aid = AttachmentId::from_uuid(uuid_for(SourceKind::Attachment, &a.id, created));
            let content = self.export.attachment_content(a)?;
            let (storage_key, size_bytes) = match &content {
                Some(bytes) => {
                    let sha = hex::encode(Sha256::digest(bytes));
                    let key = format!("import/sha256/{sha}");
                    let size = bytes.len() as u64;
                    self.blobs.push(Blob {
                        attachment: aid,
                        storage_key: key.clone(),
                        sha256: sha,
                        size_bytes: size,
                        content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    });
                    (key, size)
                }
                None => {
                    self.report.warnings.push(format!(
                        "attachment {}: no content in the export; metadata only",
                        crate::label(&a.id)
                    ));
                    (
                        format!("import/missing/{}", crate::label(&a.id)),
                        a.size.unwrap_or(0),
                    )
                }
            };
            self.push(
                SourceKind::Attachment,
                &a.id,
                Entity::Attachment(AttachmentRef {
                    id: aid,
                    task: id,
                    file_name: mapping::file_name(&a.name),
                    media_type: a
                        .content_type
                        .clone()
                        .filter(|m| !m.trim().is_empty())
                        .unwrap_or_else(|| "application/octet-stream".to_owned()),
                    size_bytes,
                    storage_key,
                    created: stamp(created),
                    deleted: None,
                }),
            );
        }

        if t.is_reminder_on {
            let at = match &t.reminder_date_time {
                Some(r) => mapping::date_time(r).map_err(|e| e.0),
                None => Err("missing on a task whose reminder is on".to_owned()),
            };
            match at {
                Ok(at) => self.push(
                    SourceKind::Reminder,
                    &t.id,
                    Entity::Reminder(Reminder {
                        id: ReminderId::from_uuid(uuid_for(SourceKind::Reminder, &t.id, f.created)),
                        task: id,
                        at,
                        created: stamp(f.created),
                        deleted: None,
                    }),
                ),
                Err(problem) => self.lossy(
                    SourceKind::Task,
                    &t.id,
                    "reminderDateTime",
                    problem,
                    "imported without the reminder",
                ),
            }
        }
        Ok(())
    }
}

/// Builds a bundle from an export and a cache copy.
pub fn build(export: &Export, cache: &Cache, opts: &BuildOptions) -> Result<Bundle, BundleError> {
    let mut b = Builder {
        export,
        cache,
        opts,
        items: Vec::new(),
        blobs: Vec::new(),
        report: Report {
            orphan_cache_steps: cache.orphan_steps,
            my_day_marks_skipped: cache.my_day_marks,
            ..Report::default()
        },
        tags: BTreeMap::new(),
    };
    let fallback = b.earliest();
    b.tags(fallback);
    let lists = b.structure(fallback)?;
    for (i, list) in lists {
        let l = &export.lists[i];
        let order = b.task_order(l, fallback);
        for (ti, key) in order.into_iter().zip(keys(l.tasks.len())?) {
            b.task(l, list, &l.tasks[ti], key, fallback)?;
        }
    }
    if !opts.allow_lossy
        && let Some(first) = b.report.lossy.first()
    {
        return Err(BundleError::Lossy {
            first: first.to_string(),
            lossy: b.report.lossy,
        });
    }
    let counts = Counts::of(&b.items, &b.blobs);
    Ok(Bundle {
        format: FORMAT.to_owned(),
        version: VERSION,
        protocol: PROTOCOL_VERSION,
        time_zone: opts.time_zone_name.clone(),
        counts,
        items: b.items,
        blobs: b.blobs,
        report: b.report,
    })
}
