//! The raw Microsoft Graph export, as written by `spikes/graph-export/export.py`.
//!
//! Layout of the export directory:
//!
//! ```text
//! <export-dir>/
//!   lists.json                       every list (/me/todo/lists)
//!   summary.json                     counts per list, and the export start time
//!   lists/NN_<hash>/list.json        one list
//!   lists/NN_<hash>/tasks.json       its tasks (completed included)
//!   lists/NN_<hash>/task_children.json
//!                                    per task ID: checklistItems, linkedResources, attachments
//!   lists/NN_<hash>/attachments/...  attachment content (also inline as base64 `contentBytes`)
//! ```
//!
//! Only the fields the importer needs are modelled; unknown fields are ignored.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Errors while reading an export.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        source: std::io::Error,
    },
    /// A file is not valid JSON of the expected shape.
    #[error("cannot parse {path}: {source}")]
    Json {
        /// The file.
        path: PathBuf,
        /// The error.
        source: serde_json::Error,
    },
    /// The export is inconsistent.
    #[error("{0}")]
    Invalid(String),
}

/// A Graph `dateTimeTimeZone`: a civil date-time plus a time zone name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DateTimeTz {
    /// For example `2026-01-31T23:00:00.0000000`.
    #[serde(rename = "dateTime")]
    pub date_time: String,
    /// For example `UTC`.
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

/// A Graph `itemBody`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemBody {
    /// The text.
    #[serde(default)]
    pub content: String,
    /// `text` or `html`.
    #[serde(default)]
    pub content_type: String,
}

/// A To Do list (`todoTaskList`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphList {
    /// Graph ID.
    pub id: String,
    /// Name.
    #[serde(default)]
    pub display_name: String,
    /// `none`, `defaultList` or `flaggedEmails`.
    #[serde(default)]
    pub wellknown_list_name: Option<String>,
}

impl GraphList {
    /// True for the "Flagged Emails" list.
    #[must_use]
    pub fn is_flagged_emails(&self) -> bool {
        self.wellknown_list_name.as_deref() == Some("flaggedEmails")
    }

    /// True for the default "Tasks" list.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.wellknown_list_name.as_deref() == Some("defaultList")
    }
}

/// A task (`todoTask`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphTask {
    /// Graph ID.
    pub id: String,
    /// Title.
    #[serde(default)]
    pub title: Option<String>,
    /// `notStarted`, `inProgress`, `completed`, `waitingOnOthers` or `deferred`.
    #[serde(default)]
    pub status: String,
    /// `low`, `normal` or `high`.
    #[serde(default)]
    pub importance: String,
    /// Notes.
    #[serde(default)]
    pub body: Option<ItemBody>,
    /// Creation time (UTC, ISO 8601).
    pub created_date_time: String,
    /// Last modification time (UTC, ISO 8601).
    #[serde(default)]
    pub last_modified_date_time: Option<String>,
    /// Completion time.
    #[serde(default)]
    pub completed_date_time: Option<DateTimeTz>,
    /// Due date, as midnight in some time zone.
    #[serde(default)]
    pub due_date_time: Option<DateTimeTz>,
    /// Reminder time.
    #[serde(default)]
    pub reminder_date_time: Option<DateTimeTz>,
    /// Whether the reminder is on.
    #[serde(default)]
    pub is_reminder_on: bool,
    /// Category names.
    #[serde(default)]
    pub categories: Vec<String>,
    /// `patternedRecurrence`, kept raw so that unmappable shapes can be reported verbatim.
    #[serde(default)]
    pub recurrence: Option<serde_json::Value>,
}

impl GraphTask {
    /// True if Graph reports the task as completed.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        self.status == "completed"
    }
}

/// A step (`checklistItem`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    /// Graph ID.
    pub id: String,
    /// Title.
    #[serde(default)]
    pub display_name: String,
    /// Checked.
    #[serde(default)]
    pub is_checked: bool,
    /// When it was checked.
    #[serde(default)]
    pub checked_date_time: Option<String>,
    /// Creation time.
    pub created_date_time: String,
}

/// A linked resource (`linkedResource`), for example a flagged email.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedResource {
    /// Graph ID.
    pub id: String,
    /// Link that opens the item.
    #[serde(default)]
    pub web_url: Option<String>,
    /// For example `Outlook`.
    #[serde(default)]
    pub application_name: Option<String>,
    /// Title (an email's subject).
    #[serde(default)]
    pub display_name: Option<String>,
    /// ID in the source application (an Outlook item ID).
    #[serde(default)]
    pub external_id: Option<String>,
}

