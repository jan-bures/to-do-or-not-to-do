//! OpenAI device authorization (Codex login) and SIWC dynamic loopback registration.
//! Their token audiences, model catalogs, and Responses endpoints are distinct.

use std::{
    collections::BTreeSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use reqwest::{Client, RequestBuilder, StatusCode, header};
use ring::{digest, signature};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use todo_tools::assistant::relay::Model;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    Error, InitError,
    upstream::{client, network_error, read_catalog, read_json, valid_model, valid_token},
    vault::{Credential, Flow},
};

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const CODEX: &str = "https://chatgpt.com/backend-api/codex";
const ENTRY_CLIENT: &str = "dynamic_agent_client";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const AUTH_TIMEOUT: Duration = Duration::from_secs(7);

/// Revocation failures do not affect local sign-out. Only transient ones are retried.
#[derive(Debug, thiserror::Error)]
pub(super) enum RevokeError {
    #[error("revocation failed temporarily")]
    Retry,
    #[error("revocation is unsupported or was rejected")]
    Terminal,
}

impl RevokeError {
    fn check(status: StatusCode) -> Result<(), Self> {
        if status.is_success() {
            Ok(())
        } else if status.is_client_error()
            && !matches!(
                status,
                StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
            )
            || status == StatusCode::NOT_IMPLEMENTED
        {
            Err(Self::Terminal)
        } else {
            Err(Self::Retry)
        }
    }
}

pub(super) struct OpenAi {
    http: Client,
    pub issuer: String,
    resource: String,
    codex: String,
    device_client: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct DeviceGrant {
    pub device_auth_id: String,
    #[serde(alias = "usercode")]
    pub user_code: String,
    #[serde(
        default = "default_interval",
        deserialize_with = "deserialize_interval"
    )]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}
fn deserialize_interval<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u64, D::Error> {
    let interval = String::deserialize(de)?;
    interval.trim().parse().map_err(serde::de::Error::custom)
}

pub(super) struct LoopbackGrant {
    pub state: Zeroizing<String>,
    redirect: String,
    nonce: Zeroizing<String>,
    verifier: Zeroizing<String>,
}

#[derive(Deserialize)]
struct DeviceCode {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    token_type: Option<String>,
}

impl Drop for Tokens {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
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
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    azp: Option<String>,
    exp: u64,
    nbf: Option<u64>,
    nonce: Option<String>,
    email: Option<String>,
    name: Option<String>,
    #[serde(rename = "https://api.openai.com/auth")]
    workspace: Option<Workspace>,
}

#[derive(Deserialize)]
struct Workspace {
    chatgpt_account_id: Option<String>,
}

pub(super) fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value != ENTRY_CLIENT
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn secret() -> Result<Zeroizing<String>, Error> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *bytes).map_err(|_| Error::Storage)?;
    Ok(Zeroizing::new(B64.encode(*bytes)))
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn decode<T: DeserializeOwned>(segment: &str) -> Result<T, Error> {
    let bytes = Zeroizing::new(B64.decode(segment).map_err(|_| Error::Identity)?);
    serde_json::from_slice(&bytes).map_err(|_| Error::Identity)
}

fn validate_claims(
    claims: &Claims,
    issuer: &str,
    client_id: &str,
    nonce: Option<&str>,
    now: u64,
) -> Result<(), Error> {
    let audience = match &claims.aud {
        Audience::One(aud) => aud == client_id,
        Audience::Many(aud) => aud.as_slice() == [client_id],
    };
    if !audience
        || claims.exp <= now
        || claims.nbf.is_some_and(|t| t > now)
        || claims.iss != issuer
        || claims.azp.as_deref().is_some_and(|azp| azp != client_id)
        || !valid_token(&claims.sub)
        || claims.sub.len() > 512
        || nonce.is_some_and(|expected| {
            !claims
                .nonce
                .as_deref()
                .is_some_and(|got| bool::from(got.as_bytes().ct_eq(expected.as_bytes())))
        })
    {
        return Err(Error::Identity);
    }
    Ok(())
}

