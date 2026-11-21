//! A browser session's CSRF token, and sending again after the session behind the cookie changed.
//!
//! The web app authenticates with the owner's session cookie plus the session's CSRF token in a header.
//! The session can change while a tab is open (the owner signs in again in another tab, replacing the
//! cookie), and the tab's token then no longer fits: the server answers `403 csrf`. [`with_fresh_token`]
//! asks for the session's current token and sends the request once more with it.
//!
//! Several requests can be refused at once (the event stream and a command, say). Each one compares
//! the fresh token with **the token it was sent with**, not with the shared token, which a concurrent
//! refresh may already have updated: so every refused request is retried once with the new token, and
//! none mistakes a token another request just fetched for "unchanged". One refresh and one retry per
//! request: a `403 csrf` that persists after that is returned, and the engine retries it later with
//! backoff (see [`crate::protocol::classify_send`]); it is never a definitive rejection. A signed-out
//! session answers the refresh with 401, which is returned instead.
//!
//! This module has no platform code, so the logic is tested on the host; the browser transport
//! (`web::FetchTransport`) supplies the requests.

use crate::platform::{Response, TransportError};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// The session's CSRF token, shared by the transport and the page (which needs it for its own
/// requests, such as the sign-out form). Clones share the value.
#[derive(Clone, Default)]
pub struct CsrfToken(Arc<Mutex<String>>);

impl fmt::Debug for CsrfToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the token.
        f.debug_struct("CsrfToken").finish_non_exhaustive()
    }
}

impl CsrfToken {
    /// A token with this value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(Mutex::new(value.into())))
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the value.
    pub fn set(&self, value: String) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }
}

/// True if a response is the server's `403 csrf`.
#[must_use]
pub fn is_csrf_refusal(res: &Response) -> bool {
    res.status == 403
        && crate::protocol::error_detail(res).is_some_and(|d| d.code == crate::protocol::CSRF)
}

/// What one try of a request found.
#[derive(Debug)]
pub enum Attempt<O> {
    /// An answer that stands (success, or any refusal but `403 csrf`).
    Done(O),
    /// `403 csrf`: the token didn't fit. Carries what to return if the refusal stands.
    CsrfRefused(O),
}

/// What asking for the session's current token found.
#[derive(Debug)]
pub enum Fresh {
    /// The session's token now.
    Token(String),
    /// There is no session: this (401) is the answer.
    SignedOut(Response),
}

