#![allow(clippy::unwrap_used)]
//! The importer against a scripted HTTP server: what counts as an acknowledgement, `Retry-After`,
//! the wait budget, the log epoch, and the journal header.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_import::apply::{ApplyError, ApplyOptions, apply, read_journal};
use todo_import::bundle::Bundle;
use todo_import::http::{Client, HttpError};
use uuid::Uuid;

const EPOCH: &str = "0190a000-0000-7000-8000-000000000001";
const OTHER_EPOCH: &str = "0190a000-0000-7000-8000-000000000002";

/// One request the fake server saw.
struct Request {
    method: String,
    path: String,
    body: serde_json::Value,
}

type Handler = dyn Fn(usize, &Request) -> String + Send + Sync;

/// Serves raw HTTP responses from `handler` (given the number of earlier POSTs and the request),
/// one per connection. Returns the base URL.
fn serve(handler: Box<Handler>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut posts = 0usize;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("").to_owned();
            let path = parts.next().unwrap_or("").to_owned();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            let _ = reader.read_exact(&mut body);
            let req = Request {
                method,
                path,
                body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
            };
            let resp = handler(posts, &req);
            if req.method == "POST" {
                posts += 1;
            }
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    url
}

fn response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn epoch_page(epoch: &str) -> String {
    response(
        "200 OK",
        "",
        &format!(
            r#"{{"events":[],"next_since":0,"head_seq":0,"has_more":false,"epoch":"{epoch}"}}"#
        ),
    )
}

/// A correct acknowledgement of the request's command.
fn ack(req: &Request, epoch: &str) -> String {
    let id = req.body["command_id"].as_str().unwrap();
    response(
        "200 OK",
        "",
        &format!(
            r#"{{"command_id":"{id}","events":[{{"seq":1,"id":"{}"}}],"summary":"Imported","epoch":"{epoch}"}}"#,
            Uuid::from_u128(1)
        ),
    )
}

/// A server that answers the epoch query with [`EPOCH`] and every POST with `post`.
fn server(post: impl Fn(usize, &Request) -> String + Send + Sync + 'static) -> String {
    serve(Box::new(move |n, req| {
        if req.method == "GET" && req.path.starts_with("/api/v1/events") {
            epoch_page(EPOCH)
        } else {
            post(n, req)
        }
    }))
}

type Waits = Arc<Mutex<Vec<Duration>>>;

/// A client whose waits are recorded instead of slept.
fn client(url: &str) -> (Client, Waits) {
    let waits: Waits = Arc::default();
    let w = waits.clone();
    let c = Client::new(url, "test-token".into())
        .unwrap()
        .with_sleeper(Arc::new(move |d| w.lock().unwrap().push(d)));
    (c, waits)
}

fn opts(journal: &Path) -> ApplyOptions {
    ApplyOptions {
        batch_size: 10,
        journal: journal.to_owned(),
        abort_after: None,
    }
}

fn journal_of(b: &Bundle, path: &Path, url: &str) -> todo_import::apply::Journal {
    read_journal(path, &b.digest().unwrap(), url).unwrap()
}

