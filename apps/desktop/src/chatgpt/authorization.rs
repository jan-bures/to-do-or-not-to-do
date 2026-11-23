//! The public SIWC desktop OAuth flow. The listener and its verifier belong to one attempt.

use super::{Error, ISSUER, RESOURCE, Service};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
    signature,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddrV4},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::Instant,
};
use url::Url;
use zeroize::Zeroizing;

const ENTRY: &str = "dynamic_agent_client";
const PATH: &str = "/auth/callback";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DEADLINE: Duration = Duration::from_secs(180);
const HTTP_LIMIT: usize = 12 * 1024;

pub struct Attempt {
    tcp: TcpListener,
    redirect: Url,
    expires: Instant,
    state: Zeroizing<String>,
    nonce: Zeroizing<String>,
    verifier: Zeroizing<String>,
    client: Option<String>,
}

pub(super) struct Grant {
    pub client: String,
    pub code: Zeroizing<String>,
    pub redirect: Url,
    pub verifier: Zeroizing<String>,
    pub nonce: Zeroizing<String>,
}

fn secret() -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(&mut *bytes)
        .map_err(|_| Error::Protocol)?;
    Ok(Zeroizing::new(B64.encode(*bytes)))
}

fn challenge(verifier: &str) -> String {
    B64.encode(digest::digest(&digest::SHA256, verifier.as_bytes()).as_ref())
}

impl Attempt {
    pub(super) async fn prepare(
        host: uuid::Uuid,
        client: Option<&str>,
        email: Option<&str>,
        hint: Option<&str>,
        reconsent: bool,
    ) -> Result<(Self, Zeroizing<String>), Error> {
        let tcp = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::Callback)?;
        let port = tcp.local_addr().map_err(|_| Error::Callback)?.port();
        let redirect =
            Url::parse(&format!("http://127.0.0.1:{port}{PATH}")).map_err(|_| Error::Protocol)?;
        let state = secret()?;
        let nonce = secret()?;
        let verifier = secret()?;
        let mut browser =
            Url::parse(&format!("{ISSUER}/api/accounts/authorize")).map_err(|_| Error::Protocol)?;
        {
            let mut params = browser.query_pairs_mut();
            params.append_pair("client_id", client.unwrap_or(ENTRY));
            params.append_pair("ext_agent_host_id", &host.urn().to_string());
            if client.is_none() {
                params.append_pair("agent_name_hint", "To-do or not to do");
            } else {
                if let Some(email) = email {
                    params.append_pair("login_hint", email);
                }
                if let Some(hint) = hint {
                    params.append_pair("id_token_hint", hint);
                }
                if reconsent {
                    params.append_pair("prompt", "consent");
                }
            }
            params.extend_pairs([
                ("response_type", "code"),
                ("redirect_uri", redirect.as_str()),
                ("scope", SCOPES),
                ("resource", RESOURCE),
                ("state", state.as_str()),
                ("nonce", nonce.as_str()),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge(&verifier)),
            ]);
        }
        Ok((
            Self {
                tcp,
                redirect,
                expires: Instant::now() + DEADLINE,
                state,
                nonce,
                verifier,
                client: client.map(ToOwned::to_owned),
            },
            Zeroizing::new(browser.into()),
        ))
    }

    pub(super) async fn receive(self) -> Result<Grant, Error> {
        let (code, client) = tokio::time::timeout_at(self.expires, async {
            loop {
                let (mut stream, peer) = self.tcp.accept().await.map_err(|_| Error::Callback)?;
                if !peer.ip().is_loopback() { continue; }
                let Ok(Ok(request)) = tokio::time::timeout(Duration::from_secs(2), request(&mut stream)).await else { continue; };
                let parsed = decode(&request, &self.redirect, &self.state, self.client.as_deref());
                let (status, text) = match &parsed {
                    Ok(_) => ("200 OK", "Authorization received. Return to To-do or not to do to finish signing in."),
                    Err(Error::Declined) => ("200 OK", "Sign-in was canceled. You can close this tab."),
                    Err(_) => ("400 Bad Request", "Not a valid sign-in callback."),
                };
                let _ = tokio::time::timeout(Duration::from_secs(1), stream.write_all(format!(
                    "HTTP/1.1 {status}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{text}", text.len(),
                ).as_bytes())).await;
                match parsed {
                    Err(Error::Callback) => continue,
                    _ => return parsed,
                }
            }
        }).await.map_err(|_| Error::TimedOut)??;
        Ok(Grant {
            client,
            code,
            redirect: self.redirect,
            verifier: self.verifier,
            nonce: self.nonce,
        })
    }
}

async fn request(stream: &mut TcpStream) -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut data = Zeroizing::new([0u8; 1024]);
    loop {
        let read = stream.read(&mut *data).await.map_err(|_| Error::Callback)?;
        if read == 0 || bytes.len() + read > HTTP_LIMIT {
            return Err(Error::Callback);
        }
        bytes.extend_from_slice(&data[..read]);
        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            return Ok(Zeroizing::new(
                String::from_utf8(bytes[..end].to_vec()).map_err(|_| Error::Callback)?,
            ));
        }
    }
}

