//! Outbound HTTP is fixed by the operator and by the provider contract, never by a request URL.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use futures_util::stream;
use reqwest::{Client, dns, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use todo_tools::assistant::relay::Model;
use tokio::sync::OwnedSemaphorePermit;
use url::{Host, Url};
use zeroize::Zeroizing;

use super::{Error, InitError};

const CONNECT: Duration = Duration::from_secs(5);
const IDLE: Duration = Duration::from_secs(20);
const TOTAL: Duration = Duration::from_secs(80);
const MAX_STREAM: usize = 4 * 1024 * 1024;
const MAX_JSON: usize = 256 * 1024;

/// Operator-only settings: no key material is kept in TOML.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrivateConfig {
    /// HTTP(S) base on loopback or a Tailscale address.
    pub base_url: String,
    /// A file holding the bearer key, readable only by the service account.
    pub key_file: PathBuf,
}

pub(super) fn client() -> reqwest::ClientBuilder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(CONNECT)
        .read_timeout(IDLE)
        .timeout(TOTAL)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Network {
    Loopback,
    Tailnet,
}

fn network(ip: IpAddr) -> Option<Network> {
    let loopback = match ip {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST,
        IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
    };
    if loopback {
        return Some(Network::Loopback);
    }
    let tailnet = match ip {
        IpAddr::V4(v4) => u32::from(v4) >> 22 == u32::from(Ipv4Addr::new(100, 64, 0, 0)) >> 22,
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    };
    tailnet.then_some(Network::Tailnet)
}

fn base_address(input: &str) -> Option<(Url, Network)> {
    let mut url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let target = match url.host()? {
        Host::Ipv4(ip) => network(ip.into())?,
        Host::Ipv6(ip) => network(ip.into())?,
        Host::Domain("localhost") => Network::Loopback,
        Host::Domain(name)
            if name
                .strip_suffix(".ts.net")
                .is_some_and(|name| !name.is_empty()) =>
        {
            Network::Tailnet
        }
        _ => return None,
    };
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/v1") {
        format!("{path}/")
    } else {
        format!("{path}/v1/")
    };
    url.set_path(&path);
    Some((url, target))
}

impl PrivateConfig {
    pub(super) fn valid(&self) -> bool {
        base_address(&self.base_url).is_some() && !self.key_file.as_os_str().is_empty()
    }
}

struct CheckedDns(Network);

impl dns::Resolve for CheckedDns {
    fn resolve(&self, name: dns::Name) -> dns::Resolving {
        let name = name.as_str().to_owned();
        let required = self.0;
        Box::pin(async move {
            let peers = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|addr| network(addr.ip()) == Some(required))
                .collect::<Vec<_>>();
            if peers.is_empty() {
                return Err(std::io::Error::other("model host has no permitted address").into());
            }
            Ok(Box::new(peers.into_iter()) as dns::Addrs)
        })
    }
}

pub(super) struct Private {
    http: Client,
    base: Url,
    key: Zeroizing<String>,
}

impl Private {
    pub fn open(config: &PrivateConfig) -> Result<Self, InitError> {
        let (base, network) = base_address(&config.base_url).ok_or(InitError::PrivateAddress)?;
        let bytes = Zeroizing::new(
            todo_fs::read_private_file(&config.key_file, 8192).map_err(InitError::PrivateKey)?,
        );
        let key = std::str::from_utf8(&bytes)
            .map_err(|_| InitError::BadPrivateKey)?
            .trim();
        if !valid_token(key) {
            return Err(InitError::BadPrivateKey);
        }
        let key = Zeroizing::new(key.to_owned());
        let http = client()
            .dns_resolver(Arc::new(CheckedDns(network)))
            .build()
            .map_err(|_| InitError::Http)?;
        Ok(Self { http, base, key })
    }

    pub async fn models(&self) -> Result<Vec<Model>, Error> {
        #[derive(Deserialize)]
        struct Catalog {
            data: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: String,
        }
        let url = self.base.join("models").map_err(|_| Error::Upstream)?;
        let catalog: Catalog = read_json(
            self.http
                .get(url)
                .bearer_auth(self.key.as_str())
                .send()
                .await
                .map_err(network_error)?,
        )
        .await?;
        Ok(catalog
            .data
            .into_iter()
            .filter(|e| valid_model(&e.id))
            .take(256)
            .map(|e| Model {
                label: e.id.clone(),
                id: e.id,
            })
            .collect())
    }

    pub async fn chat(&self, request: &serde_json::Value) -> Result<reqwest::Response, Error> {
        let url = self
            .base
            .join("chat/completions")
            .map_err(|_| Error::Upstream)?;
        self.http
            .post(url)
            .bearer_auth(self.key.as_str())
            .header(header::ACCEPT, "text/event-stream, application/json")
            .json(request)
            .send()
            .await
            .map_err(network_error)
    }
}

pub(super) fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8192 && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

pub(super) fn valid_model(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 800 && !value.chars().any(char::is_control)
}

pub(super) fn network_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Upstream
    }
}

pub(super) fn check_status(status: StatusCode) -> Result<(), Error> {
    match status {
        status if status.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::UpstreamAuthorization),
        StatusCode::NOT_FOUND => Err(Error::ModelMissing),
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => Err(Error::Unsupported),
        StatusCode::TOO_MANY_REQUESTS => Err(Error::UpstreamLimit),
        _ => Err(Error::Upstream),
    }
}

pub(super) async fn read_json<T: DeserializeOwned>(mut res: reqwest::Response) -> Result<T, Error> {
    check_status(res.status())?;
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = res.chunk().await.map_err(network_error)? {
        if bytes.len() + chunk.len() > MAX_JSON {
            return Err(Error::Upstream);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Upstream)
}

pub(super) fn relay(
    res: reqwest::Response,
    permit: OwnedSemaphorePermit,
    allow_json: bool,
) -> Result<Response, Error> {
    check_status(res.status())?;
    if res.content_length().is_some_and(|n| n > MAX_STREAM as u64) {
        return Err(Error::Upstream);
    }
    let media = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(str::trim);
    let content_type = match media {
        Some(s) if s.eq_ignore_ascii_case("text/event-stream") => "text/event-stream",
        Some(s) if allow_json && s.eq_ignore_ascii_case("application/json") => "application/json",
        _ => return Err(Error::Upstream),
    };
    let body = Body::from_stream(stream::try_unfold(
        (res, permit, 0usize),
        |(mut res, permit, used)| async move {
            match res.chunk().await {
                Ok(Some(chunk)) if chunk.len() <= MAX_STREAM - used => {
                    let used = used + chunk.len();
                    Ok(Some((chunk, (res, permit, used))))
                }
                Ok(None) => Ok(None),
                // Never propagate upstream errors or their headers. Drop the socket and the slot.
                Ok(Some(_)) | Err(_) => Err(std::io::Error::other("assistant stream interrupted")),
            }
        },
    ));
    // Construct headers from scratch (including no cookies, redirects, or hop-by-hop headers).
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store, no-transform")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .map_err(|_| Error::Upstream)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