/// Runs `attempt` with the current token; on `403 csrf`, asks `refresh` for the session's token and,
/// if it differs from the one the request was sent with, stores it and runs `attempt` once more. See
/// the module docs. A failed refresh is a [`TransportError`], so the caller retries later.
pub async fn with_fresh_token<O, A, AF, R, RF>(
    token: &CsrfToken,
    attempt: A,
    refresh: R,
    signed_out: impl FnOnce(Response) -> O,
) -> Result<O, TransportError>
where
    A: Fn(String) -> AF,
    AF: Future<Output = Result<Attempt<O>, TransportError>>,
    R: FnOnce() -> RF,
    RF: Future<Output = Result<Fresh, TransportError>>,
{
    let sent = token.get();
    let refused = match attempt(sent.clone()).await? {
        Attempt::Done(o) => return Ok(o),
        Attempt::CsrfRefused(o) => o,
    };
    match refresh().await? {
        Fresh::SignedOut(r) => Ok(signed_out(r)),
        // The token the request carried is the session's: the refusal was about something else (the
        // `Origin`). It stands for now.
        Fresh::Token(t) if t == sent => Ok(refused),
        Fresh::Token(t) => {
            if token.get() != t {
                tracing::debug!("the session's CSRF token changed; retrying with the new one");
                token.set(t.clone());
            }
            match attempt(t).await? {
                Attempt::Done(o) | Attempt::CsrfRefused(o) => Ok(o),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join;
    use std::cell::{Cell, RefCell};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// Pending once, so concurrent futures interleave.
    struct YieldNow(bool);

    impl Future for YieldNow {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    fn res(status: u16, code: &str) -> Response {
        Response {
            status,
            retry_after: None,
            body: format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#).into_bytes(),
        }
    }

    /// A server whose session token is `server`; it accepts only that token (or none, when `origin_ok`
    /// is false, to model an `Origin` refusal).
    struct Server {
        token: RefCell<String>,
        origin_ok: bool,
        signed_in: bool,
        sends: RefCell<Vec<String>>,
        refreshes: Cell<u32>,
    }

    impl Server {
        fn new(token: &str) -> Self {
            Self {
                token: RefCell::new(token.into()),
                origin_ok: true,
                signed_in: true,
                sends: RefCell::new(Vec::new()),
                refreshes: Cell::new(0),
            }
        }

        async fn send(&self, with: String) -> Result<Attempt<Response>, TransportError> {
            YieldNow(false).await;
            self.sends.borrow_mut().push(with.clone());
            if !self.signed_in {
                return Ok(Attempt::Done(res(401, "unauthorized")));
            }
            let r = if self.origin_ok && with == *self.token.borrow() {
                res(200, "ok")
            } else {
                res(403, "csrf")
            };
            Ok(if is_csrf_refusal(&r) {
                Attempt::CsrfRefused(r)
            } else {
                Attempt::Done(r)
            })
        }

        async fn refresh(&self) -> Result<Fresh, TransportError> {
            YieldNow(false).await;
            self.refreshes.set(self.refreshes.get() + 1);
            if self.signed_in {
                Ok(Fresh::Token(self.token.borrow().clone()))
            } else {
                Ok(Fresh::SignedOut(res(401, "signed_out")))
            }
        }

        async fn request(&self, token: &CsrfToken) -> Response {
            with_fresh_token(token, |t| self.send(t), || self.refresh(), |r| r)
                .await
                .unwrap()
        }
    }

    #[test]
    fn two_concurrent_refusals_both_retry_with_the_new_token() {
        // The session changed: the server's token is "new", the tab still holds "old". The event
        // stream and a command are both refused; whichever refreshes first updates the shared token,
        // and the other must still see that *its* token was stale.
        let server = Server::new("new");
        let token = CsrfToken::new("old");
        let (a, b) =
            futures_executor::block_on(join(server.request(&token), server.request(&token)));
        assert_eq!((a.status, b.status), (200, 200));
        assert_eq!(token.get(), "new");
        assert_eq!(server.refreshes.get(), 2);
        assert_eq!(*server.sends.borrow(), ["old", "old", "new", "new"]);
    }

    #[test]
    fn a_refusal_with_the_current_token_stands_after_one_refresh() {
        // The token is right, but the request is refused anyway (its `Origin`): one refresh, no
        // retry, and the 403 is returned for the engine to retry later.
        let mut server = Server::new("t");
        server.origin_ok = false;
        let token = CsrfToken::new("t");
        let r = futures_executor::block_on(server.request(&token));
        assert_eq!(r.status, 403);
        assert_eq!(server.refreshes.get(), 1);
        assert_eq!(server.sends.borrow().len(), 1);
        // And the engine does not treat it as a verdict on the command.
        assert!(matches!(
            crate::protocol::classify_send(&r),
            crate::protocol::SendOutcome::Retry {
                unauthorized: false,
                ..
            }
        ));
    }

    #[test]
    fn a_refusal_that_persists_after_the_new_token_is_retried_only_once() {
        // The token changes again between the refresh and the retry: one retry, then the 403 stands.
        let server = Server::new("new");
        let token = CsrfToken::new("old");
        let r = futures_executor::block_on(with_fresh_token(
            &token,
            |t| server.send(t),
            || async {
                let fresh = server.refresh().await;
                *server.token.borrow_mut() = "newer".into();
                fresh
            },
            |r| r,
        ))
        .unwrap();
        assert_eq!(r.status, 403);
        assert_eq!(server.sends.borrow().len(), 2);
    }

    #[test]
    fn a_signed_out_session_answers_401() {
        let mut server = Server::new("t");
        server.signed_in = false;
        let token = CsrfToken::new("t");
        let r = futures_executor::block_on(server.request(&token));
        assert_eq!(r.status, 401);
    }

    #[test]
    fn other_answers_pass_through_without_a_refresh() {
        let server = Server::new("t");
        let token = CsrfToken::new("t");
        let r = futures_executor::block_on(server.request(&token));
        assert_eq!(r.status, 200);
        assert_eq!(server.refreshes.get(), 0);
    }
}
