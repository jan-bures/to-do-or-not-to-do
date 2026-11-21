//! Where the shell's data comes from: the live sync session, or a local state (the `--screenshot`
//! fixtures, which never talk to a server). Both take commands the same way, so a fixture window is fully
//! interactive too.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use todo_client::{Connection, ErrorKind};
use todo_core::time::Timestamp;
use todo_core::{Actor, Command, State, apply, decide};
use uuid::Uuid;

use crate::fixtures::Fixture;
use crate::model::{Now, SyncInfo};
use crate::session::Session;

/// A fixture's data, changed in memory by the commands the window submits.
#[allow(missing_debug_implementations)]
pub struct LocalStore {
    state: RefCell<Arc<State>>,
    now: Now,
    sync: SyncInfo,
    next: Cell<u64>,
}

/// The live session, plus when it was last in sync.
#[allow(missing_debug_implementations)]
pub struct LiveStore {
    pub session: Session,
    synced_at: Cell<Option<Timestamp>>,
}

impl LiveStore {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            synced_at: Cell::new(None),
        }
    }
}

#[allow(missing_debug_implementations)]
pub enum Store {
    Local(LocalStore),
    Live(LiveStore),
}

impl Store {
    pub fn fixture(f: &Fixture) -> Self {
        Self::Local(LocalStore {
            state: RefCell::new(Arc::new(f.state.clone())),
            now: f.now.clone(),
            sync: f.sync.clone(),
            next: Cell::new(0),
        })
    }

    /// The state to show: the server's, with this device's pending changes applied.
    pub fn view(&self) -> Arc<State> {
        match self {
            Self::Local(s) => s.state.borrow().clone(),
            Self::Live(s) => s.session.client().view(),
        }
    }

    /// Applies a command at once and (live) queues it for the server. An error is a message for the
    /// user; nothing was changed then.
    pub fn submit(&self, cmd: Command) -> Result<(), String> {
        match self {
            Self::Local(s) => {
                let actor = Actor::owner("desktop").map_err(|e| e.to_string())?;
                let mut state = (**s.state.borrow()).clone();
                let events = decide(&state, &actor, &cmd, s.now.at).map_err(|e| e.to_string())?;
                for e in &events {
                    apply(&mut state, e).map_err(|e| e.to_string())?;
                }
                *s.state.borrow_mut() = Arc::new(state);
                Ok(())
            }
            Self::Live(s) => s
                .session
                .client()
                .submit(cmd)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    /// A fresh ID for something a command creates.
    pub fn new_uuid(&self) -> Uuid {
        match self {
            Self::Local(s) => {
                let n = s.next.get() + 1;
                s.next.set(n);
                let ms = u64::try_from(s.now.at.as_millisecond()).unwrap_or(0) + n;
                let mut rand = [0u8; 10];
                rand[..8].copy_from_slice(&n.to_le_bytes());
                rand[8] = 0xd5;
                todo_core::ids::uuid_v7(ms, rand)
            }
            Self::Live(s) => s.session.client().new_uuid(),
        }
    }

    pub fn now(&self) -> Now {
        match self {
            Self::Local(s) => s.now.clone(),
            Self::Live(_) => Now::system(),
        }
    }

    /// The sync status for the footer.
    pub fn sync(&self) -> SyncInfo {
        match self {
            Self::Local(s) => s.sync.clone(),
            Self::Live(s) => {
                let status = s.session.client().status();
                if status.connection == Connection::Online {
                    s.synced_at.set(Some(Timestamp::now()));
                }
                SyncInfo {
                    paired: true,
                    connection: status.connection,
                    pending: status.pending,
                    failed: status.failed,
                    unauthorized: status
                        .last_error
                        .as_ref()
                        .is_some_and(|e| e.kind == ErrorKind::Unauthorized),
                    synced_at: s.synced_at.get(),
                }
            }
        }
    }

    pub fn session(&self) -> Option<&Session> {
        match self {
            Self::Local(_) => None,
            Self::Live(s) => Some(&s.session),
        }
    }
}
