//! End-to-end sync against an in-process `todo-server` over real HTTP and SSE.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used)]

mod server;

use server::*;
use std::time::Duration;
use todo_client::{Connection, MemoryStorage, Replica, TrackedUndo};
use todo_core::Command;
use todo_core::command::{NewTask, Placement, TaskPatch};
use todo_core::ids::{ListId, TaskId};
use todo_core::recurrence::{Recurrence, Rule};

const WAIT: Duration = Duration::from_secs(20);

fn new_list<T: todo_client::Transport>(c: &Client<T>, name: &str) -> (ListId, Command) {
    let id = ListId::from_uuid(c.new_uuid());
    (
        id,
        Command::CreateList {
            id,
            collection: None,
            name: name.into(),
            color: None,
            placement: Placement::Last,
        },
    )
}

fn new_task<T: todo_client::Transport>(
    c: &Client<T>,
    list: ListId,
    title: &str,
) -> (TaskId, Command) {
    let id = TaskId::from_uuid(c.new_uuid());
    (
        id,
        Command::CreateTask(Box::new(NewTask::new(id, list, title))),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_clients_converge_and_see_each_others_edits_through_the_stream() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));

    let (list, cmd) = new_list(&a, "Groceries");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "Milk");
    a.submit(cmd).unwrap();
    assert!(a.view().tasks.contains_key(&task), "optimistic");

    eventually("b sees a's task", WAIT, || {
        b.view().tasks.contains_key(&task)
    })
    .await;
    b.submit(Command::CompleteTask { id: task }).unwrap();
    eventually("a sees b's completion", WAIT, || {
        a.view().tasks[&task].completed_at.is_some()
    })
    .await;

    eventually("both converge", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    eventually("both online", WAIT, || {
        a.status().connection == Connection::Online && b.status().connection == Connection::Online
    })
    .await;
    assert_eq!(a.status().pending, 0);
    assert_eq!(a.status().last_error, None);
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edits_made_offline_sync_after_reconnecting() {
    let server = TestServer::start().await;
    let ta = Switch::new(server.transport(TOKEN_A));
    let a = client(ta.clone(), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));
    let (list, cmd) = new_list(&a, "Inbox");
    a.submit(cmd).unwrap();
    // a must have settled its own command before going offline, or it would still be pending.
    eventually("both have the list", WAIT, || {
        b.view().lists.contains_key(&list) && converged(&a, &server)
    })
    .await;

    ta.set_online(false);
    eventually("a is offline", WAIT, || {
        a.status().connection == Connection::Offline
    })
    .await;
    let mut tasks = Vec::new();
    for i in 0..5 {
        let (t, cmd) = new_task(&a, list, &format!("offline {i}"));
        a.submit(cmd).unwrap();
        tasks.push(t);
    }
    a.submit(Command::AddNote {
        task: tasks[0],
        text: "note".into(),
    })
    .unwrap();
    // Meanwhile b keeps working.
    let (bt, cmd) = new_task(&b, list, "from b");
    b.submit(cmd).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.status().pending, 6);
    assert!(a.view().tasks.contains_key(&tasks[4]));
    assert!(!server.state().tasks.contains_key(&tasks[0]));

    ta.set_online(true);
    a.nudge();
    eventually("both converge", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    let state = server.state();
    assert!(tasks.iter().all(|t| state.tasks.contains_key(t)));
    assert!(state.tasks.contains_key(&bt));
    assert_eq!(state.tasks[&tasks[0]].notes.matches("note").count(), 1);
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_command_the_server_rejects_lands_in_couldnt_sync() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let tb = Switch::new(server.transport(TOKEN_B));
    let b = client(tb.clone(), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));
    let (list, cmd) = new_list(&a, "Shared");
    a.submit(cmd).unwrap();
    eventually("b has the list", WAIT, || {
        b.view().lists.contains_key(&list)
    })
    .await;

    tb.set_online(false);
    a.submit(Command::Delete { id: list.into() }).unwrap();
    eventually("a's delete is stored", WAIT, || converged(&a, &server)).await;
    // b still sees the list and adds a task to it.
    let (task, cmd) = new_task(&b, list, "orphan");
    let id = b.submit(cmd).unwrap();
    assert!(b.view().tasks.contains_key(&task));

    tb.set_online(true);
    b.nudge();
    eventually("b's command is refused", WAIT, || b.failed().len() == 1).await;
    eventually("b converges", WAIT, || converged(&b, &server)).await;
    let failed = &b.failed()[0];
    assert_eq!(failed.entry.command_id, id);
    assert_eq!(failed.rejection.status, 409);
    assert_eq!(failed.rejection.code, "deleted");
    assert_eq!(b.status().failed, 1);
    assert!(!b.view().tasks.contains_key(&task));
    assert!(b.dismiss(id).unwrap());
    assert_eq!(b.status().failed, 0);
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cursor_ahead_of_the_server_triggers_a_full_resync() {
    let storage = MemoryStorage::new();
    let first = TestServer::start().await;
    let a = client(first.transport(TOKEN_A), storage.clone(), alpha());
    let h = spawn(&a);
    for i in 0..4 {
        let (_, cmd) = new_list(&a, &format!("old {i}"));
        a.submit(cmd).unwrap();
    }
    eventually("synced with the first server", WAIT, || {
        converged(&a, &first)
    })
    .await;
    stop(&a, h).await;
    first.stop().await;

    // A server whose log is shorter than the cursor (for example restored from an old backup).
    let second = TestServer::start().await;
    let seed = client(second.transport(TOKEN_B), MemoryStorage::new(), beta());
    let hs = spawn(&seed);
    let (kept, cmd) = new_list(&seed, "on the new server");
    seed.submit(cmd).unwrap();
    eventually("seeded", WAIT, || converged(&seed, &second)).await;
    stop(&seed, hs).await;

    let a = client(
        Switch::new(second.transport(TOKEN_A)),
        storage.clone(),
        alpha(),
    );
    assert_eq!(a.cursor(), 4);
    let (queued, cmd) = new_list(&a, "queued before reconnecting");
    a.submit(cmd).unwrap();
    let h = spawn(&a);
    eventually("resynced", WAIT, || converged(&a, &second)).await;
    let view = a.view();
    assert!(view.lists.contains_key(&kept));
    assert!(
        view.lists.contains_key(&queued),
        "the outbox survived the resync"
    );
    assert_eq!(view.lists.len(), 2, "nothing from the old log");
    stop(&a, h).await;
    let again = Replica::open(alpha(), Box::new(storage)).unwrap();
    assert!(again.confirmed().data_eq(&second.state()));
    second.stop().await;
}

