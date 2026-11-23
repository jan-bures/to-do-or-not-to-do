//! Paired push registration using the native transport's TLS and redirect rules.

use crate::{FfiError, device_pairing::transport, runtime::runtime};
use todo_client::platform::{Method, Request, Response, Transport};

const PATH: &str = "/api/v1/pair/push";

/// Firebase client options fetched from the paired server. Never log them.
#[allow(missing_debug_implementations)]
#[derive(serde::Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PushOptions {
    /// Firebase project ID.
    pub project_id: String,
    /// Firebase Android app ID.
    pub application_id: String,
    /// Client API key.
    pub api_key: String,
    /// Firebase project number.
    pub gcm_sender_id: String,
}

fn request(
    server: &str,
    token: &str,
    allow_http: bool,
    req: Request,
) -> Result<Response, FfiError> {
    let transport = transport(server, Some(token), allow_http)?;
    runtime()?.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(4), transport.send(req))
            .await
            .map_err(|_| todo_client::platform::TransportError::Timeout)?
            .map_err(FfiError::from)
    })
}

fn refused(response: Response) -> FfiError {
    FfiError::Refused {
        status: response.status,
        code: "push_refused".into(),
        retry_after_secs: response.retry_after.map(|d| d.as_secs()),
        detail: "The server did not accept the push request.".into(),
    }
}

/// Client options, or `None` if push is off (including on older servers). Blocking.
#[uniffi::export]
pub fn push_options(
    server: String,
    token: String,
    allow_http: bool,
) -> Result<Option<PushOptions>, FfiError> {
    let response = request(&server, &token, allow_http, Request::get(PATH))?;
    match response.status {
        204 | 404 => Ok(None),
        200 => serde_json::from_slice(&response.body)
            .map(Some)
            .map_err(|_| FfiError::Protocol {
                detail: "Invalid push settings reply.".into(),
            }),
        _ => Err(refused(response)),
    }
}

/// Replaces the paired device's registration token. Blocking.
#[uniffi::export]
pub fn push_register(
    server: String,
    token: String,
    allow_http: bool,
    registration: String,
) -> Result<(), FfiError> {
    let body = serde_json::to_vec(&serde_json::json!({ "token": registration })).map_err(|_| {
        FfiError::Protocol {
            detail: "Invalid push registration.".into(),
        }
    })?;
    let response = request(
        &server,
        &token,
        allow_http,
        Request {
            method: Method::Put,
            path: PATH.into(),
            body: Some(body),
        },
    )?;
    if response.status == 204 {
        Ok(())
    } else {
        Err(refused(response))
    }
}

/// Removes the paired device's registration. Blocking.
#[uniffi::export]
pub fn push_unregister(server: String, token: String, allow_http: bool) -> Result<(), FfiError> {
    let response = request(
        &server,
        &token,
        allow_http,
        Request {
            method: Method::Delete,
            path: PATH.into(),
            body: None,
        },
    )?;
    if matches!(response.status, 204 | 404) {
        Ok(())
    } else {
        Err(refused(response))
    }
}