#[test]
fn only_a_real_command_result_counts_as_acknowledged() {
    let bundle = common::bundle();
    let first = bundle.items[0].command_id;
    let other = Uuid::from_u128(2);
    let cases: [(&str, String); 5] = [
        ("empty 200", response("200 OK", "", "")),
        (
            "204",
            "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_owned(),
        ),
        (
            "another command_id",
            response(
                "200 OK",
                "",
                &format!(
                    r#"{{"command_id":"{other}","events":[{{"seq":1,"id":"{other}"}}],"summary":"","epoch":"{EPOCH}"}}"#
                ),
            ),
        ),
        (
            "no events",
            response(
                "200 OK",
                "",
                &format!(
                    r#"{{"command_id":"{first}","events":[],"summary":"","epoch":"{EPOCH}"}}"#
                ),
            ),
        ),
        (
            "events missing",
            response(
                "200 OK",
                "",
                &format!(r#"{{"command_id":"{first}","summary":"","epoch":"{EPOCH}"}}"#),
            ),
        ),
    ];
    for (name, reply) in cases {
        let url = server(move |_, _| reply.clone());
        let dir = tempfile::tempdir().unwrap();
        let j = dir.path().join("j");
        let (c, _) = client(&url);
        let err = apply(&bundle, &c, &opts(&j)).unwrap_err();
        assert!(
            matches!(err, ApplyError::NotAcknowledged { index: 0, .. }),
            "{name}: {err}"
        );
        let journal = journal_of(&bundle, &j, &url);
        assert!(journal.done.is_empty(), "{name}: nothing may be journaled");
        assert_eq!(journal.epoch, Some(EPOCH.parse().unwrap()));
    }
}

#[test]
fn acknowledged_commands_are_journaled_in_the_servers_epoch() {
    let bundle = common::bundle();
    let total = bundle.items.len();
    let dir = tempfile::tempdir().unwrap();
    let j = dir.path().join("j");
    let url = server(|_, req| ack(req, EPOCH));
    let (c, _) = client(&url);
    let sum = apply(&bundle, &c, &opts(&j)).unwrap();
    assert_eq!((sum.stored, sum.replayed), (total, 0));
    assert_eq!(journal_of(&bundle, &j, &url).done.len(), total);

    // The same journal against a server whose log has another epoch (restored from a backup).
    let restored = serve(Box::new(|_, req| {
        if req.method == "GET" {
            epoch_page(OTHER_EPOCH)
        } else {
            ack(req, OTHER_EPOCH)
        }
    }));
    let moved = dir.path().join("moved");
    std::fs::write(
        &moved,
        std::fs::read_to_string(&j)
            .unwrap()
            .replace(&todo_import::label(&url), &todo_import::label(&restored)),
    )
    .unwrap();
    let (c, _) = client(&restored);
    let err = apply(&bundle, &c, &opts(&moved)).unwrap_err();
    assert!(matches!(err, ApplyError::EpochChanged { .. }), "{err}");
    assert!(err.to_string().contains("delete the journal"), "{err}");

    // The epoch changing during a run stops it, without journaling the commands since.
    let flips = server(|n, req| ack(req, if n < 3 { EPOCH } else { OTHER_EPOCH }));
    let k = dir.path().join("k");
    let (c, _) = client(&flips);
    let err = apply(&bundle, &c, &opts(&k)).unwrap_err();
    assert!(matches!(err, ApplyError::EpochChanged { .. }), "{err}");
    assert!(journal_of(&bundle, &k, &flips).done.is_empty());
}

#[test]
fn retry_after_is_honoured_in_full_and_before_the_body_is_read() {
    let url = server(|n, req| {
        match n {
        0 => response("429 Too Many Requests", "Retry-After: 300\r\n", r#"{"error":{}}"#),
        1 => {
            let at = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(120);
            let date = jiff::fmt::rfc2822::DateTimePrinter::new()
                .timestamp_to_rfc9110_string(&at)
                .unwrap();
            response("503 Service Unavailable", &format!("Retry-After: {date}\r\n"), "")
        }
        // A body cut short: reading it would fail, but the status and header are enough.
        2 => "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nRetry-After: 7\r\nContent-Length: 1000\r\n\r\npartial"
            .to_owned(),
        _ => ack(req, EPOCH),
    }
    });
    let (mut c, waits) = client(&url);
    c.max_wait = Duration::from_secs(3600);
    let id = Uuid::from_u128(3);
    let reply = c
        .post(
            "POST /api/v1/commands",
            "/api/v1/commands",
            &serde_json::json!({ "command_id": id }),
        )
        .unwrap();
    assert_eq!(reply.body["command_id"], id.to_string());
    let waits = waits.lock().unwrap().clone();
    assert_eq!(waits.len(), 3, "{waits:?}");
    assert_eq!(waits[0], Duration::from_secs(300), "not capped");
    assert!(
        (Duration::from_secs(117)..=Duration::from_secs(120)).contains(&waits[1]),
        "an HTTP-date: {waits:?}"
    );
    assert_eq!(waits[2], Duration::from_secs(7));
}

#[test]
fn a_wait_past_the_budget_stops_with_a_resumable_error() {
    let bundle = common::bundle();
    let total = bundle.items.len();
    let dir = tempfile::tempdir().unwrap();
    let j = dir.path().join("j");
    let url = server(|n, req| {
        if n < 3 {
            ack(req, EPOCH)
        } else {
            response("429 Too Many Requests", "Retry-After: 3600\r\n", "")
        }
    });
    let (mut c, waits) = client(&url);
    c.max_wait = Duration::from_secs(900);
    let err = apply(&bundle, &c, &opts(&j)).unwrap_err();
    let ApplyError::Http {
        index: 3, source, ..
    } = &err
    else {
        panic!("{err}")
    };
    assert!(
        matches!(**source, HttpError::WaitTooLong { status: 429, wait, .. } if wait == Duration::from_secs(3600)),
        "{source}"
    );
    assert!(
        err.to_string().contains("run again later to resume"),
        "{err}"
    );
    assert!(waits.lock().unwrap().is_empty(), "it did not wait at all");
    // What was acknowledged before the stop is journaled, so a later run resumes there.
    assert_eq!(journal_of(&bundle, &j, &url).done.len(), 3);
    let resumed = server(|_, req| ack(req, EPOCH));
    std::fs::write(
        &j,
        std::fs::read_to_string(&j)
            .unwrap()
            .replace(&todo_import::label(&url), &todo_import::label(&resumed)),
    )
    .unwrap();
    let (c, _) = client(&resumed);
    let sum = apply(&bundle, &c, &opts(&j)).unwrap();
    assert_eq!((sum.skipped, sum.stored), (3, total - 3));
}

#[test]
fn a_crash_while_creating_the_journal_does_not_block_a_resume() {
    let bundle = common::bundle();
    let dir = tempfile::tempdir().unwrap();
    let url = server(|_, req| ack(req, EPOCH));
    // A crash while the header was being written leaves only the temporary file behind.
    let j = dir.path().join("j");
    std::fs::write(dir.path().join("j.tmp"), "todo-import-jou").unwrap();
    let (c, _) = client(&url);
    let sum = apply(&bundle, &c, &opts(&j)).unwrap();
    assert_eq!(sum.stored, bundle.items.len());
    assert!(!dir.path().join("j.tmp").exists());
    let journal = journal_of(&bundle, &j, &url);
    assert_eq!(journal.epoch, Some(EPOCH.parse().unwrap()));
    assert_eq!(journal.done.len(), bundle.items.len());
    // An empty journal file is a new journal too.
    let e = dir.path().join("empty");
    std::fs::write(&e, "").unwrap();
    assert!(apply(&bundle, &c, &opts(&e)).is_ok());
}