fn expiry(tokens: &Tokens, id_exp: Option<u64>, now: u64) -> u64 {
    // Codex's token response does not require expires_in. An unverified access token's exp is
    // used only to schedule refresh, never to establish identity or authorize a workspace.
    #[derive(Deserialize)]
    struct Exp {
        exp: u64,
    }
    let access_exp = tokens
        .access_token
        .as_deref()
        .and_then(|t| t.split('.').nth(1))
        .and_then(|segment| decode::<Exp>(segment).ok())
        .map(|claims| claims.exp);
    tokens
        .expires_in
        .map(|l| now.saturating_add(l))
        .or(access_exp)
        .or(id_exp)
        .unwrap_or(now + 20 * 60)
        .min(now + 7 * 24 * 3600)
}

fn has_scope(scope: Option<&str>, name: &str) -> bool {
    scope.is_some_and(|s| s.split_ascii_whitespace().any(|item| item == name))
}

#[cfg(test)]
impl LoopbackGrant {
    pub(super) fn set_fixture_nonce(&mut self, nonce: &str) {
        self.nonce = Zeroizing::new(nonce.to_owned());
    }
}

impl OpenAi {
    #[cfg(test)]
    pub(super) fn at_fixture(issuer: String, client: Option<String>) -> Self {
        Self {
            codex: format!("{issuer}/codex"),
            resource: format!("{issuer}/siwc"),
            issuer,
            ..Self::new(client).unwrap()
        }
    }

    pub fn new(device_client: Option<String>) -> Result<Self, InitError> {
        Ok(Self {
            http: client().build().map_err(|_| InitError::Http)?,
            issuer: ISSUER.into(),
            resource: RESOURCE.into(),
            codex: CODEX.into(),
            device_client,
        })
    }

    fn token_endpoint(&self, flow: Flow) -> String {
        let path = match flow {
            Flow::Device => "/oauth/token",
            Flow::Loopback => "/api/accounts/oauth/token",
        };
        format!("{}{path}", self.issuer)
    }

    async fn auth<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T, Error> {
        read_json(
            req.timeout(AUTH_TIMEOUT)
                .send()
                .await
                .map_err(network_error)?,
        )
        .await
    }

    // Protocol followed from codex-rs/login/src/device_code_auth.rs and server.rs at
    // openai/codex 96681d86f08f880ed12ba29044e855002809e0d1. The registration is supplied by the
    // operator: do not masquerade as a different app or send the SIWC entry ID to this endpoint.
    pub async fn start_device(&self) -> Result<DeviceGrant, Error> {
        let client_id = self.device_client.as_deref().ok_or(Error::Disabled)?;
        let result: DeviceGrant = self
            .auth(
                self.http
                    .post(format!("{}/api/accounts/deviceauth/usercode", self.issuer))
                    .json(&json!({"client_id": client_id})),
            )
            .await?;
        if !valid_token(&result.user_code)
            || result.user_code.len() > 64
            || !valid_token(&result.device_auth_id)
        {
            return Err(Error::Upstream);
        }
        Ok(result)
    }

