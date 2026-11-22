//! The tokio runtime the bindings run on: the sync engine, its change watcher, and the blocking pairing
//! calls. One per process, created on first use.

use crate::FfiError;
use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared runtime.
pub(crate) fn runtime() -> Result<&'static Runtime, FfiError> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = Builder::new_multi_thread()
        // Two workers: the send loop and the event loop rarely both have work at once.
        .worker_threads(2)
        .thread_name("todo-sync")
        .enable_all()
        .build()
        .map_err(|e| FfiError::Storage {
            detail: format!("cannot start the sync runtime: {e}"),
        })?;
    // Another thread may have won the race; its runtime is used and this one is dropped.
    let _ = RUNTIME.set(rt);
    RUNTIME.get().ok_or_else(|| FfiError::Storage {
        detail: "the sync runtime is gone".into(),
    })
}
