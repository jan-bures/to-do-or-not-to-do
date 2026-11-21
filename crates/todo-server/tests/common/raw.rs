//! Raw-socket helpers, for requests a normal HTTP client won't send (a lying `Content-Length`, half a
//! request head) and for connection limits that a client's connection pool would get in the way of.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Reads until EOF (or a reset), returning what arrived. Panics if that takes longer than `within`.
pub async fn read_until_closed(sock: &mut TcpStream, within: Duration) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    tokio::time::timeout(within, async {
        loop {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    .expect("the server did not close the connection");
    out
}

/// Sends `GET /health` and returns the status line.
pub async fn health(sock: &mut TcpStream) -> String {
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .expect("no answer")
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A raw response: status code, head (status line and headers) and body.
#[derive(Debug)]
pub struct RawResponse {
    pub status: u16,
    pub head: String,
    pub body: String,
}

/// Sends `head` (a complete request head, ending in an empty line) and then `body`, on a connection
/// that is closed after one response, and reads the response.
pub async fn send(addr: SocketAddr, head: &str, body: &[u8]) -> RawResponse {
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(head.as_bytes()).await.unwrap();
    // The server may answer (and close) before it has read the body; that's what some tests check.
    let _ = sock.write_all(body).await;
    let buf = read_until_closed(&mut sock, Duration::from_secs(10)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    RawResponse {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

/// Sends only a request head that announces `len` body bytes, plus `extra` header lines. The server
/// must answer without waiting for (or reading) the body.
pub async fn oversized(
    addr: SocketAddr,
    method: &str,
    path: &str,
    len: usize,
    extra: &[(&str, &str)],
) -> RawResponse {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n"
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    send(addr, &head, b"").await
}

/// Sends `len` body bytes with chunked transfer encoding (no `Content-Length` to check up front).
pub async fn chunked(
    addr: SocketAddr,
    method: &str,
    path: &str,
    len: usize,
    extra: &[(&str, &str)],
) -> RawResponse {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Transfer-Encoding: chunked\r\nConnection: close\r\n"
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut body = Vec::new();
    let chunk = vec![b' '; 1024];
    let mut left = len;
    while left > 0 {
        let n = left.min(chunk.len());
        body.extend_from_slice(format!("{n:x}\r\n").as_bytes());
        body.extend_from_slice(&chunk[..n]);
        body.extend_from_slice(b"\r\n");
        left -= n;
    }
    body.extend_from_slice(b"0\r\n\r\n");
    send(addr, &head, &body).await
}
