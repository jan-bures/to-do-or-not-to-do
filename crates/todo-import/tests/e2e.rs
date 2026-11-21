#![allow(clippy::unwrap_used)]
//! End to end: the real server in-process on a temporary data directory; apply a fixture bundle,
//! verify it over REST, re-apply without duplicates, and resume after a simulated crash; then the same
//! for attachment content (`apply --attachments`).

mod common;

use std::path::Path;
use tempfile::TempDir;
use todo_core::DeviceId;
use todo_import::apply::{
    ApplyError, ApplyOptions, ApplySummary, ContentOptions, ContentSummary, apply,
    apply_attachments, read_journal,
};
use todo_import::bundle::{Blob, Bundle, Counts};
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
    // The static owner tokens are the server's development fallback, off by default.
    config.auth.allow_config_owner_tokens = true;
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

async fn run_content(
    url: &str,
    bundle: &Bundle,
    journal: &Path,
    abort_after: Option<usize>,
) -> Result<ContentSummary, ApplyError> {
    let (url, bundle) = (url.to_owned(), bundle.clone());
    let o = ContentOptions {
        journal: journal.to_owned(),
        abort_after,
    };
    blocking(move || apply_attachments(&bundle, &client(&url), &o)).await
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
    // Every entity landed; the one attachment's content is still only in the bundle.
    let rep = run_verify(&url, &bundle).await;
    assert_eq!(rep.matched, total);
    assert_eq!((rep.contents, rep.contents_stored), (1, 0));
    assert_eq!(rep.problems.len(), 1, "{}", rep.render());
    assert!(
        rep.problems[0].contains("content missing"),
        "{}",
        rep.render()
    );
    let c = run_content(&url, &bundle, &tmp.path().join("j1"), None)
        .await
        .unwrap();
    assert_eq!((c.total, c.uploaded, c.present, c.skipped), (1, 1, 0, 0));
    assert!(c.ok(), "{:?}", c.failed);
    let rep = run_verify(&url, &bundle).await;
    assert!(rep.ok(), "{}", rep.render());
    assert_eq!((rep.contents, rep.contents_stored), (1, 1));

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
    run_content(&url, &bundle, &journal, None).await.unwrap();
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

/// The fixture bundle with `sizes.len()` more attachments on the task that holds the fixture's one.
fn with_attachments(sizes: &[usize]) -> Bundle {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use todo_core::model::Entity;
    let mut b = common::bundle();
    let at = b
        .items
        .iter()
        .position(|i| matches!(i.entity(), Some(Entity::Attachment(_))))
        .unwrap();
    for (n, size) in sizes.iter().enumerate() {
        let content: Vec<u8> = (0..*size).map(|i| (i * 7 + n) as u8).collect();
        let sha = hex::encode(Sha256::digest(&content));
        let mut item = b.items[at].clone();
        item.command_id = uuid::Uuid::from_u128(0xC0DE_0000 + n as u128);
        item.source.id = format!("A-extra-{n}");
        let todo_core::Command::ImportEntity { entity } = &mut item.command else {
            unreachable!()
        };
        let Entity::Attachment(a) = entity.as_mut() else {
            unreachable!()
        };
        a.id = todo_core::ids::AttachmentId::from_uuid(uuid::Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0000 + n as u128,
        ));
        a.file_name = format!("extra-{n}.bin");
        a.size_bytes = content.len() as u64;
        a.storage_key = format!("import/sha256/{sha}");
        b.blobs.push(Blob {
            attachment: a.id,
            storage_key: a.storage_key.clone(),
            sha256: sha,
            size_bytes: content.len() as u64,
            content_base64: base64::engine::general_purpose::STANDARD.encode(&content),
        });
        b.items.insert(at + 1 + n, item);
    }
    b.counts = Counts::of(&b.items, &b.blobs);
    b
}

