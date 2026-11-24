//! One callback on 127.0.0.1. It never receives the server's PKCE verifier or tokens.

use super::{native_relay::Relay, relay};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::BTreeMap,
    io,
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Couldn't reach your To-do server or ChatGPT.")]
    Network,
    #[error("Couldn't open or verify the local sign-in callback.")]
    Callback,
    #[error("Sign-in timed out. Start a new sign-in to try again.")]
    TimedOut,
    #[error("ChatGPT sign-in was canceled or declined.")]
    Declined,
    #[error("ChatGPT didn't return the authorization details. Start a new sign-in to try again.")]
    MissingAuthorization,
}

const PATH: &str = "/auth/callback";
const LIMIT: usize = 12 * 1024;
const DEADLINE: Duration = Duration::from_secs(600);
const READ_LIMIT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 16;

pub struct Attempt {
    listener: TcpListener,
    port: u16,
    id: Uuid,
    state: Arc<Zeroizing<String>>,
}

impl std::fmt::Debug for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attempt").finish_non_exhaustive()
    }
}

impl Attempt {
    pub async fn start(relay: &Relay) -> Result<(Self, Zeroizing<String>), Error> {
        let listener = bind_callback().map_err(|_| Error::Callback)?;
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
                state: Arc::new(state),
            },
            Zeroizing::new(authorize_url),
        ))
    }

    pub async fn finish(self, relay: &Relay) -> Result<relay::Account, Error> {
        let input = tokio::time::timeout(DEADLINE, self.wait_for_callback())
            .await
            .map_err(|_| Error::TimedOut)??;
        relay
            .handoff(self.id, input)
            .await
            .map_err(|_| Error::Network)
    }

    async fn wait_for_callback(&self) -> Result<relay::Handoff, Error> {
        // Dropping the attempt (completion, timeout, or Cancel) also drops all connection tasks.
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                result = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Ok(Some(result))) = result {
                        return result;
                    }
                }
                connected = self.listener.accept() => {
                    let (stream, peer) = connected.map_err(|_| Error::Callback)?;
                    if peer.ip().is_loopback() && tasks.len() < MAX_CONNECTIONS {
                        tasks.spawn(handle_connection(stream, self.port, self.state.clone()));
                    }
                }
            }
        }
    }
}

fn bind_callback() -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(windows)]
    exclusive_address(&socket)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())?;
    socket.listen(MAX_CONNECTIONS as i32)?;
    socket.set_nonblocking(true)?;
    TcpListener::from_std(socket.into())
}

#[cfg(windows)]
#[allow(unsafe_code)] // socket2 doesn't expose this Windows-specific option.
fn exclusive_address(socket: &Socket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET_ERROR, SOL_SOCKET, WSAGetLastError, setsockopt,
    };

    let enabled = 1i32;
    // SAFETY: socket owns a valid Winsock handle; the option is a pointer to a live Win32 BOOL
    // of the size passed. This occurs before bind, as required by SO_EXCLUSIVEADDRUSE.
    if unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw const enabled).cast(),
            size_of::<i32>() as i32,
        )
    } == SOCKET_ERROR
    {
        // SAFETY: WSAGetLastError reads thread-local Winsock state immediately after failure.
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    port: u16,
    state: Arc<Zeroizing<String>>,
) -> Option<Result<relay::Handoff, Error>> {
    let request = tokio::time::timeout(READ_LIMIT, request(&mut stream))
        .await
        .ok()?
        .ok()?;
    let parsed = decode(&request, port, &state);
    let (status, text) = match &parsed {
        Ok(_) => (
            "200 OK",
            "Authorization received. Return to To-do to finish signing in.",
        ),
        Err(Error::Declined) => (
            "200 OK",
            "Sign-in was canceled or declined. You can close this tab.",
        ),
        Err(Error::MissingAuthorization) => (
            "400 Bad Request",
            "ChatGPT didn’t return the authorization details. Return to To-do to try again.",
        ),
        _ => ("400 Bad Request", "Not a valid sign-in callback."),
    };
    let answer = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{text}",
        text.len()
    );
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.write_all(answer.as_bytes())).await;
    (!matches!(parsed, Err(Error::Callback))).then_some(parsed)
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
            .ok_or(Error::MissingAuthorization)
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

    fn local_attempt() -> Attempt {
        let listener = bind_callback().expect("bind test listener");
        let port = listener.local_addr().expect("loopback address").port();
        Attempt {
            listener,
            port,
            id: Uuid::nil(),
            state: Arc::new(Zeroizing::new("expected".into())),
        }
    }

    async fn send_callback(port: u16, query: &str) {
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("callback socket");
        client
            .write_all(
                format!("GET {PATH}?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
            )
            .await
            .expect("send callback");
        let mut answer = Vec::new();
        client
            .read_to_end(&mut answer)
            .await
            .expect("read callback response");
        assert!(answer.starts_with(b"HTTP/1.1 "));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn callback_port_cannot_be_rebound_with_reuse_address() {
        let attempt = local_attempt();
        let other =
            Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).expect("test socket");
        other.set_reuse_address(true).expect("enable reuse");
        assert!(
            other
                .bind(
                    &attempt
                        .listener
                        .local_addr()
                        .expect("callback address")
                        .into()
                )
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_stalled_connection_does_not_block_the_callback() {
        let attempt = local_attempt();
        let port = attempt.port;
        let mut stalled = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("stall socket");
        stalled
            .write_all(b"GET /auth/callback?")
            .await
            .expect("partial request");
        let sign_in = tokio::spawn(async move { attempt.wait_for_callback().await });
        tokio::time::timeout(READ_LIMIT / 2, async {
            send_callback(port, "state=expected&code=sample&client_id=fixture-client").await;
            assert_eq!(
                sign_in
                    .await
                    .expect("callback task")
                    .expect("valid callback")
                    .code,
                "sample"
            );
        })
        .await
        .expect("valid callback finishes before stalled read expires");
    }

    #[tokio::test]
    async fn a_matching_error_or_missing_code_ends_the_attempt() {
        for (query, expected) in [
            (
                "state=expected&client_id=fixture-client",
                Error::MissingAuthorization,
            ),
            (
                "state=expected&code=&client_id=fixture-client",
                Error::MissingAuthorization,
            ),
            ("state=expected&error=", Error::Declined),
        ] {
            let attempt = local_attempt();
            let port = attempt.port;
            let sign_in = tokio::spawn(async move { attempt.wait_for_callback().await });
            tokio::time::timeout(READ_LIMIT, async {
                send_callback(port, "state=wrong&error=").await;
                assert!(
                    !sign_in.is_finished(),
                    "a foreign state must not terminate sign-in"
                );
                send_callback(port, query).await;
                assert_eq!(
                    sign_in
                        .await
                        .expect("callback task")
                        .expect_err("matching OAuth state"),
                    expected
                );
            })
            .await
            .expect("matching callback terminates immediately");
        }
    }

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