async fn restart_resumes(storage: impl todo_client::Storage + Clone + 'static) {
    let server = TestServer::start().await;
    let t = Switch::new(server.transport(TOKEN_A));
    let a = client(t.clone(), storage.clone(), alpha());
    let h = spawn(&a);
    let (list, cmd) = new_list(&a, "Persisted");
    a.submit(cmd).unwrap();
    for i in 0..10 {
        let (_, cmd) = new_task(&a, list, &format!("t{i}"));
        a.submit(cmd).unwrap();
    }
    eventually("synced", WAIT, || converged(&a, &server)).await;
    t.set_online(false);
    let (pending, cmd) = new_task(&a, list, "queued while offline");
    a.submit(cmd).unwrap();
    stop(&a, h).await;
    let cursor = a.cursor();
    drop(a);

    let t = Switch::new(server.transport(TOKEN_A));
    let a = client(t.clone(), storage, alpha());
    assert_eq!(a.cursor(), cursor, "the cursor survived");
    assert_eq!(a.pending().len(), 1, "the outbox survived");
    assert!(a.view().tasks.contains_key(&pending));
    let h = spawn(&a);
    eventually("synced after the restart", WAIT, || converged(&a, &server)).await;
    assert!(server.state().tasks.contains_key(&pending));
    let first_stream = t
        .paths
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.starts_with("/api/v1/events?since=") && !p.contains("limit"))
        .cloned()
        .unwrap();
    assert_eq!(
        first_stream,
        format!("/api/v1/events?since={cursor}&epoch={}", server.epoch())
    );
    stop(&a, h).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_resumes_from_the_cursor_and_keeps_the_outbox() {
    restart_resumes(MemoryStorage::new()).await;
}

#[cfg(feature = "sqlite")]
#[derive(Clone)]
struct SqliteFile(std::sync::Arc<tempfile::TempDir>);

#[cfg(feature = "sqlite")]
impl todo_client::Storage for SqliteFile {
    fn load(&mut self) -> Result<todo_client::storage::Loaded, todo_client::StorageError> {
        self.open().load()
    }

