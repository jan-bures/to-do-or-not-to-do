//! The real S3 client and the CLI binary against a local fake S3 server on 127.0.0.1. No network.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::fake_s3::{FakeS3, SECRET_ACCESS_KEY};
use common::{Fixture, counters, ts};
use todo_backup::config::Credentials;
use todo_backup::ops::{self, RunOutcome, Target};
use todo_backup::store::{ObjectStore, S3Store, StoreError};
use todo_backup::{Error, Kind};

fn s3(fake: &FakeS3) -> S3Store {
    let creds: Credentials = serde_json::from_str(&fake.credentials_json()).unwrap();
    S3Store::new(&creds).unwrap()
}

#[test]
fn s3_store_signs_requests_and_round_trips() {
    let fake = FakeS3::start();
    let store = s3(&fake);
    store.put("frequent/a", b"hello").unwrap();
    let mut out = Vec::new();
    assert_eq!(store.get("frequent/a", &mut out).unwrap(), 5);
    assert_eq!(out, b"hello");
    assert!(matches!(
        store.get("frequent/missing", &mut Vec::new()),
        Err(StoreError::NotFound { .. })
    ));
    let page = store.list_page("frequent/", None).unwrap();
    assert_eq!(page.keys, ["frequent/a"]);
    assert_eq!(page.next, None);

    // Wrong credentials are refused by the fake's signature check, and the error names the S3 code but
    // carries no secret.
    let mut creds: Credentials = serde_json::from_str(&fake.credentials_json()).unwrap();
    creds.access_key_id = "someone-else".into();
    let err = S3Store::new(&creds)
        .unwrap()
        .put("frequent/b", b"x")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("403") && msg.contains("SignatureDoesNotMatch"),
        "{msg}"
    );
    assert!(!msg.contains(SECRET_ACCESS_KEY));
}

#[test]
fn s3_store_paginates_listings() {
    let fake = FakeS3::start();
    fake.set_page_size(2);
    for k in ["daily/1", "daily/2", "daily/3", "frequent/x"] {
        fake.insert(k, vec![1]);
    }
    let store = s3(&fake);
    let p1 = store.list_page("daily/", None).unwrap();
    assert_eq!(p1.keys, ["daily/1", "daily/2"]);
    let p2 = store.list_page("daily/", p1.next.as_deref()).unwrap();
    assert_eq!(p2.keys, ["daily/3"]);
    assert_eq!(p2.next, None);
}

