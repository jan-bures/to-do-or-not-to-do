//! Email ingest end to end (`POST /api/v1/ingest/email`, see `docs/ingest-email.md`): the shared
//! signing vector, the order of the request checks, nonce replay and Message-ID dedupe (also under
//! concurrency), the sender allowlist, list routing, `#keep`, the activity feed and undo, the limits,
//! and the route being off without a secret. The admin page that edits the allowlist and the list keys
//! is tested with the other step-up pages in `tests/auth.rs`; the signature, replay and timestamp
//! guarantees are also in the security suite (`tests/security.rs`). Helpers are in `common::ingest`.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::ingest::{
    BY, OWNER, PATH, SECRET, add_key, enabled, ledger, live_tasks, make_inbox, nonce, now_secs,
    payload, post_raw, post_signed, secret_file, send, server, set_inbox, sign, task_id,
};
use common::{TOKEN_A, TestServer};
use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde_json::{Value, json};
use sha2::Sha256;
use std::io::Write as _;
use std::sync::Arc;
use todo_core::time::Timestamp;
use todo_server::{Config, Options, Server};

#[tokio::test]
async fn the_shared_vector_is_accepted_as_sent() {
    let v: Value =
        serde_json::from_str(include_str!("../../../docs/ingest-email-signature.json")).unwrap();
    let at: i64 = v["timestamp"].as_str().unwrap().parse().unwrap();
    // The server's clock reads the vector's time, so its exact request is fresh.
    let options = Options {
        clock: Arc::new(move || Timestamp::from_second(at).unwrap()),
        ..Options::default()
    };
    let t = TestServer::start_with_options(
        |cfg| {
            secret_file(cfg, v["secret"].as_str().unwrap());
            cfg.ingest.allowed_senders = vec![OWNER.into()];
        },
        options,
    )
    .await;
    let inbox = make_inbox(&t).await;
    let shopping = t.create_list("Shopping").await;
    add_key(&t, "shopping", &shopping);
    let headers = [
        ("x-todo-timestamp", v["timestamp"].as_str().unwrap()),
        ("x-todo-nonce", v["nonce"].as_str().unwrap()),
        ("x-todo-signature", v["signature"].as_str().unwrap()),
    ];
    let body = v["body"].as_str().unwrap().as_bytes().to_vec();
    let (s, r) = post_raw(&t, &headers, body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(r["status"], "created");
    // `todo+shopping@` went to the list with that key, not the Inbox.
    let d = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await;
    assert_eq!(d["task"]["list"], json!(shopping));
    assert_ne!(d["task"]["list"], json!(inbox));
    assert_eq!(d["task"]["title"], "Nákup – mléko #2");
    let notes = d["task"]["notes"].as_str().unwrap();
    assert!(
        notes.starts_with(
            "From: Alice Example <alice@example.org>\nForwarded by you@example.com on 2025-11-21 08:00 (UTC)\nSubject: Fwd: Nákup – mléko #2 #keep\n"
        ),
        "{notes}"
    );
    // `#keep`: the body follows.
    assert!(notes.ends_with("\n\nLine one\nLine \"two\" ✓"), "{notes}");
    assert_eq!(
        d["task"]["created"]["by"],
        json!({ "type": "email_ingest" })
    );
    let link = &d["links"][0]["kind"];
    assert_eq!(link["type"], "email");
    assert_eq!(link["message_id"], "<CAexample-1234@mail.example.org>");
    assert_eq!(link["subject"], "Fwd: Nákup – mléko #2 #keep");
    assert_eq!(link["sender"], "alice@example.org");
    assert_eq!(link["received_at"], "2025-11-21T08:00:00Z");
    // The same request again is a replay.
    let (s, _) = post_raw(&t, &headers, body).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(live_tasks(&t).len(), 1);
}

#[tokio::test]
async fn bad_headers_signatures_and_timestamps_are_401_and_store_nothing() {
    let t = server().await;
    make_inbox(&t).await;
    let body = serde_json::to_vec(&payload("<a@mail.example.org>")).unwrap();
    let ts = now_secs().to_string();
    let n = nonce();
    let sig = sign(SECRET, &ts, &n, &body);
    let all = [
        ("x-todo-timestamp", ts.as_str()),
        ("x-todo-nonce", n.as_str()),
        ("x-todo-signature", sig.as_str()),
    ];
    // Each header missing, or repeated with the same value.
    for skip in 0..3 {
        let h: Vec<_> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != skip)
            .map(|(_, h)| *h)
            .collect();
        let (s, r) = post_raw(&t, &h, body.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "without {}: {r}", all[skip].0);
        let mut h = all.to_vec();
        h.push(all[skip]);
        let (s, _) = post_raw(&t, &h, body.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "repeated {}", all[skip].0);
    }
    // Uppercase hex, a wrong key, a changed body byte.
    let upper = sig.to_uppercase();
    let (s, _) = post_raw(
        &t,
        &[all[0], all[1], ("x-todo-signature", &upper)],
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let wrong = sign("another-secret-of-at-least-32-characters", &ts, &n, &body);
    let (s, _) = post_raw(
        &t,
        &[all[0], all[1], ("x-todo-signature", &wrong)],
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let mut changed = body.clone();
    changed.push(b' ');
    let (s, _) = post_raw(&t, &all, changed).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Too old and too far in the future (correctly signed), and just inside the window.
    for (offset, ok) in [(-301, false), (301, false), (-290, true)] {
        let p = payload(&format!("<window{offset}@mail.example.org>"));
        let b = serde_json::to_vec(&p).unwrap();
        let (s, r) = post_signed(&t, now_secs() + offset, &nonce(), &b).await;
        let want = if ok {
            StatusCode::CREATED
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(s, want, "offset {offset}: {r}");
    }
    assert_eq!(live_tasks(&t).len(), 1, "only the in-window request");
    // A rejected request never reached the nonce table.
    let (s, _) = post_raw(&t, &all, body).await;
    assert_eq!(s, StatusCode::CREATED, "the nonce was not burnt");
}

#[tokio::test]
async fn a_replayed_nonce_is_rejected_even_when_racing() {
    let t = server().await;
    make_inbox(&t).await;
    let n = nonce();
    let ts = now_secs();
    let body = serde_json::to_vec(&payload("<race@mail.example.org>")).unwrap();
    let t = Arc::new(t);
    let mut handles = Vec::new();
    for _ in 0..6 {
        let (t, n, body) = (t.clone(), n.clone(), body.clone());
        handles.push(tokio::spawn(async move {
            post_signed(&t, ts, &n, &body).await.0
        }));
    }
    let mut statuses = Vec::new();
    for h in handles {
        statuses.push(h.await.unwrap());
    }
    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    let refused = statuses
        .iter()
        .filter(|s| **s == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!((created, refused), (1, 5), "{statuses:?}");
    assert_eq!(live_tasks(&t).len(), 1);
}

#[tokio::test]
async fn message_ids_dedupe_even_when_racing() {
    let t = server().await;
    make_inbox(&t).await;
    let p = payload("<dup@mail.example.org>");
    let (s, first) = send(&t, &p).await;
    assert_eq!(s, StatusCode::CREATED, "{first}");
    // A retry: new timestamp, nonce and signature, same Message-ID.
    let (s, again) = send(&t, &p).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "duplicate");
    assert_eq!(task_id(&again), task_id(&first));
    // Many retries of a new message at once: one task.
    let t = Arc::new(t);
    let p = payload("<dup-race@mail.example.org>");
    let mut handles = Vec::new();
    for _ in 0..8 {
        let (t, p) = (t.clone(), p.clone());
        handles.push(tokio::spawn(async move { send(&t, &p).await }));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut created = 0;
    for h in handles {
        let (s, r) = h.await.unwrap();
        assert!(
            matches!(s, StatusCode::CREATED | StatusCode::OK),
            "{s}: {r}"
        );
        created += usize::from(s == StatusCode::CREATED);
        ids.insert(task_id(&r));
    }
    assert_eq!(created, 1);
    assert_eq!(ids.len(), 1);
    assert_eq!(live_tasks(&t).len(), 2);
}

#[tokio::test]
async fn senders_off_the_allowlist_are_403_and_store_nothing() {
    let t = server().await;
    make_inbox(&t).await;
    let mut p = payload("<stranger@mail.example.org>");
    p["forwarded_by"] = json!("stranger@example.net");
    let (s, r) = send(&t, &p).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
    assert_eq!(r["error"]["code"], "sender_not_allowed");
    let body = r.to_string();
    assert!(
        !body.contains("stranger"),
        "the error echoes nothing: {body}"
    );
    assert!(live_tasks(&t).is_empty());
    assert_eq!(t.server.engine.head_seq(), 1, "only the list was stored");
    // The seed is normalized: a differently cased address on it is allowed.
    p["forwarded_by"] = json!("SECOND@example.net");
    let (s, r) = send(&t, &p).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
}

#[tokio::test]
async fn invalid_payloads_are_422() {
    let t = server().await;
    make_inbox(&t).await;
    let mut cases = Vec::new();
    let mut p = payload("<v2@mail.example.org>");
    p["version"] = json!(2);
    cases.push(p);
    let mut p = payload("<no-title@mail.example.org>");
    p["title"] = json!("  ");
    cases.push(p);
    let mut p = payload("<bad-list@mail.example.org>");
    p["list"] = json!("Bad List");
    cases.push(p);
    let mut p = payload("<bad-date@mail.example.org>");
    p["received_at"] = json!("yesterday");
    cases.push(p);
    let mut p = payload("<no-sender@mail.example.org>");
    p.as_object_mut().unwrap().remove("forwarded_by");
    cases.push(p);
    for p in &cases {
        let (s, r) = send(&t, p).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{p}: {r}");
    }
    let (s, _) = post_signed(&t, now_secs(), &nonce(), b"not json").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Unknown fields are ignored; a missing `parse_failed` means false.
    let mut p = payload("<extra@mail.example.org>");
    p["future_field"] = json!({ "x": 1 });
    p.as_object_mut().unwrap().remove("parse_failed");
    let (s, r) = send(&t, &p).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(live_tasks(&t).len(), 1);
}

#[tokio::test]
async fn lists_are_routed_by_explicit_keys_only_and_fall_back_to_the_set_inbox() {
    let t = server().await;
    // A list named Inbox is not the Inbox until the owner picks it: refused as transient, so the
    // Worker retries.
    let inbox = t.create_list("Inbox").await;
    let (s, r) = send(&t, &payload("<early@mail.example.org>")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{r}");
    assert_eq!(r["error"]["code"], "inbox_not_found");
    assert!(live_tasks(&t).is_empty());
    set_inbox(&t, &inbox);
    let (s, _) = send(&t, &payload("<early@mail.example.org>")).await;
    assert_eq!(s, StatusCode::CREATED, "the retry goes through");
    let work = t.create_list("Work stuff").await;
    let errands = t.create_list("Errands").await;
    add_key(&t, "shop", &errands);
    let route = |mid: &str, list: &str| {
        let mut p = payload(mid);
        p["list"] = json!(list);
        p
    };
    let notes_of = |d: &Value| d["task"]["notes"].as_str().unwrap().to_owned();
    for (mid, key, want, note) in [
        ("<k1@m.example.org>", "shop", &errands, None),
        ("<k2@m.example.org>", "inbox", &inbox, None),
        // Names are never matched: only keys the owner set.
        (
            "<k3@m.example.org>",
            "errands",
            &inbox,
            Some("No list has the key \"errands\", so this task went to the Inbox."),
        ),
        (
            "<k4@m.example.org>",
            "work-stuff",
            &inbox,
            Some("No list has the key \"work-stuff\""),
        ),
    ] {
        let (s, r) = send(&t, &route(mid, key)).await;
        assert_eq!(s, StatusCode::CREATED, "{key}: {r}");
        let d = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await;
        assert_eq!(d["task"]["list"], json!(want), "{key}");
        let notes = notes_of(&d);
        match note {
            Some(n) => assert!(notes.contains(n), "{key}: {notes}"),
            None => assert!(!notes.contains("went to the Inbox"), "{key}: {notes}"),
        }
    }
    // Renaming lists changes nothing.
    t.ok(json!({ "cmd": "update_list", "id": errands, "name": "Shop" }))
        .await;
    t.ok(json!({ "cmd": "update_list", "id": work, "name": "Inbox" }))
        .await;
    let (_, r) = send(&t, &route("<k5@m.example.org>", "shop")).await;
    let d = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await;
    assert_eq!(d["task"]["list"], json!(errands));
    let (_, r) = send(&t, &payload("<k6@m.example.org>")).await;
    let d = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await;
    assert_eq!(d["task"]["list"], json!(inbox));
    // A deleted list's key goes to the Inbox, with a note, even though another list now has its old
    // name: nothing else takes the key over.
    t.ok(json!({ "cmd": "delete", "id": { "kind": "list", "id": errands } }))
        .await;
    t.create_list("Shop").await;
    let (s, r) = send(&t, &route("<k7@m.example.org>", "shop")).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let d = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await;
    assert_eq!(d["task"]["list"], json!(inbox));
    assert!(
        notes_of(&d).contains(
            "The list with the key \"shop\" was deleted, so this task went to the Inbox."
        ),
        "{d}"
    );
    // A deleted Inbox fails closed, also for keyed mail that would fall back to it.
    t.ok(json!({ "cmd": "delete", "id": { "kind": "list", "id": inbox } }))
        .await;
    let before = live_tasks(&t).len();
    for p in [
        payload("<k8@m.example.org>"),
        route("<k9@m.example.org>", "shop"),
    ] {
        let (s, r) = send(&t, &p).await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{r}");
        assert!(
            r.to_string().len() < 200 && !r.to_string().contains("shop"),
            "{r}"
        );
    }
    assert_eq!(live_tasks(&t).len(), before);
}

#[tokio::test]
async fn a_sender_removed_while_a_delivery_waits_for_the_writer_is_refused() {
    let t = Arc::new(server().await);
    make_inbox(&t).await;
    let nonces = t.server.ingest.store.nonce_count().unwrap();
    // Hold the writer, so the delivery passes the allowlist check and then waits.
    let engine = t.server.engine.clone();
    let hold = engine.hold_writer();
    let delivery = {
        let t = t.clone();
        tokio::spawn(async move { send(&t, &payload("<queued@mail.example.org>")).await })
    };
    // Its nonce is reserved once it is authenticated; the allowlist check follows at once.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while t.server.ingest.store.nonce_count().unwrap() == nonces {
        assert!(
            std::time::Instant::now() < deadline,
            "the delivery never arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!delivery.is_finished(), "the delivery waits for the writer");
    assert!(t.server.ingest.store.remove_sender(OWNER, BY).unwrap());
    drop(hold);
    let (s, r) = delivery.await.unwrap();
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
    assert_eq!(r["error"]["code"], "sender_not_allowed");
    assert!(live_tasks(&t).is_empty());
    assert_eq!(t.server.engine.head_seq(), 1, "only the list was stored");
    // Nothing was reserved for the Message-ID either: once allowed again, it is ingested.
    assert!(t.server.ingest.store.add_sender(OWNER, BY).unwrap());
    let (s, r) = send(&t, &payload("<queued@mail.example.org>")).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
}

#[tokio::test]
async fn the_message_id_ledger_is_rebuilt_from_the_event_log() {
    let t = server().await;
    make_inbox(&t).await;
    let (s, first) = send(&t, &payload("<ledger-1@mail.example.org>")).await;
    assert_eq!(s, StatusCode::CREATED, "{first}");
    let (_, second) = send(&t, &payload("  <ledger-2@mail.example.org>\r\n")).await;
    // The second is undone: its Message-ID stays taken.
    let seq = t.server.engine.head_seq();
    assert_eq!(t.undo(seq, None).await.0, StatusCode::OK);
    let rows = ledger(&t.db_path());
    assert_eq!(rows.len(), 2);
    // The ledger key is the SHA-256 of the Message-ID as the link stores it (trimmed).
    let key = hex::encode(<Sha256 as sha2::Digest>::digest(
        b"<ledger-2@mail.example.org>",
    ));
    assert!(rows.contains(&(key, task_id(&second))), "{rows:?}");

    // Lost (say, by a bad restore of that one table): rebuilt at startup ...
    let t = t
        .restart_with(|db| {
            let conn = rusqlite::Connection::open(db).unwrap();
            conn.execute("DELETE FROM ingest_messages", []).unwrap();
        })
        .await;
    assert_eq!(ledger(&t.db_path()), rows);
    let (s, r) = send(&t, &payload("<ledger-1@mail.example.org>")).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(task_id(&r), task_id(&first));
    let (s, r) = send(&t, &payload("<ledger-2@mail.example.org>")).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(task_id(&r), task_id(&second));
    assert_eq!(live_tasks(&t).len(), 1);

    // ... and by `rebuild-projections`, which also drops rows the log doesn't explain.
    let t = t
        .restart_with(|db| {
            let conn = rusqlite::Connection::open(db).unwrap();
            conn.execute("DELETE FROM ingest_messages", []).unwrap();
            conn.execute(
                "INSERT INTO ingest_messages (mid_hash, task_id) VALUES ('bogus', ?1)",
                [uuid::Uuid::new_v4().to_string()],
            )
            .unwrap();
            drop(conn);
            todo_server::engine::Engine::rebuild_projections(db).unwrap();
            assert_eq!(ledger(db), rows);
        })
        .await;
    assert_eq!(ledger(&t.db_path()), rows);
}

#[tokio::test]
async fn the_body_is_stored_only_with_keep() {
    let t = server().await;
    make_inbox(&t).await;
    let body = json!({ "text": "Please read\nthis.", "source": "text", "truncated": true });
    let mut kept = payload("<keep@mail.example.org>");
    kept["keep"] = json!(true);
    kept["body"] = body.clone();
    let mut not_kept = payload("<nokeep@mail.example.org>");
    not_kept["body"] = body;
    let (_, r) = send(&t, &kept).await;
    let notes = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await["task"]["notes"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(notes.contains("\n\nPlease read\nthis."), "{notes}");
    assert!(notes.contains("cut short"), "{notes}");
    let (_, r) = send(&t, &not_kept).await;
    let notes = t.get_ok(&format!("/api/v1/tasks/{}", task_id(&r))).await["task"]["notes"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!notes.contains("Please read"), "{notes}");
    assert_eq!(
        notes,
        "From: Alice Example <alice@example.org>\nForwarded by you@example.com on 2025-11-21 08:00 (UTC)\nSubject: Fwd: Quarterly report\n"
    );
}

#[tokio::test]
async fn ingested_tasks_show_in_activity_as_email_ingest_and_can_be_undone() {
    let t = server().await;
    make_inbox(&t).await;
    let (s, r) = send(&t, &payload("<undo@mail.example.org>")).await;
    assert_eq!(s, StatusCode::CREATED);
    let task = task_id(&r);
    // Newest first: the email link, then the task.
    let feed = t.get_ok("/api/v1/activity").await;
    for item in &feed["items"].as_array().unwrap()[..2] {
        assert_eq!(
            item["event"]["actor"],
            json!({ "type": "email_ingest" }),
            "{item}"
        );
    }
    assert_eq!(
        feed["items"][0]["summary"],
        "Created link \"Fwd: Quarterly report\""
    );
    assert_eq!(
        feed["items"][1]["summary"],
        "Created task \"Quarterly report\""
    );
    // The task and its link are one group: one undo removes both.
    let events = t.server.engine.events_since(1, 10).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].group, events[1].group);
    let seq = events[1].seq.unwrap();
    let (s, u) = t.undo(seq, None).await;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert!(live_tasks(&t).is_empty());
    let s = t.server.engine.state();
    assert!(s.links.values().all(|l| !s.is_live(l.id.into())));
    drop(s);
    // The Message-ID stays taken: forwarding it again does not bring the task back.
    let (s, r) = send(&t, &payload("<undo@mail.example.org>")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(task_id(&r), task);
    assert!(live_tasks(&t).is_empty());
}

#[tokio::test]
async fn bearer_tokens_neither_open_nor_bypass_the_route() {
    let t = server().await;
    make_inbox(&t).await;
    let body = serde_json::to_vec(&payload("<bearer@mail.example.org>")).unwrap();
    // An owner token alone is not a signature.
    let res = t
        .client
        .post(format!("{}{PATH}", t.url))
        .bearer_auth(TOKEN_A)
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // Nor does it rescue a bad signature.
    let ts = now_secs().to_string();
    let n = nonce();
    let res = t
        .client
        .post(format!("{}{PATH}", t.url))
        .bearer_auth(TOKEN_A)
        .header("x-todo-timestamp", &ts)
        .header("x-todo-nonce", &n)
        .header("x-todo-signature", "0".repeat(64))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(live_tasks(&t).is_empty());
    // And nothing an API client sends can record the email-ingest actor: commands are attributed
    // to the token's own principal.
    let list = t.create_list("Other").await;
    let task = t.create_task(&list, "Mine", json!({})).await;
    let d = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(d["task"]["created"]["by"]["type"], "owner");
}

#[tokio::test]
async fn bodies_over_the_limit_are_413() {
    let t = server().await;
    make_inbox(&t).await;
    let big = vec![b' '; todo_server::ingest::MAX_BODY_BYTES + 1];
    // The server answers before reading the whole body; the connection may also be reset under the
    // client while it is still sending (Windows does that), so only an answer is checked.
    let ts = now_secs().to_string();
    let n = nonce();
    let sig = sign(SECRET, &ts, &n, &big);
    let res = t
        .client
        .post(format!("{}{PATH}", t.url))
        .header("content-type", "application/json")
        .header("x-todo-timestamp", &ts)
        .header("x-todo-nonce", &n)
        .header("x-todo-signature", &sig)
        .body(big)
        .send()
        .await;
    if let Ok(res) = res {
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
    // Without a Content-Length (a streamed body), reading stops at the limit.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        vec![Ok(vec![b' '; 100 * 1024]), Ok(vec![b' '; 100 * 1024])];
    let ts = now_secs().to_string();
    let n = nonce();
    let res = t
        .client
        .post(format!("{}{PATH}", t.url))
        .header("x-todo-timestamp", &ts)
        .header("x-todo-nonce", &n)
        .header("x-todo-signature", "0".repeat(64))
        .body(reqwest::Body::wrap_stream(futures_util::stream::iter(
            chunks,
        )))
        .send()
        .await;
    if let Ok(res) = res {
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
    // Just under the limit is read (and then judged by its signature).
    let mut p = payload("<big@mail.example.org>");
    p["keep"] = json!(true);
    p["body"] = json!({ "text": "x".repeat(64 * 1024), "source": "text", "truncated": false });
    let (s, r) = send(&t, &p).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
}

#[tokio::test]
async fn requests_are_rate_limited_per_address() {
    let t = TestServer::start_with(|cfg| {
        enabled(cfg);
        cfg.ingest.requests_per_minute = 1;
        cfg.ingest.request_burst = 2;
    })
    .await;
    make_inbox(&t).await;
    for i in 0..2 {
        let (s, r) = send(&t, &payload(&format!("<rl{i}@mail.example.org>"))).await;
        assert_eq!(s, StatusCode::CREATED, "{r}");
    }
    let res = t
        .client
        .post(format!("{}{PATH}", t.url))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().get(RETRY_AFTER).is_some());
}

#[tokio::test]
async fn failed_signatures_use_up_the_address_failure_budget() {
    let t = TestServer::start_with(|cfg| {
        enabled(cfg);
        cfg.auth.bearer_failures_per_minute = 1;
        cfg.auth.bearer_failure_burst = 2;
    })
    .await;
    make_inbox(&t).await;
    let body = serde_json::to_vec(&payload("<fail@mail.example.org>")).unwrap();
    for _ in 0..2 {
        let h = [
            ("x-todo-timestamp", now_secs().to_string()),
            ("x-todo-nonce", nonce()),
            ("x-todo-signature", "0".repeat(64)),
        ];
        let h: Vec<(&str, &str)> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(
            post_raw(&t, &h, body.clone()).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    // Turned away before the signature is even checked, so a valid request waits too.
    let (s, _) = post_signed(&t, now_secs(), &nonce(), &body).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn without_a_secret_the_route_is_404() {
    let t = TestServer::start().await;
    make_inbox(&t).await;
    let (s, r) = send(&t, &payload("<off@mail.example.org>")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{r}");
    assert!(live_tasks(&t).is_empty());
}

#[tokio::test]
async fn the_secret_file_must_be_owner_only_and_long_enough() {
    let dir = tempfile::tempdir().unwrap();
    let base = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        ..Config::default()
    };
    // Readable by others (created the ordinary way).
    let open = dir.path().join("open.secret");
    std::fs::write(&open, SECRET).unwrap();
    let mut cfg = base.clone();
    cfg.ingest.email_hmac_secret_file = Some(open);
    let e = Server::bind(&cfg, Options::default()).await.unwrap_err();
    assert!(e.to_string().contains("owner-only"), "{e}");
    assert!(!e.to_string().contains(SECRET));
    // Owner-only but short.
    let short = dir.path().join("short.secret");
    let mut f = todo_fs::create_private_file(&short).unwrap();
    f.write_all(b"too-short").unwrap();
    drop(f);
    let mut cfg = base.clone();
    cfg.ingest.email_hmac_secret_file = Some(short);
    let e = Server::bind(&cfg, Options::default()).await.unwrap_err();
    assert!(e.to_string().contains("at least 32"), "{e}");
    assert!(!e.to_string().contains("too-short"));
    // A symlink to a good secret file is refused, not followed (where links can be made: Windows
    // needs a privilege for them).
    let good = dir.path().join("good.secret");
    let mut f = todo_fs::create_private_file(&good).unwrap();
    f.write_all(SECRET.as_bytes()).unwrap();
    drop(f);
    let link = dir.path().join("link.secret");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&good, &link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&good, &link);
    if linked.is_ok() {
        let mut cfg = base.clone();
        cfg.ingest.email_hmac_secret_file = Some(link);
        let e = Server::bind(&cfg, Options::default()).await.unwrap_err();
        assert!(e.to_string().contains("link"), "{e}");
    }
    // A directory is not a secret file.
    let mut cfg = base;
    cfg.ingest.email_hmac_secret_file = Some(dir.path().to_owned());
    assert!(Server::bind(&cfg, Options::default()).await.is_err());
}

/// The Worker's own delivery code (`workers/email-ingest/src/deliver.js`, `postPayload`), run by Node
/// against this server. Skipped when `node` is not installed.
#[tokio::test]
async fn the_workers_signing_code_is_accepted() {
    let node = std::process::Command::new("node").arg("--version").output();
    if !node.is_ok_and(|o| o.status.success()) {
        eprintln!("skipped: node is not installed");
        return;
    }
    let t = server().await;
    make_inbox(&t).await;
    let shopping = t.create_list("Shopping").await;
    add_key(&t, "shopping", &shopping);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("crates/todo-server/tests/support/worker-post.mjs");
    let src = root.join("workers/email-ingest/src");
    let mut kept = payload("<node@mail.example.org>");
    kept["list"] = json!("shopping");
    kept["keep"] = json!(true);
    kept["title"] = json!("Nákup – mléko #2");
    kept["body"] =
        json!({ "text": "Line one\nLine \"two\" ✓", "source": "text", "truncated": false });
    let mut stranger = payload("<node-stranger@mail.example.org>");
    stranger["forwarded_by"] = json!("stranger@example.net");
    let payloads = json!([kept, kept, stranger]);
    let url = format!("{}{PATH}", t.url.replace("127.0.0.1", "localhost"));
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .arg(&script)
            .arg(&src)
            .env("INGEST_URL", &url)
            .env("INGEST_HMAC_SECRET", SECRET)
            .env("PAYLOADS", payloads.to_string())
            .output()
    })
    .await
    .unwrap()
    .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let results: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        results,
        json!([
            { "outcome": "ok", "status": 201 },
            { "outcome": "ok", "status": 200 },
            { "outcome": "permanent", "status": 403 },
        ])
    );
    let tasks = live_tasks(&t);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].list.to_string(), shopping);
    assert_eq!(tasks[0].title, "Nákup – mléko #2");
}
