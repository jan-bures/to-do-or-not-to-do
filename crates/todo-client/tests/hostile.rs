//! The native transport against servers that misbehave: redirects that would carry secrets elsewhere,
//! and answers that stall. A raw TCP listener plays the server, so nothing here needs the network.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_client::native::{BuildError, HttpTransport, server_origin};
use todo_client::pairing::{self, PairError};
use todo_client::platform::{Request, StreamOpen, TransportError};
use todo_client::{EventStream, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What a fake server does with each request.
#[derive(Clone)]
enum Behaviour {
    /// Answers with these bytes and closes.
    Answer(String),
    /// Sends these bytes, then keeps the connection open without another byte.
    Stall(String),
}

/// A one-trick HTTP server on loopback that records every request it receives.
struct Fake {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    async fn start(behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let seen = seen.clone();
                let behaviour = behaviour.clone();
                tokio::spawn(async move { serve(stream, behaviour, seen).await });
            }
        });
        Self { port, requests }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(mut stream: TcpStream, behaviour: Behaviour, seen: Arc<Mutex<Vec<String>>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read the head, then as much body as Content-Length says.
    let body_start = loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..body_start]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < body_start + length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    seen.lock()
        .unwrap()
        .push(String::from_utf8_lossy(&buf).into_owned());
    match behaviour {
        Behaviour::Answer(bytes) => {
            let _ = stream.write_all(bytes.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        Behaviour::Stall(bytes) => {
            let _ = stream.write_all(bytes.as_bytes()).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
}

fn redirect(status: &str, location: &str) -> Behaviour {
    Behaviour::Answer(format!(
        "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ))
}

fn assert_redirect_refused(err: &PairError) {
    assert!(
        matches!(err, PairError::Transport(TransportError::Protocol(m)) if m.contains("redirect")),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_redirect_on_poll_never_carries_the_poll_token_elsewhere() {
    let elsewhere = Fake::start(Behaviour::Answer(
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into(),
    ))
    .await;
    for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
        let server = Fake::start(redirect(
            status,
            &format!("http://localhost:{}/steal", elsewhere.port),
        ))
        .await;
        let anon = HttpTransport::anonymous(&server.url()).unwrap();
        let err = pairing::poll(&anon, "s3cret-poll-token").await.unwrap_err();
        assert_redirect_refused(&err);
        // The configured server got the poll once; the other origin got nothing at all.
        assert_eq!(server.requests().len(), 1);
    }
    assert!(elsewhere.requests().is_empty());
}

#[tokio::test]
async fn a_redirect_to_another_origin_or_to_plain_http_is_an_error() {
    let elsewhere = Fake::start(Behaviour::Answer(
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into(),
    ))
    .await;
    let targets = [
        // Another origin on this machine (a different host and port).
        format!("http://localhost:{}/api/v1/pair/start", elsewhere.port),
        // A downgrade to plain http on another host: refused before any connection is tried.
        "http://todo.example.com/api/v1/pair/start".to_string(),
    ];
    for (status, target) in [
        ("301 Moved Permanently", &targets[0]),
        ("302 Found", &targets[0]),
        ("303 See Other", &targets[1]),
        ("307 Temporary Redirect", &targets[1]),
    ] {
        let server = Fake::start(redirect(status, target)).await;
        let anon = HttpTransport::anonymous(&server.url()).unwrap();
        assert_redirect_refused(&pairing::start(&anon, "Test desktop").await.unwrap_err());

        // An authenticated request: the bearer token isn't forwarded either.
        let authed = HttpTransport::new(&server.url(), "s3cret-device-token").unwrap();
        let err = authed
            .send(Request::get("/api/v1/sync?since=0"))
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)), "{err:?}");
        let opened = authed.open_stream(Request::get("/api/v1/events")).await;
        assert!(
            matches!(opened, Err(TransportError::Protocol(_))),
            "a redirected event stream is refused"
        );
    }
    assert!(elsewhere.requests().is_empty());
}

#[tokio::test]
async fn a_stalled_server_times_out() {
    let timeout = Duration::from_millis(300);
    // Accepts the request and never answers.
    let silent = Fake::start(Behaviour::Stall(String::new())).await;
    // Sends the headers and part of the body, then nothing.
    let midway = Fake::start(Behaviour::Stall(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"user"
            .into(),
    ))
    .await;
    for server in [&silent, &midway] {
        let anon = HttpTransport::anonymous(&server.url())
            .unwrap()
            .with_request_timeout(timeout);
        let started = std::time::Instant::now();
        let err = pairing::start(&anon, "Test desktop").await.unwrap_err();
        assert_eq!(err, PairError::Transport(TransportError::Timeout));
        let err = pairing::poll(&anon, "token").await.unwrap_err();
        assert_eq!(err, PairError::Transport(TransportError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    // An event stream's headers are waited for as long, but an open stream may stay quiet (the sync
    // engine's idle timeout watches it).
    let authed = HttpTransport::new(&silent.url(), "token")
        .unwrap()
        .with_request_timeout(timeout);
    assert!(matches!(
        authed.open_stream(Request::get("/api/v1/events")).await,
        Err(TransportError::Timeout)
    ));
    let quiet = Fake::start(Behaviour::Stall(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n".into(),
    ))
    .await;
    let authed = HttpTransport::new(&quiet.url(), "token")
        .unwrap()
        .with_request_timeout(timeout);
    let open = authed.open_stream(Request::get("/api/v1/events")).await;
    let Ok(StreamOpen::Open(mut stream)) = open else {
        panic!("the stream opens");
    };
    assert!(
        tokio::time::timeout(timeout * 3, stream.next_frame())
            .await
            .is_err(),
        "an open stream is not cut by the request timeout"
    );
}

#[test]
fn server_origins_are_checked_with_one_parser() {
    let ok = |s: &str| server_origin(s).unwrap();
    assert_eq!(ok("https://todo.example.com/"), "https://todo.example.com");
    assert_eq!(
        ok(" https://TODO.example.com:8443 "),
        "https://todo.example.com:8443"
    );
    assert_eq!(ok("http://localhost:3000/"), "http://localhost:3000");
    assert_eq!(ok("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
    assert_eq!(ok("http://[::1]:8080"), "http://[::1]:8080");

    let insecure = [
        "http://todo.example.com",
        "http://localhost.example.com",
        "http://10.0.0.1",
    ];
    for s in insecure {
        assert!(matches!(server_origin(s), Err(BuildError::Insecure)), "{s}");
    }
    let bad = [
        "",
        "todo.example.com",
        "https://",
        "ftp://todo.example.com",
        "file:///C:/Windows",
        "https://user:pass@todo.example.com",
        "https://user@todo.example.com",
        "https://todo.example.com/?next=x",
        "https://todo.example.com/#x",
        "https://todo example.com",
        // The classic confusion: the host here is evil.example, not todo.example.com.
        "https://todo.example.com@evil.example",
    ];
    for s in bad {
        assert!(
            matches!(server_origin(s), Err(BuildError::BadUrl(_))),
            "{s}"
        );
    }
    assert!(HttpTransport::anonymous("http://todo.example.com").is_err());
    assert!(HttpTransport::new("https://u:p@todo.example.com", "t").is_err());
}

#[tokio::test]
async fn a_request_path_can_never_leave_the_server() {
    let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
    let server = Fake::start(Behaviour::Answer(ok.into())).await;
    let elsewhere = Fake::start(Behaviour::Answer(ok.into())).await;
    let other = format!("localhost:{}", elsewhere.port);
    let authed = HttpTransport::new(&server.url(), "s3cret-device-token").unwrap();
    let hostile = [
        // Without a leading slash the path becomes part of the authority.
        format!("@{other}/api/v1/sync"),
        format!(".{other}/api/v1/sync"),
        format!(":{}/api/v1/sync", elsewhere.port),
        "api/v1/sync".to_string(),
        String::new(),
        // A network-path reference, a backslash (read as a slash) and control characters.
        format!("//{other}/api/v1/sync"),
        format!("/\\{other}/api/v1/sync"),
        "/api/v1/sync\r\nHost: evil.example".to_string(),
    ];
    for path in &hostile {
        let err = authed.send(Request::get(path.clone())).await.unwrap_err();
        assert!(
            matches!(err, TransportError::Protocol(_)),
            "{path:?}: {err:?}"
        );
        let opened = authed.open_stream(Request::get(path.clone())).await;
        assert!(
            matches!(opened, Err(TransportError::Protocol(_))),
            "{path:?}"
        );
    }
    assert!(server.requests().is_empty(), "nothing was sent at all");
    assert!(elsewhere.requests().is_empty());

    // A normal path still works, and stays under a base path prefix.
    authed.send(Request::get("/api/v1/sync")).await.unwrap();
    assert_eq!(server.requests().len(), 1);
    let prefixed =
        HttpTransport::new(&format!("{}/todo", server.url()), "s3cret-device-token").unwrap();
    prefixed.send(Request::get("/api/v1/sync")).await.unwrap();
    assert!(server.requests()[1].starts_with("GET /todo/api/v1/sync "));
}