/// An attachment (`taskFileAttachment`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphAttachment {
    /// Graph ID.
    pub id: String,
    /// File name.
    #[serde(default)]
    pub name: String,
    /// MIME type.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Size in bytes, as reported by Graph.
    #[serde(default)]
    pub size: Option<u64>,
    /// Last modification time.
    #[serde(default)]
    pub last_modified_date_time: Option<String>,
    /// Base64 content.
    #[serde(default)]
    pub content_bytes: Option<String>,
    /// Path of the downloaded file, relative to the export directory.
    #[serde(default, rename = "_exportedFile")]
    pub exported_file: Option<String>,
}

/// The children of one task.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskChildren {
    /// Steps.
    #[serde(default)]
    pub checklist_items: Vec<ChecklistItem>,
    /// Linked resources.
    #[serde(default)]
    pub linked_resources: Vec<LinkedResource>,
    /// Attachments.
    #[serde(default)]
    pub attachments: Vec<GraphAttachment>,
}

/// One list directory of the export.
#[derive(Clone, Debug)]
pub struct ExportList {
    /// The list directory.
    pub dir: PathBuf,
    /// The list.
    pub list: GraphList,
    /// Its tasks, in export order.
    pub tasks: Vec<GraphTask>,
    /// Children by task ID.
    pub children: BTreeMap<String, TaskChildren>,
}

impl ExportList {
    /// The children of a task (empty if the export has none).
    #[must_use]
    pub fn children_of(&self, task: &str) -> TaskChildren {
        self.children.get(task).cloned().unwrap_or_default()
    }
}

/// A whole export.
#[derive(Clone, Debug)]
pub struct Export {
    /// The export directory.
    pub root: PathBuf,
    /// Lists in export order.
    pub lists: Vec<ExportList>,
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    lists: Vec<SummaryList>,
}

#[derive(Deserialize)]
struct SummaryList {
    id: String,
    dir: String,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ExportError> {
    let text = std::fs::read_to_string(path).map_err(|source| ExportError::Io {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| ExportError::Json {
        path: path.to_owned(),
        source,
    })
}

impl Export {
    /// Loads an export directory.
    ///
    /// The list order and directories come from `summary.json`; every list in `lists.json` must have a
    /// directory there.
    pub fn load(root: &Path) -> Result<Self, ExportError> {
        let all: Vec<GraphList> = read_json(&root.join("lists.json"))?;
        let summary: Summary = read_json(&root.join("summary.json"))?;
        let dirs: BTreeMap<String, String> =
            summary.lists.into_iter().map(|l| (l.id, l.dir)).collect();
        let mut lists = Vec::with_capacity(all.len());
        for l in all {
            let rel = dirs.get(&l.id).ok_or_else(|| {
                ExportError::Invalid(format!(
                    "list {} has no directory in summary.json (was the export cut short?)",
                    crate::label(&l.id)
                ))
            })?;
            let dir = root.join(rel);
            let list: GraphList = read_json(&dir.join("list.json"))?;
            if list.id != l.id {
                return Err(ExportError::Invalid(format!(
                    "{} holds a different list",
                    dir.display()
                )));
            }
            let tasks: Vec<GraphTask> = read_json(&dir.join("tasks.json"))?;
            let children: BTreeMap<String, TaskChildren> =
                read_json(&dir.join("task_children.json"))?;
            lists.push(ExportList {
                dir,
                list,
                tasks,
                children,
            });
        }
        Ok(Self {
            root: root.to_owned(),
            lists,
        })
    }

    /// Reads the content of an attachment: the inline base64 if present, else the exported file.
    pub fn attachment_content(&self, a: &GraphAttachment) -> Result<Option<Vec<u8>>, ExportError> {
        use base64::Engine;
        if let Some(b) = a.content_bytes.as_deref().filter(|b| !b.is_empty()) {
            return base64::engine::general_purpose::STANDARD
                .decode(b)
                .map(Some)
                .map_err(|e| {
                    ExportError::Invalid(format!(
                        "attachment {} has invalid base64: {e}",
                        crate::label(&a.id)
                    ))
                });
        }
        match &a.exported_file {
            Some(rel) => {
                let path = self.root.join(rel);
                std::fs::read(&path)
                    .map(Some)
                    .map_err(|source| ExportError::Io { path, source })
            }
            None => Ok(None),
        }
    }
}