fn attachment_command(cmd: &str, id: todo_core::ids::AttachmentId, n: u128) -> serde_json::Value {
    serde_json::json!({
        "command_id": uuid::Uuid::from_u128(0xFEED_0000 + n),
        "command": { "cmd": cmd, "id": { "kind": "attachment", "id": id } },
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_content_is_uploaded_resumed_and_failures_reported() {
    // Over the global 1 MiB body limit too: the content route has the upload cap.
    let bundle = with_attachments(&[10, 2 * 1024 * 1024 + 3, 5_000]);
    assert_eq!(bundle.blobs.len(), 4);
    let tmp = tempfile::tempdir().unwrap();
    let s = start().await;
    let url = s.server.url();
    let journal = tmp.path().join("j");
    run_apply(&url, &bundle, opts(&journal, 50, None))
        .await
        .unwrap();
    let rep = run_verify(&url, &bundle).await;
    assert_eq!((rep.contents, rep.contents_stored), (4, 0));
    assert_eq!(rep.problems.len(), 4, "{}", rep.render());

    // A crash after two contents: both are journaled (each is synced at once).
    let err = run_content(&url, &bundle, &journal, Some(2))
        .await
        .unwrap_err();
    assert!(matches!(err, ApplyError::Aborted(2)), "{err}");
    let base = todo_import::http::check_base_url(&url).unwrap();
    let j = read_journal(&journal, &bundle.digest().unwrap(), &base).unwrap();
    assert_eq!(j.contents.len(), 2);
    assert_eq!(
        j.done.len(),
        bundle.items.len(),
        "content lines are not commands"
    );
    // Resume: the journal skips two, the rest is sent.
    let c = run_content(&url, &bundle, &journal, None).await.unwrap();
    assert_eq!((c.skipped, c.uploaded, c.present), (2, 2, 0), "{c:?}");
    assert!(c.ok(), "{:?}", c.failed);
    let rep = run_verify(&url, &bundle).await;
    assert!(rep.ok(), "{}", rep.render());
    assert_eq!(rep.contents_stored, 4);
    // Without the journal, nothing is sent again: the server already holds every content.
    let c = run_content(&url, &bundle, &tmp.path().join("fresh"), None)
        .await
        .unwrap();
    assert_eq!((c.uploaded, c.present), (0, 4), "{c:?}");
    // The re-run sent no content: still four objects, four PUTs.
    let a = &s.server.attachments;
    let usage = a.registry().usage(&a.month()).unwrap();
    assert_eq!((usage.objects, usage.class_a), (4, 4));
    s.server.shutdown().await.unwrap();

    // Failures don't stop the run: one attachment deleted on the server, one corrupted in the bundle.
    let mut broken = bundle.clone();
    broken.blobs[3].content_base64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, vec![0u8; 5_000]);
    let s = start().await;
    let url = s.server.url();
    let journal = tmp.path().join("broken");
    run_apply(&url, &broken, opts(&journal, 50, None))
        .await
        .unwrap();
    let deleted = broken.blobs[1].attachment;
    let u = url.clone();
    blocking(move || {
        client(&u)
            .post(
                "delete",
                "/api/v1/commands",
                &attachment_command("delete", deleted, 1),
            )
            .unwrap()
    })
    .await;
    let c = run_content(&url, &broken, &journal, None).await.unwrap();
    assert_eq!((c.uploaded, c.failed.len()), (2, 2), "{c:?}");
    assert!(
        c.failed.iter().any(|f| f.contains("does not match")),
        "{c:?}"
    );
    assert!(c.failed.iter().any(|f| f.contains("404")), "{c:?}");
    let rep = run_verify(&url, &broken).await;
    assert!(!rep.ok());
    // Restored on the server, the deleted one goes through on the next run; only failures are retried.
    let u = url.clone();
    blocking(move || {
        client(&u)
            .post(
                "restore",
                "/api/v1/commands",
                &attachment_command("restore", deleted, 2),
            )
            .unwrap()
    })
    .await;
    let c = run_content(&url, &broken, &journal, None).await.unwrap();
    assert_eq!((c.skipped, c.uploaded, c.failed.len()), (2, 1, 1), "{c:?}");
    let rep = run_verify(&url, &broken).await;
    assert_eq!(rep.problems.len(), 1, "{}", rep.render());
    assert!(
        rep.problems[0].contains("content missing"),
        "{}",
        rep.render()
    );
    s.server.shutdown().await.unwrap();
}
