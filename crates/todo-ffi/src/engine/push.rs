//! Paired push registration using the native transport's TLS and redirect rules.

use super::TodoEngine;
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
    future: impl Future<Output = Result<Response, todo_client::platform::TransportError>>,
) -> Result<Response, FfiError> {
    runtime()?.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(4), future)
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

#[uniffi::export]
impl TodoEngine {
    /// Client options, or `None` if push is off (including on older servers). Blocking.
    /// A 401 is persisted by the same revocation latch as sync and direct file reads.
    pub fn push_options(&self) -> Result<Option<PushOptions>, FfiError> {
        let response = request(self.client.send_request(Request::get(PATH)))?;
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

    /// Replaces this pairing's registration token. A 401 revokes the pairing. Blocking.
    pub fn push_register(&self, registration: String) -> Result<(), FfiError> {
        let body =
            serde_json::to_vec(&serde_json::json!({ "token": registration })).map_err(|_| {
                FfiError::Protocol {
                    detail: "Invalid push registration.".into(),
                }
            })?;
        let response = request(self.client.send_request(Request {
            method: Method::Put,
            path: PATH.into(),
            body: Some(body),
        }))?;
        if response.status == 204 {
            Ok(())
        } else {
            Err(refused(response))
        }
    }
}

/// Removes the old pairing's registration after local sign-out, when its engine is already closed.
/// A refusal must not revoke the new pairing. Blocking.
#[uniffi::export]
pub fn push_unregister(server: String, token: String, allow_http: bool) -> Result<(), FfiError> {
    let transport = transport(&server, Some(&token), allow_http)?;
    let response = request(transport.send(Request {
        method: Method::Delete,
        path: PATH.into(),
        body: None,
    }))?;
    if matches!(response.status, 204 | 404) {
        Ok(())
    } else {
        Err(refused(response))
    }
}