    pub async fn poll_device(&self, grant: &DeviceGrant) -> Result<Option<Credential>, Error> {
        let response = self
            .http
            .post(format!("{}/api/accounts/deviceauth/token", self.issuer))
            .json(&json!({"device_auth_id": grant.device_auth_id, "user_code": grant.user_code}))
            .timeout(AUTH_TIMEOUT)
            .send()
            .await
            .map_err(network_error)?;
        if matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ) {
            return Ok(None);
        }
        let mut result: DeviceCode = read_json(response).await?;
        let code = Zeroizing::new(std::mem::take(&mut result.authorization_code));
        let verifier = Zeroizing::new(std::mem::take(&mut result.code_verifier));
        let client_id = self.device_client.as_deref().ok_or(Error::Disabled)?;
        let redirect = format!("{}/deviceauth/callback", self.issuer);
        self.exchange(Flow::Device, client_id, &redirect, &code, &verifier, None)
            .await
            .map(Some)
    }

    pub fn start_loopback(&self, host: Uuid, port: u16) -> Result<(LoopbackGrant, String), Error> {
        if port < 1024 {
            return Err(Error::BadRequest);
        }
        let grant = LoopbackGrant {
            state: secret()?,
            redirect: format!("http://127.0.0.1:{port}/auth/callback"),
            nonce: secret()?,
            verifier: secret()?,
        };
        let mut browser = Url::parse(&format!("{}/api/accounts/authorize", self.issuer))
            .map_err(|_| Error::BadRequest)?;
        let challenge =
            B64.encode(digest::digest(&digest::SHA256, grant.verifier.as_bytes()).as_ref());
        browser.query_pairs_mut().extend_pairs([
            ("client_id", ENTRY_CLIENT),
            ("ext_agent_host_id", &host.urn().to_string()),
            ("agent_name_hint", "To-do or not to do"),
            ("response_type", "code"),
            ("redirect_uri", &grant.redirect),
            ("scope", SCOPES),
            ("resource", &self.resource),
            ("state", grant.state.as_str()),
            ("nonce", grant.nonce.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ]);
        Ok((grant, browser.into()))
    }

    pub async fn handoff(
        &self,
        grant: &LoopbackGrant,
        input: &todo_tools::assistant::relay::Handoff,
    ) -> Result<Credential, Error> {
        if !bool::from(grant.state.as_bytes().ct_eq(input.state.as_bytes()))
            || !valid_client_id(&input.client_id)
            || input.code.len() > 4096
            || !valid_token(&input.code)
        {
            return Err(Error::BadRequest);
        }
        self.exchange(
            Flow::Loopback,
            &input.client_id,
            &grant.redirect,
            &input.code,
            &grant.verifier,
            Some(&grant.nonce),
        )
        .await
    }

    async fn exchange(
        &self,
        flow: Flow,
        client_id: &str,
        redirect: &str,
        code: &str,
        verifier: &str,
        nonce: Option<&str>,
    ) -> Result<Credential, Error> {
        if !valid_token(code) || !valid_token(verifier) {
            return Err(Error::BadRequest);
        }
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
        ];
        if flow == Flow::Loopback {
            form.push(("resource", self.resource.as_str()));
        }
        let mut tokens: Tokens = self
            .auth(self.http.post(self.token_endpoint(flow)).form(&form))
            .await?;
        let result = self
            .new_credential(flow, client_id, nonce, &mut tokens)
            .await;
        if result.is_err()
            && tokens.refresh_token.as_deref().is_some_and(valid_token)
            && let Some(refresh) = tokens.refresh_token.take()
        {
            let _ = self
                .revoke(&super::vault::Revocation {
                    flow,
                    client_id: client_id.into(),
                    refresh,
                })
                .await;
        }
        result
    }

    async fn new_credential(
        &self,
        flow: Flow,
        client_id: &str,
        nonce: Option<&str>,
        tokens: &mut Tokens,
    ) -> Result<Credential, Error> {
        let claims = self
            .verify(
                tokens.id_token.as_deref().ok_or(Error::Identity)?,
                client_id,
                nonce,
            )
            .await?;
        let workspace = claims.workspace.and_then(|w| w.chatgpt_account_id);
        if flow == Flow::Device
            && !workspace
                .as_deref()
                .is_some_and(|s| valid_token(s) && s.len() <= 200)
        {
            return Err(Error::Identity);
        }
        if flow == Flow::Loopback
            && (!has_scope(tokens.scope.as_deref(), "resource.invoke")
                || !has_scope(tokens.scope.as_deref(), "chatgpt.tokens.use.direct")
                || !has_scope(tokens.scope.as_deref(), "offline_access"))
        {
            return Err(Error::UpstreamAuthorization);
        }
        let expires_at = expiry(tokens, Some(claims.exp), now());
        let (access, refresh) = extract_tokens(tokens, flow, false)?;
        let label = [claims.email, claims.name]
            .into_iter()
            .flatten()
            .find(|s| {
                !s.trim().is_empty() && s.chars().count() <= 200 && !s.chars().any(char::is_control)
            })
            .unwrap_or_else(|| "ChatGPT account".into());
        Ok(Credential {
            flow,
            client_id: client_id.into(),
            subject: claims.sub,
            workspace,
            label,
            access: access.ok_or(Error::Identity)?,
            refresh: refresh.ok_or(Error::Identity)?,
            expires_at,
            scope: tokens.scope.take(),
            approved: false,
            approved_lists: BTreeSet::new(),
        })
    }

    async fn verify(
        &self,
        jwt: &str,
        client_id: &str,
        nonce: Option<&str>,
    ) -> Result<Claims, Error> {
        let (message, raw_signature) = jwt.rsplit_once('.').ok_or(Error::Identity)?;
        let (header, body) = message.split_once('.').ok_or(Error::Identity)?;
        let header: Header = decode(header)?;
        if header.alg != "RS256" || header.kid.is_empty() {
            return Err(Error::Identity);
        }
        let keys: Keys = self
            .auth(
                self.http
                    .get(format!("{}/.well-known/jwks.json", self.issuer)),
            )
            .await?;
        let mut matching = keys.keys.iter().filter(|k| {
            k.kid == header.kid
                && k.kty == "RSA"
                && k.alg.as_deref().is_none_or(|a| a == "RS256")
                && k.usage.as_deref().is_none_or(|a| a == "sig")
        });
        let key = matching.next().ok_or(Error::Identity)?;
        if matching.next().is_some() {
            return Err(Error::Identity);
        }
        let b64 = |s: &str| B64.decode(s).map_err(|_| Error::Identity);
        signature::RsaPublicKeyComponents {
            n: b64(key.n.as_deref().ok_or(Error::Identity)?)?,
            e: b64(key.e.as_deref().ok_or(Error::Identity)?)?,
        }
        .verify(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            message.as_bytes(),
            &b64(raw_signature)?,
        )
        .map_err(|_| Error::Identity)?;
        let claims = decode(body)?;
        validate_claims(&claims, ISSUER, client_id, nonce, now())?;
        Ok(claims)
    }

    pub async fn refresh(&self, credential: &mut Credential) -> Result<(), Error> {
        use zeroize::Zeroize;
        let req = self.http.post(self.token_endpoint(credential.flow));
        let req = match credential.flow {
            Flow::Device => req.json(&json!({"grant_type": "refresh_token", "client_id": credential.client_id, "refresh_token": credential.refresh})),
            Flow::Loopback => req.form(&[("grant_type", "refresh_token"), ("client_id", credential.client_id.as_str()), ("refresh_token", credential.refresh.as_str()), ("resource", self.resource.as_str())]),
        };
        let response = req
            .timeout(AUTH_TIMEOUT)
            .send()
            .await
            .map_err(network_error)?;
        if response.status() == StatusCode::BAD_REQUEST {
            return Err(Error::UpstreamAuthorization);
        }
        let mut tokens: Tokens = read_json(response).await?;
        if credential.flow == Flow::Loopback
            && let Some(scope) = tokens.scope.as_deref()
            && (!has_scope(Some(scope), "resource.invoke")
                || !has_scope(Some(scope), "chatgpt.tokens.use.direct"))
        {
            return Err(Error::UpstreamAuthorization);
        }
        let id_exp = if let Some(jwt) = &tokens.id_token {
            let claims = self.verify(jwt, &credential.client_id, None).await?;
            if claims.sub != credential.subject
                || claims.workspace.and_then(|w| w.chatgpt_account_id) != credential.workspace
            {
                return Err(Error::Identity);
            }
            Some(claims.exp)
        } else {
            None
        };
        credential.expires_at = expiry(&tokens, id_exp, now());
        let (access, refresh) = extract_tokens(&mut tokens, credential.flow, true)?;
        if let Some(access) = access {
            credential.access.zeroize();
            credential.access = access;
        }
        if let Some(refresh) = refresh {
            credential.refresh.zeroize();
            credential.refresh = refresh;
        }
        if let Some(scope) = tokens.scope.take() {
            credential.scope = Some(scope);
        }
        Ok(())
    }

    pub async fn revoke(&self, credential: &super::vault::Revocation) -> Result<(), RevokeError> {
        let req = match credential.flow {
            Flow::Device => self.http.post(format!("{}/oauth/revoke", self.issuer))
                .json(&json!({"token": credential.refresh, "token_type_hint": "refresh_token", "client_id": credential.client_id})),
            Flow::Loopback => {
                #[derive(Deserialize)]
                struct Discovery { issuer: String, revocation_endpoint: Option<Url> }
                let response = self.http.get(format!("{}/.well-known/openid-configuration", self.issuer)).timeout(AUTH_TIMEOUT).send().await.map_err(|_| RevokeError::Retry)?;
                RevokeError::check(response.status())?;
                let doc: Discovery = read_json(response).await.map_err(|_| RevokeError::Retry)?;
                let issuer = Url::parse(&self.issuer).map_err(|_| RevokeError::Terminal)?;
                let url = doc.revocation_endpoint.filter(|u| doc.issuer == self.issuer && u.origin() == issuer.origin() && u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none()).ok_or(RevokeError::Terminal)?;
                self.http.post(url).form(&[("token", credential.refresh.as_str()), ("token_type_hint", "refresh_token"), ("client_id", credential.client_id.as_str())])
            }
        };
        let res = req
            .timeout(AUTH_TIMEOUT)
            .send()
            .await
            .map_err(|_| RevokeError::Retry)?;
        RevokeError::check(res.status())
    }

    fn request(
        &self,
        credential: &Credential,
        method: reqwest::Method,
        path: &str,
    ) -> RequestBuilder {
        let base = match credential.flow {
            Flow::Device => &self.codex,
            Flow::Loopback => &self.resource,
        };
        let req = self
            .http
            .request(method, format!("{base}/{path}"))
            .bearer_auth(&credential.access);
        match (credential.flow, credential.workspace.as_deref()) {
            (Flow::Device, Some(workspace)) => req.header("ChatGPT-Account-Id", workspace),
            _ => req,
        }
    }

    pub async fn models(&self, credential: &Credential) -> Result<Vec<Model>, Error> {
        let req = self.request(credential, reqwest::Method::GET, "models");
        // Codex's catalog requires a client_version parameter. It is the API compatibility version,
        // not the login application's identity; this client uses only the original Responses fields.
        let req = if credential.flow == Flow::Device {
            req.query(&[("client_version", "0.100.0")])
        } else {
            req
        };
        let result: Value = read_catalog(req.send().await.map_err(network_error)?).await?;
        Ok(listed_models(&result))
    }

    pub async fn chat(
        &self,
        credential: &Credential,
        request: &Value,
    ) -> Result<reqwest::Response, Error> {
        self.request(credential, reqwest::Method::POST, "responses")
            .header(header::ACCEPT, "text/event-stream")
            .json(request)
            .send()
            .await
            .map_err(network_error)
    }
}

