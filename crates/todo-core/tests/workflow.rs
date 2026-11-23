//! Focused workflow invariants in addition to the randomized event histories.
#![allow(clippy::unwrap_used)]

mod common;

use common::World;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::error::{DomainError, UndoError};
use todo_core::model::{ListFeatures, ListView, WorkflowStatus};
use todo_core::query::{EvalContext, Query, QueryStatus};
use todo_core::recurrence::{Recurrence, Rule};
use todo_core::time::Date;

fn date(day: i8) -> Date {
    Date::constant(2030, 1, day)
}

#[test]
fn completion_and_board_reopen_restart_repeat_with_the_same_span_and_undo_atomically() {
    let mut w = World::new();
    let list = w.list(None, "Project");
    let anchor = w.task(list, "Anchor");
    let id = w.ids.task();
    let mut n = NewTask::new(id, list, "Repeat");
    n.start_date = Some(date(2));
    n.due = Some(date(5));
    n.recurrence = Some(Recurrence::new(Rule::Daily { interval: 7 }));
    n.status = WorkflowStatus::InProgress;
    w.run(Command::CreateTask(Box::new(n)));
    let before = w.state.clone();
    let completed = w.run(Command::CompleteTask { id });
    let next = w
        .state
        .tasks
        .values()
        .find(|t| t.id != id && t.id != anchor)
        .unwrap();
    assert_eq!((next.start_date, next.due), (Some(date(9)), Some(date(12))));
    assert_eq!(QueryStatus::of(next), QueryStatus::Todo);
    assert_eq!(QueryStatus::of(&w.state.tasks[&id]), QueryStatus::Done);
    let done = w.state.clone();
    let placed = w.run(Command::MoveTaskStatus {
        id,
        list,
        status: WorkflowStatus::Todo,
        placement: Placement::Before(anchor.into()),
    });
    assert_eq!(placed.len(), 1);
    let task = &w.state.tasks[&id];
    assert_eq!(QueryStatus::of(task), QueryStatus::Todo);
    assert!(task.position < w.state.tasks[&anchor].position);
    w.undo_group(&placed).unwrap();
    assert!(done.data_eq(&w.state));
    w.undo_group(&completed).unwrap();
    assert!(before.data_eq(&w.state));

    w.run(Command::CompleteTask { id });
    w.run(Command::UncompleteTask { id });
    assert_eq!(
        QueryStatus::of(&w.state.tasks[&id]),
        QueryStatus::InProgress
    );
}

#[test]
fn disable_only_hides_data_and_bad_date_writes_and_undos_are_refused() {
    let mut w = World::new();
    let list = w.list(None, "Project");
    let id = w.task(list, "Task");
    let settings = |features| Command::UpdateList {
        id: list,
        name: None,
        color: None,
        features: Some(features),
    };
    let disabled = ListFeatures {
        default_view: ListView::Board,
        ..ListFeatures::default()
    };
    let owner = w.owner.clone();
    assert!(matches!(
        w.run_as(&owner, settings(disabled)),
        Err(DomainError::Invalid { .. })
    ));
    let enabled = ListFeatures {
        workflow: true,
        timeline: true,
        ..disabled
    };
    w.run(settings(enabled));
    let patch = |patch| Command::UpdateTask { id, patch };
    let dates = w.run(patch(TaskPatch {
        start_date: Some(Some(date(8))),
        due: Some(Some(date(10))),
        status: Some(WorkflowStatus::Backlog),
        ..TaskPatch::default()
    }));
    let task_before = w.state.tasks[&id].clone();
    let disabled = w.run(settings(ListFeatures {
        timeline: false,
        ..disabled
    }));
    assert_eq!(w.state.lists[&list].features, ListFeatures::default());
    assert_eq!(w.state.tasks[&id], task_before);
    w.undo_group(&disabled).unwrap();
    assert_eq!(w.state.lists[&list].features, enabled);
    assert!(
        w.run_as(
            &owner,
            patch(TaskPatch {
                due: Some(Some(date(7))),
                ..TaskPatch::default()
            })
        )
        .is_err()
    );

    // A later start edit can make a separately undoable due edit invalid when reversed.
    let due = w.run(patch(TaskPatch {
        due: Some(Some(date(12))),
        ..TaskPatch::default()
    }));
    w.run(patch(TaskPatch {
        start_date: Some(Some(date(11))),
        ..TaskPatch::default()
    }));
    let state = w.state.clone();
    assert!(matches!(
        w.undo_as(&owner, &due[0], true),
        Err(UndoError::Invariant { .. })
    ));
    assert!(matches!(
        w.undo_as(&owner, &dates[0], false),
        Err(UndoError::Conflict(_))
    ));
    assert_eq!(state, w.state);
}

#[test]
fn status_and_start_queries_use_effective_stages_and_order() {
    let mut w = World::new();
    let list = w.list(None, "Checklist"); // Filters are independent of capability flags.
    let todo = w.task(list, "To do");
    let backlog = w.ids.task();
    let mut b = NewTask::new(backlog, list, "Backlog");
    b.status = WorkflowStatus::Backlog;
    b.start_date = Some(date(3));
    w.run(Command::CreateTask(Box::new(b)));
    let progress = w.ids.task();
    let mut p = NewTask::new(progress, list, "Underway");
    p.status = WorkflowStatus::InProgress;
    p.start_date = Some(date(2));
    w.run(Command::CreateTask(Box::new(p)));
    let done = w.task(list, "Done");
    w.run(Command::CompleteTask { id: done });
    let ctx = EvalContext::new(common::ts(0), date(2), 0);
    for (query, expected) in [
        ("sort:status", vec![backlog, todo, progress, done]),
        ("status!=done sort:-status", vec![progress, todo, backlog]),
        ("status:todo start:none", vec![todo]),
        ("status:done", vec![done]),
        ("start:any sort:start", vec![progress, backlog]),
        ("start<=today", vec![progress]),
    ] {
        let q = Query::parse(query).unwrap();
        assert_eq!(Query::parse(&q.to_string()).unwrap(), q);
        assert_eq!(
            q.run(&w.state, &ctx)
                .into_iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            expected
        );
    }
    assert!(Query::parse("status>=todo").is_err());
}
