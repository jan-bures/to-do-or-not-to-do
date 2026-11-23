//! Owner-local ChatGPT plan integration. Never accepts a To-do credential.
mod authorization;
mod provider;
mod records;
mod responses;

pub use authorization::Attempt;
use authorization::{Grant, Identity};
pub use provider::DesktopProvider;
pub use records::{AccountView, Accounts};
pub use responses::{CloudProvider, Events};

use futures::StreamExt;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;
use zeroize::Zeroizing;

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DIRECT: &str = "chatgpt.tokens.use.direct";
pub const USAGE: &str = "https://chatgpt.com/settings/usage";
const LIMIT: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Couldn't reach ChatGPT. Try again when you're online.")]
    Network,
    #[error("Couldn't open or verify the local sign-in callback.")]
    Callback,
    #[error("Sign-in timed out. Start a new sign-in to try again.")]
    TimedOut,
    #[error("ChatGPT sign-in was canceled.")]
    Declined,
    #[error("Couldn't verify the ChatGPT account. No credentials were saved.")]
    Identity,
    #[error("ChatGPT didn't accept this sign-in request. Try signing in again.")]
    Protocol,
    #[error("ChatGPT did not recognize this app registration.")]
    InvalidClient,
    #[error("Couldn't save or open protected ChatGPT settings.")]
    Storage,
    #[error("ChatGPT sign-in expired. Continue with ChatGPT to reconnect.")]
    Expired,
    #[error("This ChatGPT account did not grant plan usage.")]
    NoGrant,
    #[error(
        "Signed out on this device, but remote revocation was not confirmed. Disconnect the app in ChatGPT settings if necessary."
    )]
    NotRevoked,
}

