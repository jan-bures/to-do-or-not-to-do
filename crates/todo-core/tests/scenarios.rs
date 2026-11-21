#![allow(clippy::unwrap_used)]
//! Example-based tests of commands, undo, access control and queries.

mod common;

use common::{World, ts};
use todo_core::access::{AccessLevel, Grants};
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::error::{DomainError, UndoError};
use todo_core::event::{EventPayload, FieldChange, FieldKey};
use todo_core::ids::EntityId;
use todo_core::model::{EmailLink, LinkKind, Priority};
use todo_core::query::{BUILTIN_LISTS, EvalContext, OrderKey, Query};
use todo_core::recurrence::{Recurrence, Rule};
use todo_core::time::{Date, Timestamp};
use todo_core::{Actor, State, apply, decide};

fn d(y: i16, m: i8, day: i8) -> Date {
    Date::constant(y, m, day)
}

#[test]
fn create_orders_siblings_and_rejects_duplicates() {
    let mut w = World::new();
    let list = w.list(None, "Inbox");
    let a = w.task(list, "a");
    let b = w.task(list, "b");
    let c = w.ids.task();
    w.run(Command::CreateTask(Box::new({
        let mut n = NewTask::new(c, list, "c");
        n.placement = Placement::Before(b.into());
        n
    })));
    let order: Vec<_> = w
        .state
        .siblings(todo_core::state::Scope::Tasks(list))
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert_eq!(order, vec![a.into(), c.into(), b.into()]);

    let dup = Command::CreateTask(Box::new(NewTask::new(a, list, "again")));
    assert!(matches!(
        decide(&w.state, &w.owner, &dup, ts(100)),
        Err(DomainError::AlreadyExists(_))
    ));
    let empty = Command::CreateTask(Box::new(NewTask::new(w.ids.task(), list, "   ")));
    assert!(matches!(
        decide(&w.state, &w.owner, &empty, ts(100)),
        Err(DomainError::Invalid { field: "title", .. })
    ));
}

#[test]
fn decide_is_deterministic_for_retries() {
    let mut w = World::new();
    let list = w.list(None, "Inbox");
    let t = w.ids.task();
    let cmd = Command::CreateTask(Box::new(NewTask::new(t, list, "x")));
    let a = decide(&w.state, &w.owner, &cmd, ts(50)).unwrap();
    let b = decide(&w.state, &w.owner, &cmd, ts(50)).unwrap();
    assert_eq!(a, b);
    // Applying the same events twice is rejected, which makes retries safe.
    apply(&mut w.state, &a[0]).unwrap();
    assert!(apply(&mut w.state, &a[0]).is_err());
}

#[test]
fn update_records_previous_values() {
    let mut w = World::new();
    let list = w.list(None, "Inbox");
    let t = w.task(list, "old");
    let ev = w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            title: Some("new".into()),
            priority: Some(Priority::High),
            ..TaskPatch::default()
        },
    });
    let EventPayload::Updated { changes, .. } = &ev[0].payload else {
        panic!("expected update");
    };
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, FieldChange::Title(c) if c.old == "old" && c.new == "new"))
    );
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, FieldChange::Priority(c) if c.old == Priority::None))
    );
    // A no-op update yields no events.
    let again = w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            title: Some("new".into()),
            ..TaskPatch::default()
        },
    });
    assert!(again.is_empty());
}

#[test]
fn recurring_completion_spawns_next_with_steps_and_reminders() {
    let mut w = World::new();
    let list = w.list(None, "Chores");
    let t = w.ids.task();
    let mut n = NewTask::new(t, list, "water plants");
    n.due = Some(d(2026, 3, 2));
    n.recurrence = Some(Recurrence::new(Rule::Weekly {
        interval: 1,
        days: [todo_core::time::Weekday::Mon, todo_core::time::Weekday::Thu].into(),
    }));
    w.run(Command::CreateTask(Box::new(n)));
    let step = w.ids.step();
    w.run(Command::CreateStep {
        id: step,
        task: t,
        title: "balcony".into(),
        placement: Placement::Last,
    });
    w.run(Command::CompleteStep { id: step });
    let rem = w.ids.reminder();
    w.run(Command::AddReminder {
        id: rem,
        task: t,
        at: ts(10_000),
    });

    let events = w.run(Command::CompleteTask { id: t });
    assert_eq!(events.len(), 4); // complete, spawn, step copy, reminder copy
    assert!(w.state.tasks[&t].is_done());
    let next = w
        .state
        .tasks
        .values()
        .find(|x| x.id != t)
        .expect("spawned task");
    assert_eq!(next.due, Some(d(2026, 3, 5)));
    assert!(!next.is_done());
    let steps: Vec<_> = w
        .state
        .steps
        .values()
        .filter(|s| s.task == next.id)
        .collect();
    assert_eq!(steps.len(), 1);
    assert!(steps[0].completed_at.is_none());
    let rems: Vec<_> = w
        .state
        .reminders
        .values()
        .filter(|r| r.task == next.id)
        .collect();
    assert_eq!(rems[0].at.as_second(), ts(10_000).as_second() + 3 * 86_400);

    // Completing again is a no-op; undoing the whole command removes the spawned task.
    assert!(w.run(Command::CompleteTask { id: t }).is_empty());
    let before = w.state.clone();
    let comp = todo_core::undo::undo_batch(
        &w.state,
        &events,
        &w.owner.clone(),
        ts(500),
        Default::default(),
    )
    .unwrap();
    for e in &comp {
        apply(&mut w.state, e).unwrap();
    }
    assert_eq!(w.state.tasks.len(), 1);
    assert!(!w.state.tasks[&t].is_done());
    assert_ne!(before.tasks.len(), w.state.tasks.len());
}

#[test]
fn recurrence_requires_due_date() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let mut n = NewTask::new(w.ids.task(), list, "x");
    n.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    let r = decide(&w.state, &w.owner, &Command::CreateTask(Box::new(n)), ts(1));
    assert!(matches!(
        r,
        Err(DomainError::Invalid {
            field: "recurrence",
            ..
        })
    ));
}

#[test]
fn delete_restore_and_visibility() {
    let mut w = World::new();
    let col = w.collection("Work");
    let list = w.list(Some(col), "Proj");
    let t = w.task(list, "write");
    w.run(Command::Delete { id: col.into() });
    assert!(!w.state.is_live(t.into()));
    assert!(w.state.tasks.contains_key(&t)); // tombstoned parent hides, not removes
    let bad = decide(
        &w.state,
        &w.owner,
        &Command::Delete { id: col.into() },
        ts(99),
    );
    assert!(matches!(bad, Err(DomainError::AlreadyDeleted(_))));
    let restoring_child = decide(
        &w.state,
        &w.owner,
        &Command::Restore { id: list.into() },
        ts(99),
    );
    assert!(restoring_child.is_err() || w.state.lists[&list].deleted.is_none());
    w.run(Command::Restore { id: col.into() });
    assert!(w.state.is_live(t.into()));
    // Operating on a task under a deleted list is rejected.
    w.run(Command::Delete { id: list.into() });
    let r = decide(&w.state, &w.owner, &Command::CompleteTask { id: t }, ts(99));
    assert!(matches!(r, Err(DomainError::Deleted(_))));
}

#[test]
fn undo_create_and_redo() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let new_id = w.ids.task();
    let create = w.run(Command::CreateTask(Box::new(NewTask::new(
        new_id, list, "t",
    ))));
    let task_id = match &create[0].payload {
        EventPayload::Created { entity } => entity.id(),
        _ => panic!(),
    };
    let EntityId::Task(task_id) = task_id else {
        panic!()
    };
    let comp = w.undo(&create[0]).unwrap();
    assert!(!w.state.tasks.contains_key(&task_id));
    // Undoing the original again is refused: it is already undone.
    assert!(matches!(
        w.undo(&create[0]),
        Err(UndoError::AlreadyUndone(_))
    ));
    // Redo = undo of the compensating event.
    w.undo(&comp[0]).unwrap();
    assert!(w.state.tasks.contains_key(&task_id));
    assert_eq!(w.state.tasks[&task_id].title, "t");
}

