//! End-to-end behaviour of run, restore and drill against the in-memory store. No network.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{Fixture, counters, ts};
use todo_backup::budget::{Budget, BudgetCaps, Clock, OpClass};
use todo_backup::ops::{self, RunOutcome, Target};
use todo_backup::state::{SCRATCH_DIR, State};
use todo_backup::store::{MemoryStore, Metered};
use todo_backup::{Error, Kind, snapshot};

#[test]
fn snapshot_is_consistent_while_another_connection_writes_in_wal_mode() {
    let f = Fixture::new();
    f.append(10);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        let mut conn = f.open();
        std::thread::spawn(move || {
            let mut txns = 0u32;
            while !stop.load(Ordering::Relaxed) {
                common::append(&mut conn, 25);
                txns += 1;
            }
            // Keep the connection (and so the WAL) open until here: no checkpoint-on-close in between.
            txns
        })
    };

    let mut seen = Vec::new();
    for i in 0..8 {
        let snap = f.dir.path().join(format!("snap-{i}.sqlite3"));
        snapshot::take(&f.db_path(), &snap).unwrap();
        let info = snapshot::inspect(&snap).unwrap();
        let (count, max, tally) = counters(&snap);
        // One committed state: the projection agrees with the events, and nothing is half-applied.
        assert_eq!(count, tally, "snapshot {i} mixes two states");
        assert_eq!(
            count % 25,
            10,
            "snapshot {i} contains part of a transaction"
        );
        assert_eq!(info.events_count, Some(count as u64));
        assert_eq!(info.max_seq, max);
        // The snapshot is a single self-contained file in rollback-journal mode.
        assert!(!f.dir.path().join(format!("snap-{i}.sqlite3-wal")).exists());
        seen.push(count);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    stop.store(true, Ordering::Relaxed);
    let txns = writer.join().unwrap();
    assert!(txns > 0);
    // The writer kept going while the snapshots were taken.
    assert!(seen.windows(2).any(|w| w[1] > w[0]), "{seen:?}");
}

#[test]
fn run_uploads_then_skips_when_unchanged_then_uploads_again() {
    let f = Fixture::new();
    f.append(3);
    let cfg = f.config();
    let store = MemoryStore::new();

    let first = ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T06:00:00Z"),
    )
    .unwrap();
    let RunOutcome::Uploaded(rec) = first else {
        panic!("expected an upload, got {first:?}")
    };
    assert_eq!(rec.key, "frequent/20261121T060000Z-3.sqlite3.zst.age");
    assert_eq!((rec.max_seq, rec.events_count), (Some(3), Some(3)));
    assert!(
        store
            .object(&rec.key)
            .unwrap()
            .starts_with(b"age-encryption.org/v1")
    );

    // Nothing changed: no request at all.
    let again = ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T12:00:00Z"),
    )
    .unwrap();
    assert_eq!(again, RunOutcome::Unchanged(rec.clone()));
    assert_eq!(store.ops(), ["PUT"]);

    // The other kind has its own record, so it uploads.
    assert!(matches!(
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T12:00:01Z")).unwrap(),
        RunOutcome::Uploaded(_)
    ));

    // A change that leaves max(seq) alone is still a change.
    f.open().execute("UPDATE tally SET n = n", []).unwrap();
    f.open()
        .execute("CREATE TABLE sessions (id INTEGER PRIMARY KEY)", [])
        .unwrap();
    let RunOutcome::Uploaded(next) = ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T18:00:00Z"),
    )
    .unwrap() else {
        panic!("schema change must upload")
    };
    assert_eq!(next.max_seq, Some(3));
    assert_ne!(next.snapshot_sha256, rec.snapshot_sha256);

    // --force uploads even when unchanged.
    assert!(matches!(
        ops::run(
            &cfg,
            &store,
            Kind::Frequent,
            true,
            ts("2026-11-21T19:00:00Z")
        )
        .unwrap(),
        RunOutcome::Uploaded(_)
    ));

    // The normal path never lists.
    assert!(!store.ops().contains(&"LIST"), "{:?}", store.ops());
    let st = State::load(&cfg.state_dir).unwrap();
    assert_eq!(
        st.get(Kind::Frequent).unwrap().key,
        "frequent/20261121T190000Z-3.sqlite3.zst.age"
    );
    let budget = Budget::open(&cfg.state_dir, cfg.budget, ts("2026-11-21T20:00:00Z")).unwrap();
    assert_eq!((budget.usage().class_a, budget.usage().class_b), (4, 0));
}

