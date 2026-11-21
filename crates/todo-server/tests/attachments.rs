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
    let res = req.send().await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let text = res.text().await.unwrap();
    Upload {
        status,
        headers,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    }
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
    let up = upload_as(&t, TOKEN_A, &task, "big.bin", None, body, &[]).await;
    assert_eq!(up.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", up.body);
    assert_eq!(up.body["error"]["code"], "payload_too_large");
    // A declared length over the cap is refused before anything is read.
    let up = upload(&t, &task, "big.bin", None, &vec![0; MAX as usize + 1]).await;
    assert_eq!(up.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", up.body);
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