    fn write(
        &mut self,
        ops: &[todo_client::storage::StorageOp<'_>],
    ) -> Result<(), todo_client::StorageError> {
        self.open().write(ops)
    }
}

#[cfg(feature = "sqlite")]
impl SqliteFile {
    fn open(&self) -> todo_client::sqlite::SqliteStorage {
        todo_client::sqlite::SqliteStorage::open(&self.0.path().join("client.db")).unwrap()
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_on_sqlite_resumes_from_the_cursor_and_keeps_the_outbox() {
    restart_resumes(SqliteFile(std::sync::Arc::new(
        tempfile::tempdir().unwrap(),
    )))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_far_behind_catches_up_through_pages() {
    let server = TestServer::start_with(|c| {
        c.limits.max_replay_events = 3;
        c.limits.max_replay_bytes = 4096;
    })
    .await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let ha = spawn(&a);
    let (list, cmd) = new_list(&a, "Busy");
    a.submit(cmd).unwrap();
    for i in 0..30 {
        let (_, cmd) = new_task(&a, list, &format!("task {i} {}", "x".repeat(200)));
        a.submit(cmd).unwrap();
    }
    eventually("a synced", WAIT, || converged(&a, &server)).await;
    // b starts from 0: 409 catch_up_required at connect, then pages of 7.
    let tb = Switch::new(server.transport(TOKEN_B));
    let b = client(tb.clone(), MemoryStorage::new(), beta());
    let hb = spawn(&b);
    eventually("b caught up", WAIT, || converged(&b, &server)).await;
    let pages = |t: &Switch<_>| {
        t.paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("limit="))
            .count()
    };
    assert!(pages(&tb) >= 4, "31 events in pages of 7");
    let before = pages(&tb);
    // An event larger than the stream's byte budget while b is live: the server sends a `catch_up`
    // frame instead, and b fetches it through a page.
    let (task, cmd) = new_task(&a, list, "big");
    a.submit(cmd).unwrap();
    a.submit(Command::AddNote {
        task,
        text: "y".repeat(8000),
    })
    .unwrap();
    eventually("b followed", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    assert!(pages(&tb) > before, "the stream sent `catch_up`");
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

/// A page can end inside a command's batch: here every page holds one event and every completion of a
/// repeating task makes two. The client holds the start of a batch and pages on after it, so it never
/// stalls and never shows half a completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pages_that_end_inside_a_batch_still_catch_up() {
    let server = TestServer::start_with(|c| c.limits.max_replay_events = 1).await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let ha = spawn(&a);
    let (list, cmd) = new_list(&a, "Repeats");
    a.submit(cmd).unwrap();
    for i in 0..3 {
        let task = TaskId::from_uuid(a.new_uuid());
        let mut repeat = NewTask::new(task, list, format!("repeat {i}"));
        repeat.due = Some("2026-11-21".parse().unwrap());
        repeat.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
        a.submit(Command::CreateTask(Box::new(repeat))).unwrap();
        a.submit(Command::CompleteTask { id: task }).unwrap();
    }
    eventually("a synced", WAIT, || converged(&a, &server)).await;
    assert_eq!(
        server.head(),
        10,
        "a list, then per task a creation and two events"
    );

    let mut cfg = config(beta());
    cfg.page_limit = 1;
    let tb = Switch::new(server.transport(TOKEN_B));
    let b = client_with(tb.clone(), MemoryStorage::new(), cfg);
    let hb = spawn(&b);
    eventually("b caught up", WAIT, || converged(&b, &server)).await;
    let pages = tb
        .paths
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.contains("limit="))
        .count();
    assert!(pages >= 10, "one event per page: {pages}");
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undo_goes_through_the_outbox_and_reaches_the_other_client() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));
    let (list, cmd) = new_list(&a, "Undo");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "rename me");
    a.submit(cmd).unwrap();
    a.submit(Command::UpdateTask {
        id: task,
        patch: todo_core::command::TaskPatch {
            title: Some("renamed".into()),
            ..Default::default()
        },
    })
    .unwrap();
    eventually("b sees the rename", WAIT, || {
        b.view()
            .tasks
            .get(&task)
            .is_some_and(|t| t.title == "renamed")
    })
    .await;
    // B didn't submit the rename, so there is no tracked local command to undo. The feed
    // supplies the identity for a checked compensation of the authoritative batch instead.
    let page = b
        .activity(&todo_client::activity::ActivityQuery {
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    let item = &page.items[0];
    assert!(item.can_undo);
    assert!(item.what_happened().contains("renamed"));
    let event = item.event_ref().unwrap();
    assert_eq!(event.seq, server.head());
    b.set_sending(false);
    let undo_id = b.undo_activity(event).unwrap();
    assert_eq!(b.pending_undo(event.id), Some(undo_id));
    assert_eq!(b.undo_activity(event).unwrap(), undo_id);
    assert_eq!(b.pending().len(), 1, "the same undo isn't queued twice");
    assert_eq!(b.undo_result(undo_id), None, "still queued");
    assert_eq!(b.activity_confirmed(event.id, false), Some((false, true)));
    b.set_sending(true);
    eventually("b sees the undo", WAIT, || {
        b.view().tasks[&task].title == "rename me"
    })
    .await;
    eventually("converged", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    assert_eq!(b.undo_result(undo_id), Some(Ok(())));
    assert_eq!(b.activity_confirmed(event.id, false), Some((true, false)));
    assert!(
        server
            .stored_command_ids()
            .contains(&(undo_id.to_string(), "undo".to_owned()))
    );
    // Undoing the same event again is refused for good: it is already undone.
    let again = b.undo_activity(event).unwrap();
    eventually("refused", WAIT, || b.failed().len() == 1).await;
    assert_eq!(b.failed()[0].rejection.code, "already_undone");
    assert_eq!(
        b.undo_result(again),
        Some(Err("Couldn't undo: the change was already undone."))
    );
    // Only the outbox and the "couldn't sync" list are consulted: once dismissed, or for a command
    // this client never queued, the answer is no longer a refusal. It isn't proof of acceptance.
    assert!(b.dismiss(again).unwrap());
    assert_eq!(b.undo_result(again), Some(Ok(())));
    assert_eq!(b.undo_result(todo_client::CommandId::nil()), Some(Ok(())));
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activity_undo_refuses_a_later_edit_of_the_same_field() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));
    let (list, cmd) = new_list(&a, "Undo conflict");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "before the edits");
    a.submit(cmd).unwrap();
    let rename = |title: &str| Command::UpdateTask {
        id: task,
        patch: TaskPatch {
            title: Some(title.into()),
            ..Default::default()
        },
    };
    a.submit(rename("first edit")).unwrap();
    eventually("first edit synced", WAIT, || converged(&a, &server)).await;
    let page = b
        .activity(&todo_client::activity::ActivityQuery {
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    let event = page.items[0].event_ref().unwrap();

    a.submit(rename("later edit")).unwrap();
    eventually("both see the later edit", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    let undo = b.undo_activity(event).unwrap();
    eventually("undo refused", WAIT, || !b.failed().is_empty()).await;
    let failure = &b.failed()[0];
    assert_eq!(failure.entry.command_id, undo);
    assert_eq!(failure.rejection.code, "undo_conflict");
    assert_eq!(
        b.undo_result(undo),
        Some(Err("Couldn't undo: the item was changed again."))
    );
    assert_eq!(b.pending_undo(event.id), None);
    assert_eq!(b.view().tasks[&task].title, "later edit");
    assert_eq!(server.state().tasks[&task].title, "later edit");
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tracked_undo_restores_a_repeat_as_a_unit_but_refuses_a_conflicting_remote_completion() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let (ha, hb) = (spawn(&a), spawn(&b));
    let (list, make_list) = new_list(&a, "Undo tracked");
    a.submit(make_list).unwrap();
    let task = TaskId::from_uuid(a.new_uuid());
    let mut repeat = NewTask::new(task, list, "Water seedlings");
    repeat.due = Some("2026-11-21".parse().unwrap());
    repeat.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    a.submit(Command::CreateTask(Box::new(repeat))).unwrap();
    let complete = Command::CompleteTask { id: task };
    let first = a.submit_tracked(complete.clone()).unwrap();
    eventually("another device sees the whole repeat batch", WAIT, || {
        converged(&a, &server) && converged(&b, &server) && b.view().tasks.len() == 2
    })
    .await;
    b.submit(Command::UpdateTask {
        id: task,
        patch: TaskPatch {
            title: Some("Water the seedlings after lunch".into()),
            ..TaskPatch::default()
        },
    })
    .unwrap();
    eventually("a sees the unrelated edit", WAIT, || {
        a.view().tasks[&task].title.ends_with("lunch")
    })
    .await;
    a.set_sending(false);
    assert!(matches!(
        a.undo_tracked(&first).unwrap(),
        TrackedUndo::Queued(_)
    ));
    drop(first);
    assert!(!a.view().tasks[&task].is_done());
    assert_eq!(
        a.view().tasks.len(),
        1,
        "the queued inverse includes the spawned repeat"
    );
    assert!(server.state().tasks[&task].is_done(), "not yet sent");
    a.set_sending(true);
    eventually("the exact inverse reached both clients", WAIT, || {
        converged(&a, &server) && converged(&b, &server) && !b.view().tasks[&task].is_done()
    })
    .await;
    assert_eq!(b.view().tasks.len(), 1, "the spawned repeat is gone too");
    assert_eq!(
        b.view().tasks[&task].title,
        "Water the seedlings after lunch"
    );

    // A newer completion of the same task must not be undone, even though the task looks done again.
    let second = a.submit_tracked(complete.clone()).unwrap();
    eventually("b sees the new completion", WAIT, || {
        b.view().tasks[&task].is_done()
    })
    .await;
    b.submit(Command::UncompleteTask { id: task }).unwrap();
    b.submit(complete).unwrap();
    eventually("the remote completion settles", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    a.set_sending(false);
    let TrackedUndo::Queued(undo) = a.undo_tracked(&second).unwrap() else {
        panic!("the undo should be queued")
    };
    assert!(
        a.view().tasks[&task].is_done(),
        "an undo that would conflict is not shown"
    );
    a.set_sending(true);
    eventually("server refuses the conflicting undo", WAIT, || {
        a.failed().iter().any(|f| f.entry.command_id == undo)
    })
    .await;
    assert_eq!(a.failed()[0].rejection.code, "undo_conflict");
    assert!(a.view().tasks[&task].is_done());
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undo_can_cancel_the_latest_unsent_command() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let (list, make_list) = new_list(&a, "Undo unsent");
    a.submit(make_list).unwrap();
    let (task, make_task) = new_task(&a, list, "Send invoice");
    a.submit(make_task).unwrap();
    // There is no send loop yet. Undo must take back just the local completion.
    let done = a
        .submit_tracked(Command::CompleteTask { id: task })
        .unwrap();
    assert!(a.view().tasks[&task].is_done());
    assert_eq!(a.undo_tracked(&done).unwrap(), TrackedUndo::Cancelled);
    assert!(!a.view().tasks[&task].is_done());
    assert_eq!(a.pending().len(), 2);
    let ha = spawn(&a);
    eventually("remaining work syncs", WAIT, || converged(&a, &server)).await;
    assert!(!server.state().tasks[&task].is_done());
    assert_eq!(server.head(), 2);
    stop(&a, ha).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undo_of_a_possibly_sent_completion_updates_the_view_and_survives_restart() {
    let server = TestServer::start().await;
    let storage = MemoryStorage::new();
    let transport = Switch::new(server.transport(TOKEN_A));
    let a = client(transport.clone(), storage.clone(), alpha());
    let ha = spawn(&a);
    let (list, make_list) = new_list(&a, "Undo in flight");
    a.submit(make_list).unwrap();
    let (task, make_task) = new_task(&a, list, "Post the letters");
    a.submit(make_task).unwrap();
    eventually("task is confirmed", WAIT, || converged(&a, &server)).await;

    transport.set_online(false);
    let tracked = a
        .submit_tracked(Command::CompleteTask { id: task })
        .unwrap();
    eventually("the completion may have been sent", WAIT, || {
        a.pending().first().is_some_and(|e| e.attempts > 0)
    })
    .await;
    let TrackedUndo::Queued(id) = a.undo_tracked(&tracked).unwrap() else {
        panic!("an attempted command cannot be cancelled");
    };
    assert!(!a.view().tasks[&task].is_done(), "undo is visible offline");
    drop(tracked);
    stop(&a, ha).await;
    drop(a);

    let a = client(server.transport(TOKEN_A), storage, alpha());
    assert!(
        !a.view().tasks[&task].is_done(),
        "unresolved undo survives restart"
    );
    let ha = spawn(&a);
    eventually("completion and undo both reach the server", WAIT, || {
        server.head() == 4 && converged(&a, &server)
    })
    .await;
    assert!(!a.view().tasks[&task].is_done());
    assert!(
        server
            .stored_command_ids()
            .contains(&(id.to_string(), "undo".into()))
    );
    stop(&a, ha).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undo_of_a_confirmed_completion_shows_after_an_offline_restart() {
    let server = TestServer::start().await;
    let storage = MemoryStorage::new();
    let transport = Switch::new(server.transport(TOKEN_A));
    let a = client(transport.clone(), storage.clone(), alpha());
    let ha = spawn(&a);
    let (list, make_list) = new_list(&a, "Undo after restart");
    a.submit(make_list).unwrap();
    let task = TaskId::from_uuid(a.new_uuid());
    let mut repeat = NewTask::new(task, list, "Water seedlings");
    repeat.due = Some("2026-11-21".parse().unwrap());
    repeat.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    a.submit(Command::CreateTask(Box::new(repeat))).unwrap();
    let done = a
        .submit_tracked(Command::CompleteTask { id: task })
        .unwrap();
    eventually("the completion and its repeat are confirmed", WAIT, || {
        converged(&a, &server) && a.view().tasks.len() == 2
    })
    .await;

    transport.set_online(false);
    let TrackedUndo::Queued(_) = a.undo_tracked(&done).unwrap() else {
        panic!("a confirmed completion is undone through the outbox");
    };
    let undone = |view: &todo_core::State| !view.tasks[&task].is_done() && view.tasks.len() == 1;
    assert!(undone(&a.view()), "the undo shows offline");
    drop(done);
    stop(&a, ha).await;
    drop(a);

    // Restarted, still offline: the original's events come from storage, not from the session.
    let a = client(server.transport(TOKEN_A), storage, alpha());
    assert!(undone(&a.view()), "the queued undo survives the restart");
    let ha = spawn(&a);
    eventually("the undo reaches the server", WAIT, || {
        converged(&a, &server) && !server.state().tasks[&task].is_done()
    })
    .await;
    assert!(undone(&a.view()));
    assert!(a.failed().is_empty());
    stop(&a, ha).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_that_regrew_is_detected_by_the_epoch_and_a_stale_undo_is_refused() {
    let storage = MemoryStorage::new();
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), storage.clone(), alpha());
    let h = spawn(&a);
    let (list, cmd) = new_list(&a, "L");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "before");
    a.submit(cmd).unwrap();
    eventually("synced", WAIT, || converged(&a, &server)).await;
    stop(&a, h).await;
    let (dir, config) = server.halt().await;
    let backup = dir.path().join("backup.sqlite");
    copy_db(&config.database_path(), &backup);
    let server = TestServer::boot(dir, config).await;
    let old_epoch = server.epoch();

    // A rename the restore will lose; the client follows it.
    let a = client(server.transport(TOKEN_A), storage.clone(), alpha());
    let h = spawn(&a);
    a.submit(rename(task, "lost")).unwrap();
    eventually("followed the rename", WAIT, || converged(&a, &server)).await;
    let lost_seq = server.head();
    let lost_id = server.event_id_at(lost_seq);
    stop(&a, h).await;

    // Restore the backup and rotate the epoch, as the restore procedure does; then another rename
    // takes the lost one's sequence number, so the client's cursor is valid but means another history.
    let (dir, config) = server.halt().await;
    copy_db(&backup, &config.database_path());
    let new_epoch = todo_server::Engine::rotate_log_epoch(&config.database_path()).unwrap();
    let server = TestServer::boot(dir, config).await;
    let b = client(server.transport(TOKEN_B), MemoryStorage::new(), beta());
    let hb = spawn(&b);
    eventually("b synced", WAIT, || converged(&b, &server)).await;
    b.submit(rename(task, "other")).unwrap();
    eventually("regrown", WAIT, || converged(&b, &server)).await;
    stop(&b, hb).await;
    assert_eq!(server.head(), lost_seq);
    assert_ne!(server.event_id_at(lost_seq), lost_id);

    // Before reconnecting, the user undoes the lost rename (as this client still sees it) and adds a
    // note.
    let a = client(server.transport(TOKEN_A), storage.clone(), alpha());
    assert_eq!((a.cursor(), a.epoch()), (lost_seq, Some(old_epoch)));
    assert_eq!(a.view().tasks[&task].title, "lost");
    let undo_id = a.undo(lost_seq, lost_id, false).unwrap();
    a.submit(Command::AddNote {
        task,
        text: "queued".into(),
    })
    .unwrap();
    let h = spawn(&a);
    eventually("resynced onto the new log", WAIT, || {
        converged(&a, &server) && a.failed().len() == 1
    })
    .await;
    assert_eq!(a.epoch(), Some(new_epoch));
    let view = a.view();
    assert_eq!(
        view.tasks[&task].title, "other",
        "the other rename was not undone"
    );
    assert_eq!(view.tasks[&task].notes.matches("queued").count(), 1);
    let failed = &a.failed()[0];
    assert_eq!(failed.entry.command_id, undo_id);
    assert_eq!(failed.rejection.code, "epoch_mismatch");
    assert!(
        failed.rejection.message.contains("restored"),
        "{}",
        failed.rejection.message
    );
    stop(&a, h).await;
    let again = Replica::open(alpha(), Box::new(storage)).unwrap();
    assert_eq!(again.epoch(), Some(new_epoch));
    assert!(again.confirmed().data_eq(&server.state()));
    server.stop().await;
}

fn rename(task: TaskId, title: &str) -> Command {
    Command::UpdateTask {
        id: task,
        patch: todo_core::command::TaskPatch {
            title: Some(title.into()),
            ..Default::default()
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_frame_over_the_cap_ends_the_stream_and_the_event_is_paged() {
    let server = TestServer::start().await;
    let a = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let ha = spawn(&a);
    let tb = Switch::new(server.transport(TOKEN_B).with_max_frame_bytes(2048));
    let b = client(tb.clone(), MemoryStorage::new(), beta());
    let hb = spawn(&b);
    let (list, cmd) = new_list(&a, "L");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "big");
    a.submit(cmd).unwrap();
    eventually("b live", WAIT, || {
        converged(&b, &server) && b.status().connection == Connection::Online
    })
    .await;
    let pages = || {
        tb.paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.contains("limit="))
            .count()
    };
    let before = pages();
    a.submit(Command::AddNote {
        task,
        text: "z".repeat(4000),
    })
    .unwrap();
    // And a small one after it, which the reopened stream delivers.
    a.submit(Command::AddNote {
        task,
        text: "small".into(),
    })
    .unwrap();
    eventually("b got both", WAIT, || {
        converged(&a, &server) && converged(&b, &server)
    })
    .await;
    assert!(pages() > before, "the big event came through a page");
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

#[cfg(feature = "sqlite")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_sqlite_cache_gives_up_its_outbox() {
    use todo_client::StorageError;
    use todo_client::sqlite::SqliteStorage;
    use todo_client::storage::StorageOp;

    let server = TestServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("client.db");
    let t = Switch::new(server.transport(TOKEN_A));
    let a = client(t.clone(), SqliteStorage::open(&db).unwrap(), alpha());
    let h = spawn(&a);
    let (list, cmd) = new_list(&a, "Kept");
    a.submit(cmd).unwrap();
    eventually("synced", WAIT, || converged(&a, &server)).await;
    t.set_online(false);
    let (pending, cmd) = new_task(&a, list, "queued while offline");
    a.submit(cmd).unwrap();
    stop(&a, h).await;
    drop(a);

    // Damage the confirmed state; the outbox table is intact.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let damaged = conn
        .execute("UPDATE snapshot SET state = 'not json'", [])
        .unwrap()
        + conn
            .execute("UPDATE events SET event = 'not json'", [])
            .unwrap();
    assert!(damaged > 0);
    drop(conn);
    let mut broken = SqliteStorage::open(&db).unwrap();
    assert!(matches!(
        todo_client::Storage::load(&mut broken),
        Err(StorageError::Corrupt(_))
    ));
    drop(broken);

    let salvaged = SqliteStorage::salvage(&db).unwrap();
    assert_eq!(salvaged.outbox.len(), 1);
    assert_eq!(salvaged.lost, 0);
    let mut fresh = SqliteStorage::open(&dir.path().join("rebuilt.db")).unwrap();
    let ops: Vec<_> = salvaged.outbox.iter().map(StorageOp::PutOutbox).collect();
    todo_client::Storage::write(&mut fresh, &ops).unwrap();

    let a = client(server.transport(TOKEN_A), fresh, alpha());
    assert_eq!(a.pending().len(), 1, "the queued edit survived");
    let h = spawn(&a);
    eventually("synced after the rebuild", WAIT, || converged(&a, &server)).await;
    assert!(server.state().tasks.contains_key(&pending));
    stop(&a, h).await;
    server.stop().await;
}

#[cfg(feature = "sqlite")]
#[test]
fn a_file_that_is_not_a_database_is_corrupt_not_unavailable() {
    use todo_client::StorageError;
    use todo_client::sqlite::SqliteStorage;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("client.db");
    std::fs::write(&db, vec![0x5a; 8192]).unwrap();
    assert!(matches!(
        SqliteStorage::open(&db),
        Err(StorageError::Corrupt(_))
    ));
    assert!(SqliteStorage::salvage(&db).is_err());
    assert!(SqliteStorage::salvage_all(&db).is_err());
}

#[cfg(feature = "sqlite")]
#[test]
fn a_full_salvage_fails_rather_than_come_back_short() {
    use todo_client::sqlite::SqliteStorage;

    let dir = tempfile::tempdir().unwrap();
    // An outbox spread over many pages, whose last page is then damaged: the scan stops partway.
    let db = dir.path().join("partway.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "PRAGMA page_size = 4096;
         CREATE TABLE outbox (pos INTEGER PRIMARY KEY, entry TEXT NOT NULL);
         CREATE TABLE failed (pos INTEGER PRIMARY KEY, entry TEXT NOT NULL);",
    )
    .unwrap();
    for pos in 0..200 {
        conn.execute(
            "INSERT INTO outbox (pos, entry) VALUES (?1, ?2)",
            rusqlite::params![pos, "x".repeat(500)],
        )
        .unwrap();
    }
    drop(conn);
    let mut bytes = std::fs::read(&db).unwrap();
    let len = bytes.len();
    assert!(len >= 4096 * 10, "the outbox spans pages");
    bytes[len - 4096..].fill(0xff);
    std::fs::write(&db, bytes).unwrap();
    let lenient = SqliteStorage::salvage(&db).unwrap();
    assert!(lenient.lost >= 1, "the lenient salvage stops at the damage");
    assert!(SqliteStorage::salvage_all(&db).is_err());

    // A table that can't be read at all is an error too, not an empty list.
    let db = dir.path().join("no-failed-table.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE outbox (pos INTEGER PRIMARY KEY, entry TEXT NOT NULL);")
        .unwrap();
    drop(conn);
    assert!(SqliteStorage::salvage(&db).is_ok());
    assert!(SqliteStorage::salvage_all(&db).is_err());

    // A database that reads cleanly and has no unsent changes is empty.
    let db = dir.path().join("empty.db");
    drop(SqliteStorage::open(&db).unwrap());
    let empty = SqliteStorage::salvage_all(&db).unwrap();
    assert!(empty.outbox.is_empty() && empty.failed.is_empty() && empty.lost == 0);
}

/// Two tabs of the browser app: only `a` sends; `b` queues, and `a` takes `b`'s entries in (the web
/// app reads them from IndexedDB when `b` says so over a BroadcastChannel). Edits reach the server in
/// submission order across both tabs, even while `a` is stalled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_tab_sends_for_both_in_submission_order() {
    let server = TestServer::start().await;
    let ta = Switch::new(server.transport(TOKEN_A));
    let a = client(ta.clone(), MemoryStorage::new(), alpha());
    let b = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    b.set_sending(false);
    assert!(!b.is_sending());
    let (ha, hb) = (spawn(&a), spawn(&b));

    let (list, cmd) = new_list(&a, "Inbox");
    a.submit(cmd).unwrap();
    let (task, cmd) = new_task(&a, list, "Title 0");
    a.submit(cmd).unwrap();
    eventually("both have the task", WAIT, || {
        b.view().tasks.contains_key(&task) && a.status().pending == 0
    })
    .await;

    // `a` stalls with its rename in flight; `b` renames after it.
    ta.set_online(false);
    a.submit(rename(task, "Title A")).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    b.submit(rename(task, "Title B")).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let head = server.head();
    assert_eq!(
        server.state().tasks[&task].title,
        "Title 0",
        "b sends nothing"
    );
    assert_eq!(b.status().pending, 1);

    assert_eq!(a.adopt(b.pending()).unwrap(), 1);
    assert_eq!(a.adopt(b.pending()).unwrap(), 0, "adopting is idempotent");
    assert_eq!(
        a.view().tasks[&task].title,
        "Title B",
        "a shows b's edit at once"
    );
    ta.set_online(true);
    a.nudge();
    eventually("both converge on b's title", WAIT, || {
        converged(&a, &server)
            && converged(&b, &server)
            && a.status().pending == 0
            && b.status().pending == 0
    })
    .await;
    assert_eq!(server.state().tasks[&task].title, "Title B");
    assert_eq!(server.head(), head + 2, "a's rename, then b's");
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}
