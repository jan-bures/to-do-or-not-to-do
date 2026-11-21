//! Attachment upload and download through the real server, with the local blob store.
#![allow(clippy::unwrap_used)]

mod common;

use common::{TOKEN_A, TestServer, id};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde_json::{Value, json};
use std::path::Path;
use todo_core::{AccessLevel, AgentId, Grants};
use todo_server::auth::store::ShareTarget;

const MAX: u64 = 100_000;

async fn server() -> TestServer {
    TestServer::start_with(|c| c.attachments.max_upload_bytes = MAX).await
}

struct Upload {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn upload_as(
    t: &TestServer,
    token: &str,
    task: &str,
    name: &str,
    media: Option<&str>,
    body: impl Into<reqwest::Body>,
    extra: &[(&str, &str)],
) -> Upload {
    try_upload_as(t, token, task, name, media, body, extra)
        .await
        .unwrap()
}

/// For a body the server refuses before reading all of it: it answers, but the connection may also
/// be reset under the client while it is still sending (Windows does that), which is `Err`.
async fn try_upload_as(
    t: &TestServer,
    token: &str,
    task: &str,
    name: &str,
    media: Option<&str>,
    body: impl Into<reqwest::Body>,
    extra: &[(&str, &str)],
) -> Result<Upload, reqwest::Error> {
    let mut q = vec![("file_name", name)];
    q.extend_from_slice(extra);
    let mut req = t
        .client
        .post(format!("{}/api/v1/tasks/{task}/attachments", t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .query(&q)
        .body(body);
    if let Some(m) = media {
        req = req.header(CONTENT_TYPE, m);
    }
    let res = req.send().await?;
    let status = res.status();
    let headers = res.headers().clone();
    let text = res.text().await?;
    Ok(Upload {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    })
}

async fn upload(
    t: &TestServer,
    task: &str,
    name: &str,
    media: Option<&str>,
    body: &[u8],
) -> Upload {
    upload_as(t, TOKEN_A, task, name, media, body.to_vec(), &[]).await
}

async fn download_as(
    t: &TestServer,
    token: &str,
    attachment: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let res = t
        .client
        .get(format!("{}/api/v1/attachments/{attachment}", t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let (s, h) = (res.status(), res.headers().clone());
    (s, h, res.bytes().await.unwrap().to_vec())
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn files_in(dir: &Path) -> Vec<Vec<u8>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| std::fs::read(e.path()).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

async fn agent(t: &TestServer, name: &str, list: &str, level: AccessLevel) -> String {
    let svc = &t.server.authsvc;
    let (row, token) = svc
        .create_agent(
            &AgentId::new(name).unwrap(),
            AccessLevel::None,
            None,
            None,
            None,
        )
        .unwrap();
    let mut g = Grants::default();
    g.lists.insert(list.parse().unwrap(), level);
    assert!(svc.set_grants(row, &g, None).unwrap());
    token
}

async fn inbox_task(t: &TestServer) -> (String, String) {
    let list = t.create_list("Inbox").await;
    let task = t.create_task(&list, "with files", json!({})).await;
    (list, task)
}

#[tokio::test]
async fn upload_and_download_round_trip_encrypted_at_rest() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let content = b"hello attachment world, this is the plaintext".to_vec();
    let up = upload(
        &t,
        &task,
        "hello.txt",
        Some("Text/Plain; charset=utf-8"),
        &content,
    )
    .await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    let a = &up.body["attachment"];
    assert_eq!(a["file_name"], "hello.txt");
    assert_eq!(a["media_type"], "text/plain; charset=utf-8");
    assert_eq!(a["size_bytes"], content.len());
    let key = a["storage_key"].as_str().unwrap();
    assert!(
        key.starts_with("a1/") && key.len() == 35 && !key.contains("hello"),
        "{key}"
    );
    let att = a["id"].as_str().unwrap();

    let detail = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(detail["attachments"][0]["id"], att);

    let (s, h, body) = download_as(&t, TOKEN_A, att).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, content);
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert_eq!(h["content-length"], content.len().to_string().as_str());
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"hello.txt\"; filename*=UTF-8''hello.txt"
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(
        h["content-security-policy"],
        "default-src 'none'; sandbox; frame-ancestors 'none'"
    );

    // At rest: one object, sealed; plaintext appears nowhere in the data directory's blob store.
    let objects = files_in(&t.dir.path().join("blobs"));
    assert_eq!(objects.len(), 1);
    assert!(objects[0].starts_with(b"TODOATT1"));
    assert!(!contains(&objects[0], b"plaintext"));
    assert!(files_in(&t.dir.path().join("attachments-spool")).is_empty());
    assert!(t.dir.path().join("attachments.key").is_file());

    // Survives a restart (same key file, same registry).
    let t = t.restart().await;
    let (s, _, body) = download_as(&t, TOKEN_A, att).await;
    assert_eq!((s, body), (StatusCode::OK, content));
}

#[tokio::test]
async fn html_is_only_ever_served_as_a_download() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let page = b"<html><body><script>alert(document.cookie)</script></body></html>";
    for (name, media) in [("page.html", "text/html"), ("pic.svg", "image/svg+xml")] {
        let up = upload(&t, &task, name, Some(media), page).await;
        assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
        let (s, h, body) =
            download_as(&t, TOKEN_A, up.body["attachment"]["id"].as_str().unwrap()).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body, page);
        assert_eq!(h["content-type"], media);
        assert!(
            h["content-disposition"]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        assert_eq!(h["x-content-type-options"], "nosniff");
        let csp = h["content-security-policy"].to_str().unwrap();
        assert!(
            csp.contains("sandbox") && csp.contains("default-src 'none'"),
            "{csp}"
        );
    }
}

#[tokio::test]
async fn file_names_are_encoded_safely() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let name = "naïve \"q\"; x.pdf";
    let up = upload(&t, &task, name, None, b"%PDF").await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    assert_eq!(
        up.body["attachment"]["media_type"],
        "application/octet-stream"
    );
    let (_, h, _) = download_as(&t, TOKEN_A, up.body["attachment"]["id"].as_str().unwrap()).await;
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"na_ve _q__ x.pdf\"; filename*=UTF-8''na%C3%AFve%20%22q%22%3B%20x.pdf"
    );
    for bad in ["../etc/passwd", "a\\b", "", "line\nbreak", ".."] {
        let up = upload(&t, &task, bad, None, b"x").await;
        assert_eq!(
            up.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad:?}: {}",
            up.body
        );
    }
    let up = upload(&t, &task, "x", Some("not a type"), b"x").await;
    assert_eq!(up.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        files_in(&t.dir.path().join("blobs")).len() == 1,
        "refused uploads store nothing"
    );
}

