//! Sticky authentication refusal for one device pairing or one web page session.

use crate::notify::Notify;
use std::sync::{Arc, Mutex, PoisonError};

/// Shared by sync and direct authenticated requests. A new pairing uses a new latch.
#[derive(Clone, Debug, Default)]
pub struct Revocation {
    state: Arc<Mutex<State>>,
    pub(crate) changes: Notify,
}

#[derive(Debug, Default)]
struct State {
    revoked: bool,
    #[cfg(feature = "native")]
    pending_marker: Option<std::path::PathBuf>,
}

impl Revocation {
    /// Opens the marker beside a native pairing's cache. It survives a restart or a cache
    /// rebuild, but never carries over to another pairing's cache path.
    #[cfg(feature = "native")]
    pub fn for_cache(cache: &std::path::Path) -> std::io::Result<Self> {
        let marker = cache.with_added_extension("revoked");
        let revoked = match std::fs::symlink_metadata(&marker) {
            Ok(m) if todo_fs::is_plain(&m) && m.is_file() => true,
            Ok(_) => return Err(std::io::Error::other("not a plain revocation marker")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                revoked,
                pending_marker: (!revoked).then_some(marker),
            })),
            changes: Notify::default(),
        })
    }

    /// Whether this credential was refused. Connectivity and successful requests never clear it.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .revoked
    }

    /// Records a response to an authenticated request. Only a 401 counts as revocation.
    pub fn observe_status(&self, status: u16) {
        if status != 401 {
            return;
        }
        let newly_revoked = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let newly_revoked = !std::mem::replace(&mut state.revoked, true);
            #[cfg(feature = "native")]
            if let Some(marker) = &state.pending_marker {
                match persist(marker) {
                    Ok(()) => state.pending_marker = None,
                    // Stay signed out in memory and retry the write on the next refusal.
                    Err(e) => {
                        tracing::warn!(kind = ?e.kind(), "could not persist pairing revocation")
                    }
                }
            }
            newly_revoked
        };
        if newly_revoked {
            self.changes.notify();
        }
    }
}

#[cfg(feature = "native")]
fn persist(marker: &std::path::Path) -> std::io::Result<()> {
    use std::{fs, path::Path};
    let parent = marker
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = fs::symlink_metadata(parent)?;
    if !todo_fs::is_plain(&metadata) || !metadata.is_dir() {
        return Err(std::io::Error::other("not a plain pairing directory"));
    }
    // Do not create the parent: the pairing may have been removed while the request was in flight.
    let tmp = marker.with_added_extension(uuid::Uuid::new_v4().simple().to_string());
    let file = todo_fs::create_private_file(&tmp)?;
    let flushed = file.sync_all();
    drop(file);
    let result = flushed.and_then(|()| todo_fs::replace_durably(&tmp, marker));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
