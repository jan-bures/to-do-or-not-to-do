#![allow(clippy::unwrap_used)]

use todo_client::{
    board,
    intent::{self, TaskRoom, quick_add::Draft, workflow},
    room_header::{RoomKey, SmartList},
    timeline,
};
use todo_core::{
    Actor, Command, State, apply,
    command::{ListFeaturesPatch, NewTask, Placement},
    decide,
    ids::{ListId, TaskId},
    model::{ListView, WorkflowStatus},
    query::{EvalContext, QueryStatus},
    time::{Date, Timestamp},
};
use uuid::Uuid;

fn date(day: i8) -> Date {
    Date::constant(2030, 1, day)
}
fn time(d: Date, hour: i8) -> Timestamp {
    format!("{d}T{hour:02}:00:00Z").parse().unwrap()
}

const FEATURES: ListFeaturesPatch = ListFeaturesPatch {
    workflow: Some(true),
    timeline: Some(true),
    default_view: Some(ListView::Board),
};

struct World {
    state: State,
    next_id: u128,
    list: ListId,
    now: Timestamp,
}
impl World {
    fn new() -> Self {
        let list = ListId::from_uuid(Uuid::from_u128(1));
        let mut w = Self {
            state: State::new(),
            next_id: 10,
            list,
            now: time(date(20), 12),
        };
        w.run(Command::CreateList {
            id: list,
            collection: None,
            name: "Project".into(),
            color: None,
            placement: Placement::Last,
        });
        w
    }
    fn run(&mut self, cmd: Command) {
        self.run_at(cmd, self.now);
    }
    fn run_at(&mut self, cmd: Command, at: Timestamp) {
        let owner = Actor::owner("test").unwrap();
        for e in decide(&self.state, &owner, &cmd, at).unwrap() {
            apply(&mut self.state, &e).unwrap();
        }
    }
    fn enable(&mut self) {
        self.run(workflow::set_list_features(&self.state, self.list, FEATURES).unwrap());
    }
    fn task(
        &mut self,
        status: WorkflowStatus,
        start_date: Option<Date>,
        due: Option<Date>,
    ) -> TaskId {
        let id = TaskId::from_uuid(Uuid::from_u128(self.next_id));
        self.next_id += 1;
        let mut n = NewTask::new(id, self.list, "Task");
        n.status = status;
        n.start_date = start_date;
        n.due = due;
        self.run(Command::CreateTask(Box::new(n)));
        id
    }
}

#[test]
fn board_respects_manual_columns_and_caps_recent_done_without_losing_the_total() {
    let mut w = World::new();
    let ctx = EvalContext::utc(w.now);
    assert!(board::project(&w.state, w.list, &ctx).is_none());
    w.enable();
    let first = w.task(WorkflowStatus::Todo, None, None);
    let moving = w.task(WorkflowStatus::InProgress, None, None);
    let last = w.task(WorkflowStatus::Todo, None, None);
    let old = w.task(WorkflowStatus::Backlog, None, None);
    w.run_at(Command::CompleteTask { id: old }, time(date(13), 23));
    let oldest_shown = w.task(WorkflowStatus::Todo, None, None);
    w.run_at(
        Command::CompleteTask { id: oldest_shown },
        time(date(14), 0),
    );
    let board = board::project(&w.state, w.list, &ctx).unwrap();
    assert_eq!(
        board.columns.map(|c| c.cards),
        [vec![], vec![first, last], vec![moving], vec![oldest_shown]]
    );
    assert_eq!(board.total_done, 2);
    let mut recent = Vec::new();
    for hour in 0..22 {
        let id = w.task(WorkflowStatus::Todo, None, None);
        w.run_at(Command::CompleteTask { id }, time(date(19), hour));
        recent.push(id);
    }
    let board = board::project(&w.state, w.list, &ctx).unwrap();
    assert_eq!(board.total_done, 24);
    assert_eq!(
        board.columns[3].cards,
        recent.into_iter().rev().take(20).collect::<Vec<_>>()
    );
    let resolve =
        |w: &World, id, target, slot| board::drop_command(&w.state, w.list, id, target, slot);
    assert!(resolve(&w, first, QueryStatus::Todo, Placement::Before(last.into())).is_none());
    assert!(
        resolve(
            &w,
            last,
            QueryStatus::InProgress,
            Placement::Before(first.into())
        )
        .is_none()
    );
    let moved = resolve(
        &w,
        moving,
        QueryStatus::Todo,
        Placement::Before(last.into()),
    )
    .unwrap();
    w.run(moved);
    assert_eq!(
        board::project(&w.state, w.list, &ctx).unwrap().columns[1].cards,
        [first, moving, last]
    );
    let done = resolve(&w, moving, QueryStatus::Done, Placement::Last).unwrap();
    assert_eq!(done, Command::CompleteTask { id: moving });
    w.run(done);
    let reopened = resolve(&w, moving, QueryStatus::Backlog, Placement::First).unwrap();
    w.run(reopened);
    assert_eq!(
        board::project(&w.state, w.list, &ctx).unwrap().columns[0].cards,
        [moving]
    );
}