#[test]
fn restore_round_trips_and_refuses_to_overwrite() {
    let f = Fixture::new();
    f.append(7);
    let cfg = f.config();
    let store = MemoryStore::new();
    let RunOutcome::Uploaded(rec) =
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap()
    else {
        panic!()
    };

    let out = f.out("restored.sqlite3");
    let r = ops::restore(
        &cfg,
        &store,
        &Target::Key(rec.key.clone()),
        std::slice::from_ref(&f.identity),
        &out,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert_eq!(r.sha256, rec.snapshot_sha256);
    assert_eq!(counters(&out), (7, Some(7), 7));

    // An existing file is never touched, and no request is spent finding that out.
    std::fs::write(f.out("taken.sqlite3"), b"precious").unwrap();
    let ops_before = store.ops().len();
    let err = ops::restore(
        &cfg,
        &store,
        &Target::Key(rec.key.clone()),
        std::slice::from_ref(&f.identity),
        &f.out("taken.sqlite3"),
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::OutputExists(_)), "{err}");
    assert_eq!(std::fs::read(f.out("taken.sqlite3")).unwrap(), b"precious");
    assert_eq!(store.ops().len(), ops_before);

    // The wrong identity cannot decrypt, and leaves nothing behind.
    let err = ops::restore(
        &cfg,
        &store,
        &Target::Key(rec.key),
        &[age::x25519::Identity::generate()],
        &f.out("wrong.sqlite3"),
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Crypto(_)), "{err}");
    assert!(!f.out("wrong.sqlite3").exists());
}

