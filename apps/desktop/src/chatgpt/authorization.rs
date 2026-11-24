//! One callback on 127.0.0.1. It never receives the server's PKCE verifier or tokens.

use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
use subtle::ConstantTimeEq;
use todo_client::assistant::{native_relay::Relay, relay};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::Error;

const PATH: &str = "/auth/callback";
const LIMIT: usize = 12 * 1024;
const DEADLINE: Duration = Duration::from_secs(180);

pub struct Attempt {
    listener: TcpListener,
    port: u16,
    id: Uuid,
    state: Zeroizing<String>,
}

impl Attempt {
    pub async fn start(relay: &Relay) -> Result<(Self, Zeroizing<String>), Error> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::Callback)?;
        let port = listener.local_addr().map_err(|_| Error::Callback)?.port();
        let relay::SignInStarted::Loopback {
            attempt,
            authorize_url,
        } = relay
            .start(relay::SignIn::Loopback { port })
            .await
            .map_err(|_| Error::Network)?
        else {
            return Err(Error::Callback);
        };
        let url = Url::parse(&authorize_url).map_err(|_| Error::Callback)?;
        if url.scheme() != "https"
            || url.host_str() != Some("auth.openai.com")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/api/accounts/authorize"
            || url.fragment().is_some()
        {
            return Err(Error::Callback);
        }
        let mut state = None;
        let mut redirect = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "state" if state.is_none() => state = Some(Zeroizing::new(value.into_owned())),
                "redirect_uri" if redirect.is_none() => redirect = Some(value.into_owned()),
                "state" | "redirect_uri" => return Err(Error::Callback),
                _ => {}
            }
        }
        if redirect.as_deref() != Some(&format!("http://127.0.0.1:{port}{PATH}")) {
            return Err(Error::Callback);
        }
        let state = state
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or(Error::Callback)?;
        Ok((
            Self {
                listener,
                port,
                id: attempt,
                state,
            },
            Zeroizing::new(authorize_url),
        ))
    }

    pub async fn finish(self, relay: &Relay) -> Result<relay::Account, Error> {
        let input = tokio::time::timeout(DEADLINE, async {
            loop {
                let (mut stream, peer) = self.listener.accept().await.map_err(|_| Error::Callback)?;
                if !peer.ip().is_loopback() { continue; }
                let Ok(Ok(request)) = tokio::time::timeout(Duration::from_secs(2), request(&mut stream)).await else { continue; };
                let parsed = decode(&request, self.port, &self.state);
                let (status, text) = match &parsed {
                    Ok(_) => ("200 OK", "Authorization received. Return to To-do to finish signing in."),
                    Err(Error::Declined) => ("200 OK", "Sign-in was canceled. You can close this tab."),
                    _ => ("400 Bad Request", "Not a valid sign-in callback."),
                };
                let answer = format!("HTTP/1.1 {status}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{text}", text.len());
                let _ = tokio::time::timeout(Duration::from_secs(1), stream.write_all(answer.as_bytes())).await;
                if !matches!(parsed, Err(Error::Callback)) { return parsed; }
            }
        }).await.map_err(|_| Error::TimedOut)??;
        relay
            .handoff(self.id, input)
            .await
            .map_err(|_| Error::Network)
    }
}

async fn request(stream: &mut TcpStream) -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let n = stream
            .read(&mut *chunk)
            .await
            .map_err(|_| Error::Callback)?;
        if n == 0 || bytes.len() + n > LIMIT {
            return Err(Error::Callback);
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            return Ok(Zeroizing::new(
                String::from_utf8(bytes[..end].to_vec()).map_err(|_| Error::Callback)?,
            ));
        }
    }
}

fn decode(request: &str, port: u16, state: &str) -> Result<relay::Handoff, Error> {
    let host = format!("127.0.0.1:{port}");
    let mut lines = request.split("\r\n");
    let mut start = lines.next().ok_or(Error::Callback)?.split(' ');
    if start.next() != Some("GET") {
        return Err(Error::Callback);
    }
    let target = start.next().ok_or(Error::Callback)?;
    if !matches!(start.next(), Some("HTTP/1.0" | "HTTP/1.1")) || start.next().is_some() {
        return Err(Error::Callback);
    }
    let mut hosts = lines.filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("Host").then_some(value.trim())
    });
    if hosts.next() != Some(host.as_str()) || hosts.next().is_some() {
        return Err(Error::Callback);
    }
    let (path, query) = target.split_once('?').ok_or(Error::Callback)?;
    if path != PATH || query.contains('#') {
        return Err(Error::Callback);
    }
    let mut params = BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if matches!(key.as_ref(), "state" | "error" | "code" | "client_id")
            && params
                .insert(key, Zeroizing::new(value.into_owned()))
                .is_some()
        {
            return Err(Error::Callback);
        }
    }
    if !params
        .get("state")
        .is_some_and(|v| bool::from(state.as_bytes().ct_eq(v.as_bytes())))
    {
        return Err(Error::Callback);
    }
    if params.contains_key("error") {
        return Err(Error::Declined);
    }
    let get = |name| {
        params
            .get(name)
            .filter(|s| !s.is_empty() && s.len() <= 4096 && s.is_ascii())
            .map(|s| s.to_string())
            .ok_or(Error::Callback)
    };
    Ok(relay::Handoff {
        state: state.to_owned(),
        code: get("code")?,
        client_id: get("client_id")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_requires_exact_host_and_oauth_state() {
        let request = |host, query| format!("GET /auth/callback?{query} HTTP/1.1\r\nHost: {host}");
        let valid = "code=sample&state=expected&client_id=fixture-client";
        assert_eq!(
            decode(&request("127.0.0.1:1455", valid), 1455, "expected")
                .expect("valid callback")
                .code,
            "sample"
        );
        assert!(decode(&request("localhost:1455", valid), 1455, "expected").is_err());
        assert!(decode(&request("127.0.0.1:1455", valid), 1455, "different").is_err());
        assert!(
            decode(
                &request("127.0.0.1:1455", &format!("{valid}&state=expected")),
                1455,
                "expected"
            )
            .is_err()
        );
    }
}