#[test]
fn timeline_shapes_clip_inclusively_and_planned_includes_start_only_tasks() {
    use timeline::SpanKind;
    let mut w = World::new();
    let ctx = EvalContext::utc(w.now);
    w.enable();
    let range = w.task(WorkflowStatus::Todo, Some(date(5)), Some(date(9)));
    let due = w.task(WorkflowStatus::Todo, None, Some(date(2)));
    let open = w.task(WorkflowStatus::InProgress, Some(date(8)), None);
    let tray = w.task(WorkflowStatus::Backlog, None, None);
    let t = timeline::room(&w.state, RoomKey::List(w.list), &ctx).unwrap();
    assert_eq!(t.unscheduled, [tray]);
    assert_eq!(
        t.lanes
            .iter()
            .map(|l| (l.task, l.span.kind))
            .collect::<Vec<_>>(),
        [
            (range, SpanKind::Range),
            (due, SpanKind::Milestone),
            (open, SpanKind::OpenEnded)
        ]
    );
    assert_eq!(t.lanes[0].span.in_window(date(6), date(10)), Some((0, 4)));
    assert_eq!(t.lanes[2].span.in_window(date(7), date(10)), Some((1, 1)));
    assert_eq!(t.lanes[1].span.in_window(date(3), date(10)), None);
    let planned = timeline::room(&w.state, RoomKey::Smart(SmartList::Planned), &ctx).unwrap();
    assert!(planned.unscheduled.is_empty());
    assert_eq!(
        planned.lanes.iter().map(|l| l.task).collect::<Vec<_>>(),
        [due, range, open]
    );
    let shift = workflow::set_start(&w.state, range, Some(date(10))).unwrap();
    w.run(shift);
    assert_eq!(w.state.tasks[&range].due, Some(date(10)));
    assert_eq!(w.state.tasks[&range].start_date, Some(date(10)));
}

#[test]
fn timeline_keeps_the_open_slot_of_a_held_completion_in_list_and_planned() {
    let mut w = World::new();
    w.enable();
    let first = w.task(WorkflowStatus::InProgress, Some(date(5)), None);
    let second = w.task(WorkflowStatus::Todo, None, Some(date(9)));
    w.run(Command::CompleteTask { id: first });
    let ctx = EvalContext::utc(w.now);
    let held = [first].into_iter().collect();
    let list = RoomKey::List(w.list);
    let planned = RoomKey::Smart(SmartList::Planned);
    for room in [list, planned] {
        let timeline = timeline::room_with_held(&w.state, room, &ctx, &held).unwrap();
        assert_eq!(
            timeline
                .lanes
                .iter()
                .map(|lane| (lane.task, lane.stage))
                .collect::<Vec<_>>(),
            [
                (first, QueryStatus::InProgress),
                (second, QueryStatus::Todo)
            ]
        );
    }
    assert_eq!(
        timeline::room(&w.state, list, &ctx).unwrap().lanes[1].stage,
        QueryStatus::Done
    );
    assert_eq!(
        timeline::room(&w.state, planned, &ctx).unwrap().lanes.len(),
        1
    );
}

#[test]
fn timeline_moves_both_dates_in_one_undo_and_refuses_a_stale_gesture() {
    use workflow::{Dates, Schedule, schedule};
    let mut w = World::new();
    w.enable();
    let id = w.task(WorkflowStatus::InProgress, Some(date(5)), Some(date(9)));
    let before = Dates::of(&w.state.tasks[&id]);
    let room = RoomKey::List(w.list);
    assert!(schedule(&w.state, room, id, before, Schedule::Move(0)).is_none());
    let command = schedule(&w.state, room, id, before, Schedule::Move(7)).unwrap();
    let actor = Actor::owner("test").unwrap();
    let events = decide(&w.state, &actor, &command, w.now).unwrap();
    let [event] = &events[..] else {
        panic!("one atomic date event")
    };
    apply(&mut w.state, event).unwrap();
    assert_eq!(
        Dates::of(&w.state.tasks[&id]),
        Dates {
            start: Some(date(12)),
            due: Some(date(16))
        }
    );
    assert!(schedule(&w.state, room, id, before, Schedule::StartEdge(-1)).is_none());
    for inverse in todo_core::undo::undo(&w.state, event, &actor, w.now).unwrap() {
        apply(&mut w.state, &inverse).unwrap();
    }
    assert_eq!(Dates::of(&w.state.tasks[&id]), before);
}

