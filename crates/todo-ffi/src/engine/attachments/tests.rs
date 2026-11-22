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
                            r#"{"error":{"details":{"max_bytes":2}}}"#,
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
        .download_attachment(file.id.to_string(), path.clone())
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    std::fs::write(&path, [0xff, 0, 5]).unwrap();
    engine
        .upload_attachment(
            file.task.to_string(),
            "čaj & +.png".into(),
            "image/png".into(),
            path.clone(),
        )
        .unwrap();
    let refused = engine
        .upload_attachment(
            file.task.to_string(),
            "smaller-limit.png".into(),
            "image/png".into(),
            path.clone(),
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
        )
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "That file is too large. The limit is 25 MiB."
    );
    assert_eq!(engine.status().pending, 0);
    server.abort();
}

#[test]
fn open_uses_a_safe_leaf_and_never_dispatches_a_supplied_mime_type() {
    let state = fixtures::build(
        Fixture::World,
        &Now::new(LocalNow {
            epoch_ms: 1_800_000_000_000,
            utc_offset_secs: 0,
        }),
    );
    let mut file = state.attachments.values().next().unwrap().clone();
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
