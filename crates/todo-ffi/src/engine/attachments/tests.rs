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
    let other = state
        .tasks
        .keys()
        .copied()
        .find(|&id| id != file.task)
        .unwrap();
    let app = Router::new()
        .route(
            &format!("/api/v1/tasks/{other}/attachments"),
            post(|| async {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    r#"{"error":{"code":"payload_too_large","details":{"max_bytes":2}}}"#,
                )
            }),
        )
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
    tmp.as_file().set_len(8 * 1024 * 1024).unwrap();
    let early = engine
        .upload_attachment(
            other.to_string(),
            "early.bin".into(),
            "application/octet-stream".into(),
            path.clone(),
            0,
        )
        .unwrap_err()
        .to_string();
    assert!(early.contains("too large"), "{early}");
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
    // A stop interrupts a blocked wait and prevents staging work from starting a late request.
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
        engine.stop();
        assert_eq!(
            worker.join().unwrap().unwrap_err().to_string(),
            "Transfer cancelled."
        );
        assert!(engine.transfer(0, async { Ok(()) }).is_err());
    });
    assert_eq!(engine.status().pending, 0);
    server.abort();
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
        }),
    );
    let mut file = state.attachments.values().next().unwrap().clone();
    for name in ["..", "", ".pdf"] {
        file.file_name = name.into();
        let row = AttachmentRow::from(&file);
        assert_eq!(row.name, if name == ".pdf" { name } else { "attachment" });
        assert_eq!(row.open_type, None);
    }
    file.file_name = "../../x\u{202e}.apk".into();
    file.media_type = "application/pdf".into();
    let row = AttachmentRow::from(&file);
    assert_eq!(row.name, "x.apk");
    assert_eq!(row.open_type, None);
    file.file_name = "../Guide.PDF".into();
    file.media_type = "application/vnd.android.package-archive".into();
    assert_eq!(
        AttachmentRow::from(&file).open_type.as_deref(),
        Some("application/pdf")
    );
}
