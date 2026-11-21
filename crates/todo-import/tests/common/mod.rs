//! Shared helpers: the synthetic export fixture, a synthetic cache, and a bundle built from both.
#![allow(dead_code, clippy::unwrap_used)]

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use todo_import::bundle::{BuildOptions, Bundle, build};
use todo_import::cache::Cache;
use todo_import::graph::Export;

/// Time zone of the fixture's owner (due dates are stored as local midnight in UTC).
pub const TZ: &str = "Europe/Berlin";

pub fn export_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export")
}

/// The cache tables and columns the importer reads, with synthetic rows matching the fixture:
///
/// - groups `Work` (earlier) and `Home`; Groceries in Home, Projects in Work (ascending task order);
/// - top level: Tasks (default, no position), Someday, Flagged Emails;
/// - Groceries tasks ordered G-3, G-1, G-2 (descending positions);
/// - steps of P-1 ordered S-2, S-3, S-1 (descending positions), plus two orphan steps;
/// - email metadata for MSG-1 only; two stale My Day marks.
pub const CACHE_SQL: &str = r#"
CREATE TABLE groups (online_id TEXT, local_id TEXT, name TEXT, position TEXT, deleted INTEGER DEFAULT 0);
CREATE TABLE task_folders (online_id TEXT, local_id TEXT, name TEXT, group_local_id TEXT, position TEXT,
    is_default INTEGER DEFAULT 0, theme_color TEXT, sorting_direction TEXT, folder_type TEXT,
    deleted INTEGER DEFAULT 0);
CREATE TABLE tasks (online_id TEXT, local_id TEXT, task_folder_local_id TEXT, position TEXT, due_date TEXT,
    committed_date TEXT, deleted INTEGER DEFAULT 0);
CREATE TABLE steps (online_id TEXT, local_id TEXT, task_local_id TEXT, position TEXT, deleted INTEGER DEFAULT 0);
CREATE TABLE linked_entities (online_id TEXT, local_id TEXT, entity_type TEXT, preview TEXT,
    task_local_id TEXT, deleted INTEGER DEFAULT 0);

INSERT INTO groups VALUES ('GR-home', 'g-home', 'Home', '2025-01-02T10:00:00Z', 0),
                          ('GR-work', 'g-work', 'Work', '2025-01-01T10:00:00Z', 0),
                          ('GR-gone', 'g-gone', 'Deleted group', '2025-01-03T10:00:00Z', 1);
INSERT INTO task_folders VALUES
  ('L-tasks', 'f-tasks', 'Tasks', NULL, NULL, 1, 'dark_blue', 'DESCENDING', NULL, 0),
  ('L-flagged', 'f-flagged', 'Flagged Emails', NULL, '2025-02-01T10:00:00Z', 0, 'blue', 'DESCENDING',
   'com.microsoft.outlook.email.flagged', 0),
  ('L-groceries', 'f-groceries', 'Groceries', 'g-home', '2025-01-05T10:00:00Z', 0, 'light_green', 'DESCENDING', NULL, 0),
  ('L-projects', 'f-projects', 'Projects', 'g-work', '2025-01-06T10:00:00Z', 0, 'mystery_theme', 'ASCENDING', NULL, 0),
  ('L-someday', 'f-someday', 'Someday', NULL, '2025-01-15T10:00:00Z', 0, NULL, NULL, NULL, 0),
  ('L-cache-only', 'f-cache-only', 'Only in the cache', NULL, '2025-01-16T10:00:00Z', 0, NULL, NULL, NULL, 0);
INSERT INTO tasks VALUES
  ('T-1', 't1', 'f-tasks', '2026-01-10T08:00:00Z', '2026-03-02', '2026-01-11', 0),
  ('T-2', 't2', 'f-tasks', '2026-01-01T09:00:00Z', NULL, NULL, 0),
  ('T-3', 't3', 'f-tasks', '2026-01-02T09:00:00Z', '2026-03-09', '2026-01-02', 0),
  ('T-4', 't4', 'f-tasks', '2026-01-03T09:00:00Z', '2026-03-02', NULL, 0),
  ('F-1', 'f1', 'f-flagged', '2026-01-20T12:00:00Z', NULL, NULL, 0),
  ('F-2', 'f2', 'f-flagged', '2026-01-21T12:00:00Z', NULL, NULL, 0),
  ('G-1', 'g1', 'f-groceries', '2026-01-05T09:00:00.5Z', NULL, NULL, 0),
  ('G-2', 'g2', 'f-groceries', '2026-01-05T09:00:00.25Z', NULL, NULL, 0),
  ('G-3', 'g3', 'f-groceries', '2026-01-07T09:00:00Z', NULL, NULL, 0),
  ('P-1', 'p1', 'f-projects', '2026-01-09T09:00:00Z', NULL, NULL, 0),
  ('P-2', 'p2', 'f-projects', '2026-01-08T09:00:00Z', NULL, NULL, 0),
  ('X-deleted', 'xd', 'f-projects', '2026-01-08T09:00:00Z', NULL, NULL, 1);
INSERT INTO steps VALUES
  ('S-1', 's1', 'p1', '2026-01-08T09:01:00Z', 0),
  ('S-2', 's2', 'p1', '2026-01-08T09:09:00Z', 0),
  ('S-3', 's3', 'p1', '2026-01-08T09:05:00Z', 0),
  ('S-orphan-1', 'so1', 'no-such-task', '2026-01-08T09:05:00Z', 0),
  ('S-orphan-2', 'so2', 'xd', '2026-01-08T09:05:00Z', 0);
INSERT INTO linked_entities VALUES
  ('MSG-1', 'le1', 'message',
   '{"From": {"EmailAddress": {"Name": "A. Sender", "Address": "sender@example.com"}}, "ReceivedDateTime": "2026-01-20T11:58:00Z", "RestId": "REST-1"}',
   'f1', 0),
  ('A-1', 'le2', 'file', '{"ContentType": "text/plain", "Size": 11}', 'g2', 0);
"#;

pub fn cache_conn() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(CACHE_SQL).unwrap();
    c
}

/// Writes the synthetic cache to a file (for `Cache::load` and the CLI).
pub fn cache_file(dir: &Path) -> PathBuf {
    let path = dir.join("todosqlite-copy.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(CACHE_SQL).unwrap();
    path
}

pub fn export() -> Export {
    Export::load(&export_dir()).unwrap()
}

pub fn options() -> BuildOptions {
    BuildOptions {
        time_zone: jiff::tz::TimeZone::get(TZ).unwrap(),
        time_zone_name: TZ.to_owned(),
    }
}

pub fn bundle() -> Bundle {
    let cache = Cache::read(&cache_conn()).unwrap();
    build(&export(), &cache, &options()).unwrap()
}
