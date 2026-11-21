#![allow(clippy::unwrap_used)]
//! The access log names route templates, never raw paths or query strings, and keeps traffic that
//! fails authentication (or matches no route) out of `info`.
//!
//! A separate test binary, because it installs a capturing `tracing` subscriber.

mod common;

use common::{TOKEN_A, TestServer};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

#[tokio::test]
async fn access_log_uses_route_templates_and_keeps_rejections_at_debug() {
    let info = Capture::default();
    let debug = Capture::default();
    let subscriber = {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(info.clone())
                    .with_filter(tracing_subscriber::filter::LevelFilter::INFO),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(debug.clone())
                    .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
            )
    };
    let _guard = tracing::subscriber::set_default(subscriber);

    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    // Authenticated traffic: logged at info, by template.
    t.get_ok(&format!(
        "/api/v1/lists/{list}/tasks?done=false&secret-query"
    ))
    .await;
    // Unauthenticated traffic and unknown paths: debug only.
    let (s, _, _) = t
        .request(
            reqwest::Method::GET,
            "/api/v1/tasks?query=unauth-secret",
            None,
            None,
        )
        .await;
    assert_eq!(s, reqwest::StatusCode::UNAUTHORIZED);
    let (s, _, _) = t
        .request(
            reqwest::Method::GET,
            "/probe-unmatched-secret?x=1",
            Some(TOKEN_A),
            None,
        )
        .await;
    assert_eq!(s, reqwest::StatusCode::NOT_FOUND);
    t.stop().await;

    let info = info.text();
    let debug = debug.text();
    assert!(
        info.contains("route=/api/v1/lists/{id}/tasks"),
        "info log:\n{info}"
    );
    assert!(info.contains("status=200"), "info log:\n{info}");
    for secret in [
        list.as_str(),
        "secret-query",
        "unauth-secret",
        "probe-unmatched-secret",
        "rejected request",
    ] {
        assert!(!info.contains(secret), "`{secret}` in info log:\n{info}");
    }
    assert!(
        debug.contains("route=/api/v1/tasks status=401"),
        "debug log:\n{debug}"
    );
    assert!(
        debug.contains("route=unmatched status=404"),
        "debug log:\n{debug}"
    );
    for secret in ["unauth-secret", "probe-unmatched-secret", "secret-query"] {
        assert!(!debug.contains(secret), "`{secret}` in debug log:\n{debug}");
    }
}