#[tokio::test]
async fn the_size_cap_is_enforced_while_streaming() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    // No Content-Length: the server only finds out while reading, and stops at the cap.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        (0..4).map(|_| Ok(vec![b'z'; 40_000])).collect();
    let body = reqwest::Body::wrap_stream(futures_util::stream::iter(chunks));
    if let Ok(up) = try_upload_as(&t, TOKEN_A, &task, "big.bin", None, body, &[]).await {
        assert_eq!(up.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", up.body);
        assert_eq!(up.body["error"]["code"], "payload_too_large");
    }
    // A declared length over the cap is refused before anything is read.
    let big = vec![0; MAX as usize + 1];
    if let Ok(up) = try_upload_as(&t, TOKEN_A, &task, "big.bin", None, big, &[]).await {
        assert_eq!(up.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", up.body);
        assert_eq!(up.body["error"]["code"], "payload_too_large");
    }
    // Nothing was kept anywhere.
    assert!(files_in(&t.dir.path().join("blobs")).is_empty());
    assert!(files_in(&t.dir.path().join("attachments-spool")).is_empty());
    assert!(t.server.engine.state().attachments.is_empty());
    // Exactly at the cap is fine, and larger than the global 1 MiB body limit is only allowed here.
    let up = upload(&t, &task, "max.bin", None, &vec![1; MAX as usize]).await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);

    let big = TestServer::start().await;
    let (_, task) = inbox_task(&big).await;
    let up = upload(&big, &task, "2mib.bin", None, &vec![2; 2 * 1024 * 1024]).await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    // Other routes keep the 1 MiB limit (the server may answer 413 or just close the connection).
    let res = big
        .client
        .post(format!("{}/api/v1/commands", big.url))
        .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
        .json(&json!({ "command_id": id(), "command": {
            "cmd": "add_note", "task": task, "text": "x".repeat(2 * 1024 * 1024) } }))
        .send()
        .await;
    if let Ok(res) = res {
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
    assert!(
        big.server
            .engine
            .state()
            .tasks
            .values()
            .all(|t| t.notes.is_empty())
    );
}

#[tokio::test]
async fn agents_need_capture_to_upload_and_read_to_download() {
    let t = server().await;
    let shared = t.create_list("Shared").await;
    let private = t.create_list("Private").await;
    let shared_task = t.create_task(&shared, "s", json!({})).await;
    let private_task = t.create_task(&private, "p", json!({})).await;
    let reader = agent(&t, "reader", &shared, AccessLevel::Read).await;
    let capturer = agent(&t, "capturer", &shared, AccessLevel::Capture).await;

    let up = upload_as(&t, &reader, &shared_task, "r.txt", None, b"r".to_vec(), &[]).await;
    assert_eq!(up.status, StatusCode::FORBIDDEN, "{}", up.body);
    let up = upload_as(
        &t,
        &capturer,
        &shared_task,
        "c.txt",
        None,
        b"c".to_vec(),
        &[],
    )
    .await;
    assert_eq!(up.status, StatusCode::CREATED, "{}", up.body);
    let from_agent = up.body["attachment"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        up.body["attachment"]["created"]["by"]["agent_id"],
        "capturer"
    );

    // A task the agent cannot see looks exactly like a missing one.
    let hidden = upload_as(
        &t,
        &capturer,
        &private_task,
        "c.txt",
        None,
        b"c".to_vec(),
        &[],
    )
    .await;
    let missing = upload_as(&t, &capturer, &id(), "c.txt", None, b"c".to_vec(), &[]).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.body["error"]["code"], missing.body["error"]["code"]);

    let owner_private = upload(&t, &private_task, "secret.txt", None, b"secret").await;
    let secret = owner_private.body["attachment"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, _, body) = download_as(&t, &reader, &from_agent).await;
    assert_eq!((s, body.as_slice()), (StatusCode::OK, b"c".as_slice()));
    let (s1, _, hidden) = download_as(&t, &reader, &secret).await;
    let (s2, _, missing) = download_as(&t, &reader, &id()).await;
    assert_eq!((s1, s2), (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND));
    let (mut hidden, mut missing) = (json_of(&hidden), json_of(&missing));
    hidden["error"]["message"] = Value::Null;
    missing["error"]["message"] = Value::Null;
    assert_eq!(hidden, missing);

    // Deleted attachments are not served, to anyone.
    t.ok(json!({ "cmd": "delete", "id": { "kind": "attachment", "id": secret } }))
        .await;
    let (s, _, _) = download_as(&t, TOKEN_A, &secret).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // No token: 401 on both routes.
    let res = t
        .client
        .post(format!(
            "{}/api/v1/tasks/{shared_task}/attachments?file_name=x",
            t.url
        ))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = t
        .client
        .get(format!("{}/api/v1/attachments/{from_agent}", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn retries_with_the_same_attachment_id_store_once() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let att = id();
    let extra = [("attachment_id", att.as_str())];
    let first = upload_as(&t, TOKEN_A, &task, "a.txt", None, b"once".to_vec(), &extra).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    let again = upload_as(&t, TOKEN_A, &task, "a.txt", None, b"once".to_vec(), &extra).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert_eq!(again.headers["idempotent-replayed"], "true");
    assert_eq!(again.body["attachment"], first.body["attachment"]);
    assert_eq!(files_in(&t.dir.path().join("blobs")).len(), 1);
    // Another task with the same ID is a conflict, not a replay.
    let other = t
        .create_task(&t.create_list("Other").await, "o", json!({}))
        .await;
    let clash = upload_as(&t, TOKEN_A, &other, "a.txt", None, b"x".to_vec(), &extra).await;
    assert_eq!(clash.status, StatusCode::CONFLICT, "{}", clash.body);
}

fn warning_tasks(t: &TestServer) -> Vec<todo_core::model::Task> {
    t.server
        .engine
        .state()
        .tasks
        .values()
        .filter(|x| x.title.starts_with("Attachment storage budget reached"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_budget_refuses_uploads_and_files_one_warning() {
    let t = TestServer::start_with(|c| {
        c.attachments.budget.class_a_monthly = 1;
        c.attachments.budget.class_b_monthly = 1;
    })
    .await;
    let (inbox, task) = inbox_task(&t).await;
    let up = upload(&t, &task, "one.txt", None, b"1").await;
    assert_eq!(up.status, StatusCode::CREATED);
    for _ in 0..3 {
        let up = upload(&t, &task, "two.txt", None, b"2").await;
        assert_eq!(up.status, StatusCode::TOO_MANY_REQUESTS, "{}", up.body);
        assert_eq!(up.body["error"]["code"], "r2_budget_exhausted");
        assert!(up.headers.contains_key("retry-after"));
    }
    let att = up.body["attachment"]["id"].as_str().unwrap();
    assert_eq!(download_as(&t, TOKEN_A, att).await.0, StatusCode::OK);
    let (s, _, body) = download_as(&t, TOKEN_A, att).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(json_of(&body)["error"]["code"], "r2_budget_exhausted");

    let w = warning_tasks(&t);
    assert_eq!(w.len(), 1, "one warning a month, however many refusals");
    assert_eq!(w[0].list.to_string(), inbox);
    assert_eq!(w[0].created.by, todo_core::Actor::System);
    assert_eq!(files_in(&t.dir.path().join("blobs")).len(), 1);

    let full = TestServer::start_with(|c| c.attachments.budget.max_storage_bytes = 5_000).await;
    let (_, task) = inbox_task(&full).await;
    let up = upload(&full, &task, "big.bin", None, &[0; 10_000]).await;
    assert_eq!(up.status, StatusCode::INSUFFICIENT_STORAGE, "{}", up.body);
    assert_eq!(up.body["error"]["code"], "storage_budget_exhausted");
    let body =
        reqwest::Body::wrap_stream(futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
            vec![0u8; 10_000],
        )]));
    let up = upload_as(&full, TOKEN_A, &task, "big.bin", None, body, &[]).await;
    assert_eq!(
        up.status,
        StatusCode::INSUFFICIENT_STORAGE,
        "undeclared size: {}",
        up.body
    );
    assert_eq!(warning_tasks(&full).len(), 1);
    assert!(files_in(&full.dir.path().join("blobs")).is_empty());
}

#[tokio::test]
async fn downloads_are_rate_limited_per_token() {
    let t = TestServer::start_with(|c| {
        c.attachments.downloads_per_minute = 1;
        c.attachments.download_burst = 1;
    })
    .await;
    let (_, task) = inbox_task(&t).await;
    let up = upload(&t, &task, "a", None, b"a").await;
    let att = up.body["attachment"]["id"].as_str().unwrap();
    assert_eq!(download_as(&t, TOKEN_A, att).await.0, StatusCode::OK);
    let (s, h, _) = download_as(&t, TOKEN_A, att).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key("retry-after"));
    // Another token has its own allowance.
    assert_eq!(
        download_as(&t, common::TOKEN_B, att).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn share_pages_do_not_expose_attachments() {
    let t = server().await;
    let (list, task) = inbox_task(&t).await;
    let up = upload(&t, &task, "quarterly-secret.pdf", None, b"%PDF").await;
    let att = up.body["attachment"]["id"].as_str().unwrap().to_owned();
    let share = t
        .server
        .authsvc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap();
    let res = t
        .client
        .get(format!("{}/s/{share}", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let page = res.text().await.unwrap();
    assert!(page.contains("with files"), "the task is shown");
    for leak in ["quarterly-secret", att.as_str(), "attachments", "a1/"] {
        assert!(!page.contains(leak), "{leak} on the share page");
    }
}

#[tokio::test]
async fn both_endpoints_are_in_the_openapi_document() {
    let t = server().await;
    let doc = t.get_ok("/api/v1/openapi.json").await;
    let paths = &doc["paths"];
    assert!(paths["/api/v1/tasks/{id}/attachments"]["post"].is_object());
    assert!(paths["/api/v1/attachments/{id}"]["get"].is_object());
    assert!(
        paths["/api/v1/tasks/{id}/attachments"]["post"]["requestBody"]["content"]
            ["application/octet-stream"]
            .is_object()
    );
}

/// Replaces `probe` everywhere in `v`, so responses about different IDs can be compared.
fn without(v: &Value, probe: &str) -> Value {
    serde_json::from_str(&v.to_string().replace(probe, "<probe>")).unwrap()
}

#[tokio::test]
async fn a_taken_attachment_id_answers_the_same_whoever_holds_it() {
    let t = server().await;
    let visible = t.create_list("Visible").await;
    let hidden = t.create_list("Hidden").await;
    let mine = t.create_task(&visible, "mine", json!({})).await;
    let theirs = t.create_task(&hidden, "theirs", json!({})).await;
    let capturer = agent(&t, "capturer", &visible, AccessLevel::Capture).await;
    let owned = |b: &Upload| b.body["attachment"]["id"].as_str().unwrap().to_owned();
    // An attachment the agent can see, one in a list it cannot see, and an ID held only by the undo
    // journal (its creation was undone).
    let seen = owned(&upload(&t, &mine, "seen.txt", None, b"s").await);
    let unseen = owned(&upload(&t, &theirs, "unseen.txt", None, b"u").await);
    let withdrawn = upload(&t, &theirs, "gone.txt", None, b"g").await;
    let (s, v) = t
        .undo(common::first_seq(&withdrawn.body["result"]), None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let withdrawn = owned(&withdrawn);

    let mut answers = Vec::new();
    for probe in [&seen, &unseen, &withdrawn] {
        let extra = [("attachment_id", probe.as_str())];
        let up = upload_as(&t, &capturer, &mine, "p.txt", None, b"p".to_vec(), &extra).await;
        answers.push((up.status, without(&up.body, probe)));
        let (s, _, v) = t
            .command_with_id(
                &capturer,
                &id(),
                json!({ "cmd": "add_attachment", "id": probe, "task": mine, "file_name": "p.txt",
                        "media_type": "text/plain", "size_bytes": 1, "storage_key": "k" }),
            )
            .await;
        answers.push((s, without(&v, probe)));
    }
    assert_eq!(answers[0].0, StatusCode::CONFLICT, "{}", answers[0].1);
    assert_eq!(answers[0].1["error"]["code"], "id_unavailable");
    for a in &answers {
        assert_eq!(a, &answers[0]);
    }
    // The owner still gets the precise answer.
    let extra = [("attachment_id", unseen.as_str())];
    let up = upload_as(&t, TOKEN_A, &mine, "p.txt", None, b"p".to_vec(), &extra).await;
    assert_eq!(up.body["error"]["code"], "already_exists", "{}", up.body);
}

#[tokio::test]
async fn concurrent_uploads_with_one_attachment_id_store_and_charge_once() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let att = id();
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|chunk| (Ok::<_, std::io::Error>(chunk), rx))
    }));
    let first = {
        let (client, url, task, att) = (t.client.clone(), t.url.clone(), task.clone(), att.clone());
        tokio::spawn(async move {
            let res = client
                .post(format!("{url}/api/v1/tasks/{task}/attachments"))
                .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
                .query(&[("file_name", "slow.bin"), ("attachment_id", att.as_str())])
                .body(body)
                .send()
                .await
                .unwrap();
            res.status()
        })
    };
    tx.send(vec![1; 1000]).await.unwrap();
    // Wait until the first upload is sealing (its spool file exists).
    let spool = t.dir.path().join("attachments-spool");
    // Only counted: an open file cannot be read on every platform.
    let sealing = || std::fs::read_dir(&spool).unwrap().count() > 0;
    for _ in 0..500 {
        if sealing() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(sealing(), "the first upload started");

    let extra = [("attachment_id", att.as_str())];
    let second = upload_as(&t, TOKEN_A, &task, "slow.bin", None, vec![1; 2000], &extra).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{}", second.body);
    assert_eq!(second.body["error"]["code"], "upload_in_progress");
    assert!(second.headers.contains_key("retry-after"));

    tx.send(vec![1; 1000]).await.unwrap();
    drop(tx);
    assert_eq!(first.await.unwrap(), StatusCode::CREATED);
    let third = upload_as(&t, TOKEN_A, &task, "slow.bin", None, vec![1; 2000], &extra).await;
    assert_eq!(third.status, StatusCode::OK, "{}", third.body);
    assert_eq!(third.headers["idempotent-replayed"], "true");

    let a = &t.server.attachments;
    let usage = a.registry().usage(&a.month()).unwrap();
    assert_eq!((usage.class_a, usage.objects), (1, 1), "exactly one PUT");
    assert_eq!(files_in(&t.dir.path().join("blobs")).len(), 1);
}

// ---- filling in imported content (`PUT /api/v1/attachments/{id}/content`) --------------------------

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Imports an attachment reference to `task` as the importer does: `import/sha256/<hex>`, no object.
async fn import_attachment(t: &TestServer, task: &str, size: u64, sha: &str) -> String {
    let att = id();
    let stamp = json!({ "at": "2025-01-02T03:04:05Z", "by": { "type": "importer" } });
    t.ok(
        json!({ "cmd": "import_entity", "entity": { "kind": "attachment", "data": {
        "id": att, "task": task, "file_name": "imported.txt", "media_type": "text/plain",
        "size_bytes": size, "storage_key": format!("import/sha256/{sha}"), "created": stamp,
        "deleted": null
    } } }),
    )
    .await;
    att
}

async fn fill_as(
    t: &TestServer,
    token: Option<&str>,
    path: &str,
    query: &[(&str, String)],
    body: impl Into<reqwest::Body>,
) -> Result<Upload, reqwest::Error> {
    let mut req = t
        .client
        .put(format!("{}{path}", t.url))
        .query(query)
        .body(body);
    if let Some(token) = token {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let res = req.send().await?;
    let status = res.status();
    let headers = res.headers().clone();
    let text = res.text().await?;
    Ok(Upload {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    })
}

fn content_path(att: &str) -> String {
    format!("/api/v1/attachments/{att}/content")
}

fn expect(size: usize, sha: &str) -> Vec<(&'static str, String)> {
    vec![("size_bytes", size.to_string()), ("sha256", sha.to_owned())]
}

/// PUT of `body` as the owner, expecting the server to answer.
async fn fill(t: &TestServer, att: &str, size: usize, sha: &str, body: &[u8]) -> Upload {
    fill_as(
        t,
        Some(TOKEN_A),
        &content_path(att),
        &expect(size, sha),
        body.to_vec(),
    )
    .await
    .unwrap()
}

async fn head_as(t: &TestServer, token: &str, att: &str) -> (StatusCode, HeaderMap) {
    let res = t
        .client
        .head(format!("{}/api/v1/attachments/{att}", t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    (res.status(), res.headers().clone())
}

/// Nothing stored, charged or left behind.
fn assert_nothing_stored(t: &TestServer, what: &str) {
    let a = &t.server.attachments;
    let usage = a.registry().usage(&a.month()).unwrap();
    assert_eq!((usage.class_a, usage.objects), (0, 0), "{what}: charged");
    assert!(files_in(&t.dir.path().join("blobs")).is_empty(), "{what}");
    assert!(
        files_in(&t.dir.path().join("attachments-spool")).is_empty(),
        "{what}: spool"
    );
}

#[tokio::test]
async fn imported_content_is_verified_stored_and_replayed() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let content = b"imported attachment content".to_vec();
    let sha = sha_hex(&content);
    let att = import_attachment(&t, &task, content.len() as u64, &sha).await;
    let (s, _, body) = download_as(&t, TOKEN_A, &att).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(json_of(&body)["error"]["code"], "content_unavailable");
    assert_eq!(head_as(&t, TOKEN_A, &att).await.0, StatusCode::NOT_FOUND);

    // A body that does not hash to `sha256`: 422, and nothing is stored or charged.
    let mut wrong = content.clone();
    wrong[0] ^= 1;
    let r = fill(&t, &att, content.len(), &sha, &wrong).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "content_mismatch");
    assert_nothing_stored(&t, "wrong hash");
    // Shorter or longer than `size_bytes` (sent without a Content-Length).
    for body in [&content[..5], &[&content[..], b"!"].concat()[..]] {
        let chunked =
            reqwest::Body::wrap_stream(futures_util::stream::iter(vec![Ok::<_, std::io::Error>(
                body.to_vec(),
            )]));
        let r = fill_as(
            &t,
            Some(TOKEN_A),
            &content_path(&att),
            &expect(content.len(), &sha),
            chunked,
        )
        .await
        .unwrap();
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
        assert_eq!(r.body["error"]["code"], "content_mismatch");
    }
    // A Content-Length other than `size_bytes`.
    let r = fill(&t, &att, content.len() + 1, &sha, &content).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    // Expectations that do not match the reference: another size, another hash, a bad hash.
    let r = fill(&t, &att, 3, &sha, b"abc").await;
    assert_eq!(r.body["error"]["code"], "content_mismatch", "{}", r.body);
    let r = fill(&t, &att, 3, &sha_hex(b"abc"), b"abc").await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "content_mismatch");
    let r = fill(&t, &att, content.len(), "nothex", &content).await;
    assert_eq!(r.body["error"]["code"], "invalid", "{}", r.body);
    assert_nothing_stored(&t, "mismatches");

    // The right content: stored, encrypted, and served.
    let r = fill(&t, &att, content.len(), &sha.to_uppercase(), &content).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["attachment"]["id"], att.as_str());
    assert_eq!(r.body["sha256"], sha.as_str());
    let (s, h, body) = download_as(&t, TOKEN_A, &att).await;
    assert_eq!((s, &body), (StatusCode::OK, &content));
    assert_eq!(h["content-type"], "text/plain");
    let (s, h) = head_as(&t, TOKEN_A, &att).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-length"], content.len().to_string().as_str());
    assert_eq!(h["x-content-type-options"], "nosniff");
    let objects = files_in(&t.dir.path().join("blobs"));
    assert_eq!(objects.len(), 1);
    assert!(objects[0].starts_with(b"TODOATT1"));
    assert!(!contains(&objects[0], b"imported attachment"));
    // The reference is unchanged: no command was written.
    let detail = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(
        detail["attachments"][0]["storage_key"],
        format!("import/sha256/{sha}").as_str()
    );

    // The same content again: replayed without reading the body.
    let r = fill(&t, &att, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.headers["idempotent-replayed"], "true");
    // Other content for an attachment that has some: 409.
    let r = fill(&t, &att, 3, &sha_hex(b"abc"), b"abc").await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "content_exists");

    let a = &t.server.attachments;
    let usage = a.registry().usage(&a.month()).unwrap();
    assert_eq!((usage.class_a, usage.objects), (1, 1), "exactly one PUT");
    // A HEAD fetches nothing: no GET was charged by the HEADs above.
    assert_eq!(usage.class_b, 1, "only the one download");

    // Survives a restart, and the collector keeps it (it is referenced).
    let t = t.restart().await;
    let report = t.server.attachments.collect_garbage().unwrap();
    assert_eq!((report.kept, report.deleted), (1, 0));
    let (s, _, body) = download_as(&t, TOKEN_A, &att).await;
    assert_eq!((s, body), (StatusCode::OK, content));
}