#[test]
fn undo_delete_restores_tombstoned_entity() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "t");
    let del = w.run(Command::Delete { id: t.into() });
    assert!(w.state.tasks[&t].deleted.is_some());
    w.undo(&del[0]).unwrap();
    assert!(w.state.tasks[&t].deleted.is_none());
}

#[test]
fn undo_conflicts_report_later_events_and_can_be_forced() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "one");
    let rename = |w: &mut World, title: &str| {
        w.run(Command::UpdateTask {
            id: t,
            patch: TaskPatch {
                title: Some(title.into()),
                ..TaskPatch::default()
            },
        })
    };
    let first = rename(&mut w, "two");
    let second = rename(&mut w, "three");
    let err = w.undo(&first[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.event, first[0].id);
    assert_eq!(c.conflicts.len(), 1);
    assert_eq!(c.conflicts[0].field, FieldKey::Title);
    assert_eq!(c.conflicts[0].later_events, vec![second[0].id]);
    assert_eq!(w.state.tasks[&t].title, "three"); // nothing changed

    // Forcing overrides.
    let owner = w.owner.clone();
    let forced = w.undo_as(&owner, &first[0], true).unwrap();
    assert!(forced[0].forced);
    assert_eq!(w.state.tasks[&t].title, "one");

    // Unrelated fields do not conflict.
    let prio = w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            priority: Some(Priority::Low),
            ..TaskPatch::default()
        },
    });
    rename(&mut w, "four");
    w.undo(&prio[0]).unwrap();
    assert_eq!(w.state.tasks[&t].priority, Priority::None);
    assert_eq!(w.state.tasks[&t].title, "four");
}

#[test]
fn undo_create_with_dependents_is_refused() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let create_list_event = w.log[0].clone();
    w.task(list, "t");
    assert!(matches!(
        w.undo(&create_list_event),
        Err(UndoError::HasDependents(_))
    ));
}

#[test]
fn merge_moves_children_and_undo_batch_reverts() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let tag = w.ids.tag();
    w.run(Command::CreateTag {
        id: tag,
        name: "x".into(),
        color: None,
    });
    let a = w.task(list, "a");
    let b = w.task(list, "b");
    let s = w.ids.step();
    w.run(Command::CreateStep {
        id: s,
        task: b,
        title: "step".into(),
        placement: Placement::Last,
    });
    w.run(Command::AddTag { task: b, tag });
    w.run(Command::AddNote {
        task: b,
        text: "note b".into(),
    });
    let link = w.ids.link();
    w.run(Command::AddLink {
        id: link,
        task: b,
        kind: LinkKind::Email(EmailLink {
            subject: "Hi".into(),
            sender: "a@example.com".into(),
            received_at: ts(5),
            message_id: "<1@example.com>".into(),
            web_link: None,
        }),
    });
    let before = w.state.clone();
    let events = w.run(Command::MergeTasks {
        source: b,
        target: a,
    });
    assert!(w.state.tasks[&b].deleted.is_some());
    assert_eq!(w.state.tasks[&b].merged_into, Some(a));
    assert_eq!(w.state.steps[&s].task, a);
    assert_eq!(w.state.links[&link].task, a);
    assert!(w.state.tasks[&a].tags.contains(&tag));
    assert!(w.state.tasks[&a].notes.contains("note b"));

    let owner = w.owner.clone();
    let comp = todo_core::undo::undo_batch(&w.state, &events, &owner, ts(900), Default::default())
        .unwrap();
    for e in &comp {
        apply(&mut w.state, e).unwrap();
    }
    assert!(w.state.data_eq(&before));
}

#[test]
fn tags_are_unique_case_insensitively() {
    let mut w = World::new();
    let t1 = w.ids.tag();
    w.run(Command::CreateTag {
        id: t1,
        name: "Home".into(),
        color: Some("#fff".into()),
    });
    let dup = Command::CreateTag {
        id: w.ids.tag(),
        name: "home".into(),
        color: None,
    };
    assert!(matches!(
        decide(&w.state, &w.owner, &dup, ts(99)),
        Err(DomainError::DuplicateName(_))
    ));
}

#[test]
fn link_and_attachment_validation() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "t");
    let bad = Command::AddLink {
        id: w.ids.link(),
        task: t,
        kind: LinkKind::Web {
            url: "javascript:alert(1)".into(),
            title: None,
        },
    };
    assert!(decide(&w.state, &w.owner, &bad, ts(99)).is_err());
    let bad = Command::AddAttachment {
        id: w.ids.attachment(),
        task: t,
        file_name: "../x.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: 1,
        storage_key: "k".into(),
    };
    assert!(decide(&w.state, &w.owner, &bad, ts(99)).is_err());
    let att = w.ids.attachment();
    w.run(Command::AddAttachment {
        id: att,
        task: t,
        file_name: "x.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: 1,
        storage_key: "blob/1".into(),
    });
}

#[test]
fn agents_follow_grants_and_undo_only_their_own_events() {
    let mut w = World::new();
    let col = w.collection("Work");
    let proj = w.list(Some(col), "Project");
    let personal = w.list(None, "Personal");
    let mut g = Grants::with_default(AccessLevel::Read);
    g.collections.insert(col, AccessLevel::Plan);
    g.lists.insert(personal, AccessLevel::Capture);
    w.grant("claude", g);
    w.grant_default("codex", AccessLevel::Plan);
    let claude = Actor::agent("claude").unwrap();
    let codex = Actor::agent("codex").unwrap();

    // Capture on Personal: may create, may not complete.
    let t = w.ids.task();
    w.run_as(
        &claude,
        Command::CreateTask(Box::new(NewTask::new(t, personal, "buy milk"))),
    )
    .unwrap();
    assert!(matches!(
        w.run_as(&claude, Command::CompleteTask { id: t }),
        Err(DomainError::Denied(_))
    ));
    // Plan on the Work collection (inherited by its list).
    let p = w.ids.task();
    let created = w
        .run_as(
            &claude,
            Command::CreateTask(Box::new(NewTask::new(p, proj, "plan"))),
        )
        .unwrap();
    w.run_as(&claude, Command::CompleteTask { id: p }).unwrap();
    // Structure changes are owner-only, even with Plan.
    assert!(matches!(
        w.run_as(
            &claude,
            Command::UpdateList {
                id: proj,
                name: Some("x".into()),
                color: None
            }
        ),
        Err(DomainError::Denied(_))
    ));
    // Moving a task out to a list where the agent only has Capture is denied.
    assert!(matches!(
        w.run_as(
            &claude,
            Command::MoveTask {
                id: p,
                list: personal,
                placement: Placement::Last
            }
        ),
        Err(DomainError::Denied(_))
    ));

    // Undo: agents only their own events; the owner anything.
    assert!(matches!(
        w.undo_as(&codex, &created[0], false),
        Err(UndoError::NotOwnEvent)
    ));
    let owner = w.owner.clone();
    let owner_event = w.log[0].clone();
    assert!(matches!(
        w.undo_as(&claude, &owner_event, false),
        Err(UndoError::NotOwnEvent)
    ));
    let complete_event = w.log.last().unwrap().clone();
    w.undo_as(&claude, &complete_event, false).unwrap();
    w.undo_as(&owner, &created[0], false).unwrap();
}

#[test]
fn email_ingest_may_capture_only() {
    let mut w = World::new();
    let list = w.list(None, "Inbox");
    let t = w.ids.task();
    let ingest = Actor::EmailIngest;
    w.run_as(
        &ingest,
        Command::CreateTask(Box::new(NewTask::new(t, list, "mail"))),
    )
    .unwrap();
    assert!(w.run_as(&ingest, Command::CompleteTask { id: t }).is_err());
}