#[test]
fn run_restore_and_drill_over_http_including_multipart() {
    let fake = FakeS3::start();
    let f = Fixture::new();
    common::fill_incompressible(&f, 48);
    std::fs::write(f.dir.path().join("r2.json"), fake.credentials_json()).unwrap();
    let cfg = f.config_with(
        "[upload]\nmultipart_threshold_bytes = 65536\npart_size_bytes = 65536\nzstd_level = 1\n",
    );
    let store = S3Store::new(&cfg.credentials().unwrap()).unwrap();

    let RunOutcome::Uploaded(rec) =
        ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap()
    else {
        panic!()
    };
    let log = fake.log();
    assert_eq!(
        log.first().map(String::as_str),
        Some(format!("POST CREATE {}", rec.key).as_str())
    );
    assert_eq!(
        log.last().map(String::as_str),
        Some(format!("POST COMPLETE {}", rec.key).as_str())
    );
    assert!(log.iter().filter(|l| l.starts_with("PUT PART")).count() >= 3);
    assert_eq!(fake.open_uploads(), 0);

    let out = f.out("restored.sqlite3");
    ops::restore(
        &cfg,
        &store,
        &Target::Latest(Kind::Daily),
        std::slice::from_ref(&f.identity),
        &out,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert_eq!(counters(&out), (48, Some(48), 48));

    let report = ops::drill(
        &cfg,
        &store,
        std::slice::from_ref(&f.identity),
        None,
        ts("2026-11-21T01:00:00Z"),
    )
    .unwrap();
    assert!(report.passed(), "{report}");
}

#[test]
fn failed_multipart_is_aborted() {
    let fake = FakeS3::start();
    let f = Fixture::new();
    common::fill_incompressible(&f, 32);
    // A store whose part uploads always fail, as if the connection dropped mid-upload.
    struct FailingParts(S3Store);
    impl ObjectStore for FailingParts {
        fn put(&self, k: &str, b: &[u8]) -> Result<(), StoreError> {
            self.0.put(k, b)
        }
        fn create_multipart(&self, k: &str) -> Result<String, StoreError> {
            self.0.create_multipart(k)
        }
        fn upload_part(&self, k: &str, _: &str, _: u16, _: &[u8]) -> Result<String, StoreError> {
            Err(StoreError::Transport {
                op: "UploadPart",
                key: k.into(),
                message: "connection reset".into(),
            })
        }
        fn complete_multipart(&self, k: &str, u: &str, e: &[String]) -> Result<(), StoreError> {
            self.0.complete_multipart(k, u, e)
        }
        fn abort_multipart(&self, k: &str, u: &str) -> Result<(), StoreError> {
            self.0.abort_multipart(k, u)
        }
        fn get(&self, k: &str, o: &mut dyn std::io::Write) -> Result<u64, StoreError> {
            self.0.get(k, o)
        }
        fn list_page(
            &self,
            p: &str,
            c: Option<&str>,
        ) -> Result<todo_backup::store::ListPage, StoreError> {
            self.0.list_page(p, c)
        }
    }
    let cfg = f.config_with(
        "[upload]\nmultipart_threshold_bytes = 65536\npart_size_bytes = 65536\nzstd_level = 1\n",
    );
    let store = FailingParts(s3(&fake));
    let err = ops::run(&cfg, &store, Kind::Daily, false, ts("2026-11-21T00:00:00Z")).unwrap_err();
    assert!(
        matches!(err, Error::Store(StoreError::Transport { .. })),
        "{err}"
    );
    let log = fake.log();
    assert!(log[0].starts_with("POST CREATE"));
    assert!(log[1].starts_with("DELETE ABORT"), "{log:?}");
    assert_eq!(fake.open_uploads(), 0);
    // Nothing was recorded, so the next run tries again.
    assert!(
        todo_backup::state::State::load(&cfg.state_dir)
            .unwrap()
            .daily
            .is_none()
    );
}

// ---------------------------------------------------------------------------------------------------------
// The binary

fn cli(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_todo-backup"))
        .args(args)
        .env("TODO_BACKUP_CONFIG", config)
        .env("RUST_LOG", "trace")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn cli_end_to_end() {
    let fake = FakeS3::start();
    let f = Fixture::new();
    f.append(9);
    let dir = f.dir.path();
    std::fs::write(dir.join("r2.json"), fake.credentials_json()).unwrap();
    let identity = dir.join("identity.txt");
    let config = dir.join("backup.toml");

    // keygen prints only the recipient on stdout and never overwrites.
    let out = cli(&config, &["keygen", "--out", identity.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out));
    let recipient = String::from_utf8(out.stdout.clone())
        .unwrap()
        .trim()
        .to_owned();
    assert!(recipient.starts_with("age1"), "{recipient}");
    assert!(!text(&out).contains("AGE-SECRET-KEY"));
    assert!(text(&out).contains("password manager"));
    let again = cli(&config, &["keygen", "--out", identity.to_str().unwrap()]);
    assert!(!again.status.success());

    let cfg_text = f
        .config_text("")
        .replace(&f.identity.to_public().to_string(), &recipient);
    std::fs::write(&config, &cfg_text).unwrap();

    let all = |o: &Output| {
        let t = text(o);
        // Nothing secret ever reaches the output, even at debug level.
        assert!(!t.contains(SECRET_ACCESS_KEY), "{t}");
        assert!(!t.contains("AGE-SECRET-KEY"), "{t}");
        assert!(!t.contains("X-Amz-Signature"), "{t}");
        assert!(!t.contains("task 3"), "database contents leaked: {t}");
        t
    };

    let run = cli(&config, &["run", "--kind", "frequent"]);
    let t = all(&run);
    assert!(run.status.success(), "{t}");
    assert!(t.contains("uploaded frequent/"), "{t}");
    assert_eq!(fake.keys().len(), 1);

    let run = cli(&config, &["run", "--kind", "frequent"]);
    let t = all(&run);
    assert!(run.status.success() && t.contains("unchanged"), "{t}");
    assert_eq!(fake.log().len(), 1);

    let status = cli(&config, &["status"]);
    let t = all(&status);
    assert!(
        t.contains("Class A 1/20000") && t.contains("Class B 0/200000"),
        "{t}"
    );

    let drill = cli(
        &config,
        &["drill", "--identity", identity.to_str().unwrap()],
    );
    let t = all(&drill);
    assert!(drill.status.success(), "{t}");
    assert!(t.contains("PASSED") && t.contains("9 rows"), "{t}");

    let restored = dir.join("restored.sqlite3");
    let args = [
        "restore",
        "--latest",
        "--kind",
        "frequent",
        "--identity",
        identity.to_str().unwrap(),
        "--out",
        restored.to_str().unwrap(),
    ];
    let r = cli(&config, &args);
    let t = all(&r);
    assert!(r.status.success(), "{t}");
    assert_eq!(counters(&restored), (9, Some(9), 9));
    assert!(
        fake.log()
            .iter()
            .any(|l| l.starts_with("GET LIST frequent/"))
    );

    let r = cli(&config, &args);
    let t = all(&r);
    assert!(!r.status.success());
    assert!(t.contains("refusing to overwrite"), "{t}");

    // A corrupted object fails the drill with a non-zero exit.
    let key = fake.keys().pop().unwrap();
    let mut bytes = fake.object(&key).unwrap();
    let n = bytes.len();
    bytes[n - 20] ^= 0xff;
    fake.insert(&key, bytes);
    let drill = cli(
        &config,
        &["drill", "--identity", identity.to_str().unwrap()],
    );
    let t = all(&drill);
    assert_eq!(drill.status.code(), Some(1), "{t}");
    assert!(t.contains("restore drill FAILED"), "{t}");

    // Over budget: exit code 3 and a clear message, with nothing sent.
    std::fs::write(
        &config,
        format!("{cfg_text}[budget]\nclass_a_monthly = 2\n"),
    )
    .unwrap();
    f.append(1);
    let sent = fake.log().len();
    let run = cli(&config, &["run", "--kind", "daily"]);
    let t = all(&run);
    assert_eq!(run.status.code(), Some(3), "{t}");
    assert!(t.contains("monthly budget for Class A"), "{t}");
    assert_eq!(fake.log().len(), sent);

    // No config at all is a usage error, not a panic.
    let out = Command::new(env!("CARGO_BIN_EXE_todo-backup"))
        .args(["status"])
        .env_remove("TODO_BACKUP_CONFIG")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(text(&out).contains("TODO_BACKUP_CONFIG"));
}
