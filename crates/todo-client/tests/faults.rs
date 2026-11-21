//! Property: under dropped requests, lost responses, late responses and cut streams, every command a
//! client accepted is applied by the server exactly once (or refused for good), and once the faults
//! stop both clients converge on the server's state with empty outboxes.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used)]

mod server;

use proptest::prelude::*;
use server::*;
use std::collections::BTreeSet;
use std::time::Duration;
use todo_client::{MemoryStorage, Transport};
use todo_core::command::{NewTask, Placement, TaskPatch};
use todo_core::ids::{ListId, TaskId};
use todo_core::{Command, State};

#[derive(Clone, Debug)]
enum Op {
    CreateList,
    CreateTask(usize),
    AddNote(usize),
    Rename(usize),
    Complete(usize),
    DeleteTask(usize),
    DeleteList(usize),
    /// Undo the event at this distance below the client's cursor.
    Undo(u64),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => Just(Op::CreateList),
        4 => any::<usize>().prop_map(Op::CreateTask),
        5 => any::<usize>().prop_map(Op::AddNote),
        2 => any::<usize>().prop_map(Op::Rename),
        2 => any::<usize>().prop_map(Op::Complete),
        1 => any::<usize>().prop_map(Op::DeleteTask),
        1 => any::<usize>().prop_map(Op::DeleteList),
        1 => (0u64..4).prop_map(Op::Undo),
    ]
}

#[derive(Default)]
struct Book {
    lists: Vec<ListId>,
    tasks: Vec<TaskId>,
    /// Accepted command IDs with their kind.
    accepted: BTreeSet<(String, String)>,
    /// Marker of each accepted note, by command ID.
    notes: Vec<(String, String)>,
}

fn pick<T: Copy>(items: &[T], i: usize) -> Option<T> {
    (!items.is_empty()).then(|| items[i % items.len()])
}

fn command<T: Transport>(
    c: &Client<T>,
    book: &mut Book,
    op: &Op,
    n: usize,
) -> Option<(Command, Option<String>)> {
    Some(match op {
        Op::CreateList => {
            let id = ListId::from_uuid(c.new_uuid());
            book.lists.push(id);
            (
                Command::CreateList {
                    id,
                    collection: None,
                    name: format!("list {n}"),
                    color: None,
                    placement: Placement::Last,
                },
                None,
            )
        }
        Op::CreateTask(i) => {
            let id = TaskId::from_uuid(c.new_uuid());
            let list = pick(&book.lists, *i)?;
            book.tasks.push(id);
            (
                Command::CreateTask(Box::new(NewTask::new(id, list, format!("task {n}")))),
                None,
            )
        }
        Op::AddNote(i) => {
            let marker = format!("<note {n}>");
            (
                Command::AddNote {
                    task: pick(&book.tasks, *i)?,
                    text: marker.clone(),
                },
                Some(marker),
            )
        }
        Op::Rename(i) => (
            Command::UpdateTask {
                id: pick(&book.tasks, *i)?,
                patch: TaskPatch {
                    title: Some(format!("renamed {n}")),
                    ..TaskPatch::default()
                },
            },
            None,
        ),
        Op::Complete(i) => (
            Command::CompleteTask {
                id: pick(&book.tasks, *i)?,
            },
            None,
        ),
        Op::DeleteTask(i) => (
            Command::Delete {
                id: pick(&book.tasks, *i)?.into(),
            },
            None,
        ),
        Op::DeleteList(i) => (
            Command::Delete {
                id: pick(&book.lists, *i)?.into(),
            },
            None,
        ),
        Op::Undo(_) => return None,
    })
}

fn all_notes(state: &State) -> String {
    state.tasks.values().map(|t| t.notes.as_str()).collect()
}

async fn scenario(seed: u64, plan: Vec<(bool, Op, u8)>) {
    let server = TestServer::start().await;
    let faults = Faults::new(
        seed,
        FaultRates {
            drop_request: 10,
            drop_response: 15,
            delay: 10,
            max_delay: Duration::from_millis(2000),
            refuse_stream: 20,
            stream_frames: 6,
        },
    );
    let a = client(
        Faulty::new(server.transport(TOKEN_A), faults.clone()),
        MemoryStorage::new(),
        alpha(),
    );
    let b = client(
        Faulty::new(server.transport(TOKEN_B), faults.clone()),
        MemoryStorage::new(),
        beta(),
    );
    let (ha, hb) = (spawn(&a), spawn(&b));
    let mut book = Book::default();

    for (n, (on_b, op, pause)) in plan.iter().enumerate() {
        let c = if *on_b { &b } else { &a };
        let submitted = match op {
            Op::Undo(back) => {
                let cursor = c.cursor();
                if cursor <= *back {
                    None
                } else {
                    c.undo(cursor - back, false)
                        .ok()
                        .map(|id| (id, "undo", None))
                }
            }
            _ => command(c, &mut book, op, n)
                .and_then(|(cmd, marker)| c.submit(cmd).ok().map(|id| (id, "command", marker))),
        };
        if let Some((id, kind, marker)) = submitted {
            book.accepted.insert((id.to_string(), kind.to_owned()));
            if let Some(m) = marker {
                book.notes.push((id.to_string(), m));
            }
        }
        tokio::time::sleep(Duration::from_millis(u64::from(*pause % 40))).await;
    }

    // Keep the faults on until some were injected (a short plan can be done before the first one;
    // heartbeats use up a stream's frame budget soon enough), then the network heals.
    let injected = || faults.injected.load(std::sync::atomic::Ordering::Relaxed);
    eventually("a fault", Duration::from_secs(10), || injected() > 0).await;
    faults.disable();
    a.nudge();
    b.nudge();
    let limit = Duration::from_secs(60);
    let start = tokio::time::Instant::now();
    while !(converged(&a, &server) && converged(&b, &server)) {
        assert!(
            start.elapsed() < limit,
            "no convergence: a {:?}, b {:?}, head {}",
            a.status(),
            b.status(),
            server.head()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let refused: BTreeSet<String> = a
        .failed()
        .iter()
        .chain(b.failed().iter())
        .map(|f| f.entry.command_id.to_string())
        .collect();
    let expected: BTreeSet<(String, String)> = book
        .accepted
        .iter()
        .filter(|(id, _)| !refused.contains(id))
        .cloned()
        .collect();
    // Exactly the accepted, unrefused commands were stored; idempotency by command ID makes each one
    // a single execution.
    assert_eq!(server.stored_command_ids(), expected, "seed {seed}");
    let state = server.state();
    let notes = all_notes(&state);
    // A note applied twice would show twice. (An undo may legitimately take a note back.)
    let undos = expected.iter().any(|(_, kind)| kind == "undo");
    for (id, marker) in &book.notes {
        let count = notes.matches(marker.as_str()).count();
        if refused.contains(id) {
            assert_eq!(count, 0, "{marker}, seed {seed}");
        } else if undos {
            assert!(count <= 1, "{marker}, seed {seed}");
        } else {
            assert_eq!(count, 1, "{marker}, seed {seed}");
        }
    }
    for c in [&a, &b] {
        assert!(c.pending().is_empty());
        assert!(c.view().data_eq(&state));
        assert!(c.with_confirmed(|s| s.data_eq(&state)));
    }
    stop(&a, ha).await;
    stop(&b, hb).await;
    server.stop().await;
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 12,
        max_shrink_iters: 16,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_command_is_applied_exactly_once_despite_network_faults(
        seed in any::<u64>(),
        plan in prop::collection::vec((any::<bool>(), op(), any::<u8>()), 8..30),
    ) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(scenario(seed, plan));
    }
}