#[test]
fn events_and_state_round_trip_through_json() {
    let mut w = World::new();
    let col = w.collection("c");
    let list = w.list(Some(col), "l");
    let t = w.task(list, "t");
    w.run(Command::SetMyDay {
        id: t,
        date: Some(d(2026, 5, 5)),
    });
    w.grant_default("bot", AccessLevel::Capture);
    for e in &w.log {
        let json = serde_json::to_string(e).unwrap();
        assert_eq!(&serde_json::from_str::<todo_core::Event>(&json).unwrap(), e);
    }
    let json = serde_json::to_string(&w.state).unwrap();
    let back: State = serde_json::from_str(&json).unwrap();
    assert_eq!(back, w.state);
    let cmd = Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            due: Some(None),
            ..TaskPatch::default()
        },
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
}

// ---- queries -----------------------------------------------------------------------------------

fn query_world() -> (World, EvalContext) {
    let mut w = World::new();
    let col = w.collection("Work");
    let shop = w.list(None, "Shopping");
    let proj = w.list(Some(col), "Project");
    let milk = w.ids.task();
    let mut n = NewTask::new(milk, shop, "Koupit mléko");
    n.due = Some(d(2026, 3, 1));
    w.run(Command::CreateTask(Box::new(n)));
    let report = w.ids.task();
    let mut n = NewTask::new(report, proj, "Write report");
    n.priority = Priority::High;
    n.due = Some(d(2026, 3, 10));
    w.run(Command::CreateTask(Box::new(n)));
    let bot = Actor::agent("codex").unwrap();
    w.grant_default("codex", AccessLevel::Plan);
    let agent_task = w.ids.task();
    w.run_as(
        &bot,
        Command::CreateTask(Box::new(NewTask::new(agent_task, proj, "Refactor"))),
    )
    .unwrap();
    let done = w.task(proj, "Old thing");
    w.run(Command::CompleteTask { id: done });
    w.run(Command::SetMyDay {
        id: milk,
        date: Some(d(2026, 3, 5)),
    });
    let tag = w.ids.tag();
    w.run(Command::CreateTag {
        id: tag,
        name: "Errand".into(),
        color: None,
    });
    w.run(Command::AddTag { task: milk, tag });
    let ctx = EvalContext::new(ts(100), d(2026, 3, 5), 0);
    (w, ctx)
}

fn titles(w: &World, ctx: &EvalContext, q: &str) -> Vec<String> {
    Query::parse(q)
        .unwrap()
        .run(&w.state, ctx)
        .into_iter()
        .map(|t| t.title.clone())
        .collect()
}

#[test]
fn queries_filter_tasks() {
    let (w, ctx) = query_world();
    assert_eq!(titles(&w, &ctx, "list:shopping"), ["Koupit mléko"]);
    assert_eq!(
        titles(&w, &ctx, "collection:work and not done sort:title"),
        ["Refactor", "Write report"]
    );
    assert_eq!(titles(&w, &ctx, "due:overdue"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "due<=today sort:due"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "due>=+3d"), ["Write report"]);
    assert_eq!(titles(&w, &ctx, "my_day"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "priority>=high"), ["Write report"]);
    assert_eq!(titles(&w, &ctx, "tag:errand"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "done"), ["Old thing"]);
    assert_eq!(titles(&w, &ctx, "created_by:agent"), ["Refactor"]);
    assert_eq!(titles(&w, &ctx, "created_by:codex"), ["Refactor"]);
    assert_eq!(titles(&w, &ctx, "created_by:other"), Vec::<String>::new());
    assert_eq!(
        titles(
            &w,
            &ctx,
            "list:\"Project\" (priority>=high or created_by:agent) not done sort:-title"
        ),
        ["Write report", "Refactor"]
    );
    assert_eq!(titles(&w, &ctx, "due:none and not done"), ["Refactor"]);
}

#[test]
fn free_text_ignores_case_and_diacritics() {
    let (w, ctx) = query_world();
    assert_eq!(titles(&w, &ctx, "mleko"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "MLÉKO"), ["Koupit mléko"]);
    assert_eq!(titles(&w, &ctx, "\"write report\""), ["Write report"]);
    assert_eq!(
        titles(&w, &ctx, "REPORT or mleko sort:title"),
        ["Koupit mléko", "Write report"]
    );
}

#[test]
fn free_text_matches_live_step_titles_and_agrees_with_matches() {
    let (mut w, ctx) = query_world();
    let report = *w
        .state
        .tasks
        .iter()
        .find(|(_, t)| t.title == "Write report")
        .unwrap()
        .0;
    let step = w.ids.step();
    w.run(Command::CreateStep {
        id: step,
        task: report,
        title: "Přehled čísel".into(),
        placement: Placement::Last,
    });
    let gone = w.ids.step();
    w.run(Command::CreateStep {
        id: gone,
        task: report,
        title: "obsolete".into(),
        placement: Placement::Last,
    });
    w.run(Command::Delete { id: gone.into() });
    assert_eq!(titles(&w, &ctx, "prehled"), ["Write report"]);
    assert_eq!(titles(&w, &ctx, "obsolete"), Vec::<String>::new());
    // `run` (which indexes steps once) agrees with the per-task `matches`.
    for q in ["prehled", "cisel or mleko", "not prehled", "report prehled"] {
        let q = Query::parse(q).unwrap();
        let mut by_matches: Vec<_> = w
            .state
            .tasks
            .values()
            .filter(|t| w.state.is_live(t.id.into()) && q.matches(&w.state, t, &ctx))
            .map(|t| t.id)
            .collect();
        by_matches.sort();
        let mut by_run: Vec<_> = q.run(&w.state, &ctx).iter().map(|t| t.id).collect();
        by_run.sort();
        assert_eq!(by_run, by_matches, "{q}");
    }
}

/// Final review, finding 1: a repeated sort key counts once, so keys (and cursors) stay bounded.
#[test]
fn repeated_sort_keys_count_once() {
    let (w, _ctx) = query_world();
    let q = Query::parse("sort:title sort:-title sort:due sort:title sort:-due").unwrap();
    assert_eq!(q.to_string(), "sort:title sort:due");
    let plain = Query::parse("sort:title sort:due").unwrap();
    assert_eq!(q, plain);
    // Hand-built queries with repeats produce the same key as well.
    let mut built = plain.clone();
    built.sort.extend(plain.sort.iter().copied());
    for t in w.state.tasks.values() {
        assert_eq!(built.order_key(&w.state, t), plain.order_key(&w.state, t));
    }
}

/// Final review, finding 2: the step index and per-task step scans honour a stop check.
#[test]
fn a_stoppable_matcher_gives_up_on_steps() {
    let (mut w, ctx) = query_world();
    let task = *w
        .state
        .tasks
        .iter()
        .find(|(_, t)| t.title == "Write report")
        .unwrap()
        .0;
    for _ in 0..600 {
        let id = w.ids.step();
        w.run(Command::CreateStep {
            id,
            task,
            title: "needle".into(),
            placement: Placement::Last,
        });
    }
    let q = Query::parse("needle").unwrap();
    let t = &w.state.tasks[&task];
    // Stopped while indexing: no matcher.
    let always = || true;
    assert!(q.matcher_until(&w.state, &ctx, &always).is_none());
    // Not stopped: same answers as `matcher`.
    let stopped = std::cell::Cell::new(false);
    let check = || stopped.get();
    let m = q.matcher_until(&w.state, &ctx, &check).unwrap();
    assert!(m.matches(t));
    assert!(q.matcher(&w.state, &ctx).matches(t));
    // Stopped after indexing: the step scan gives up.
    stopped.set(true);
    assert!(!m.matches(t));
    // A query without free text needs no index, and the stop check does not apply to it.
    let q = Query::parse("not done").unwrap();
    assert!(q.matcher_until(&w.state, &ctx, &always).unwrap().matches(t));
}

