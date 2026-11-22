//! The one error type the bindings raise. Kotlin sees it as `FfiException` with a subclass per variant.
//!
//! No variant carries a field called `message`: Kotlin exceptions already have one.

use todo_client::platform::TransportError;
use todo_client::{StorageError, SubmitError};

/// Something the app asked for didn't work.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// The server address isn't one the app will talk to.
    #[error("{reason}")]
    BadAddress {
        /// Why, for the user.
        reason: String,
    },
    /// No response: offline, DNS, TLS or a timeout.
    #[error("cannot reach the server: {detail}")]
    Network {
        /// What failed. Never contains the token.
        detail: String,
    },
    /// The pairing request expired or was already used: start again.
    #[error("the pairing request expired; start again")]
    Expired,
    /// The server answered with an error.
    #[error("the server refused: {detail} ({status})")]
    Refused {
        /// HTTP status.
        status: u16,
        /// The server's error code, if any.
        code: String,
        /// The server's description.
        detail: String,
        /// Seconds to wait before trying again, if the server said.
        retry_after_secs: Option<u64>,
    },
    /// The server's answer wasn't understood.
    #[error("unexpected answer from the server: {detail}")]
    Protocol {
        /// What was wrong.
        detail: String,
    },
    /// The local cache failed.
    #[error("local storage failed: {detail}")]
    Storage {
        /// What failed.
        detail: String,
    },
    /// The change isn't valid (an empty title, a task that no longer exists, ...).
    #[error("{detail}")]
    Invalid {
        /// Why.
        detail: String,
    },
}

impl From<TransportError> for FfiError {
    fn from(e: TransportError) -> Self {
        match e {
            TransportError::Network(d) => Self::Network { detail: d },
            TransportError::Timeout => Self::Network {
                detail: "no answer in time".into(),
            },
            TransportError::Protocol(d) => Self::Protocol { detail: d },
        }
    }
}

impl From<StorageError> for FfiError {
    fn from(e: StorageError) -> Self {
        Self::Storage {
            detail: e.to_string(),
        }
    }
}

impl From<SubmitError> for FfiError {
    fn from(e: SubmitError) -> Self {
        match e {
            SubmitError::Invalid(d) => Self::Invalid {
                detail: d.to_string(),
            },
            SubmitError::Storage(s) => s.into(),
        }
    }
}
