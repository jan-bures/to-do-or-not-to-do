use super::*;
use axum::{Json, Router, body::Bytes, routing::get};
use reqwest::dns::Resolve;
use tokio::{net::TcpListener, sync::Semaphore};

#[tokio::test]
async fn checked_dns_refuses_a_non_tailnet_answer() {
    let name = || "localhost".parse::<dns::Name>().unwrap();
    assert!(CheckedDns(Network::Loopback).resolve(name()).await.is_ok());
    assert!(CheckedDns(Network::Tailnet).resolve(name()).await.is_err());
}

#[tokio::test]
async fn connector_refuses_redirects_and_relay_caps_chunked_or_known_length_streams() {
    fn oversized_body() -> Body {
        Body::from_stream(stream::unfold(0, |index| async move {
            (index < 18 * 16).then(|| {
                (
                    Ok::<_, std::io::Error>(Bytes::from_static(&[b'x'; 64 * 1024])),
                    index + 1,
                )
            })
        }))
    }
    let mock = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [(header::LOCATION, "/unexpected")],
                )
            }),
        )
        .route(
            "/unexpected",
            get(|| async { Json(serde_json::json!({"data":[{"id":"not-allowed"}]})) }),
        )
        .route(
            "/stream",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    oversized_body(),
                )
            }),
        )
        .route(
            "/length",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/event-stream"),
                        (header::CONTENT_LENGTH, "18874368"),
                    ],
                    oversized_body(),
                )
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("model.secret");
    use std::io::Write;
    todo_fs::create_private_file(&key_file)
        .unwrap()
        .write_all(b"fixture-model-key")
        .unwrap();
    let gateway = Private::open(&PrivateConfig {
        base_url: base.clone(),
        key_file,
    })
    .unwrap();
    assert!(matches!(gateway.models().await, Err(Error::Upstream)));
    let http = client().build().unwrap();
    for (private, limit) in [(false, MAX_STREAM), (true, PRIVATE_MAX_STREAM)] {
        let sem = Arc::new(Semaphore::new(1));
        let response = http.get(format!("{base}/length")).send().await.unwrap();
        assert!(matches!(
            relay(
                response,
                sem.clone().acquire_owned().await.unwrap(),
                private
            ),
            Err(Error::Upstream)
        ));
        let response = http.get(format!("{base}/stream")).send().await.unwrap();
        assert!(response.content_length().is_none());
        let mut body = relay(
            response,
            sem.clone().acquire_owned().await.unwrap(),
            private,
        )
        .unwrap()
        .into_body()
        .into_data_stream();
        use futures_util::StreamExt;
        let mut received = 0;
        loop {
            match body.next().await {
                Some(Ok(chunk)) => received += chunk.len(),
                Some(Err(_)) => break,
                None => panic!("oversized stream was not interrupted"),
            }
        }
        assert!(
            (limit - 64 * 1024..=limit).contains(&received),
            "{received} vs {limit}"
        );
        assert_eq!(sem.available_permits(), 1);
    }
    server.abort();
}

#[tokio::test]
async fn model_catalogs_may_exceed_the_general_json_bound() {
    fn catalog(bytes: usize) -> serde_json::Value {
        let note = "x".repeat(4096);
        let models = (0..bytes / 4096)
            .map(|i| serde_json::json!({"id": format!("model-{i}"), "note": note}))
            .collect::<Vec<_>>();
        serde_json::json!({ "data": models })
    }
    let mock = Router::new()
        .route("/large", get(|| async { Json(catalog(1024 * 1024)) }))
        .route("/huge", get(|| async { Json(catalog(9 * 1024 * 1024)) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let http = client().build().unwrap();
    let get = |path: &str| http.get(format!("{base}/{path}")).send();

    let listed: serde_json::Value = read_catalog(get("large").await.unwrap()).await.unwrap();
    assert_eq!(listed["data"].as_array().unwrap().len(), 256);
    let general = read_json::<serde_json::Value>(get("large").await.unwrap()).await;
    assert!(matches!(general, Err(Error::Upstream)));
    let huge = read_catalog::<serde_json::Value>(get("huge").await.unwrap()).await;
    assert!(matches!(huge, Err(Error::Upstream)));
    server.abort();
}

#[test]
fn private_addresses_are_exact_and_have_fixed_paths() {
    for url in [
        "http://localhost:8030",
        "http://127.0.0.1",
        "https://[::1]",
        "http://100.64.0.0",
        "http://100.127.255.255",
        "http://[fd7a:115c:a1e0::ffff]",
        "https://model.example.ts.net/v1/",
    ] {
        assert_eq!(base_address(url).unwrap().0.path(), "/v1/", "{url}");
    }
    for url in [
        "https://example.com",
        "http://100.63.255.255",
        "http://100.128.0.0",
        "http://[fd7a:115c:a1e1::1]",
        "http://169.254.169.254",
        "http://192.168.1.2",
        "http://example.ts.net.example.com",
        "http://localhost?x=1",
        "http://localhost#x",
        "https://user@example.ts.net",
        "ftp://localhost",
        "https://[::ffff:127.0.0.1]",
    ] {
        assert!(base_address(url).is_none(), "{url}");
    }
}