#[test]
fn order_keys_are_the_run_order_and_survive_serialization() {
    let (w, ctx) = query_world();
    for text in [
        "",
        "sort:due",
        "sort:-due",
        "sort:priority sort:title",
        "sort:-priority",
        "sort:-completed",
        "sort:created",
        "sort:-updated sort:-title",
    ] {
        let q = Query::parse(text).unwrap();
        let run = q.run(&w.state, &ctx);
        assert_eq!(run.len(), 4, "{text}");
        let keys: Vec<OrderKey> = run.iter().map(|t| q.order_key(&w.state, t)).collect();
        assert!(
            keys.windows(2).all(|p| p[0] < p[1]),
            "{text}: not strictly increasing"
        );
        for (a, b) in run.iter().zip(run.iter().skip(1)) {
            assert_eq!(
                q.compare(&w.state, a, b),
                std::cmp::Ordering::Less,
                "{text}"
            );
        }
        let json = serde_json::to_string(&keys).unwrap();
        let back: Vec<OrderKey> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, keys, "{text}");
    }
    // Missing values sort last ascending and first descending.
    assert_eq!(
        titles(&w, &ctx, "not done sort:due").last().unwrap(),
        "Refactor"
    );
    assert_eq!(titles(&w, &ctx, "not done sort:-due")[0], "Refactor");
}

#[test]
fn deleted_tasks_and_lists_are_not_returned() {
    let (mut w, ctx) = query_world();
    let list = *w.state.lists.keys().next().unwrap();
    w.run(Command::Delete { id: list.into() });
    let all = titles(&w, &ctx, "");
    assert!(all.len() < 4);
}

#[test]
fn builtin_lists_parse_and_run() {
    let (w, ctx) = query_world();
    for b in BUILTIN_LISTS {
        let q = b.parse().unwrap();
        assert_eq!(Query::parse(&q.to_string()).unwrap(), q, "{}", b.name);
    }
    let by_id = |id: &str| {
        BUILTIN_LISTS
            .iter()
            .find(|b| b.id == id)
            .unwrap()
            .parse()
            .unwrap()
    };
    let names = |q: Query| {
        q.run(&w.state, &ctx)
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(by_id("my_day")), ["Koupit mléko"]);
    assert_eq!(names(by_id("important")), ["Write report"]);
    assert_eq!(names(by_id("planned")), ["Koupit mléko", "Write report"]);
    assert_eq!(names(by_id("all")).len(), 3);
    assert_eq!(names(by_id("completed")), ["Old thing"]);
    // The agent-activity list needs `now` close to the events (all happen around ts(0..100)).
    let ctx_now = EvalContext::new(ts(100), d(2023, 11, 15), 0);
    assert_eq!(by_id("agent_activity").run(&w.state, &ctx_now).len(), 1);
}

#[test]
fn parse_errors_have_positions() {
    let e = Query::parse("list:a and (due<=today").unwrap_err();
    assert_eq!(e.pos, 11);
    assert!(e.to_string().contains("at byte 11"), "{e}");
    let e = Query::parse("foo:bar").unwrap_err();
    assert_eq!((e.pos, e.len), (0, 3));
    let e = Query::parse("due<=someday").unwrap_err();
    assert_eq!(e.pos, 5);
    let e = Query::parse("a )").unwrap_err();
    assert_eq!(e.pos, 2);
    let e = Query::parse("\"abc").unwrap_err();
    assert_eq!(e.pos, 0);
    let e = Query::parse("a or").unwrap_err();
    assert_eq!(e.pos, 4);
    let e = Query::parse("priority>=huge").unwrap_err();
    assert_eq!(e.pos, 10);
    assert!(Query::parse("(a sort:due)").is_err());
    assert!(Query::parse("tag>x").is_err());
    assert!(Query::parse("due:").is_err());
    assert!(Query::parse(&"(".repeat(500)).is_err());
}

#[test]
fn timestamps_and_dates_helpers_are_consistent() {
    let t: Timestamp = ts(0);
    assert_eq!(todo_core::time::utc_date(t), d(2023, 11, 14));
}

// ---- review regressions --------------------------------------------------------------------------

fn rename(w: &mut World, t: todo_core::ids::TaskId, title: &str) -> Vec<todo_core::Event> {
    w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            title: Some(title.into()),
            ..TaskPatch::default()
        },
    })
}

fn conflict_fields(err: UndoError) -> Vec<FieldKey> {
    let UndoError::Conflict(c) = err else {
        panic!("expected a conflict, got {err:?}")
    };
    c.conflicts.into_iter().map(|c| c.field).collect()
}

/// Finding 1: undo must not trust a supplied event body that reuses a real event ID.
#[test]
fn undo_rejects_forged_event_bodies() {
    let mut w = World::new();
    let list = w.list(None, "L");
    w.grant_default("bot", AccessLevel::Plan);
    let bot = Actor::agent("bot").unwrap();
    let t = w.task(list, "one");
    let owner_rename = rename(&mut w, t, "two").remove(0);

    // The agent claims the owner's event as its own: refused before ownership is even considered.
    let mut forged = owner_rename.clone();
    forged.actor = bot.clone();
    assert_eq!(
        w.undo_as(&bot, &forged, false),
        Err(UndoError::EventMismatch(owner_rename.id))
    );
    // A different payload under a real ID is refused, even for the owner.
    let mut forged = owner_rename.clone();
    if let EventPayload::Updated { changes, .. } = &mut forged.payload {
        changes[0] =
            FieldChange::Title(todo_core::event::Change::new("two".into(), "pwned".into()));
    }
    let owner = w.owner.clone();
    assert_eq!(
        w.undo_as(&owner, &forged, false),
        Err(UndoError::EventMismatch(owner_rename.id))
    );
    let mut forged = owner_rename.clone();
    forged.forced = true;
    assert!(matches!(
        todo_core::access::authorize_undo(&w.state, &owner, &forged),
        Err(UndoError::EventMismatch(_))
    ));
    assert_eq!(w.state.tasks[&t].title, "two");

    // Only the server-assigned sequence number may differ.
    let mut from_log = owner_rename.clone();
    from_log.seq = Some(42);
    w.undo(&from_log).unwrap();
    assert_eq!(w.state.tasks[&t].title, "one");
}

/// Finding 2: undoing a move (or a merge) needs access to where things move back to.
#[test]
fn undoing_a_move_or_merge_needs_access_to_the_destination() {
    let mut w = World::new();
    let a = w.list(None, "A");
    let b = w.list(None, "B");
    let mut g = Grants::with_default(AccessLevel::None);
    g.lists.insert(a, AccessLevel::Plan);
    g.lists.insert(b, AccessLevel::Plan);
    w.grant("bot", g.clone());
    let bot = Actor::agent("bot").unwrap();

    let t = w.task(a, "move me");
    let moved = w
        .run_as(
            &bot,
            Command::MoveTask {
                id: t,
                list: b,
                placement: Placement::Last,
            },
        )
        .unwrap();
    let src = w.task(a, "source");
    let dst = w.task(b, "target");
    let step = w.ids.step();
    w.run(Command::CreateStep {
        id: step,
        task: src,
        title: "child".into(),
        placement: Placement::Last,
    });
    let merge = w
        .run_as(
            &bot,
            Command::MergeTasks {
                source: src,
                target: dst,
            },
        )
        .unwrap();
    assert_eq!(w.state.steps[&step].task, dst);

    // The owner revokes write access to A.
    g.lists.insert(a, AccessLevel::Read);
    w.grant("bot", g);
    let denied = |r: Result<Vec<todo_core::Event>, UndoError>| {
        assert!(matches!(r, Err(UndoError::Denied(_))), "{r:?}");
    };
    // Moving the task back into A is denied although the agent may still plan in B.
    denied(w.undo_as(&bot, &moved[0], false));
    assert_eq!(w.state.tasks[&t].list, b);
    // Reparenting the merged step back to the source task in A is denied too.
    let reparent = merge
        .iter()
        .find(|e| e.payload.target() == EntityId::Step(step))
        .unwrap();
    denied(w.undo_as(&bot, reparent, false));
    let now = w.now();
    denied(todo_core::undo::undo_batch(
        &w.state,
        &merge,
        &bot,
        now,
        Default::default(),
    ));
    // The owner can undo both.
    w.undo(&moved[0]).unwrap();
    assert_eq!(w.state.tasks[&t].list, a);
}