#[test]
fn restore_latest_lists_once_and_counts_it() {
    let f = Fixture::new();
    let cfg = f.config();
    let store = MemoryStore::new();
    for (i, t) in [
        "2026-11-19T00:00:00Z",
        "2026-11-20T00:00:00Z",
        "2026-11-21T00:00:00Z",
    ]
    .iter()
    .enumerate()
    {
        f.append(i + 1);
        ops::run(&cfg, &store, Kind::Daily, false, ts(t)).unwrap();
    }
    // A stray object that is not a backup is ignored.
    store.insert("daily/zz-notes.txt", b"x".to_vec());

    let out = f.out("latest.sqlite3");
    let r = ops::restore(
        &cfg,
        &store,
        &Target::Latest(Kind::Daily),
        std::slice::from_ref(&f.identity),
        &out,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert_eq!(r.key, "daily/20261121T000000Z-6.sqlite3.zst.age");
    assert_eq!(counters(&out).1, Some(6));
    assert_eq!(&store.ops()[3..], ["LIST", "GET"]);
    let b = Budget::open(&cfg.state_dir, cfg.budget, ts("2026-11-21T02:00:00Z")).unwrap();
    assert_eq!((b.usage().class_a, b.usage().class_b), (3 + 1, 1));

    assert!(matches!(
        ops::restore(
            &cfg,
            &store,
            &Target::Latest(Kind::Frequent),
            std::slice::from_ref(&f.identity),
            &f.out("none.sqlite3"),
            ts("2026-11-21T01:00:00Z"),
        ),
        Err(Error::NoBackup(_))
    ));
}

#[test]
fn budget_refuses_before_any_request() {
    let f = Fixture::new();
    f.append(1);
    let cfg = f.config_with("[budget]\nclass_a_monthly = 2\nclass_b_monthly = 1\n");
    let store = MemoryStore::new();
    let now = ts("2026-11-21T00:00:00Z");

    ops::run(&cfg, &store, Kind::Frequent, false, now).unwrap();
    f.append(1);
    ops::run(&cfg, &store, Kind::Frequent, false, now).unwrap();
    f.append(1);
    let err = ops::run(&cfg, &store, Kind::Frequent, false, now).unwrap_err();
    assert!(
        matches!(
            err,
            Error::BudgetExceeded {
                class: OpClass::A,
                used: 2,
                cap: 2,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(store.ops(), ["PUT", "PUT"]);

    // Class B is separate: one GET fits, the second is refused without being sent.
    let key = State::load(&cfg.state_dir).unwrap().frequent.unwrap().key;
    let restore = |name: &str| {
        ops::restore(
            &cfg,
            &store,
            &Target::Key(key.clone()),
            std::slice::from_ref(&f.identity),
            &f.out(name),
            now,
        )
    };
    restore("a.sqlite3").unwrap();
    assert!(matches!(
        restore("b.sqlite3"),
        Err(Error::BudgetExceeded {
            class: OpClass::B,
            ..
        })
    ));
    assert_eq!(store.ops(), ["PUT", "PUT", "GET"]);

    // A new month starts from zero.
    f.append(1);
    ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-12-01T00:00:00Z"),
    )
    .unwrap();
}

#[test]
fn large_snapshots_use_multipart() {
    let f = Fixture::new();
    // Incompressible rows, so the sealed object spans several parts.
    common::fill_incompressible(&f, 64);
    let cfg = f.config_with(
        "[upload]\nmultipart_threshold_bytes = 65536\npart_size_bytes = 65536\nzstd_level = 1\n",
    );
    let store = MemoryStore::new();
    let RunOutcome::Uploaded(rec) =
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap()
    else {
        panic!()
    };
    let parts = rec.object_bytes.div_ceil(65536) as usize;
    assert!(parts >= 3, "{rec:?}");
    let ops_seen = store.ops();
    assert_eq!(ops_seen.first(), Some(&"CREATE"));
    assert_eq!(ops_seen.last(), Some(&"COMPLETE"));
    assert_eq!(ops_seen.iter().filter(|o| **o == "PART").count(), parts);
    assert_eq!(store.open_uploads(), 0);
    assert_eq!(
        store.object(&rec.key).unwrap().len() as u64,
        rec.object_bytes
    );

    let b = Budget::open(&cfg.state_dir, cfg.budget, ts("2026-11-21T00:00:00Z")).unwrap();
    assert_eq!(b.usage().class_a, parts as u64 + 2);

    let report = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T00:00:00Z"),
    )
    .unwrap();
    assert!(report.passed(), "{report}");
}

#[test]
fn multipart_that_cannot_finish_is_not_started() {
    let f = Fixture::new();
    common::fill_incompressible(&f, 64);
    let cfg = f.config_with(
        "[budget]\nclass_a_monthly = 3\n[upload]\nmultipart_threshold_bytes = 65536\npart_size_bytes = 65536\nzstd_level = 1\n",
    );
    let store = MemoryStore::new();
    let err = ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap_err();
    assert!(matches!(err, Error::BudgetExceeded { .. }), "{err}");
    assert!(store.ops().is_empty());
}

#[test]
fn drill_passes_on_a_good_backup() {
    let f = Fixture::new();
    f.append(5);
    let cfg = f.config();
    let store = MemoryStore::new();
    ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-20T00:00:00Z")).unwrap();
    f.append(2);
    ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T00:00:00Z"),
    )
    .unwrap();

    let report = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert!(report.passed(), "{report}");
    assert!(report.recorded);
    // The newest of either kind, from the local record: one GET, no LIST.
    assert_eq!(report.key, "frequent/20261121T000000Z-7.sqlite3.zst.age");
    assert_eq!(report.restored.info.events_count, Some(7));
    assert_eq!(store.ops(), ["PUT", "PUT", "GET"]);
    assert!(report.to_string().contains("PASSED"));

    let daily = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        Some(Kind::Daily),
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert!(daily.passed(), "{daily}");
    assert_eq!(daily.restored.info.max_seq, Some(5));

    // The scratch copy is cleaned up.
    // The scratch copy is cleaned up.
    let leftovers: Vec<_> = std::fs::read_dir(cfg.state_dir.join(SCRATCH_DIR))
        .unwrap()
        .flatten()
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn unchanged_database_is_uploaded_again_before_retention_could_expire_it() {
    let f = Fixture::new();
    f.append(2);
    let cfg = f.config();
    let store = MemoryStore::new();
    let run = |kind, t: &str| ops::run(&cfg, &store, kind, false, ts(t)).unwrap();
    let uploaded = |o: &RunOutcome| matches!(o, RunOutcome::Uploaded(_));

    // frequent: a copy younger than 3 days is relied on; at 3 days a fresh one goes up.
    assert!(uploaded(&run(Kind::Frequent, "2026-11-01T00:00:00Z")));
    assert!(!uploaded(&run(Kind::Frequent, "2026-11-03T23:00:00Z")));
    assert!(uploaded(&run(Kind::Frequent, "2026-11-04T00:00:00Z")));
    assert!(!uploaded(&run(Kind::Frequent, "2026-11-04T06:00:00Z")));

    // daily: the same with 30 days.
    assert!(uploaded(&run(Kind::Daily, "2026-11-01T00:00:00Z")));
    assert!(!uploaded(&run(Kind::Daily, "2026-11-30T23:00:00Z")));
    assert!(uploaded(&run(Kind::Daily, "2026-12-01T00:00:00Z")));

    // A record dated in the future (the clock went back) is not trusted.
    assert!(uploaded(&run(Kind::Daily, "2026-11-20T00:00:00Z")));

    // With a maximum age of zero, every run uploads.
    let always = f.config_with("[dedupe]\nfrequent_max_age_hours = 0\n");
    assert!(matches!(
        ops::run(
            &always,
            &store,
            Kind::Frequent,
            false,
            ts("2026-11-04T06:00:01Z")
        )
        .unwrap(),
        RunOutcome::Uploaded(_)
    ));
}

struct Settable(std::sync::Mutex<jiff::Timestamp>);

impl Clock for Settable {
    fn now(&self) -> jiff::Timestamp {
        *self.0.lock().unwrap()
    }
}

#[test]
fn each_request_is_charged_to_the_month_it_is_sent_in() {
    let dir = tempfile::tempdir().unwrap();
    let caps = BudgetCaps {
        class_a_monthly: 1,
        class_b_monthly: 1,
    };
    let clock = Settable(std::sync::Mutex::new(ts("2026-11-30T23:59:59Z")));
    let mut budget = Budget::open(dir.path(), caps, clock.now()).unwrap();
    let store = MemoryStore::new();
    let mut metered = Metered::new(&store, &mut budget, &clock);
    metered.put("a", b"1").unwrap();
    assert!(matches!(
        metered.put("b", b"2"),
        Err(Error::BudgetExceeded { .. })
    ));
    // Midnight passes during the run: the next request belongs to December.
    *clock.0.lock().unwrap() = ts("2026-12-01T00:00:00Z");
    metered.put("b", b"2").unwrap();
    assert_eq!(store.ops(), ["PUT", "PUT"]);
    let saved = Budget::open(dir.path(), caps, ts("2026-12-15T00:00:00Z")).unwrap();
    assert_eq!(
        (saved.usage().month.as_str(), saved.usage().class_a),
        ("2026-12", 1)
    );
}

#[test]
fn restore_stages_privately_and_leaves_nothing_behind() {
    let f = Fixture::new();
    f.append(3);
    let cfg = f.config();
    let store = MemoryStore::new();
    let RunOutcome::Uploaded(rec) =
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap()
    else {
        panic!()
    };
    let dest = f.out("restore-here");
    std::fs::create_dir(&dest).unwrap();
    let out = dest.join("todo.sqlite3");
    ops::restore(
        &cfg,
        &store,
        &Target::Key(rec.key),
        std::slice::from_ref(&f.identity),
        &out,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert_eq!(counters(&out), (3, Some(3), 3));
    // Only the published file is left next to it, and nothing stays registered.
    let names: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["todo.sqlite3"]);
    let registry = cfg.state_dir.join("restore-staging.json");
    assert_eq!(std::fs::read_to_string(registry).unwrap().trim(), "[]");
    // The restored file is readable only by the current user.
    common::assert_owner_only_file(&out);
}

#[test]
fn drill_without_a_local_record_lists_and_checks_the_key() {
    let f = Fixture::new();
    f.append(4);
    let cfg = f.config();
    let store = MemoryStore::new();
    ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap();
    std::fs::remove_file(cfg.state_dir.join(State::FILE)).unwrap();

    let report = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert!(report.passed(), "{report}");
    assert!(!report.recorded);
    assert_eq!(&store.ops()[1..], ["LIST", "LIST", "GET"]);
}

#[test]
fn drill_detects_a_corrupted_object() {
    let f = Fixture::new();
    f.append(50);
    let cfg = f.config();
    let store = MemoryStore::new();
    let RunOutcome::Uploaded(rec) =
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap()
    else {
        panic!()
    };

    let mut bytes = store.object(&rec.key).unwrap();
    let mid = bytes.len() / 2 + 40;
    bytes[mid] ^= 0x20;
    store.insert(&rec.key, bytes.clone());
    let err = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Crypto(_)), "{err}");

    // A truncated object fails too.
    bytes.truncate(bytes.len() - 10);
    store.insert(&rec.key, bytes);
    assert!(
        ops::drill(
            &cfg,
            &store,
            std::slice::from_ref(&f.identity),
            None,
            ts("2026-11-21T01:00:00Z")
        )
        .is_err()
    );
}