fn client_id(id: &str) -> bool {
    !id.is_empty()
        && id != ENTRY
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn decode(
    request: &str,
    redirect: &Url,
    state: &str,
    expected_client: Option<&str>,
) -> Result<(Zeroizing<String>, String), Error> {
    let origin = format!("127.0.0.1:{}", redirect.port().ok_or(Error::Callback)?);
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
    if hosts.next() != Some(origin.as_str()) || hosts.next().is_some() {
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
        .is_some_and(|value| bool::from(value.as_bytes().ct_eq(state.as_bytes())))
    {
        return Err(Error::Callback);
    }
    if let Some(error) = params.get("error") {
        return Err(super::oauth_code(Some(error.as_str())));
    }
    let supplied = params.get("client_id").map(|id| id.as_str());
    let client = match (expected_client, supplied) {
        (Some(expected), Some(actual)) if expected != actual => return Err(Error::Protocol),
        (Some(expected), _) => expected,
        (None, Some(actual)) => actual,
        _ => return Err(Error::Protocol),
    };
    if !client_id(client) {
        return Err(Error::Protocol);
    }
    let client = client.to_owned();
    let code = params
        .remove("code")
        .filter(|code| !code.is_empty() && code.len() <= 4096 && code.is_ascii())
        .ok_or(Error::Protocol)?;
    Ok((code, client))
}

#[derive(Deserialize)]
struct Keys {
    keys: Vec<Jwk>,
}
#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    n: Option<String>,
    e: Option<String>,
}
#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Multiple(Vec<String>),
}
#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    azp: Option<String>,
    exp: u64,
    nbf: Option<u64>,
    nonce: String,
    email: Option<String>,
    name: Option<String>,
}

pub(super) struct Identity {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
}

impl Service {
    pub(super) async fn verify_id(
        &self,
        jwt: &str,
        client: &str,
        nonce: &str,
    ) -> Result<Identity, Error> {
        let (signed, raw_signature) = jwt.rsplit_once('.').ok_or(Error::Identity)?;
        let (header, claims) = signed.split_once('.').ok_or(Error::Identity)?;
        let header: Header = json_segment(header)?;
        if header.alg != "RS256" || header.kid.is_empty() {
            return Err(Error::Identity);
        }
        let keys: Keys = self
            .json(self.http.get(format!("{ISSUER}/.well-known/jwks.json")))
            .await?;
        let mut matching = keys.keys.iter().filter(|key| {
            key.kid == header.kid
                && key.kty == "RSA"
                && key.alg.as_deref().is_none_or(|a| a == "RS256")
                && key.usage.as_deref().is_none_or(|u| u == "sig")
        });
        let key = matching.next().ok_or(Error::Identity)?;
        if matching.next().is_some() {
            return Err(Error::Identity);
        }
        let n = B64
            .decode(key.n.as_ref().ok_or(Error::Identity)?)
            .map_err(|_| Error::Identity)?;
        let e = B64
            .decode(key.e.as_ref().ok_or(Error::Identity)?)
            .map_err(|_| Error::Identity)?;
        let sig = B64.decode(raw_signature).map_err(|_| Error::Identity)?;
        signature::RsaPublicKeyComponents { n, e }
            .verify(
                &signature::RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &sig,
            )
            .map_err(|_| Error::Identity)?;
        let claims: Claims = json_segment(claims)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Identity)?
            .as_secs();
        validate_identity(claims, client, nonce, now)
    }
}

fn validate_identity(
    claims: Claims,
    client: &str,
    nonce: &str,
    now: u64,
) -> Result<Identity, Error> {
    let audience_ok = match &claims.aud {
        Audience::One(aud) => aud == client,
        Audience::Multiple(aud) => aud.as_slice() == [client],
    };
    if claims.iss != ISSUER
        || claims.exp <= now
        || claims.nbf.is_some_and(|t| t > now)
        || !audience_ok
        || claims.azp.is_some_and(|azp| azp != client)
        || claims.sub.is_empty()
        || !bool::from(claims.nonce.as_bytes().ct_eq(nonce.as_bytes()))
    {
        return Err(Error::Identity);
    }
    Ok(Identity {
        sub: claims.sub,
        email: claims.email,
        name: claims.name,
    })
}