#[test]
fn timeline_resizes_missing_edges_and_keeps_valid_inclusive_spans() {
    use workflow::{Dates, Schedule};
    let milestone = Dates {
        start: None,
        due: Some(date(9)),
    };
    assert_eq!(
        Schedule::Move(-2).preview(milestone),
        Some(Dates {
            start: None,
            due: Some(date(7))
        })
    );
    assert_eq!(Schedule::StartEdge(1).preview(milestone), None);
    assert_eq!(
        Schedule::StartEdge(-2).preview(milestone).unwrap().start,
        Some(date(7))
    );
    assert_eq!(
        Schedule::Duration(1).preview(milestone),
        Some(Dates {
            start: Some(date(9)),
            due: Some(date(10))
        })
    );
    let open = Dates {
        start: Some(date(5)),
        due: None,
    };
    assert_eq!(Schedule::DueEdge(-2).preview(open), None);
    let range = Schedule::DueEdge(3).preview(open).unwrap();
    assert_eq!(range.due, Some(date(8)));
    assert_eq!(
        Schedule::DueEdge(-6).preview(range).unwrap().due,
        range.start
    );
    assert_eq!(
        Schedule::StartEdge(6).preview(range).unwrap().start,
        range.due
    );
    assert_eq!(
        Schedule::Move(-1).preview(Dates {
            start: Some(Date::MIN),
            due: None
        }),
        None
    );
}

#[test]
fn timeline_scheduling_tray_and_cross_list_planned_share_membership_rules() {
    use workflow::{Dates, Schedule, schedule};
    let mut w = World::new();
    let id = w.task(WorkflowStatus::Todo, None, None);
    let planned = RoomKey::Smart(SmartList::Planned);
    let list = RoomKey::List(w.list);
    assert!(
        schedule(
            &w.state,
            planned,
            id,
            Dates::default(),
            Schedule::Place(date(20))
        )
        .is_none()
    );
    assert!(
        schedule(
            &w.state,
            list,
            id,
            Dates::default(),
            Schedule::Place(date(20))
        )
        .is_none()
    );
    w.enable();
    w.run(
        schedule(
            &w.state,
            list,
            id,
            Dates::default(),
            Schedule::Place(date(20)),
        )
        .unwrap(),
    );
    let before = Dates::of(&w.state.tasks[&id]);
    assert!(schedule(&w.state, list, id, before, Schedule::Place(date(21))).is_none());
    w.run(
        workflow::set_list_features(
            &w.state,
            w.list,
            ListFeaturesPatch {
                timeline: Some(false),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    assert!(schedule(&w.state, list, id, before, Schedule::Move(1)).is_none());
    w.run(schedule(&w.state, planned, id, before, Schedule::StartEdge(-1)).unwrap());
    assert_eq!(w.state.tasks[&id].start_date, Some(date(19)));
    w.run(Command::CompleteTask { id });
    assert!(
        schedule(
            &w.state,
            planned,
            id,
            Dates::of(&w.state.tasks[&id]),
            Schedule::Move(1)
        )
        .is_none()
    );
}

#[test]
fn start_picker_does_not_overwrite_a_newer_due_date() {
    let mut w = World::new();
    w.enable();
    let id = w.task(WorkflowStatus::Todo, Some(date(5)), Some(date(9)));
    let local_edit = workflow::set_start(&w.state, id, Some(date(10))).unwrap();
    let Command::UpdateTask { patch, .. } = &local_edit else {
        panic!("expected a patch")
    };
    assert_eq!(patch.due, None);
    w.run(intent::set_due(&w.state, id, Some(date(12))).unwrap());
    w.run(local_edit);
    let task = &w.state.tasks[&id];
    assert_eq!(
        (task.start_date, task.due),
        (Some(date(10)), Some(date(12)))
    );
    w.run(intent::set_due(&w.state, id, Some(date(8))).unwrap());
    let task = &w.state.tasks[&id];
    assert_eq!((task.start_date, task.due), (Some(date(8)), Some(date(8))));
}

#[test]
fn workflow_quick_add_applies_only_the_destination_lists_features() {
    let mut w = World::new();
    let day = EvalContext::utc(w.now);
    let room = TaskRoom::List(w.list);
    let draft = Draft {
        text: "Prepare status:wip today..+3d".into(),
        ..Draft::default()
    };
    let submit = |w: &World| {
        intent::submit_workflow_draft(&w.state, room, &draft, &day, || Uuid::from_u128(100))
    };
    let check = |commands: &[Command]| {
        let [Command::CreateTask(task)] = commands else {
            panic!("expected a create")
        };
        (task.title.clone(), task.status, task.start_date, task.due)
    };
    assert_eq!(
        check(&submit(&w).1),
        (draft.text.clone(), WorkflowStatus::Todo, None, None)
    );
    w.enable();
    assert_eq!(
        check(&submit(&w).1),
        (
            "Prepare".into(),
            WorkflowStatus::InProgress,
            Some(date(20)),
            Some(date(23))
        )
    );
    assert_eq!(
        check(&intent::submit_draft(&w.state, room, &draft, &day, || Uuid::from_u128(101)).1).0,
        draft.text
    );
}