#[test]
fn drill_detects_a_valid_object_with_the_wrong_contents() {
    let f = Fixture::new();
    f.append(5);
    let cfg = f.config();
    let store = MemoryStore::new();
    let RunOutcome::Uploaded(old) = ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T00:00:00Z"),
    )
    .unwrap() else {
        panic!()
    };
    f.append(5);
    let RunOutcome::Uploaded(new) = ops::run(
        &cfg,
        &store,
        Kind::Frequent,
        false,
        ts("2026-11-21T06:00:00Z"),
    )
    .unwrap() else {
        panic!()
    };
    // Someone swaps the newest object for an older (perfectly valid) one.
    store.insert(&new.key, store.object(&old.key).unwrap());

    let report = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T07:00:00Z"),
    )
    .unwrap();
    assert!(!report.passed(), "{report}");
    let failed: Vec<&str> = report
        .checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.name)
        .collect();
    assert!(
        failed.contains(&"max(seq) matches the record"),
        "{failed:?}"
    );
    assert!(failed.contains(&"sha256 matches the uploaded snapshot"));
    assert!(report.to_string().contains("FAILED"));
}

#[test]
fn concurrent_runs_are_refused() {
    let f = Fixture::new();
    f.append(1);
    let cfg = f.config();
    let _held = todo_backup::state::StateLock::acquire(&cfg.state_dir).unwrap();
    let store = MemoryStore::new();
    assert!(matches!(
        ops::run(
            &cfg,
            &store,
            Kind::Frequent,
            false,
            ts("2026-11-21T00:00:00Z")
        ),
        Err(Error::Locked(_))
    ));
    assert!(store.ops().is_empty());
}

#[test]
fn missing_database_is_an_error_not_an_empty_backup() {
    let f = Fixture::new();
    let mut cfg = f.config();
    cfg.db_path = f.out("nope.sqlite3");
    let store = MemoryStore::new();
    assert!(
        ops::run(
            &cfg,
            &store,
            Kind::Frequent,
            false,
            ts("2026-11-21T00:00:00Z")
        )
        .is_err()
    );
    assert!(!f.out("nope.sqlite3").exists());
    assert!(store.ops().is_empty());
}