#[tokio::test]
async fn only_imported_live_attachments_can_be_filled_in() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let content = b"x".repeat(10);
    let sha = sha_hex(&content);
    // Missing.
    let r = fill(&t, &id(), content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "not_found");
    // Deleted.
    let att = import_attachment(&t, &task, content.len() as u64, &sha).await;
    t.ok(json!({ "cmd": "delete", "id": { "kind": "attachment", "id": att } }))
        .await;
    let r = fill(&t, &att, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
    // Uploaded (it has content already).
    let up = upload(&t, &task, "up.txt", None, &content).await;
    let uploaded = up.body["attachment"]["id"].as_str().unwrap();
    let r = fill(&t, uploaded, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "content_exists");
    // A reference that was not imported and has no content.
    let other = id();
    t.ok(
        json!({ "cmd": "add_attachment", "id": other, "task": task, "file_name": "o.txt",
        "media_type": "text/plain", "size_bytes": 10, "storage_key": "elsewhere/1" }),
    )
    .await;
    let r = fill(&t, &other, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "not_imported");
    let a = &t.server.attachments;
    assert_eq!(a.registry().usage(&a.month()).unwrap().objects, 1);
}

#[tokio::test]
async fn agents_share_links_and_anonymous_callers_find_no_content_route() {
    let t = server().await;
    let (list, task) = inbox_task(&t).await;
    let content = b"secret imported bytes".to_vec();
    let sha = sha_hex(&content);
    let att = import_attachment(&t, &task, content.len() as u64, &sha).await;
    let planner = agent(&t, "planner", &list, AccessLevel::Plan).await;
    let share = t
        .server
        .authsvc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap();
    let share_token = share.to_string();
    let good = expect(content.len(), &sha);
    let missing_route = |token: Option<&str>| {
        let (t, token) = (&t, token.map(str::to_owned));
        async move {
            fill_as(t, token.as_deref(), "/api/v1/nope", &[], b"x".to_vec())
                .await
                .unwrap()
        }
    };
    for (who, token) in [
        ("agent", Some(planner.as_str())),
        ("share link", Some(share_token.as_str())),
        ("anonymous", None),
    ] {
        let want = missing_route(token).await;
        assert!(
            [StatusCode::NOT_FOUND, StatusCode::UNAUTHORIZED].contains(&want.status),
            "{who}: {}",
            want.body
        );
        for (path, query) in [
            (content_path(&att), good.clone()),
            (content_path(&id()), good.clone()),
            (content_path("not-a-uuid"), good.clone()),
            (content_path(&att), vec![("bogus", "1".to_owned())]),
        ] {
            let r = fill_as(&t, token, &path, &query, content.clone())
                .await
                .unwrap();
            assert_eq!(
                (r.status, &r.body),
                (want.status, &want.body),
                "{who} {path} {query:?}"
            );
        }
    }
    assert_nothing_stored(&t, "refused callers");
    // The owner can.
    let r = fill(&t, &att, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    // An agent that may read the list sees the content like any download, HEAD included.
    assert_eq!(head_as(&t, &planner, &att).await.0, StatusCode::OK);
    let outsider = agent(&t, "outsider", &id(), AccessLevel::Plan).await;
    assert_eq!(head_as(&t, &outsider, &att).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn filling_in_content_is_capped_like_uploads() {
    let t = server().await;
    let (_, task) = inbox_task(&t).await;
    let max = usize::try_from(MAX).unwrap();
    // A reference larger than the cap: refused before the body is read.
    let big = import_attachment(&t, &task, MAX + 1, &sha_hex(b"big")).await;
    let r = fill_as(
        &t,
        Some(TOKEN_A),
        &content_path(&big),
        &expect(max + 1, &sha_hex(b"big")),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "payload_too_large");
    // An announced body over the cap.
    let content = vec![7u8; max];
    let sha = sha_hex(&content);
    let att = import_attachment(&t, &task, MAX, &sha).await;
    let r = fill_as(
        &t,
        Some(TOKEN_A),
        &content_path(&att),
        &expect(max, &sha),
        vec![7u8; max + 1],
    )
    .await;
    if let Ok(r) = r {
        assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    }
    // A chunked body that runs over the cap is cut off while it streams.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        vec![Ok(vec![7u8; max / 2]), Ok(vec![7u8; max / 2 + 100])];
    let r = fill_as(
        &t,
        Some(TOKEN_A),
        &content_path(&att),
        &expect(max, &sha),
        reqwest::Body::wrap_stream(futures_util::stream::iter(chunks)),
    )
    .await;
    if let Ok(r) = r {
        assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    }
    assert_nothing_stored(&t, "over the cap");
    // Exactly at the cap is fine.
    let r = fill(&t, &att, max, &sha, &content).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
}

#[tokio::test]
async fn the_budget_refuses_fills_like_uploads() {
    let t = TestServer::start_with(|c| c.attachments.budget.class_a_monthly = 1).await;
    let (inbox, task) = inbox_task(&t).await;
    let (one, two) = (b"one".to_vec(), b"two".to_vec());
    let a1 = import_attachment(&t, &task, 3, &sha_hex(&one)).await;
    let a2 = import_attachment(&t, &task, 3, &sha_hex(&two)).await;
    assert_eq!(
        fill(&t, &a1, 3, &sha_hex(&one), &one).await.status,
        StatusCode::CREATED
    );
    for _ in 0..2 {
        let r = fill(&t, &a2, 3, &sha_hex(&two), &two).await;
        assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
        assert_eq!(r.body["error"]["code"], "r2_budget_exhausted");
        assert!(r.headers.contains_key("retry-after"));
    }
    let w = warning_tasks(&t);
    assert_eq!(w.len(), 1, "one warning a month");
    assert_eq!(w[0].list.to_string(), inbox);
    assert_eq!(files_in(&t.dir.path().join("blobs")).len(), 1);

    let full = TestServer::start_with(|c| c.attachments.budget.max_storage_bytes = 5_000).await;
    let (_, task) = inbox_task(&full).await;
    let content = vec![1u8; 10_000];
    let att = import_attachment(&full, &task, 10_000, &sha_hex(&content)).await;
    let r = fill(&full, &att, 10_000, &sha_hex(&content), &content).await;
    assert_eq!(r.status, StatusCode::INSUFFICIENT_STORAGE, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "storage_budget_exhausted");
    assert_eq!(warning_tasks(&full).len(), 1);
    assert_nothing_stored(&full, "storage budget");
}

#[tokio::test]
async fn the_content_route_and_head_are_in_the_openapi_document() {
    let t = server().await;
    let doc = t.get_ok("/api/v1/openapi.json").await;
    let put = &doc["paths"]["/api/v1/attachments/{id}/content"]["put"];
    assert!(put["requestBody"]["content"]["application/octet-stream"].is_object());
    let params: Vec<&str> = put["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        params.contains(&"size_bytes") && params.contains(&"sha256"),
        "{params:?}"
    );
    assert!(put["responses"]["422"].is_object());
    assert!(doc["paths"]["/api/v1/attachments/{id}"]["head"].is_object());
}

#[tokio::test]
async fn head_shares_the_download_allowance_and_charges_nothing() {
    let t = TestServer::start_with(|c| {
        c.attachments.downloads_per_minute = 1;
        c.attachments.download_burst = 1;
    })
    .await;
    let (_, task) = inbox_task(&t).await;
    let up = upload(&t, &task, "a", None, b"a").await;
    let att = up.body["attachment"]["id"].as_str().unwrap();
    assert_eq!(download_as(&t, TOKEN_A, att).await.0, StatusCode::OK);
    // The allowance is used up: HEAD is limited like GET.
    let (s, h) = head_as(&t, TOKEN_A, att).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key("retry-after"));
    // Another token: HEAD answers, and still charges no GET.
    assert_eq!(head_as(&t, common::TOKEN_B, att).await.0, StatusCode::OK);
    let a = &t.server.attachments;
    assert_eq!(a.registry().usage(&a.month()).unwrap().class_b, 1);
}

#[tokio::test]
async fn a_fill_past_its_deadline_charges_and_sends_nothing() {
    let t = TestServer::start_with(|c| {
        c.attachments.max_upload_bytes = MAX;
        c.attachments.upload_timeout_ms = 300;
    })
    .await;
    let (_, task) = inbox_task(&t).await;
    let content = b"late content".to_vec();
    let sha = sha_hex(&content);
    let att = import_attachment(&t, &task, content.len() as u64, &sha).await;
    // The body is read and verified in time; the commit would be claimed only after the deadline.
    t.server
        .attachments
        .set_fill_delay(std::time::Duration::from_millis(600));
    let r = fill(&t, &att, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::REQUEST_TIMEOUT, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "timeout");
    // Let the abandoned work finish: it refuses to commit.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_nothing_stored(&t, "past the deadline");
    assert_eq!(head_as(&t, TOKEN_A, &att).await.0, StatusCode::NOT_FOUND);
    // A retry in time stores it.
    t.server
        .attachments
        .set_fill_delay(std::time::Duration::ZERO);
    let r = fill(&t, &att, content.len(), &sha, &content).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
}

#[tokio::test]
async fn an_announced_size_tells_non_owners_nothing_about_the_fill_route() {
    let t = server().await;
    let (list, task) = inbox_task(&t).await;
    let att = import_attachment(&t, &task, 10, &sha_hex(b"0123456789")).await;
    let planner = agent(&t, "planner", &list, AccessLevel::Plan).await;
    let share = t
        .server
        .authsvc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap()
        .to_string();
    let addr = t.server.addr;
    let global = t.config.limits.max_body_bytes;
    let path = format!(
        "{}?size_bytes=10&sha256={}",
        content_path(&att),
        sha_hex(b"0123456789")
    );
    let (agent_auth, share_auth) = (format!("Bearer {planner}"), format!("Bearer {share}"));
    // Below the attachment cap, between it and the global limit, and over both.
    let sizes = [
        MAX as usize - 1,
        MAX as usize + 1,
        global + 1,
        30 * 1024 * 1024,
    ];
    for (who, extra) in [
        ("agent", vec![("Authorization", agent_auth.as_str())]),
        ("share token", vec![("Authorization", share_auth.as_str())]),
        ("anonymous", vec![]),
    ] {
        for len in sizes {
            let want = common::raw::oversized(addr, "PUT", "/api/v1/nope", len, &extra).await;
            let got = common::raw::oversized(addr, "PUT", &path, len, &extra).await;
            assert_eq!(
                (got.status, got.body.trim()),
                (want.status, want.body.trim()),
                "{who}, Content-Length {len}"
            );
        }
    }
    // The owner gets the attachment cap.
    let owner = format!("Bearer {TOKEN_A}");
    for len in [MAX as usize + 1, global + 1] {
        let r = common::raw::oversized(addr, "PUT", &path, len, &[("Authorization", &owner)]).await;
        assert_eq!(r.status, 413, "{}", r.body);
        assert!(
            r.body.contains(&format!("limited to {MAX} bytes")),
            "{}",
            r.body
        );
    }
    assert_nothing_stored(&t, "announced sizes");
}
