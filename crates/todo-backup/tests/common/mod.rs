#![allow(dead_code, clippy::unwrap_used)]

pub mod fake_s3;

use std::path::{Path, PathBuf};

use age::x25519::Identity;
use rusqlite::Connection;
use todo_backup::config::Config;

/// A data directory with a WAL database shaped like the server's: an append-only `events` table, plus a
/// projection that must always agree with it.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub identity: Identity,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let f = Self {
            dir,
            identity: Identity::generate(),
        };
        let conn = f.open();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE tally (id INTEGER PRIMARY KEY CHECK (id = 1), n INTEGER NOT NULL);
             INSERT INTO tally VALUES (1, 0);",
        )
        .unwrap();
        f
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.path().join("todo.sqlite3")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir.path().join("backup-state")
    }

    pub fn open(&self) -> Connection {
        let c = Connection::open(self.db_path()).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        c
    }

    /// Appends `n` events in one transaction and bumps the projection with them.
    pub fn append(&self, n: usize) {
        append(&mut self.open(), n);
    }

    pub fn config_text(&self, extra: &str) -> String {
        format!(
            "db_path = '{}'\nstate_dir = '{}'\nrecipient = '{}'\ncredentials_file = '{}'\n{extra}",
            self.db_path().display(),
            self.state_dir().display(),
            self.identity.to_public(),
            self.dir.path().join("r2.json").display(),
        )
    }

    pub fn config(&self) -> Config {
        self.config_with("")
    }

    pub fn config_with(&self, extra: &str) -> Config {
        Config::parse(&self.config_text(extra), self.dir.path()).unwrap()
    }

    pub fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

pub fn append(conn: &mut Connection, n: usize) {
    let tx = conn.transaction().unwrap();
    for i in 0..n {
        tx.execute(
            "INSERT INTO events (kind, body) VALUES ('task.created', ?1)",
            [format!("{{\"title\":\"task {i}\"}}")],
        )
        .unwrap();
    }
    tx.execute("UPDATE tally SET n = n + ?1", [n as i64])
        .unwrap();
    tx.commit().unwrap();
}

/// Appends `n` events of 4 KiB of random hex each.
pub fn fill_incompressible(f: &Fixture, n: usize) {
    let mut conn = f.open();
    let tx = conn.transaction().unwrap();
    for _ in 0..n {
        tx.execute(
            "INSERT INTO events (kind, body) VALUES ('blob', hex(randomblob(4096)))",
            [],
        )
        .unwrap();
    }
    tx.execute("UPDATE tally SET n = n + ?1", [n as i64])
        .unwrap();
    tx.commit().unwrap();
}

/// `(count(*) of events, max(seq), tally)` of a database file.
pub fn counters(path: &Path) -> (i64, Option<i64>, i64) {
    let c = Connection::open(path).unwrap();
    let (count, max): (i64, Option<i64>) = c
        .query_row("SELECT count(*), max(seq) FROM events", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let tally: i64 = c
        .query_row("SELECT n FROM tally", [], |r| r.get(0))
        .unwrap();
    (count, max, tally)
}

/// Asserts that only the current user has access to the file `path`: exactly one allow ACE, for the current
/// user's SID, on Windows (inherited or not); mode 0600 on Unix.
pub fn assert_owner_only_file(path: &Path) {
    let access = todo_backup::secure::describe_access(path).unwrap();
    match todo_backup::secure::expected_private_sddl(false).unwrap() {
        Some(expected) => {
            let sid = expected.rsplit(";;;").next().unwrap().trim_end_matches(')');
            assert_eq!(access.matches("(A;").count(), 1, "{access}");
            assert!(access.contains(&format!(";FA;;;{sid})")), "{access}");
            assert!(!access.contains("(D;"), "{access}");
        }
        None => assert_eq!(access, "mode=600"),
    }
}

pub fn ts(s: &str) -> jiff::Timestamp {
    s.parse().unwrap()
}
