//! Where the shell's data comes from: the live sync session, or a local state (the `--screenshot`
//! fixtures, which never talk to a server). Both take commands the same way, so a fixture window is fully
//! interactive too.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use todo_client::outbox::{FailedEntry, Rejection};
use todo_client::{CommandId, Connection, ErrorKind, TrackedCommand, TrackedUndo};
use todo_core::event::Event;
use todo_core::time::Timestamp;
use todo_core::undo::undo_batch;
use todo_core::{Actor, Command, State, UndoOptions, apply, decide};
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
    /// The fixture's "couldn't sync" list, which Dismiss empties.
    failed: RefCell<Vec<FailedEntry>>,
    next: Cell<u64>,
    /// Tests: refuse every submit, as a cache that can't be written would.
    #[cfg(test)]
    refuse: Cell<bool>,
}

#[derive(Debug)]
pub enum UndoTarget {
    Local(Vec<Event>),
    Live(TrackedCommand),
}

impl LocalStore {
    fn submit(&self, cmd: &Command) -> Result<Vec<Event>, String> {
        #[cfg(test)]
        if self.refuse.get() {
            return Err("storage backend error: disk full".into());
        }
        let actor = Actor::owner("desktop").map_err(|e| e.to_string())?;
        let mut state = (**self.state.borrow()).clone();
        let events = decide(&state, &actor, cmd, self.now.at).map_err(|e| e.to_string())?;
        for e in &events {
            apply(&mut state, e).map_err(|e| e.to_string())?;
        }
        *self.state.borrow_mut() = Arc::new(state);
        Ok(events)
    }
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
            failed: RefCell::new(f.failed.clone()),
            next: Cell::new(0),
            #[cfg(test)]
            refuse: Cell::new(false),
        })
    }

    /// Tests: makes a local store refuse submits (a full disk, a locked cache).
    #[cfg(test)]
    pub fn refuse_submits(&self, refuse: bool) {
        if let Self::Local(s) = self {
            s.refuse.set(refuse);
        }
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
            Self::Local(s) => s.submit(&cmd).map(drop),
            Self::Live(s) => s
                .session
                .client()
                .submit(cmd)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    pub fn submit_undoable(&self, cmd: Command) -> Result<UndoTarget, String> {
        match self {
            Self::Local(s) => s.submit(&cmd).map(UndoTarget::Local),
            Self::Live(s) => s
                .session
                .client()
                .submit_tracked(cmd)
                .map(UndoTarget::Live)
                .map_err(|e| e.to_string()),
        }
    }

    pub fn undo(&self, target: &UndoTarget) -> Result<TrackedUndo, String> {
        match (self, target) {
            (Self::Local(s), UndoTarget::Local(events)) => {
                let actor = Actor::owner("desktop").map_err(|e| e.to_string())?;
                let mut state = (**s.state.borrow()).clone();
                let inverse = undo_batch(&state, events, &actor, s.now.at, UndoOptions::default())
                    .map_err(|e| e.to_string())?;
                for event in &inverse {
                    apply(&mut state, event).map_err(|e| e.to_string())?;
                }
                *s.state.borrow_mut() = Arc::new(state);
                Ok(TrackedUndo::Cancelled)
            }
            (Self::Live(s), UndoTarget::Live(t)) => s
                .session
                .client()
                .undo_tracked(t)
                .map_err(|e| e.to_string()),
            _ => Ok(TrackedUndo::Unavailable),
        }
    }

    /// None while the compensation is pending, otherwise its outcome.
    pub fn undo_result(&self, id: CommandId) -> Option<Result<(), String>> {
        let client = self.session()?.client();
        if client.pending().iter().any(|entry| entry.command_id == id) {
            return None;
        }
        Some(
            client
                .failed()
                .iter()
                .find(|entry| entry.entry.command_id == id)
                .map_or(Ok(()), |err| Err(undo_failure_message(&err.rejection))),
        )
    }

    /// The changes the server refused ("couldn't sync"), oldest first.
    pub fn failed(&self) -> Vec<FailedEntry> {
        match self {
            Self::Local(s) => s.failed.borrow().clone(),
            Self::Live(s) => s.session.client().failed(),
        }
    }

    /// Takes a refused change off the "couldn't sync" list. Returns whether it was there.
    pub fn dismiss_failed(&self, id: CommandId) -> Result<bool, String> {
        match self {
            Self::Local(s) => {
                let mut failed = s.failed.borrow_mut();
                let before = failed.len();
                failed.retain(|f| f.entry.command_id != id);
                Ok(failed.len() != before)
            }
            Self::Live(s) => s.session.client().dismiss(id).map_err(|e| e.to_string()),
        }
    }

    /// Empties the "couldn't sync" list. Returns how many it held.
    pub fn dismiss_all_failed(&self) -> Result<usize, String> {
        match self {
            Self::Local(s) => Ok(std::mem::take(&mut *s.failed.borrow_mut()).len()),
            Self::Live(s) => s.session.client().dismiss_all().map_err(|e| e.to_string()),
        }
    }

    /// The shell has reported this refusal; it isn't an ordinary change to retry.
    pub fn dismiss_undo_failure(&self, id: CommandId) -> bool {
        self.session().is_none_or(|s| match s.client().dismiss(id) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(%error, "couldn't dismiss the reported undo failure");
                false
            }
        })
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
            Self::Local(s) => SyncInfo {
                failed: s.failed.borrow().len(),
                ..s.sync.clone()
            },
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

    /// The server the app is paired with, as learnt at pairing. A fixture that shows a paired app names
    /// the placeholder server.
    pub fn server(&self) -> Option<String> {
        match self {
            Self::Local(s) => s
                .sync
                .paired
                .then(|| crate::fixtures::PAIRING_SERVER.to_string()),
            Self::Live(s) => Some(s.session.server().to_string()),
        }
    }

    pub fn session(&self) -> Option<&Session> {
        match self {
            Self::Local(_) => None,
            Self::Live(s) => Some(&s.session),
        }
    }
}

