//! Focused workflow invariants in addition to the randomized event histories.
#![allow(clippy::unwrap_used)]

mod common;

use common::World;
use todo_core::command::{
    BulkTaskAction, Command, ListFeaturesPatch, NewTask, Placement, TaskPatch,
};
use todo_core::error::{DomainError, UndoError};
use todo_core::event::{EventPayload, FieldChange};
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
    let disabled = ListFeaturesPatch {
        default_view: Some(ListView::Board),
        ..ListFeaturesPatch::default()
    };
    let owner = w.owner.clone();
    assert!(matches!(
        w.run_as(&owner, settings(disabled)),
        Err(DomainError::Invalid { .. })
    ));
    let enabled = ListFeaturesPatch {
        workflow: Some(true),
        timeline: Some(true),
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
    let disabled = w.run(settings(ListFeaturesPatch {
        workflow: Some(false),
        timeline: Some(false),
        ..ListFeaturesPatch::default()
    }));
    assert_eq!(w.state.lists[&list].features, ListFeatures::default());
    assert_eq!(w.state.tasks[&id], task_before);
    w.undo_group(&disabled).unwrap();
    assert_eq!(
        w.state.lists[&list].features,
        enabled.merge(ListFeatures::default())
    );
    assert!(
        w.run_as(
            &owner,
            patch(TaskPatch {
                due: Some(Some(date(7))),
                start_date: Some(Some(date(8))),
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
fn one_endpoint_edits_preserve_the_requested_date_and_undo_together() {
    let mut w = World::new();
    let list = w.list(None, "Project");
    let id = w.ids.task();
    let mut task = NewTask::new(id, list, "Task");
    task.status = WorkflowStatus::InProgress;
    task.start_date = Some(date(8));
    task.due = Some(date(12));
    task.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    w.run(Command::CreateTask(Box::new(task)));
    let before = w.state.clone();
    let legacy: Command = serde_json::from_value(serde_json::json!({
        "cmd": "update_task", "id": id, "patch": {"due": "2030-01-05", "title": null, "notes": null, "priority": null}
    })).unwrap();
    for (command, day) in [
        (legacy, date(5)),
        (
            Command::UpdateTask {
                id,
                patch: TaskPatch {
                    start_date: Some(Some(date(16))),
                    ..TaskPatch::default()
                },
            },
            date(16),
        ),
    ] {
        let events = w.run(command);
        let [event] = events.as_slice() else {
            panic!("expected one event")
        };
        let EventPayload::Updated { changes, .. } = &event.payload else {
            panic!("expected an update")
        };
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, FieldChange::StartDate(c) if c.new == Some(day)))
        );
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, FieldChange::Due(c) if c.new == Some(day)))
        );
        let task = &w.state.tasks[&id];
        assert_eq!(task.status, WorkflowStatus::InProgress);
        assert_eq!(task.recurrence, before.tasks[&id].recurrence);
        w.undo_group(&events).unwrap();
        assert!(before.data_eq(&w.state));
    }
}

#[test]
fn bulk_due_reconciles_each_span_and_undoes_as_one_group() {
    let mut w = World::new();
    let list = w.list(None, "Project");
    let moving = w.task(list, "Later start");
    let unchanged_start = w.task(list, "Earlier start");
    let without_start = w.task(list, "Due only");
    for (id, start) in [
        (moving, Some(date(8))),
        (unchanged_start, Some(date(2))),
        (without_start, None),
    ] {
        w.run(Command::UpdateTask {
            id,
            patch: TaskPatch {
                start_date: Some(start),
                due: Some(Some(date(10))),
                ..TaskPatch::default()
            },
        });
    }
    let before = w.state.clone();
    let ids = vec![moving, unchanged_start, without_start];
    for (due, moving_start) in [(Some(date(5)), Some(date(5))), (None, Some(date(8)))] {
        let events = w.run(Command::BulkTasks {
            tasks: ids.clone(),
            action: BulkTaskAction::Due { date: due },
        });
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|e| e.group == events[0].id));
        for (id, start) in [
            (moving, moving_start),
            (unchanged_start, Some(date(2))),
            (without_start, None),
        ] {
            let task = &w.state.tasks[&id];
            assert_eq!((task.start_date, task.due), (start, due));
        }
        w.undo_group(&events).unwrap();
        assert!(before.data_eq(&w.state));
    }
}

#[test]
fn disjoint_feature_patches_keep_each_others_changes() {
    let mut w = World::new();
    let id = w.list(None, "Project");
    let patch = |features| Command::UpdateList {
        id,
        name: None,
        color: None,
        features: Some(features),
    };
    // Both commands can be built from the same initial checklist without copying its other fields.
    let board = patch(ListFeaturesPatch {
        workflow: Some(true),
        default_view: Some(ListView::Board),
        ..ListFeaturesPatch::default()
    });
    let timeline = patch(ListFeaturesPatch {
        timeline: Some(true),
        ..ListFeaturesPatch::default()
    });
    w.run(board);
    let events = w.run(timeline);
    assert_eq!(
        w.state.lists[&id].features,
        ListFeatures {
            workflow: true,
            timeline: true,
            default_view: ListView::Board
        }
    );
    w.undo_group(&events).unwrap();
    assert_eq!(
        w.state.lists[&id].features,
        ListFeatures {
            workflow: true,
            timeline: false,
            default_view: ListView::Board
        }
    );
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
