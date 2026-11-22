use super::*;
use crate::engine::{
    Client, ClientConfig, MemoryStorage, OsEntropy, Storage, StorageOp, TokioClock,
};
use crate::{
    fixtures::{self, Fixture},
    labels::{LocalNow, Now},
};
use axum::{
    Router,
    body::Bytes,
    extract::Query,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use std::collections::HashMap;
use todo_client::native::HttpTransport;
use todo_core::Actor;

#[test]
fn paired_transfers_preserve_bytes_and_send_metadata_not_json() {
    let now = LocalNow {
        epoch_ms: 1_800_000_000_000,
        utc_offset_secs: 0,
        local_day: None,
        zone_id: "UTC".into(),
    };
    let state = fixtures::build(Fixture::World, &Now::new(now));
    let file = state
        .attachments
        .values()
        .find(|a| a.file_name == "Appointment.pdf")
        .unwrap();
    let data = vec![0x9d; file.size_bytes as usize];
    let auth = |headers: &HeaderMap| {
        headers
            .get("Authorization")
            .is_some_and(|v| v == "Bearer fixture-only")
    };
    let payload = data.clone();
    let app = Router::new()
        .route(
            &format!("/api/v1/attachments/{}", file.id),
            get(move |headers: HeaderMap| async move {
                assert!(auth(&headers));
                payload
            }),
        )
        .route(
            &format!("/api/v1/tasks/{}/attachments", file.task),
            post(
                move |headers: HeaderMap,
                      Query(query): Query<HashMap<String, String>>,
                      body: Bytes| async move {
                    assert!(auth(&headers));
                    assert_eq!(headers["content-type"], "image/png");
                    assert!(query["attachment_id"].parse::<AttachmentId>().is_ok());
                    if query["file_name"] == "smaller-limit.png" {
                        return (
                            StatusCode::PAYLOAD_TOO_LARGE,
                            r#"{"error":{"code":"payload_too_large","details":{"max_bytes":2}}}"#,
                        );
                    }
                    assert_eq!(query["file_name"], "čaj & +.png");
                    assert_eq!(&body[..], [0xff, 0, 5]);
                    (StatusCode::CREATED, "")
                },
            ),
        );
    let rt = runtime().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = rt.spawn(async {
        axum::serve(listener, app).await.unwrap();
    });
    let mut storage = MemoryStorage::new();
    storage
        .write(&[
            StorageOp::SetEpoch(uuid::Uuid::nil()),
            StorageOp::Snapshot {
                state: &state,
                cursor: 0,
            },
        ])
        .unwrap();
    let engine = TodoEngine::new(
        Client::new(
            AppTransport::Http(HttpTransport::new(&url, "fixture-only").unwrap()),
            TokioClock,
            OsEntropy,
            storage,
            ClientConfig::new(Actor::System),
        )
        .unwrap(),
        None,
        false,
    );
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_string_lossy().into_owned();
    engine
        .download_attachment(file.id.to_string(), path.clone(), 0)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    std::fs::write(&path, [0xff, 0, 5]).unwrap();
    engine
        .upload_attachment(
            file.task.to_string(),
            "čaj & +.png".into(),
            "image/png".into(),
            path.clone(),
            0,
        )
        .unwrap();
    let refused = engine
        .upload_attachment(
            file.task.to_string(),
            "smaller-limit.png".into(),
            "image/png".into(),
            path.clone(),
            0,
        )
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        "That file is too large. The limit is 2 bytes."
    );
    tmp.as_file().set_len(MAX_BYTES + 1).unwrap();
    let err = engine
        .upload_attachment(
            file.task.to_string(),
            "oversized".into(),
            "text/plain".into(),
            path,
            0,
        )
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "That file is too large. The limit is 25 MiB."
    );
    // Stopping sync doesn't invalidate a current or newly staged transfer.
    engine.stop();
    assert_eq!(engine.attachment_epoch(), 0);
    engine.transfer(0, async { Ok(()) }).unwrap();
    // Explicit cancellation interrupts a blocked wait and rejects its stale epoch.
    std::thread::scope(|s| {
        let (tx, rx) = std::sync::mpsc::channel();
        let client = &engine;
        let worker = s.spawn(move || {
            client.transfer(0, async {
                tx.send(()).unwrap();
                std::future::pending::<Result<(), FfiError>>().await
            })
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        engine.cancel_attachment_transfers();
        assert_eq!(
            worker.join().unwrap().unwrap_err().to_string(),
            "Transfer cancelled."
        );
        assert!(engine.transfer(0, async { Ok(()) }).is_err());
        let fresh = engine.attachment_epoch();
        engine.stop();
        engine.transfer(fresh, async { Ok(()) }).unwrap();
    });
    assert_eq!(engine.status().pending, 0);
    server.abort();
}

#[test]
fn an_attachment_refusal_alone_notifies_and_survives_offline_reopen() {
    let state = fixtures::build(
        Fixture::World,
        &Now::new(LocalNow {
            epoch_ms: 1_800_000_000_000,
            utc_offset_secs: 0,
            local_day: None,
            zone_id: "UTC".into(),
        }),
    );
    let file = state.attachments.values().next().unwrap();
    let rt = runtime().unwrap();
    for upload in [false, true] {
        let refuse = |headers: HeaderMap| async move {
            assert_eq!(headers["Authorization"], "Bearer fixture-only");
            StatusCode::UNAUTHORIZED
        };
        let app = if upload {
            Router::new().route("/api/v1/tasks/{id}/attachments", post(refuse))
        } else {
            Router::new().route("/api/v1/attachments/{id}", get(refuse))
        };
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = rt.spawn(async { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.sqlite");
        let mut storage = todo_client::sqlite::SqliteStorage::open(&cache).unwrap();
        storage
            .write(&[StorageOp::Snapshot {
                state: &state,
                cursor: 0,
            }])
            .unwrap();
        drop(storage);
        let config = crate::EngineConfig {
            server_url: url,
            token: "fixture-only".into(),
            device_id: "fixture-phone".into(),
            database_path: cache.to_string_lossy().into_owned(),
            allow_loopback_http: true,
        };
        let engine = TodoEngine::open(config.clone()).unwrap();
        assert!(!engine.status().revoked);
        let mut changes = engine.client.changes();
        let path = dir.path().join("transfer.bin");
        std::fs::write(&path, b"data").unwrap();
        let path = path.to_string_lossy().into_owned();
        // Don't start sync: the transfer is the only request that can report revocation.
        let error = if upload {
            engine
                .upload_attachment(
                    file.task.to_string(),
                    "sample.bin".into(),
                    "application/octet-stream".into(),
                    path,
                    0,
                )
                .unwrap_err()
        } else {
            engine
                .download_attachment(file.id.to_string(), path, 0)
                .unwrap_err()
        };
        assert!(error.to_string().contains("signed out"));
        rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), changes.next())
                .await
                .unwrap();
        });
        assert!(engine.status().revoked);
        drop(engine);
        server.abort();
        assert!(TodoEngine::open(config).unwrap().status().revoked);
    }
}