/// Finding 3: withdrawing a creation (or re-deleting a restored entity) touches every field.
#[test]
fn undoing_a_creation_or_restore_conflicts_with_later_edits() {
    let mut w = World::new();
    let list = w.list(None, "L");
    w.grant_default("bot", AccessLevel::Capture);
    let bot = Actor::agent("bot").unwrap();
    let t = w.ids.task();
    let created = w
        .run_as(
            &bot,
            Command::CreateTask(Box::new(NewTask::new(t, list, "captured"))),
        )
        .unwrap();
    rename(&mut w, t, "edited by the owner");
    let err = w.undo_as(&bot, &created[0], false).unwrap_err();
    assert_eq!(conflict_fields(err), vec![FieldKey::Title]);
    assert!(w.state.tasks.contains_key(&t));

    // Forcing withdraws the task as it is now; redoing brings the edit back.
    let owner = w.owner.clone();
    let comp = w.undo_as(&owner, &created[0], true).unwrap();
    assert!(comp[0].forced);
    assert!(!w.state.tasks.contains_key(&t));
    w.undo(&comp[0]).unwrap();
    assert_eq!(w.state.tasks[&t].title, "edited by the owner");

    // Re-deleting a restored task would hide edits made since the restore.
    let del = w.run(Command::Delete { id: t.into() });
    let restored = w.run(Command::Restore { id: t.into() });
    w.run(Command::SetMyDay {
        id: t,
        date: Some(d(2026, 1, 1)),
    });
    assert_eq!(
        conflict_fields(w.undo(&restored[0]).unwrap_err()),
        vec![FieldKey::MyDay]
    );
    // ... and so would re-deleting a restored list that got a new task.
    let other = w.list(None, "other");
    w.run(Command::Delete { id: other.into() });
    let back = w.run(Command::Restore { id: other.into() });
    let fresh = w.task(other, "new in restored list");
    let err = w.undo(&back[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.conflicts[0].target, EntityId::Task(fresh));
    assert_eq!(c.conflicts[0].field, FieldKey::Existence);
    // Undoing the delete itself is fine: nothing changed while it was deleted.
    let _ = del;
}

/// Finding 4: compensations are writes; a forced undo records what it overwrote.
#[test]
fn forced_undo_is_tracked_and_its_undo_restores_what_it_overwrote() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "one");
    let e = rename(&mut w, t, "two");
    let f = rename(&mut w, t, "three");
    let owner = w.owner.clone();
    let forced = w.undo_as(&owner, &e[0], true).unwrap();
    assert_eq!(w.state.tasks[&t].title, "one");
    let EventPayload::Updated { changes, .. } = &forced[0].payload else {
        panic!()
    };
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, FieldChange::Title(c) if c.old == "three" && c.new == "one")),
        "the compensation records the value it actually replaced: {changes:?}"
    );
    // Undoing F would now silently overwrite the forced compensation's write.
    let err = w.undo(&f[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.conflicts[0].later_events, vec![forced[0].id]);
    // Undoing the forced compensation restores exactly what it overwrote ...
    let redo = w.undo(&forced[0]).unwrap();
    assert_eq!(w.state.tasks[&t].title, "three");
    // ... after which F is the latest writer again and can be undone.
    w.undo(&f[0]).unwrap();
    assert_eq!(w.state.tasks[&t].title, "two");
    // Redoing E's undo (undoing `redo`) now conflicts: F's undo wrote the field since.
    assert!(matches!(w.undo(&redo[0]), Err(UndoError::Conflict(_))));

    // A newer edit after a forced undo blocks the redo.
    let t2 = w.task(list, "a");
    let e = rename(&mut w, t2, "b");
    rename(&mut w, t2, "c");
    let forced = w.undo_as(&owner, &e[0], true).unwrap();
    let newer = rename(&mut w, t2, "d");
    let err = w.undo(&forced[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.conflicts[0].later_events, vec![newer[0].id]);
    assert_eq!(w.state.tasks[&t2].title, "d");
}

/// Finding 5: undo validates the entity it would leave behind.
#[test]
fn undo_refuses_to_leave_a_recurring_task_without_due_date() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "water plants");
    let set_due = w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            due: Some(Some(d(2026, 3, 2))),
            ..TaskPatch::default()
        },
    });
    let set_rec = w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            recurrence: Some(Some(Recurrence::new(Rule::Daily { interval: 1 }))),
            ..TaskPatch::default()
        },
    });
    let owner = w.owner.clone();
    for force in [false, true] {
        let err = w.undo_as(&owner, &set_due[0], force).unwrap_err();
        assert!(
            matches!(
                err,
                UndoError::Invariant {
                    source: DomainError::Invalid {
                        field: "recurrence",
                        ..
                    },
                    ..
                }
            ),
            "{err:?}"
        );
    }
    assert_eq!(w.state.tasks[&t].due, Some(d(2026, 3, 2)));
    // Undoing the dependent change first makes it possible.
    w.undo(&set_rec[0]).unwrap();
    w.undo(&set_due[0]).unwrap();
    assert_eq!(w.state.tasks[&t].due, None);
}

/// Finding 6: restoring gives a fresh key when the old one was reused, and placement tolerates ties.
#[test]
fn restore_gets_a_fresh_key_and_placement_tolerates_ties() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let recurring = |w: &mut World, title: &str| {
        let id = w.ids.task();
        let mut n = NewTask::new(id, list, title);
        n.due = Some(d(2026, 3, 2));
        n.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
        w.run(Command::CreateTask(Box::new(n)));
        id
    };
    let a = recurring(&mut w, "a");
    w.run(Command::Delete { id: a.into() });
    let b = w.task(list, "b");
    let restore = w.run(Command::Restore { id: a.into() });
    assert_eq!(restore.len(), 2, "restore re-keys the item: {restore:?}");
    assert_ne!(w.state.tasks[&a].position, w.state.tasks[&b].position);
    let between = w.ids.task();
    w.run(Command::CreateTask(Box::new({
        let mut n = NewTask::new(between, list, "between");
        n.placement = Placement::Before(b.into());
        n
    })));
    let order: Vec<EntityId> = w
        .state
        .siblings(todo_core::state::Scope::Tasks(list))
        .into_iter()
        .map(|x| x.1)
        .collect();
    assert_eq!(order, vec![a.into(), between.into(), b.into()]);
    assert_eq!(w.run(Command::CompleteTask { id: a }).len(), 2);

    // Undoing a move whose old key a newer sibling took meanwhile gives the item a fresh key just before
    // that sibling instead of a tie.
    let list2 = w.list(None, "L2");
    let a2 = w.task(list2, "a");
    let x = w.ids.task();
    let mut n = NewTask::new(x, list2, "x");
    n.due = Some(d(2026, 3, 2));
    n.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    w.run(Command::CreateTask(Box::new(n)));
    let b2 = w.task(list2, "b");
    let old_key = w.state.tasks[&x].position.clone();
    let reorder = w.run(Command::Reorder {
        id: x.into(),
        placement: Placement::First,
    });
    let y = w.ids.task();
    w.run(Command::CreateTask(Box::new({
        let mut n = NewTask::new(y, list2, "y");
        n.placement = Placement::After(a2.into());
        n
    })));
    assert_eq!(w.state.tasks[&y].position, old_key, "y takes x's old key");
    let comp = w.undo(&reorder[0]).unwrap();
    assert_eq!(comp.len(), 2, "the compensation and a rekey");
    let order: Vec<EntityId> = w
        .state
        .siblings(todo_core::state::Scope::Tasks(list2))
        .into_iter()
        .map(|x| x.1)
        .collect();
    assert_eq!(order, vec![a2.into(), x.into(), y.into(), b2.into()]);
    // Redoing the move (the group) puts x first again.
    w.undo_group(&comp).unwrap();
    assert!(w.state.tasks[&x].position < w.state.tasks[&a2].position);

    // Placement tolerates ties all the same (for example in data written before keys were kept unique).
    w.state.tasks.get_mut(&x).unwrap().position = w.state.tasks[&y].position.clone();
    // Deleting and undeleting x restores such a tie as it was: nobody took the key after the deletion.
    let del = w.run(Command::Delete { id: x.into() });
    assert_eq!(w.undo(&del[0]).unwrap().len(), 1);
    assert_eq!(w.state.tasks[&x].position, w.state.tasks[&y].position);
    for placement in [
        Placement::After(x.into()),
        Placement::Before(y.into()),
        Placement::Before(x.into()),
        Placement::After(y.into()),
        Placement::First,
        Placement::Last,
    ] {
        let mut n = NewTask::new(w.ids.task(), list2, "placed");
        n.placement = placement;
        w.run_as(&w.owner.clone(), Command::CreateTask(Box::new(n)))
            .unwrap();
    }
    w.run(Command::Reorder {
        id: y.into(),
        placement: Placement::After(x.into()),
    });
    assert_eq!(w.run(Command::CompleteTask { id: x }).len(), 2);
}

