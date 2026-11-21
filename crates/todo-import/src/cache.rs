//! Read-only access to a **copy** of the Microsoft To Do desktop cache (`todosqlite.db`).
//!
//! Graph does not expose list groups, list order, task order, step order or list themes, so those come
//! from the cache. Email metadata of flagged emails (sender, received time) also comes from here,
//! because Graph's `linkedResource` carries only a subject and a link.
//!
//! Positions in the cache are date-time strings. Observed semantics, which the bundle relies on:
//!
//! - groups, and lists within a group or at the top level, show in ascending position order;
//! - tasks show in the list's `sorting_direction` (descending by default: new tasks get a later
//!   position and appear on top);
//! - steps show in descending position order (new steps get an earlier position and appear last).
//!
//! The cache is never opened from the live app directory, and only read-only.

use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Errors while reading the cache.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// SQLite failed.
    #[error("cache database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The path points at the live app data, not a copy.
    #[error(
        "refusing to open the live To Do cache; copy todosqlite.db* to a temporary directory and pass the copy"
    )]
    LiveCache,
    /// The file does not exist.
    #[error("cache copy {0} does not exist")]
    Missing(String),
}

/// A position string from the cache, ordered by the time it encodes (falling back to the text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachePos(pub String);

impl CachePos {
    fn key(&self) -> (Option<Timestamp>, &str) {
        (self.0.parse::<Timestamp>().ok(), &self.0)
    }
}

impl Ord for CachePos {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for CachePos {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A list group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheGroup {
    /// Server ID (stable across devices).
    pub online_id: String,
    /// Local ID (what lists refer to).
    pub local_id: String,
    /// Name.
    pub name: String,
    /// Position.
    pub position: Option<CachePos>,
}

/// A list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFolder {
    /// Server ID: the Graph list ID.
    pub online_id: String,
    /// Local ID.
    pub local_id: String,
    /// Local ID of its group.
    pub group_local_id: Option<String>,
    /// Position among its siblings.
    pub position: Option<CachePos>,
    /// True for the default list.
    pub is_default: bool,
    /// Theme colour name, for example `dark_blue`.
    pub theme_color: Option<String>,
    /// `ASCENDING` or `DESCENDING`.
    pub sorting_direction: Option<String>,
}

impl CacheFolder {
    /// True if the list shows its tasks in ascending position order.
    #[must_use]
    pub fn tasks_ascending(&self) -> bool {
        self.sorting_direction.as_deref() == Some("ASCENDING")
    }
}

/// Metadata of a flagged email.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmailMeta {
    /// Sender name.
    pub sender_name: Option<String>,
    /// Sender address.
    pub sender_address: Option<String>,
    /// When it was received (ISO 8601).
    pub received: Option<String>,
}

/// Everything the importer takes from the cache.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    /// List groups, live only.
    pub groups: Vec<CacheGroup>,
    /// Lists by Graph ID, live only.
    pub folders: BTreeMap<String, CacheFolder>,
    /// Task position by Graph task ID.
    pub task_pos: BTreeMap<String, CachePos>,
    /// Due date (`YYYY-MM-DD`) by Graph task ID, for cross-checking the Graph due date.
    pub task_due: BTreeMap<String, String>,
    /// Step position and owning Graph task ID, by Graph step ID. Steps whose task is missing are not
    /// here.
    pub step_pos: BTreeMap<String, (String, CachePos)>,
    /// Steps in the cache whose task does not exist (ignored).
    pub orphan_steps: usize,
    /// Tasks marked for My Day (not imported).
    pub my_day_marks: usize,
    /// Email metadata by Outlook item ID (the cache's `online_id` and the preview's `RestId`).
    pub emails: BTreeMap<String, EmailMeta>,
}

/// True if `path` lies inside the To Do app's package directory.
#[must_use]
pub fn is_live_path(path: &Path) -> bool {
    let p = path.to_string_lossy().to_ascii_lowercase();
    p.contains("microsoft.todos_8wekyb3d8bbwe")
}

