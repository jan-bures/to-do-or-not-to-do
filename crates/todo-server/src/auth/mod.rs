//! Authentication.
//!
//! Every route except `GET /health` requires an [`Authenticated`] caller. The API router runs
//! [`require_auth`] as a layer, so unknown paths and handlers that do not take the extractor are
//! protected too; handlers take [`Authenticated`] to learn who is calling.
//!
//! Step 3a accepts only owner bearer tokens ([`OwnerTokens`]). New schemes implement
//! [`Authenticator`] and add a [`Principal`] variant.

mod owner_token;
mod principal;

pub use owner_token::{OwnerTokens, hash_token};
pub use principal::{Access, Principal};

use crate::api::error::ApiError;
use crate::app::AppState;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use todo_core::Actor;

/// Why a request could not be authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// No credentials were sent.
    #[error("missing credentials")]
    Missing,
    /// The credentials are malformed or unknown.
    #[error("invalid credentials")]
    Invalid,
}

/// Turns request credentials into a principal.
pub trait Authenticator: Send + Sync + std::fmt::Debug + 'static {
    /// Authenticates a request from its headers.
    fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>;
}

/// An authenticated caller: the principal, the actor its events are recorded under, and its access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authenticated {
    /// Who is calling.
    pub principal: Principal,
    /// The `todo-core` actor for commands and undo.
    pub actor: Actor,
    /// What the caller may do.
    pub access: Access,
}

impl From<Principal> for Authenticated {
    fn from(principal: Principal) -> Self {
        Self {
            actor: principal.actor(),
            access: principal.access(),
            principal,
        }
    }
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Authenticated, ApiError> {
    state
        .auth
        .authenticate(headers)
        .map(Authenticated::from)
        .map_err(|e| {
            tracing::debug!(reason = %e, "rejected request");
            ApiError::unauthorized()
        })
}

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(a) = parts.extensions.get::<Self>() {
            return Ok(a.clone());
        }
        let a = authenticate(state, &parts.headers)?;
        parts.extensions.insert(a.clone());
        Ok(a)
    }
}

/// Middleware that rejects unauthenticated requests with 401 and caches the caller for the extractor.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    match authenticate(&state, req.headers()) {
        Ok(a) => {
            req.extensions_mut().insert(a);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}