/// Finding 7: adversarial position keys are rejected instead of overflowing the stack.
#[test]
fn adversarial_position_keys_are_rejected() {
    use todo_core::ordering::{MAX_KEY_LEN, OrderError, PosKey};
    let mut w = World::new();
    let list = w.list(None, "L");
    w.task(list, "t");
    let json = serde_json::to_string(&w.log).unwrap();
    let long = "z".repeat(50_000);
    let evil = json.replacen("\"position\":\"V\"", &format!("\"position\":\"{long}\""), 1);
    assert_ne!(evil, json);
    assert!(serde_json::from_str::<Vec<todo_core::Event>>(&evil).is_err());
    let max = PosKey::new("z".repeat(MAX_KEY_LEN)).unwrap();
    assert!(matches!(max.after(), Err(OrderError::TooLong { .. })));
    assert!(matches!(
        PosKey::new("0".repeat(MAX_KEY_LEN) + "1"),
        Err(OrderError::TooLong { .. })
    ));
}

/// Finding 8: RRULE parts the frequency cannot represent are rejected by name.
#[test]
fn rrule_parts_are_not_silently_dropped() {
    use todo_core::recurrence::RecurrenceError;
    assert_eq!(
        Recurrence::from_rrule("FREQ=MONTHLY;BYMONTHDAY=15;BYDAY=MO"),
        Err(RecurrenceError::PartNotSupported {
            part: "BYDAY".into(),
            freq: "MONTHLY".into()
        })
    );
    assert_eq!(
        Recurrence::from_rrule("FREQ=MONTHLY;BYMONTHDAY=15;BYMONTHDAY=16"),
        Err(RecurrenceError::DuplicatePart("BYMONTHDAY".into()))
    );
    assert!(Recurrence::from_rrule("FREQ=MONTHLY;BYMONTHDAY=15;BYSETPOS=1").is_err());
    assert_eq!(
        Recurrence::from_rrule("FREQ=MONTHLY;BYMONTHDAY=15").unwrap(),
        Recurrence::new(Rule::Monthly {
            interval: 1,
            day: 15
        })
    );
}

/// Found by the mixed undo/redo property: redoing a merge's `merged_into` change must not point at a task
/// whose creation was undone meanwhile.
#[test]
fn redo_does_not_leave_a_dangling_merged_into() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let src = w.task(list, "source");
    let dst = w.ids.task();
    let created = w.run(Command::CreateTask(Box::new(NewTask::new(
        dst, list, "target",
    ))));
    let merge = w.run(Command::MergeTasks {
        source: src,
        target: dst,
    });
    let mark = merge
        .iter()
        .find(|e| {
            matches!(&e.payload, EventPayload::Updated { changes, .. }
                if changes.iter().any(|c| matches!(c, FieldChange::MergedInto(_))))
        })
        .unwrap()
        .clone();
    let unmark = w.undo(&mark).unwrap();
    w.undo(&created[0]).unwrap();
    assert!(!w.state.tasks.contains_key(&dst));
    assert_eq!(
        w.undo(&unmark[0]),
        Err(UndoError::BrokenReference(EntityId::Task(dst)))
    );
    assert_eq!(w.state.tasks[&src].merged_into, None);
}

/// Found by the mixed undo/redo property: redoing a creation must not bring back references to entities
/// whose own creation was undone meanwhile.
#[test]
fn redo_of_a_creation_checks_the_snapshot_references() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let tag = w.ids.tag();
    let tag_created = w.run(Command::CreateTag {
        id: tag,
        name: "x".into(),
        color: None,
    });
    let t = w.ids.task();
    let created = w.run(Command::CreateTask(Box::new(NewTask::new(t, list, "t"))));
    w.run(Command::AddTag { task: t, tag });
    let owner = w.owner.clone();
    let withdrawn = w.undo_as(&owner, &created[0], true).unwrap();
    w.undo(&tag_created[0]).unwrap();
    assert_eq!(
        w.undo(&withdrawn[0]),
        Err(UndoError::BrokenReference(EntityId::Tag(tag)))
    );
}

// ---- second review round ---------------------------------------------------------------------------

/// Round 2, finding 1: a withdrawn entity's ID is never given to a new entity, so a redo of the old
/// entity's edits cannot land on a different task.
#[test]
fn withdrawn_ids_are_never_reused() {
    use todo_core::error::ApplyError;
    let mut w = World::new();
    let list = w.list(None, "L");
    let a = w.ids.task();
    let created = w.run(Command::CreateTask(Box::new(NewTask::new(a, list, "A"))));
    let edit = rename(&mut w, a, "A edited");
    let unedit = w.undo(&edit[0]).unwrap();
    let withdrawn = w.undo(&created[0]).unwrap();
    assert!(!w.state.tasks.contains_key(&a));
    assert!(w.state.journal.id_used(a.into()));

    // A new task with the same ID is refused by decide ...
    let now = w.now();
    let reuse = Command::CreateTask(Box::new(NewTask::new(a, list, "B")));
    assert_eq!(
        decide(&w.state, &w.owner, &reuse, now),
        Err(DomainError::AlreadyExists(a.into()))
    );
    // ... and a forged creation event with that ID by apply.
    let mut forged = created[0].clone();
    forged.id = todo_core::ids::EventId::derive(now, b"forged");
    forged.group = forged.id;
    if let EventPayload::Created {
        entity: todo_core::model::Entity::Task(t),
    } = &mut forged.payload
    {
        t.title = "B".into();
    }
    let mut copy = w.state.clone();
    assert_eq!(
        apply(&mut copy, &forged),
        Err(ApplyError::AlreadyExists(a.into()))
    );
    // A forged re-creation claiming to undo some unrelated event is refused as well.
    let mut forged_redo = forged.clone();
    forged_redo.undoes = Some(edit[0].id);
    assert_eq!(
        apply(&mut copy, &forged_redo),
        Err(ApplyError::AlreadyExists(a.into()))
    );

    // Redoing the old edit while A is withdrawn fails instead of touching another task.
    assert!(matches!(
        w.undo(&unedit[0]),
        Err(UndoError::BrokenReference(_))
    ));
    // Only the chain of the original creation brings A back, and then the redo applies to A.
    w.undo(&withdrawn[0]).unwrap();
    w.undo(&unedit[0]).unwrap();
    assert_eq!(w.state.tasks[&a].title, "A edited");
}

