#![allow(clippy::unwrap_used)]
//! End to end: the real server in-process on a temporary data directory; apply a fixture bundle,
//! verify it over REST, re-apply without duplicates, and resume after a simulated crash.

mod common;

use std::path::Path;
use tempfile::TempDir;
use todo_core::DeviceId;
use todo_import::apply::{ApplyError, ApplyOptions, ApplySummary, apply, read_journal};
use todo_import::bundle::Bundle;
use todo_import::http::{Client, HttpError};
use todo_import::verify_server::{ServerReport, verify_server};
use todo_server::auth::hash_token;
use todo_server::config::OwnerToken;
use todo_server::{Config, Options, RunningServer, Server};

const TOKEN: &str = "import-test-token-0123456789";

struct Running {
    server: RunningServer,
    _dir: TempDir,
}

async fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().to_owned(),
        ..Config::default()
    };
    config.auth.owner_tokens = vec![OwnerToken {
        device: DeviceId::new("importer").unwrap(),
        sha256: hash_token(TOKEN),
    }];
    let server = Server::bind(&config, Options::default())
        .await
        .unwrap()
        .spawn()
        .unwrap();
    Running { server, _dir: dir }
}

fn client(url: &str) -> Client {
    Client::new(url, TOKEN.to_owned()).unwrap()
}

/// Runs blocking importer work off the async runtime that drives the server.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn opts(journal: &Path, batch: usize, abort_after: Option<usize>) -> ApplyOptions {
    ApplyOptions {
        batch_size: batch,
        journal: journal.to_owned(),
        abort_after,
    }
}

async fn run_apply(
    url: &str,
    bundle: &Bundle,
    o: ApplyOptions,
) -> Result<ApplySummary, ApplyError> {
    let (url, bundle) = (url.to_owned(), bundle.clone());
    blocking(move || apply(&bundle, &client(&url), &o)).await
}

async fn run_verify(url: &str, bundle: &Bundle) -> ServerReport {
    let (url, bundle) = (url.to_owned(), bundle.clone());
    blocking(move || verify_server(&bundle, &client(&url)))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_verify_reapply_and_resume() {
    let bundle = common::bundle();
    let total = bundle.items.len();
    let tmp = tempfile::tempdir().unwrap();

    // 1. A clean import, then a read-back.
    let s = start().await;
    let url = s.server.url();
    let sum = run_apply(&url, &bundle, opts(&tmp.path().join("j1"), 4, None))
        .await
        .unwrap();
    assert_eq!(
        (sum.total, sum.stored, sum.replayed, sum.skipped),
        (total, total, 0, 0)
    );
    assert_eq!(sum.batches, total.div_ceil(4));
    let rep = run_verify(&url, &bundle).await;
    assert!(rep.ok(), "{}", rep.render());
    assert_eq!(rep.matched, total);

    // 2. Re-applying with the same journal sends nothing.
    let sum = run_apply(&url, &bundle, opts(&tmp.path().join("j1"), 4, None))
        .await
        .unwrap();
    assert_eq!((sum.stored, sum.replayed, sum.skipped), (0, 0, total));

    // 3. Re-applying without a journal re-sends everything; the server replays it and stores nothing.
    let sum = run_apply(&url, &bundle, opts(&tmp.path().join("j2"), 50, None))
        .await
        .unwrap();
    assert_eq!((sum.stored, sum.replayed), (0, total));
    let rep = run_verify(&url, &bundle).await;
    assert!(rep.ok(), "duplicates after a re-run: {}", rep.render());

    // 4. A bundle whose content changed under the same command IDs is refused, not duplicated.
    let mut changed = bundle.clone();
    let i = changed
        .items
        .iter()
        .position(|i| i.source.id == "G-1")
        .unwrap();
    if let todo_core::Command::ImportEntity { entity } = &mut changed.items[i].command
        && let todo_core::model::Entity::Task(t) = entity.as_mut()
    {
        t.title = "Changed".into();
    }
    let err = run_apply(&url, &changed, opts(&tmp.path().join("j3"), 50, None))
        .await
        .unwrap_err();
    match err {
        ApplyError::Http { index, source, .. } => {
            assert_eq!(index, i);
            assert!(matches!(
                *source,
                HttpError::Status { status: 409, ref code, .. } if code == "command_id_reused"
            ));
        }
        other => panic!("unexpected {other}"),
    }
    assert!(run_verify(&url, &bundle).await.ok());
    s.server.shutdown().await.unwrap();

    // 5. A crash mid-way: 7 commands sent, only the first full batch of 5 synced to the journal.
    let s = start().await;
    let url = s.server.url();
    let journal = tmp.path().join("crash");
    let err = run_apply(&url, &bundle, opts(&journal, 5, Some(7)))
        .await
        .unwrap_err();
    assert!(matches!(err, ApplyError::Aborted(7)));
    let digest = bundle.digest().unwrap();
    let base = todo_import::http::check_base_url(&url).unwrap();
    assert_eq!(
        read_journal(&journal, &digest, &base).unwrap().done.len(),
        5
    );
    let partial = run_verify(&url, &bundle).await;
    assert!(!partial.ok(), "a partial import must not verify");
    // Resume: the journal skips 5, the server replays the 2 unsynced ones, the rest is stored.
    let sum = run_apply(&url, &bundle, opts(&journal, 5, None))
        .await
        .unwrap();
    assert_eq!((sum.skipped, sum.replayed, sum.stored), (5, 2, total - 7));
    let rep = run_verify(&url, &bundle).await;
    assert!(rep.ok(), "{}", rep.render());
    // A journal belongs to one server.
    assert!(read_journal(&journal, &digest, "http://127.0.0.1:1").is_err());
    s.server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_is_rejected_without_leaking_it() {
    let s = start().await;
    let url = s.server.url();
    let bundle = common::bundle();
    let tmp = tempfile::tempdir().unwrap();
    let journal = tmp.path().join("j");
    let err = blocking(move || {
        let c = Client::new(&url, "wrong-token-value".to_owned()).unwrap();
        apply(&bundle, &c, &opts(&journal, 10, None))
    })
    .await
    .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("401"), "{text}");
    assert!(!text.contains("wrong-token-value"));
    s.server.shutdown().await.unwrap();
}