fn undo_failure_message(rejection: &Rejection) -> String {
    match rejection.code.as_str() {
        "undo_conflict" => "Couldn't undo: the task was changed again.".into(),
        "undo_no_event" => "Couldn't undo: the original change couldn't be found.".into(),
        "epoch_mismatch" | "event_mismatch" => {
            "Couldn't undo: the server's history changed.".into()
        }
        "not_found" => "Couldn't undo: the task no longer exists.".into(),
        _ => "Couldn't undo that.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use todo_core::state::Scope;

    #[test]
    fn undo_failures_never_show_raw_server_text() {
        for code in [
            "undo_conflict",
            "undo_no_event",
            "epoch_mismatch",
            "event_mismatch",
            "not_found",
            "something_new",
        ] {
            let rejection = Rejection {
                code: code.into(),
                message: "resync from seq 0".into(),
                status: 409,
            };
            let message = undo_failure_message(&rejection);
            assert!(message.starts_with("Couldn't undo"), "{code}: {message}");
            assert!(!message.contains("seq"), "{code}: {message}");
        }
    }

    #[test]
    fn a_fixtures_refused_changes_are_listed_counted_and_dismissed() {
        let mut f = crate::fixtures::my_day();
        f.failed = crate::fixtures::failed_changes(&f.state);
        let store = Store::fixture(&f);
        assert_eq!(store.failed().len(), 3);
        assert_eq!(
            store.sync().failed,
            3,
            "the sidebar's count follows the list"
        );
        let first = store.failed()[0].entry.command_id;
        assert_eq!(store.dismiss_failed(first), Ok(true));
        assert_eq!(store.dismiss_failed(first), Ok(false));
        assert_eq!(store.sync().failed, 2);
        assert!(store.failed().iter().all(|e| e.entry.command_id != first));
        assert_eq!(store.dismiss_all_failed(), Ok(2));
        assert_eq!(store.sync().failed, 0);
        assert_eq!(store.dismiss_all_failed(), Ok(0));
    }

    proptest! {
        /// Deleting any task, or moving it to any other list, and undoing it puts back exactly what was
        /// there: the task, its list and its place in it, and everything else as it was.
        #[test]
        fn local_undo_of_a_task_delete_or_move_restores_it_exactly(
            index in any::<usize>(),
            to in any::<usize>(),
            delete in any::<bool>(),
        ) {
            use crate::model::commands;
            let store = Store::fixture(&crate::fixtures::work());
            let before = store.view();
            let tasks: Vec<_> = before.tasks.values().filter(|t| before.is_live(t.id.into())).collect();
            let task = tasks[index % tasks.len()];
            let lists: Vec<_> = before.lists.values()
                .filter(|l| before.is_live(l.id.into()) && l.id != task.list)
                .map(|l| l.id)
                .collect();
            let cmd = if delete {
                commands::delete_task(&before, task.id)
            } else {
                commands::move_to_list(&before, task.id, lists[to % lists.len()])
            };
            let undo = store.submit_undoable(cmd.expect("a change")).expect("valid");
            prop_assert_ne!(store.view().entity(task.id.into()), before.entity(task.id.into()));
            store.undo(&undo).expect("valid inverse");
            let after = store.view();
            prop_assert!(after.data_eq(&before));
            prop_assert_eq!(after.entity(task.id.into()), before.entity(task.id.into()));
            let scope = Scope::Tasks(task.list);
            prop_assert_eq!(after.siblings(scope), before.siblings(scope));
        }

        #[test]
        fn local_undo_restores_the_exact_order(index in any::<usize>()) {
            let store = Store::fixture(&crate::fixtures::my_day());
            let before = store.view();
            let scope = before.lists.keys().copied().map(Scope::Tasks)
                .find(|&scope| before.siblings(scope).len() > 1).expect("a populated list");
            let order = before.siblings(scope);
            let id = order[1 + index % (order.len() - 1)].1;
            let move_up = crate::model::commands::nudge(&before, id, -1).expect("task can move up");
            let undo = store.submit_undoable(move_up).expect("valid reorder");
            prop_assert_ne!(store.view().siblings(scope), order.clone());
            store.undo(&undo).expect("valid inverse");
            prop_assert_eq!(store.view().siblings(scope), order);
        }

        /// Deleting any list or step and undoing it puts back exactly what was there: its place among its
        /// siblings, its settings and its collection or task. (The live store's unsent and
        /// acknowledged paths are in `session::tests`.)
        #[test]
        fn local_undo_of_a_list_or_step_delete_restores_it_exactly(index in any::<usize>(), step in any::<bool>()) {
            use todo_core::ids::EntityId;
            let store = Store::fixture(&crate::fixtures::work());
            let before = store.view();
            let ids: Vec<EntityId> = if step {
                before.steps.values().filter(|s| before.is_live(s.id.into())).map(|s| s.id.into()).collect()
            } else {
                before.lists.values().filter(|l| before.is_live(l.id.into())).map(|l| l.id.into()).collect()
            };
            let id = ids[index % ids.len()];
            let undo = store.submit_undoable(Command::Delete { id }).expect("delete");
            prop_assert!(!store.view().is_live(id));
            store.undo(&undo).expect("valid inverse");
            let after = store.view();
            prop_assert!(after.data_eq(&before));
            prop_assert_eq!(after.entity(id), before.entity(id));
        }
    }
}