/// Round 2, finding 2: undoing a notes change needs plan access, unless it took back a note added with
/// `AddNote` (capture access) that is still exactly as it was added.
#[test]
fn undoing_notes_changes_needs_plan_unless_it_takes_back_an_added_note() {
    let mut w = World::new();
    let list = w.list(None, "L");
    w.grant_default("bot", AccessLevel::Plan);
    let bot = Actor::agent("bot").unwrap();
    let t = w.task(list, "t");
    let set_notes = |notes: &str| Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            notes: Some(notes.into()),
            ..TaskPatch::default()
        },
    };
    w.run(set_notes("owner's notes"));
    let replaced = w.run_as(&bot, set_notes("replaced")).unwrap();
    let cleared = w.run_as(&bot, set_notes("")).unwrap();
    w.grant_default("bot", AccessLevel::Capture);
    let denied = |r: Result<Vec<todo_core::Event>, UndoError>| {
        assert!(matches!(r, Err(UndoError::Denied(_))), "{r:?}");
    };
    denied(w.undo_as(&bot, &cleared[0], false));
    assert!(matches!(
        todo_core::access::authorize_undo(&w.state, &bot, &cleared[0]),
        Err(UndoError::Denied(_))
    ));
    assert_eq!(w.state.tasks[&t].notes, "");
    // The owner may, after which the agent still may not take back its replacement.
    w.undo(&cleared[0]).unwrap();
    denied(w.undo_as(&bot, &replaced[0], false));
    assert_eq!(w.state.tasks[&t].notes, "replaced");

    // A note the agent added with capture access can be taken back and re-added with it.
    let added = w
        .run_as(
            &bot,
            Command::AddNote {
                task: t,
                text: "from the bot".into(),
            },
        )
        .unwrap();
    let EventPayload::Updated { changes, .. } = &added[0].payload else {
        panic!()
    };
    assert!(matches!(changes[0], FieldChange::NoteAdded(_)));
    let removed = w.undo_as(&bot, &added[0], false).unwrap();
    assert_eq!(w.state.tasks[&t].notes, "replaced");
    let readded = w.undo_as(&bot, &removed[0], false).unwrap();
    assert_eq!(w.state.tasks[&t].notes, "replaced\n\nfrom the bot");
    // But forcing it over a later notes change would overwrite that change: that needs plan access.
    w.run(Command::AddNote {
        task: t,
        text: "from the owner".into(),
    });
    denied(w.undo_as(&bot, &readded[0], true));
    assert_eq!(
        w.state.tasks[&t].notes,
        "replaced\n\nfrom the bot\n\nfrom the owner"
    );
}

/// Round 2, finding 2: taking back a merge mark needs plan access on both tasks' lists.
#[test]
fn undoing_a_merge_mark_needs_plan_on_both_tasks() {
    let mut w = World::new();
    let a = w.list(None, "A");
    let b = w.list(None, "B");
    let mut g = Grants::with_default(AccessLevel::None);
    g.lists.insert(a, AccessLevel::Plan);
    g.lists.insert(b, AccessLevel::Plan);
    w.grant("bot", g.clone());
    let bot = Actor::agent("bot").unwrap();
    let src = w.task(a, "source");
    let dst = w.task(b, "target");
    let merge = w
        .run_as(
            &bot,
            Command::MergeTasks {
                source: src,
                target: dst,
            },
        )
        .unwrap();
    let mark = merge
        .iter()
        .find(|e| {
            matches!(&e.payload, EventPayload::Updated { changes, .. }
                if changes.iter().any(|c| matches!(c, FieldChange::MergedInto(_))))
        })
        .unwrap()
        .clone();
    // The agent keeps plan access on the source's list but loses it on the target's.
    g.lists.insert(b, AccessLevel::Capture);
    w.grant("bot", g.clone());
    assert!(matches!(
        w.undo_as(&bot, &mark, false),
        Err(UndoError::Denied(_))
    ));
    assert_eq!(w.state.tasks[&src].merged_into, Some(dst));
    g.lists.insert(b, AccessLevel::Plan);
    w.grant("bot", g);
    w.undo_as(&bot, &mark, false).unwrap();
    assert_eq!(w.state.tasks[&src].merged_into, None);
}

/// Round 2, finding 3: undoing a restore conflicts with any later change to the entity, including a
/// compensating one, and stops conflicting once that change is undone again.
#[test]
fn undoing_a_restore_conflicts_with_a_later_compensation() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let t = w.task(list, "one");
    let edit = rename(&mut w, t, "two");
    w.run(Command::Delete { id: t.into() });
    let restored = w.run(Command::Restore { id: t.into() });
    let unedit = w.undo(&edit[0]).unwrap();
    assert_eq!(w.state.tasks[&t].title, "one");
    let err = w.undo(&restored[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.conflicts[0].field, FieldKey::Title);
    assert_eq!(c.conflicts[0].later_events, vec![unedit[0].id]);
    assert!(w.state.is_live(t.into()));
    // Once the title is back to what the restore left, re-deleting hides nothing new.
    w.undo(&unedit[0]).unwrap();
    w.undo(&restored[0]).unwrap();
    assert!(!w.state.is_live(t.into()));

    // The same holds for compensations under a restored list.
    let other = w.list(None, "other");
    let child = w.task(other, "child");
    let child_edit = rename(&mut w, child, "child edited");
    w.run(Command::Delete { id: other.into() });
    let back = w.run(Command::Restore { id: other.into() });
    w.undo(&child_edit[0]).unwrap();
    let err = w.undo(&back[0]).unwrap_err();
    let UndoError::Conflict(c) = err else {
        panic!("{err:?}")
    };
    assert_eq!(c.conflicts[0].target, EntityId::Task(child));
    assert_eq!(c.conflicts[0].field, FieldKey::Title);
}

/// Round 2, finding 4: undoing a deletion re-keys the entity if a sibling took its key meanwhile, and
/// undoing that compensation as a group puts everything back.
#[test]
fn undoing_a_delete_rekeys_when_the_key_was_taken() {
    let mut w = World::new();
    let list = w.list(None, "L");
    let x = w.task(list, "x");
    let key = w.state.tasks[&x].position.clone();
    let del = w.run(Command::Delete { id: x.into() });
    let deleted = w.state.clone();
    let y = w.task(list, "y");
    assert_eq!(w.state.tasks[&y].position, key, "y takes the free key");
    let comp = w.undo(&del[0]).unwrap();
    assert_eq!(comp.len(), 2, "{comp:?}");
    assert_eq!(comp[1].group, comp[0].id);
    assert_eq!(comp[1].undoes, None);
    assert_ne!(w.state.tasks[&x].position, w.state.tasks[&y].position);
    let order: Vec<EntityId> = w
        .state
        .siblings(todo_core::state::Scope::Tasks(list))
        .into_iter()
        .map(|s| s.1)
        .collect();
    assert_eq!(order, vec![x.into(), y.into()]);
    // Undoing only the restore would leave the new key behind: it conflicts on the position.
    assert_eq!(
        conflict_fields(w.undo(&comp[0]).unwrap_err()),
        vec![FieldKey::Position]
    );
    // Undoing only the rekey would put x back on y's key: a position conflict, on its own and as a
    // partial batch.
    assert_eq!(
        conflict_fields(w.undo(&comp[1]).unwrap_err()),
        vec![FieldKey::Position]
    );
    assert_eq!(
        conflict_fields(w.undo_group(&comp[1..]).unwrap_err()),
        vec![FieldKey::Position]
    );
    assert_ne!(w.state.tasks[&x].position, w.state.tasks[&y].position);
    // Undoing the group re-deletes x with its old key.
    let with_y = {
        let mut s = deleted.clone();
        s.tasks.insert(y, w.state.tasks[&y].clone());
        s
    };
    w.undo_group(&comp).unwrap();
    assert!(w.state.data_eq(&with_y));
    assert_eq!(w.state.tasks[&x].position, key);
}