#[tokio::test]
async fn early_upload_refusals_keep_the_http_status_if_readable() {
    use std::time::Duration;
    use todo_client::native::attachments::FileError;
    use todo_core::ids::TaskId;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let data = tempfile::NamedTempFile::new().unwrap();
    data.as_file().set_len(8 * 1024 * 1024).unwrap();
    for (status, body, message) in [
        (
            413,
            r#"{"error":{"code":"payload_too_large","details":{"max_bytes":2}}}"#,
            "That file is too large. The limit is 2 bytes.",
        ),
        (429, "", "Couldn’t upload right now. Try again later."),
        (507, "", "Attachment storage is full."),
        (
            0,
            "",
            "Couldn’t upload the file. The connection dropped, or the server refused it.",
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut request = BufReader::new(socket);
            let mut line = String::new();
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            if status == 0 {
                // Reset *after* the request reached the server: this isn't a connect error.
                request.get_ref().set_zero_linger().unwrap();
                return;
            }
            let response = format!(
                "HTTP/1.1 {status} Refused\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            request
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
            // Don't close over unread request bytes. That can reset the connection before the
            // client gets to read the early reply. Drain until the client closes its write side.
            tokio::io::copy(&mut request, &mut tokio::io::sink())
                .await
                .unwrap();
        });
        let transport = HttpTransport::new(&url, "fixture-only").unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            transport.upload_from(
                TaskId::from_uuid(uuid::Uuid::nil()),
                AttachmentId::from_uuid(uuid::Uuid::nil()),
                "sample.bin",
                "application/octet-stream",
                data.path(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        if status == 0 {
            assert!(matches!(error, FileError::Interrupted));
            assert_ne!(error.upload_message(), failure::ATTACHMENT_OFFLINE);
        } else {
            assert!(matches!(error, FileError::Refused { status: s, .. } if s == status));
        }
        assert_eq!(error.upload_message(), message);
        drop(transport);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn downloads_reject_incomplete_content_and_distinguish_refusals() {
    use axum::{body::Body, response::Response};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use todo_client::native::attachments::FileError;
    let n = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/api/v1/attachments/{id}",
        get(move || {
            let i = n.fetch_add(1, Ordering::Relaxed);
            async move {
                match i {
                    0 => Response::new(Body::from("short")), // advertised length disagrees
                    1 => Response::new(Body::from_stream(futures_util::stream::once(async {
                        Ok::<_, std::io::Error>(Bytes::from_static(b"short"))
                    }))), // early EOF, no length
                    2 => Response::builder()
                        .status(404)
                        .body(Body::from(r#"{"error":{"code":"content_unavailable"}}"#))
                        .unwrap(),
                    _ => Response::builder().status(401).body(Body::empty()).unwrap(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async {
        axum::serve(listener, app).await.unwrap();
    });
    let t = HttpTransport::new(&url, "fixture-only").unwrap();
    let out = tempfile::NamedTempFile::new().unwrap();
    let id = AttachmentId::from_uuid(uuid::Uuid::nil());
    for _ in 0..2 {
        assert!(matches!(
            t.download_to(id, 10, out.path()).await,
            Err(FileError::WrongSize)
        ));
    }
    for message in [
        "This attachment has no stored content.",
        "This device is signed out. Pair again to download attachments.",
    ] {
        assert_eq!(
            t.download_to(id, 10, out.path())
                .await
                .unwrap_err()
                .download_message(),
            message
        );
    }
    assert_eq!(
        FileError::Timeout.download_message(),
        "The download timed out. Try again."
    );
    server.abort();
}

#[test]
fn open_uses_a_safe_leaf_and_never_dispatches_a_supplied_mime_type() {
    let state = fixtures::build(
        Fixture::World,
        &Now::new(LocalNow {
            epoch_ms: 1_800_000_000_000,
            utc_offset_secs: 0,
            local_day: None,
            zone_id: "UTC".into(),
        }),
    );
    let mut file = state.attachments.values().next().unwrap().clone();
    for name in ["..", "", ".pdf"] {
        file.file_name = name.into();
        let row = AttachmentRow::from(&file);
        assert_eq!(row.name, if name == ".pdf" { name } else { "attachment" });
        assert_eq!(row.open_type, None);
    }
    file.file_name =
        "../../x\u{202e}\u{2028}\u{2029}\u{2060}\u{206f}\u{061c}\u{ad}\u{e0001}\u{e007f}\u{7f}.apk"
            .into();
    file.media_type = "application/pdf".into();
    let row = AttachmentRow::from(&file);
    assert_eq!(row.name, "x.apk");
    assert_eq!(safe_attachment_name(file.file_name.clone()), row.name);
    let long = format!("{}.pdf", "ř🍵".repeat(100));
    let safe = safe_name(&long);
    assert!(safe.len() <= 255 && safe.ends_with(".pdf"));
    assert_eq!(safe_name(&safe), safe);
    assert_eq!(row.open_type, None);
    file.file_name = "../Guide.PDF".into();
    file.media_type = "application/vnd.android.package-archive".into();
    assert_eq!(
        AttachmentRow::from(&file).open_type.as_deref(),
        Some("application/pdf")
    );
}