fn str_or_none(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

impl Cache {
    /// Opens a copy of the cache read-only and loads what the importer needs.
    pub fn load(path: &Path) -> Result<Self, CacheError> {
        if is_live_path(path) {
            return Err(CacheError::LiveCache);
        }
        if !path.is_file() {
            return Err(CacheError::Missing(path.display().to_string()));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::read(&conn)
    }

    /// Loads from an open connection (for tests that build a synthetic cache in memory).
    pub fn read(conn: &Connection) -> Result<Self, CacheError> {
        let mut c = Self::default();

        let mut st = conn.prepare(
            "SELECT online_id, local_id, name, position FROM groups
             WHERE deleted = 0 AND online_id IS NOT NULL",
        )?;
        c.groups = st
            .query_map([], |r| {
                Ok(CacheGroup {
                    online_id: r.get(0)?,
                    local_id: r.get(1)?,
                    name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    position: str_or_none(r.get(3)?).map(CachePos),
                })
            })?
            .collect::<Result<_, _>>()?;

        let mut st = conn.prepare(
            "SELECT online_id, local_id, group_local_id, position, is_default, theme_color,
                    sorting_direction
             FROM task_folders WHERE deleted = 0 AND online_id IS NOT NULL",
        )?;
        let groups: BTreeSet<String> = c.groups.iter().map(|g| g.local_id.clone()).collect();
        for f in st.query_map([], |r| {
            Ok(CacheFolder {
                online_id: r.get(0)?,
                local_id: r.get(1)?,
                group_local_id: str_or_none(r.get(2)?),
                position: str_or_none(r.get(3)?).map(CachePos),
                is_default: r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
                theme_color: str_or_none(r.get(5)?),
                sorting_direction: str_or_none(r.get(6)?),
            })
        })? {
            let mut f = f?;
            // A list in a deleted or unknown group shows at the top level.
            if f.group_local_id
                .as_ref()
                .is_some_and(|g| !groups.contains(g))
            {
                f.group_local_id = None;
            }
            c.folders.insert(f.online_id.clone(), f);
        }

        let mut st = conn.prepare(
            "SELECT online_id, local_id, position, due_date, committed_date FROM tasks
             WHERE deleted = 0 AND online_id IS NOT NULL",
        )?;
        let mut task_by_local: BTreeMap<String, String> = BTreeMap::new();
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                str_or_none(r.get(2)?),
                str_or_none(r.get(3)?),
                str_or_none(r.get(4)?),
            ))
        })? {
            let (online, local, pos, due, my_day) = row?;
            task_by_local.insert(local, online.clone());
            if let Some(p) = pos {
                c.task_pos.insert(online.clone(), CachePos(p));
            }
            if let Some(d) = due {
                c.task_due.insert(online.clone(), d);
            }
            if my_day.is_some() {
                c.my_day_marks += 1;
            }
        }

        let mut st = conn.prepare(
            "SELECT online_id, task_local_id, position FROM steps
             WHERE deleted = 0 AND online_id IS NOT NULL",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                str_or_none(r.get(2)?),
            ))
        })? {
            let (online, task_local, pos) = row?;
            match task_local.and_then(|t| task_by_local.get(&t).cloned()) {
                Some(task) => {
                    if let Some(p) = pos {
                        c.step_pos.insert(online, (task, CachePos(p)));
                    }
                }
                None => c.orphan_steps += 1,
            }
        }

        let mut st = conn.prepare(
            "SELECT online_id, preview FROM linked_entities
             WHERE deleted = 0 AND entity_type = 'message'",
        )?;
        for row in st.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })? {
            let (online, preview) = row?;
            let v: serde_json::Value = preview
                .as_deref()
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or_default();
            let s = |v: &serde_json::Value| v.as_str().map(str::to_owned).filter(|s| !s.is_empty());
            let meta = EmailMeta {
                sender_name: s(&v["From"]["EmailAddress"]["Name"]),
                sender_address: s(&v["From"]["EmailAddress"]["Address"]),
                received: s(&v["ReceivedDateTime"]),
            };
            for key in [online, s(&v["RestId"])].into_iter().flatten() {
                c.emails.insert(key, meta.clone());
            }
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_order_by_time_not_text() {
        // Lexically "…05.12Z" sorts after "…05.123Z"; by time it is earlier.
        let a = CachePos("2024-01-01T10:00:05.12Z".into());
        let b = CachePos("2024-01-01T10:00:05.123Z".into());
        assert!(a < b);
    }

    #[test]
    fn live_path_is_detected() {
        assert!(is_live_path(Path::new(
            r"C:\Users\x\AppData\Local\Packages\Microsoft.Todos_8wekyb3d8bbwe\LocalState\db"
        )));
        assert!(!is_live_path(Path::new(r"C:\Temp\todo-copy\todosqlite.db")));
    }
}