/// Final check: a forced take-back of an added note replaces someone else's notes, so it becomes an
/// ordinary notes change, and reversing it needs plan access.
#[test]
fn forced_note_take_back_needs_plan_to_reverse() {
    let mut w = World::new();
    let list = w.list(None, "L");
    w.grant_default("bot", AccessLevel::Plan);
    let bot = Actor::agent("bot").unwrap();
    let t = w.task(list, "t");
    let added = w
        .run_as(
            &bot,
            Command::AddNote {
                task: t,
                text: "from the bot".into(),
            },
        )
        .unwrap();
    w.run(Command::UpdateTask {
        id: t,
        patch: TaskPatch {
            notes: Some("owner's notes".into()),
            ..TaskPatch::default()
        },
    });
    let forced = w.undo_as(&bot, &added[0], true).unwrap();
    assert_eq!(w.state.tasks[&t].notes, "");
    let EventPayload::Updated { changes, .. } = &forced[0].payload else {
        panic!()
    };
    assert!(matches!(changes[0], FieldChange::Notes(_)), "{changes:?}");
    // Downgraded to capture, the agent can't put its replacement back over the owner's notes.
    w.grant_default("bot", AccessLevel::Capture);
    let r = w.undo_as(&bot, &forced[0], false);
    assert!(matches!(r, Err(UndoError::Denied(_))), "{r:?}");
    assert_eq!(w.state.tasks[&t].notes, "");
}

/// Round 2, finding 6: `UNTIL` must be exactly a date or a UTC date-time.
#[test]
fn rrule_until_rejects_trailing_junk() {
    assert!(Recurrence::from_rrule("FREQ=DAILY;UNTIL=20261231junk").is_err());
    assert!(Recurrence::from_rrule("FREQ=DAILY;UNTIL=20261231T000000Zjunk").is_err());
    assert_eq!(
        Recurrence::from_rrule("FREQ=DAILY;UNTIL=20261231T000000Z")
            .unwrap()
            .until,
        Some(d(2026, 12, 31))
    );
}

// ---- import ---------------------------------------------------------------------------------------

mod import {
    use super::*;
    use todo_core::model::{Entity, Stamp, Step, Task};
    use todo_core::ordering::PosKey;
    use todo_core::state::Scope;

    fn stamp(secs: i64) -> Stamp {
        Stamp {
            at: ts(secs),
            by: Actor::System,
        }
    }

    fn imported_task(w: &mut World, list: todo_core::ids::ListId, key: &str) -> Task {
        Task {
            id: w.ids.task(),
            list,
            title: "  old task ".into(),
            notes: "from elsewhere".into(),
            due: Some(d(2024, 5, 1)),
            priority: Priority::High,
            my_day: None,
            completed_at: Some(ts(-300)),
            recurrence: Some(Recurrence::new(Rule::Daily { interval: 1 })),
            tags: Default::default(),
            position: PosKey::new(key).unwrap(),
            created: stamp(-1000),
            updated: stamp(-200),
            deleted: None,
            merged_into: None,
        }
    }

    fn import(e: Entity) -> Command {
        Command::ImportEntity {
            entity: Box::new(e),
        }
    }

    #[test]
    fn keeps_historical_times_and_restamps_the_actor() {
        let mut w = World::new();
        let list = w.list(None, "Inbox");
        let t = imported_task(&mut w, list, "V");
        let events = w.run(import(Entity::Task(t.clone())));
        // One creation, and a completed recurring task does not spawn its next occurrence.
        assert_eq!(events.len(), 1);
        let got = &w.state.tasks[&t.id];
        assert_eq!(got.title, "old task");
        assert_eq!(got.created.at, ts(-1000));
        assert_eq!(got.updated.at, ts(-200));
        assert_eq!(got.completed_at, Some(ts(-300)));
        assert_eq!(got.created.by, w.owner);
        assert_eq!(got.updated.by, w.owner);
        assert_eq!(got.position.to_string(), "V");
        assert_eq!(w.state.tasks.len(), 1);

        let step = Step {
            id: w.ids.step(),
            task: t.id,
            title: "done step".into(),
            completed_at: Some(ts(-250)),
            position: PosKey::new("V").unwrap(),
            created: stamp(-900),
            deleted: None,
        };
        w.run(import(Entity::Step(step.clone())));
        assert_eq!(w.state.steps[&step.id].completed_at, Some(ts(-250)));
        assert_eq!(w.state.steps[&step.id].created.at, ts(-900));
    }

    #[test]
    fn is_owner_only_and_validates_like_a_create() {
        let mut w = World::new();
        w.grant_default("bot", AccessLevel::Plan);
        let bot = Actor::agent("bot").unwrap();
        let list = w.list(None, "Inbox");
        let t = imported_task(&mut w, list, "V");
        assert!(matches!(
            w.run_as(&bot, import(Entity::Task(t.clone()))),
            Err(DomainError::Denied(_))
        ));
        // Importer and system actors have owner rights.
        let mut deleted = t.clone();
        deleted.deleted = Some(todo_core::model::Tombstone {
            at: ts(-1),
            by: Actor::System,
        });
        assert!(matches!(
            w.run_as(&Actor::Importer, import(Entity::Task(deleted))),
            Err(DomainError::Invalid {
                field: "deleted",
                ..
            })
        ));
        let mut merged = t.clone();
        merged.merged_into = Some(w.ids.task());
        assert!(matches!(
            w.run_as(&Actor::Importer, import(Entity::Task(merged))),
            Err(DomainError::Invalid {
                field: "merged_into",
                ..
            })
        ));
        let mut no_due = t.clone();
        no_due.due = None;
        assert!(matches!(
            w.run_as(&Actor::Importer, import(Entity::Task(no_due))),
            Err(DomainError::Invalid {
                field: "recurrence",
                ..
            })
        ));
        let mut orphan = t.clone();
        orphan.list = w.ids.list();
        assert!(matches!(
            w.run_as(&Actor::Importer, import(Entity::Task(orphan))),
            Err(DomainError::NotFound(_))
        ));
        w.run_as(&Actor::Importer, import(Entity::Task(t.clone())))
            .unwrap();
        assert_eq!(w.state.tasks[&t.id].created.by, Actor::Importer);
        assert!(matches!(
            w.run_as(&Actor::Importer, import(Entity::Task(t))),
            Err(DomainError::AlreadyExists(_))
        ));
    }

    #[test]
    fn taken_key_gets_a_fresh_one_before_the_sibling() {
        let mut w = World::new();
        let list = w.list(None, "Inbox");
        let a = imported_task(&mut w, list, "V");
        let b = imported_task(&mut w, list, "V");
        w.run(import(Entity::Task(a.clone())));
        w.run(import(Entity::Task(b.clone())));
        let order: Vec<_> = w
            .state
            .siblings(Scope::Tasks(list))
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        assert_eq!(order, vec![b.id.into(), a.id.into()]);
    }

    #[test]
    fn undo_withdraws_and_redo_brings_it_back() {
        let mut w = World::new();
        let list = w.list(None, "Inbox");
        let before = w.state.clone();
        let t = imported_task(&mut w, list, "V");
        let ev = w.run(import(Entity::Task(t.clone())));
        let comp = w.undo(&ev[0]).unwrap();
        assert!(w.state.data_eq(&before));
        w.undo(&comp[0]).unwrap();
        assert_eq!(w.state.tasks[&t.id].created.at, ts(-1000));
    }

    #[test]
    fn imported_tags_must_be_live_and_unique() {
        let mut w = World::new();
        let tag = todo_core::model::Tag {
            id: w.ids.tag(),
            name: "Work".into(),
            color: None,
            created: stamp(-10),
            deleted: None,
        };
        w.run(import(Entity::Tag(tag.clone())));
        let mut dup = tag.clone();
        dup.id = w.ids.tag();
        dup.name = "work".into();
        assert!(matches!(
            w.run_as(&w.owner.clone(), import(Entity::Tag(dup))),
            Err(DomainError::DuplicateName(_))
        ));
        let list = w.list(None, "Inbox");
        let mut t = imported_task(&mut w, list, "V");
        t.tags.insert(w.ids.tag());
        assert!(matches!(
            w.run_as(&w.owner.clone(), import(Entity::Task(t.clone()))),
            Err(DomainError::NotFound(_))
        ));
        t.tags = [tag.id].into_iter().collect();
        w.run(import(Entity::Task(t.clone())));
        assert!(w.state.tasks[&t.id].tags.contains(&tag.id));
    }
}