#[derive(Clone)]
struct Service {
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct TokenResponse {
    token_type: Option<String>,
    access_token: Option<Secret>,
    refresh_token: Option<Secret>,
    id_token: Option<Secret>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

#[derive(Clone, Default, Deserialize, serde::Serialize)]
#[serde(transparent)]
struct Secret(#[serde(with = "secret_string")] Zeroizing<String>);

mod secret_string {
    use serde::{Deserialize, Serializer};
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(value: &str, output: S) -> Result<S::Ok, S::Error> {
        output.serialize_str(value)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        input: D,
    ) -> Result<Zeroizing<String>, D::Error> {
        String::deserialize(input).map(Zeroizing::new)
    }
}

#[derive(Clone, Deserialize, serde::Serialize)]
struct Tokens {
    access: Secret,
    refresh: Option<Secret>,
    id: Secret,
    expires: u64,
    scope: String,
}

impl Tokens {
    fn plan_allowed(&self) -> bool {
        let grants = |grant| {
            self.scope
                .split_ascii_whitespace()
                .any(|value| value == grant)
        };
        grants(DIRECT)
            && grants("resource.invoke")
            && grants("offline_access")
            && !self.access.0.is_empty()
            && self.refresh.is_some()
    }

    fn from_response(
        raw: TokenResponse,
        id: Secret,
        previous_scope: Option<&str>,
    ) -> Result<Self, Error> {
        // OAuth allows a refresh response to omit scope when the original grants are unchanged.
        // A new authorization, in contrast, must explicitly confirm the granted scopes.
        let scope = raw
            .scope
            .or_else(|| previous_scope.map(ToOwned::to_owned))
            .ok_or(Error::Protocol)?;
        let has = |grant| scope.split_ascii_whitespace().any(|value| value == grant);
        let needs_refresh = previous_scope.is_some() || has("offline_access");
        let refresh = raw.refresh_token;
        let (access, expires) = if let Some(access) = raw.access_token {
            let expires_in = raw
                .expires_in
                .filter(|t| (1..=86_400).contains(t))
                .ok_or(Error::Protocol)?;
            if !raw
                .token_type
                .is_some_and(|kind| kind.eq_ignore_ascii_case("Bearer"))
                || !header_token(&access.0)
                || (needs_refresh && refresh.is_none())
                || refresh
                    .as_ref()
                    .is_some_and(|secret| !header_token(&secret.0))
            {
                return Err(Error::Protocol);
            }
            (access, now().saturating_add(expires_in))
        } else {
            if needs_refresh || has(DIRECT) || refresh.is_some() {
                return Err(Error::Protocol);
            }
            // A verified identity-only sign-in is still useful for reauthorization, but it cannot
            // reach the model catalog or inference. Only plan_allowed grants access to either.
            (Secret::default(), 0)
        };
        Ok(Self {
            access,
            refresh,
            id,
            expires,
            scope,
        })
    }
}

fn header_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32_768 && value.bytes().all(|b| b.is_ascii_graphic())
}

fn oauth_error(body: &[u8]) -> Error {
    let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    oauth_code(
        parsed["error"]
            .as_str()
            .or(parsed["error"]["code"].as_str()),
    )
}

fn oauth_code(code: Option<&str>) -> Error {
    match code {
        Some("access_denied") => Error::Declined,
        Some("invalid_client") => Error::InvalidClient,
        Some(
            "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused",
        ) => Error::Expired,
        _ => Error::Protocol,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Service {
    fn new() -> Result<Self, Error> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(85))
                .build()
                .map_err(|_| Error::Network)?,
        })
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, Error> {
        Self::read(
            request
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .map_err(|_| Error::Network)?,
        )
        .await
    }

    async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, Error> {
        let status = response.status();
        if status.is_server_error() {
            return Err(Error::Network);
        }
        let body = Self::read_body(response).await?;
        if !status.is_success() {
            return Err(oauth_error(&body));
        }
        serde_json::from_slice(&body).map_err(|_| Error::Protocol)
    }

    async fn read_body(response: reqwest::Response) -> Result<Zeroizing<Vec<u8>>, Error> {
        let mut stream = response.bytes_stream();
        let mut body = Zeroizing::new(Vec::new());
        while let Some(data) = stream.next().await {
            let chunk = data.map_err(|_| Error::Network)?;
            if body.len() + chunk.len() > LIMIT {
                return Err(Error::Protocol);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn exchange(&self, grant: Grant) -> Result<(Identity, Tokens), Error> {
        let raw: TokenResponse = self
            .json(
                self.http
                    .post(TOKEN)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .form(&[
                        ("grant_type", "authorization_code"),
                        ("client_id", grant.client.as_str()),
                        ("code", grant.code.as_str()),
                        ("code_verifier", grant.verifier.as_str()),
                        ("redirect_uri", grant.redirect.as_str()),
                        ("resource", RESOURCE),
                    ]),
            )
            .await?;
        let id = raw.id_token.clone().ok_or(Error::Identity)?;
        let identity = self.verify_id(&id.0, &grant.client, &grant.nonce).await?;
        let tokens = Tokens::from_response(raw, id, None)?;
        Ok((identity, tokens))
    }

    async fn refresh(&self, client: &str, tokens: &Tokens) -> Result<Tokens, Error> {
        let refresh = tokens.refresh.as_ref().ok_or(Error::Expired)?;
        let raw: TokenResponse = self
            .json(
                self.http
                    .post(TOKEN)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("client_id", client),
                        ("refresh_token", refresh.0.as_str()),
                        ("resource", RESOURCE),
                    ]),
            )
            .await?;
        // Keep using the original verified identity and its ID token as the browser hint. The
        // refresh's optional ID token is not used as a credential or to update the identity.
        Tokens::from_response(raw, tokens.id.clone(), Some(&tokens.scope))
    }

    async fn revoke(&self, client: &str, refresh: &Secret) -> Result<(), Error> {
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            revocation_endpoint: String,
        }
        let info: Discovery = self
            .json(
                self.http
                    .get(format!("{ISSUER}/.well-known/openid-configuration")),
            )
            .await?;
        let endpoint = Url::parse(&info.revocation_endpoint).map_err(|_| Error::Protocol)?;
        let issuer = Url::parse(ISSUER).map_err(|_| Error::Protocol)?;
        if info.issuer != ISSUER
            || endpoint.origin() != issuer.origin()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Protocol);
        }
        for delay in [Duration::ZERO, Duration::from_millis(300)] {
            tokio::time::sleep(delay).await;
            match self
                .http
                .post(endpoint.clone())
                .timeout(Duration::from_secs(5))
                .form(&[
                    ("token", refresh.0.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client),
                ])
                .send()
                .await
            {
                Ok(response) if response.status() == reqwest::StatusCode::OK => return Ok(()),
                Ok(response) if !response.status().is_server_error() => break,
                _ => {}
            }
        }
        Err(Error::NotRevoked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_identity_can_be_retained_without_the_plan_grant() {
        let raw = serde_json::from_str(r#"{"scope":"openid profile email"}"#)
            .expect("identity-only response");
        let tokens = Tokens::from_response(raw, Secret::default(), None)
            .expect("store the verified identity");
        assert!(!tokens.plan_allowed());
        assert!(tokens.access.0.is_empty());
    }

    #[test]
    fn refresh_inherits_omitted_scope_but_cannot_reuse_a_missing_refresh_token() {
        let full = "openid offline_access resource.invoke chatgpt.tokens.use.direct";
        let raw = || {
            serde_json::from_str(r#"{"token_type":"Bearer","access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#).expect("rotation response")
        };
        assert!(matches!(
            Tokens::from_response(raw(), Secret::default(), None),
            Err(Error::Protocol)
        ));
        let rotated = Tokens::from_response(raw(), Secret::default(), Some(full))
            .expect("inherit grant only during refresh");
        assert_eq!(rotated.scope, full);
        assert_eq!(
            rotated.refresh.expect("replacement token").0.as_str(),
            "new-refresh"
        );
        assert!(
            Tokens::from_response(raw(), Secret::default(), Some(full))
                .expect("complete rotation")
                .plan_allowed()
        );
        assert!(
            !Tokens::from_response(
                TokenResponse {
                    scope: Some("openid offline_access".into()),
                    ..raw()
                },
                Secret::default(),
                Some(full)
            )
            .expect("renewed identity-only scope")
            .plan_allowed()
        );
        assert!(matches!(
            Tokens::from_response(
                TokenResponse {
                    refresh_token: None,
                    ..raw()
                },
                Secret::default(),
                Some(full)
            ),
            Err(Error::Protocol)
        ));
    }

    #[test]
    fn oauth_errors_accept_both_documented_envelopes() {
        assert_eq!(oauth_error(br#"{"error":"invalid_grant"}"#), Error::Expired);
        assert_eq!(
            oauth_error(br#"{"error":{"code":"refresh_token_reused"}}"#),
            Error::Expired
        );
        assert_eq!(
            oauth_error(br#"{"error":"invalid_client"}"#),
            Error::InvalidClient
        );
        assert_eq!(
            oauth_error(br#"{"error":"access_denied"}"#),
            Error::Declined
        );
        assert_eq!(
            oauth_error(br#"{"detail":"temporary or unknown"}"#),
            Error::Protocol
        );
    }
}
