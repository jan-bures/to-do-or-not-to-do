//! Address classification shared by the native and browser assistant transports.
//!
//! This classifies an address, not its actual route or what the server does with requests.

use std::net::IpAddr;
use url::{Host, Url};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    ThisDevice,
    Tailscale,
    YourServer,
    NotPrivate,
}

impl Destination {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisDevice => "This device",
            Self::Tailscale => "Tailscale",
            Self::YourServer => "Your server",
            Self::NotPrivate => "Not private",
        }
    }
}

pub(super) fn address(input: &str) -> Option<Url> {
    let input = input.trim();
    let (_, rest) = input.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if input.len() > 2048
        || input
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        || authority.is_empty()
        || authority.contains('@')
        || authority.ends_with(':')
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return None;
    }
    Some(url)
}

pub(super) fn classify(host: Host<&str>, attested: bool) -> Destination {
    match host {
        Host::Domain(host) if host.trim_end_matches('.') == "localhost" => Destination::ThisDevice,
        Host::Ipv4(ip) if ip.is_loopback() => Destination::ThisDevice,
        Host::Ipv6(ip) if ip.is_loopback() => Destination::ThisDevice,
        Host::Domain(host) if host.trim_end_matches('.').ends_with(".ts.net") => {
            Destination::Tailscale
        }
        Host::Ipv4(ip) if is_tailnet_ip(ip.into()) => Destination::Tailscale,
        Host::Ipv6(ip) if is_tailnet_ip(ip.into()) => Destination::Tailscale,
        _ if attested => Destination::YourServer,
        _ => Destination::NotPrivate,
    }
}

/// The platform check separately verifies the native connection has an up Tailscale interface.
#[must_use]
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            a == 100 && (64..=127).contains(&b)
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Validate the optional Bearer key without formatting or logging it. Empty means no key.
pub fn bearer_key(input: &str) -> Option<&str> {
    let key = input.trim();
    (key.len() <= 4096 && key.bytes().all(|b| b.is_ascii_graphic())).then_some(key)
}