/// Read either catalog shape: Codex's `{"models":[{"slug","display_name","visibility"}]}` or the
/// public API's `{"data":[{"id"}]}`, where models that can't chat are left out.
fn listed_models(catalog: &Value) -> Vec<Model> {
    const NOT_CHAT: [&str; 10] = [
        "embedding",
        "tts",
        "whisper",
        "dall-e",
        "image",
        "audio",
        "realtime",
        "transcribe",
        "moderation",
        "search",
    ];
    let codex = catalog["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            Some((
                m["slug"].as_str()?.to_owned(),
                m["display_name"].as_str()?.to_owned(),
            ))
        });
    let public = catalog["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            (!NOT_CHAT.iter().any(|kind| id.contains(kind))).then(|| (id.to_owned(), id.to_owned()))
        });
    codex
        .chain(public)
        .filter(|(id, label)| valid_model(id) && valid_model(label))
        .map(|(id, label)| Model { id, label })
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests;

fn extract_tokens(
    tokens: &mut Tokens,
    flow: Flow,
    refresh: bool,
) -> Result<(Option<String>, Option<String>), Error> {
    if (flow == Flow::Loopback
        && !tokens
            .token_type
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("bearer")))
        || tokens
            .token_type
            .as_deref()
            .is_some_and(|s| !s.eq_ignore_ascii_case("bearer"))
        || (tokens.access_token.is_none() && (!refresh || flow == Flow::Loopback))
        || (tokens.refresh_token.is_none() && (!refresh || flow == Flow::Loopback))
        || tokens
            .access_token
            .as_deref()
            .is_some_and(|s| !valid_token(s))
        || tokens
            .refresh_token
            .as_deref()
            .is_some_and(|s| !valid_token(s))
    {
        return Err(Error::Identity);
    }
    Ok((tokens.access_token.take(), tokens.refresh_token.take()))
}