fn json_segment<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, Error> {
    let decoded = Zeroizing::new(B64.decode(raw).map_err(|_| Error::Identity)?);
    serde_json::from_slice(&decoded).map_err(|_| Error::Identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "live unauthenticated OpenAI authorize probe; does not follow the login redirect"]
    async fn live_authorize_redirects_a_fresh_host_to_login() {
        let (_attempt, browser) = Attempt::prepare(uuid::Uuid::new_v4(), None, None, None, false)
            .await
            .expect("local listener");
        let service = Service::new().expect("no-redirect HTTPS client");
        let response = service
            .http
            .get(browser.as_str())
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .expect("authorize request failed");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| response.url().join(value).ok())
            .expect("valid redirect Location");
        assert_eq!(
            location.origin(),
            Url::parse(ISSUER).expect("issuer").origin()
        );
        assert_eq!(location.path(), "/log-in");
        println!("HTTP 302 to https://auth.openai.com/log-in (not followed)");
    }

    #[tokio::test]
    async fn only_a_matching_callback_can_consume_the_loopback_listener() {
        let host =
            uuid::Uuid::parse_str("12345678-1234-4123-8123-123456789abc").expect("fixture UUID");
        let (attempt, browser) = Attempt::prepare(host, None, None, None, false)
            .await
            .expect("local listener");
        assert_eq!(
            attempt.tcp.local_addr().expect("bound socket").ip(),
            Ipv4Addr::LOCALHOST
        );
        let parsed = Url::parse(&browser).expect("authorization URL");
        let params: BTreeMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], challenge(&attempt.verifier));
        assert_eq!(params["redirect_uri"], attempt.redirect.as_str());
        assert_eq!(params["scope"], SCOPES);
        assert_eq!(params["resource"], RESOURCE);
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["client_id"], ENTRY);
        assert_eq!(params["agent_name_hint"], "To-do or not to do");
        assert_eq!(
            params["ext_agent_host_id"],
            "urn:uuid:12345678-1234-4123-8123-123456789abc"
        );
        assert!(
            parsed
                .query()
                .expect("query")
                .contains("ext_agent_host_id=urn%3Auuid%3A12345678-1234-4123-8123-123456789abc")
        );
        assert_eq!(params["state"], attempt.state.as_str());
        assert_eq!(params["nonce"], attempt.nonce.as_str());
        assert!(!params.contains_key("prompt"));
        let port = attempt.tcp.local_addr().expect("bound socket").port();
        let verifier = attempt.verifier.clone();
        let state = attempt.state.clone();
        let work = tokio::spawn(attempt.receive());
        let mut callback = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect to fixture listener");
        callback.write_all(format!("GET /auth/callback?state={}&code=sample&client_id=oaiapp_fixture HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n", state.as_str()).as_bytes()).await.expect("write callback");
        let grant = work
            .await
            .expect("join callback")
            .expect("accept callback once");
        assert_eq!(grant.verifier.as_str(), verifier.as_str());
        assert_eq!(grant.code.as_str(), "sample");
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
        let (mut expired, _) = Attempt::prepare(host, None, None, None, false)
            .await
            .expect("local listener");
        expired.expires = Instant::now();
        assert!(matches!(expired.receive().await, Err(Error::TimedOut)));
    }

    #[test]
    fn signed_claims_are_bound_to_this_attempt_and_registration() {
        let claims = || {
            serde_json::from_value(serde_json::json!({
            "iss": ISSUER, "sub": "sample-subject", "aud": "oaiapp_fixture", "exp": 200, "nbf": 100,
            "nonce": "fresh-nonce", "email": "you@example.com",
        })).expect("sample identity claims")
        };
        assert!(validate_identity(claims(), "oaiapp_fixture", "fresh-nonce", 101).is_ok());
        assert!(matches!(
            validate_identity(claims(), "oaiapp_other", "fresh-nonce", 101),
            Err(Error::Identity)
        ));
        assert!(matches!(
            validate_identity(claims(), "oaiapp_fixture", "old-nonce", 101),
            Err(Error::Identity)
        ));
        assert!(matches!(
            validate_identity(claims(), "oaiapp_fixture", "fresh-nonce", 200),
            Err(Error::Identity)
        ));
    }

    #[test]
    fn pkce_rfc_vector_and_redirect_validation() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let callback = Url::parse("http://127.0.0.1:1455/auth/callback").expect("fixture callback");
        let request = |query: &str, host: &str| {
            format!("GET /auth/callback?{query} HTTP/1.1\r\nHost: {host}")
        };
        let query = "state=sample&code=one&client_id=oaiapp_fixture";
        assert!(decode(&request(query, "127.0.0.1:1455"), &callback, "sample", None).is_ok());
        let returning = request("state=sample&code=one", "127.0.0.1:1455");
        assert!(matches!(
            decode(&returning, &callback, "sample", None),
            Err(Error::Protocol)
        ));
        assert_eq!(
            decode(&returning, &callback, "sample", Some("oaiapp_fixture"))
                .expect("reuse issued client ID")
                .1,
            "oaiapp_fixture"
        );
        let entry = request(
            "state=sample&code=one&client_id=dynamic_agent_client",
            "127.0.0.1:1455",
        );
        assert!(matches!(
            decode(&entry, &callback, "sample", None),
            Err(Error::Protocol)
        ));
        assert!(matches!(
            decode(&request(query, "localhost:1455"), &callback, "sample", None),
            Err(Error::Callback)
        ));
        assert!(matches!(
            decode(&request(query, "127.0.0.1:1455"), &callback, "wrong", None),
            Err(Error::Callback)
        ));
        assert!(matches!(
            decode(
                &request(query, "127.0.0.1:1455"),
                &callback,
                "sample",
                Some("oaiapp_other")
            ),
            Err(Error::Protocol)
        ));
    }
}
