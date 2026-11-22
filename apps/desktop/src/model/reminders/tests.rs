use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::*;
use todo_core::command::{Command, NewTask};
use todo_core::ids::{ListId, uuid_v7};
use todo_core::{Actor, apply, decide};

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

fn id(n: u8) -> ReminderId {
    ReminderId::from_uuid(uuid_v7(1_790_000_000_000 + u64::from(n), [n; 10]))
}

fn task(n: u8) -> TaskId {
    TaskId::from_uuid(uuid_v7(1_790_000_100_000 + u64::from(n), [n; 10]))
}

fn alarm(n: u8, at: &str) -> Alarm {
    Alarm {
        reminder: id(n),
        task: task(n),
        at: ts(at),
    }
}

const NOW: &str = "2026-11-21T10:00:00Z";

#[test]
fn a_future_reminder_is_looked_at_again_when_it_is_due_and_shown_once_then() {
    let soon = alarm(1, "2026-11-21T10:30:00Z");
    let later = alarm(2, "2026-11-21T12:00:00Z");
    let p = plan(&[soon, later], &Schedule::default(), ts(NOW), true);
    assert!(p.show.is_empty() && p.missed.is_empty() && p.hide.is_empty());
    assert_eq!(p.next, Some(soon.at));
    // At its time, it shows; the next look is the later one's.
    let at = plan(&[soon, later], &p.schedule, soon.at, true);
    assert_eq!(at.show, [soon]);
    assert_eq!(at.next, Some(later.at));
    // Never twice: not on the next pass, nor after a restart that read the schedule back.
    let again = plan(
        &[soon, later],
        &at.schedule,
        ts("2026-11-21T10:31:00Z"),
        true,
    );
    assert!(again.show.is_empty());
    let json = at.schedule.to_json().expect("json");
    let read = Schedule::from_json(&json);
    assert_eq!(read, at.schedule);
    assert!(
        plan(&[soon, later], &read, ts("2026-11-21T10:31:00Z"), true)
            .show
            .is_empty()
    );
}

#[test]
fn a_change_to_the_reminders_reschedules_and_takes_old_toasts_away() {
    let r = alarm(1, "2026-11-21T10:30:00Z");
    let first = plan(&[r], &Schedule::default(), ts(NOW), true);
    assert_eq!(first.next, Some(r.at));
    // Moved later elsewhere: the next look moves with it.
    let moved = Alarm {
        at: ts("2026-11-21T11:00:00Z"),
        ..r
    };
    let p = plan(&[moved], &first.schedule, ts(NOW), true);
    assert_eq!(p.next, Some(moved.at));
    assert!(p.hide.is_empty(), "nothing was up");
    // Shown, then moved again: its toast goes, and it shows afresh at its new time.
    let shown = plan(&[moved], &p.schedule, moved.at, true);
    assert_eq!(shown.show, [moved]);
    let again = Alarm {
        at: ts("2026-11-21T13:00:00Z"),
        ..r
    };
    let p = plan(&[again], &shown.schedule, ts("2026-11-21T11:01:00Z"), true);
    assert_eq!(p.hide, [r.reminder]);
    assert_eq!(p.next, Some(again.at));
    // Removed, or its task completed: no longer listed, so its toast goes and nothing is next.
    let up = plan(&[again], &p.schedule, again.at, true);
    assert_eq!(up.show, [again]);
    let gone = plan(&[], &up.schedule, ts("2026-11-21T13:02:00Z"), true);
    assert_eq!(gone.hide, [r.reminder]);
    assert_eq!(gone.next, None);
    assert!(gone.schedule.records.is_empty());
    // Brought back (Undo, or synced back): the occurrence already shown isn't shown again.
    let back = plan(&[again], &gone.schedule, ts("2026-11-21T13:03:00Z"), true);
    assert!(back.show.is_empty());
}

#[test]
fn moving_a_shown_reminder_to_a_missed_time_still_takes_its_toast_away() {
    let r = alarm(1, "2026-11-21T10:00:00Z");
    let shown = plan(&[r], &Schedule::default(), ts(NOW), true);
    assert_eq!(shown.show, [r]);
    // Moved back to two hours ago: it goes into the summary, and its own toast goes.
    let stale = Alarm {
        at: ts("2026-11-21T08:00:00Z"),
        ..r
    };
    let p = plan(&[stale], &shown.schedule, ts("2026-11-21T10:01:00Z"), true);
    assert_eq!(p.missed, [stale]);
    assert_eq!(p.hide, [r.reminder]);
}

#[test]
fn stale_reminders_on_startup_collapse_into_one_summary() {
    // Closed for the morning: one due a minute ago, two missed hours ago, one from two days ago.
    let fresh = alarm(1, "2026-11-21T09:59:00Z");
    let missed_a = alarm(2, "2026-11-21T07:00:00Z");
    let missed_b = alarm(3, "2026-11-21T08:30:00Z");
    let ancient = alarm(4, "2026-11-19T08:00:00Z");
    let future = alarm(5, "2026-11-21T18:00:00Z");
    let all = [ancient, missed_a, missed_b, fresh, future];
    let p = plan(&all, &Schedule::default(), ts(NOW), true);
    assert_eq!(p.show, [fresh], "within the grace period: its own toast");
    assert_eq!(p.missed, [missed_a, missed_b], "staler: one summary");
    assert_eq!(p.next, Some(future.at));
    assert!(!p.schedule.records.contains_key(&ancient.reminder));
    // Exactly at the grace period's end it still counts as due; a moment later it is missed.
    let edge = alarm(6, "2026-11-21T09:55:00Z");
    assert_eq!(
        plan(&[edge], &Schedule::default(), ts(NOW), true).show,
        [edge]
    );
    let late = alarm(7, "2026-11-21T09:54:59Z");
    assert_eq!(
        plan(&[late], &Schedule::default(), ts(NOW), true).missed,
        [late]
    );
    // And the summary isn't repeated on the next start.
    let restart = Schedule::from_json(&p.schedule.to_json().expect("json"));
    let again = plan(&all, &restart, ts("2026-11-21T10:05:00Z"), true);
    assert!(again.show.is_empty() && again.missed.is_empty());
}

#[test]
fn nothing_due_shows_while_catching_up_and_it_shows_once_it_can() {
    let due = alarm(1, "2026-11-21T09:59:00Z");
    let held = plan(&[due], &Schedule::default(), ts(NOW), false);
    assert!(held.show.is_empty() && held.missed.is_empty());
    assert!(held.held, "the caller looks again when the hold ends");
    assert!(held.schedule.shown.is_empty());
    let after = plan(&[due], &held.schedule, ts("2026-11-21T10:00:30Z"), true);
    assert_eq!(after.show, [due]);
    assert!(!after.held);
    let nothing_due = plan(
        &[alarm(2, "2026-11-21T12:00:00Z")],
        &Schedule::default(),
        ts(NOW),
        false,
    );
    assert!(!nothing_due.held);
}

#[test]
fn toast_arguments_round_trip_and_nothing_else_parses() {
    let o = Occurrence {
        id: id(1),
        at: 1_790_000_000_000,
        token: 7,
    };
    for a in [
        Activation::Open(o),
        Activation::Snooze(o),
        Activation::Complete(o),
        Activation::Missed(4),
    ] {
        assert_eq!(Activation::parse(&a.arguments()), Some(a));
    }
    let r = id(1);
    for junk in [
        String::new(),
        "open".into(),
        format!("open:{r}"),
        format!("open:{r}:1"),
        format!("open:{r}:x:1"),
        format!("open:{r}:1:2:3"),
        format!("delete:{r}:1:2"),
        "open:nope:1:2".into(),
        "missed".into(),
        "missed:x".into(),
        "missed:1:2".into(),
    ] {
        assert_eq!(Activation::parse(&junk), None, "{junk}");
    }
}

#[test]
fn an_unreadable_schedule_counts_as_none() {
    assert_eq!(Schedule::from_json(b"{not json"), Schedule::default());
    assert_eq!(Schedule::from_json(b"{}"), Schedule::default());
}

/// A sink that records what it was asked to do, instead of showing anything, and can refuse to show.
#[derive(Clone, Default)]
struct Fake {
    log: Rc<RefCell<Vec<String>>>,
    /// The toasts shown, last first per tag: what their buttons ask for.
    toasts: Rc<RefCell<Vec<Toast>>>,
    fail: Rc<Cell<bool>>,
    fail_tag: Rc<RefCell<Option<String>>>,
}

impl ToastSink for Fake {
    fn show(&mut self, toast: &Toast) -> Result<(), NotShown> {
        if self.fail.get() || self.fail_tag.borrow().as_deref() == Some(&toast.tag) {
            self.log.borrow_mut().push(format!("failed {}", toast.tag));
            return Err(NotShown("no notifier".into()));
        }
        let actions: Vec<_> = toast.actions.iter().map(|(l, _)| *l).collect();
        self.log.borrow_mut().push(format!(
            "{} {} | {} | {} | {}",
            if toast.quiet { "update" } else { "show" },
            toast.tag,
            toast.title,
            toast.body,
            actions.join(", ")
        ));
        self.toasts.borrow_mut().push(toast.clone());
        Ok(())
    }
    fn hide(&mut self, tag: &str) {
        self.log.borrow_mut().push(format!("hide {tag}"));
    }
    fn clear(&mut self) {
        self.log.borrow_mut().push("clear".into());
    }
}

impl Fake {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }

    /// The button `label` of the last toast with `tag`.
    fn button(&self, tag: &str, label: &str) -> Activation {
        let toasts = self.toasts.borrow();
        let toast = toasts.iter().rev().find(|t| t.tag == tag).expect("shown");
        toast
            .actions
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, a)| *a)
            .expect("a button")
    }
}

struct World {
    state: State,
    next: u64,
    list: ListId,
}

impl World {
    fn new() -> Self {
        let mut w = Self {
            state: State::new(),
            next: 1,
            list: ListId::from_uuid(uuid_v7(1, [0; 10])),
        };
        let list = w.list;
        w.run(todo_client::intent::create_list(list, None, "Errands"));
        w
    }

    fn run(&mut self, cmd: Command) {
        let actor = Actor::owner("test").expect("actor");
        let events = decide(&self.state, &actor, &cmd, ts(NOW)).expect("valid");
        for e in &events {
            apply(&mut self.state, e).expect("applies");
        }
    }

    fn uuid(&mut self) -> uuid::Uuid {
        self.next += 1;
        uuid_v7(1_790_000_000_000 + self.next, [self.next as u8; 10])
    }

    fn task(&mut self, title: &str, at: &str) -> (TaskId, ReminderId) {
        let task = TaskId::from_uuid(self.uuid());
        let list = self.list;
        self.run(Command::CreateTask(Box::new(NewTask::new(
            task, list, title,
        ))));
        let reminder = ReminderId::from_uuid(self.uuid());
        self.run(Command::AddReminder {
            id: reminder,
            task,
            at: ts(at),
        });
        (task, reminder)
    }
}

fn now(at: &str) -> Now {
    Now::fixed(at)
}

fn driver() -> (Reminders, Fake) {
    let fake = Fake::default();
    (
        Reminders::new(Schedule::default(), Box::new(fake.clone())),
        fake,
    )
}

#[test]
fn the_driver_shows_snoozes_and_takes_toasts_away_through_the_sink() {
    let mut w = World::new();
    let (dentist, call) = w.task("Call the dentist", "2026-11-21T10:30:00Z");
    let (_, gas) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
    let (_, plants) = w.task("Water the plants", "2026-11-21T07:00:00Z");
    let (_, bins) = w.task("Bins out", "2026-11-21T08:00:00Z");
    let (mut r, fake) = driver();
    // Starting at 10:00: the three missed ones in one summary, the dentist looked at again at 10:30.
    let out = r.reconcile(&w.state, &now(NOW), true);
    assert!(out.changed);
    assert_eq!(out.next, Some(ts("2026-11-21T10:30:00Z")));
    assert_eq!(
        fake.take(),
        [
            "show missed | 3 missed reminders | Read the gas meter, Water the plants, and 1 more | Open"
        ]
    );
    for id in [gas, plants, bins] {
        assert!(r.schedule().shown.iter().any(|s| s.id == id));
    }
    // Nothing new: nothing happens.
    let out = r.reconcile(&w.state, &now("2026-11-21T10:10:00Z"), true);
    assert!(!out.changed && fake.take().is_empty());
    // At 10:30 the dentist's own toast, with its three buttons.
    r.reconcile(&w.state, &now("2026-11-21T10:30:00Z"), true);
    assert_eq!(
        fake.take(),
        [format!(
            "show {call} | Call the dentist | Today, 10:30 · Errands | Open, Snooze 10 min, Complete"
        )]
    );
    // Snoozed: the toast goes, and it comes back ten minutes later.
    let snooze = fake.button(&call.to_string(), "Snooze 10 min");
    let Some(Claim::Snoozed(out)) = r.claim(snooze, &w.state, &now("2026-11-21T10:31:00Z")) else {
        panic!("snoozed");
    };
    assert!(out.changed);
    assert_eq!(out.next, Some(ts("2026-11-21T10:41:00Z")));
    assert_eq!(fake.take(), [format!("hide {call}")]);
    // Only once: the same button again does nothing.
    assert_eq!(
        r.claim(snooze, &w.state, &now("2026-11-21T10:32:00Z")),
        None
    );
    r.reconcile(&w.state, &now("2026-11-21T10:41:00Z"), true);
    let log = fake.take();
    assert_eq!(log.len(), 1);
    assert!(log[0].starts_with(&format!("show {call} | Call the dentist")));
    // The first toast's buttons don't act on the second.
    assert_eq!(
        r.claim(snooze, &w.state, &now("2026-11-21T10:42:00Z")),
        None
    );
    // Completed (here or elsewhere): its toast goes on the next pass.
    w.run(Command::CompleteTask { id: dentist });
    let out = r.reconcile(&w.state, &now("2026-11-21T10:42:00Z"), true);
    assert!(out.changed);
    assert_eq!(fake.take(), [format!("hide {call}")]);
    r.clear();
    assert_eq!(fake.take(), ["clear"]);
}

#[test]
fn a_toasts_buttons_act_only_on_the_reminder_as_it_was_shown_and_only_once() {
    let mut w = World::new();
    let (task, call) = w.task("Call the dentist", "2026-11-21T10:00:00Z");
    let (mut r, fake) = driver();
    let at = now("2026-11-21T10:00:30Z");
    r.reconcile(&w.state, &at, true);
    let tag = call.to_string();
    let complete = fake.button(&tag, "Complete");
    let snooze = fake.button(&tag, "Snooze 10 min");
    let open = fake.button(&tag, "Open");
    assert_eq!(r.claim(open, &w.state, &at), Some(Claim::Open(task)));

    // Rescheduled to tomorrow before a queued Snooze runs: it doesn't touch tomorrow's reminder.
    let mut moved = World::new();
    moved.state = w.state.clone();
    moved.run(Command::UpdateReminder {
        id: call,
        at: ts("2026-11-22T09:00:00Z"),
    });
    let before = r.schedule().clone();
    assert_eq!(r.claim(snooze, &moved.state, &at), None);
    assert_eq!(r.schedule(), &before, "nothing was snoozed");
    assert_eq!(r.claim(open, &moved.state, &at), None);
    // The pass that sees the move takes the toast away; its buttons are dead after that.
    r.reconcile(&moved.state, &at, true);
    assert_eq!(r.claim(complete, &w.state, &at), None);

    // A fresh toast: a queued Complete after the reminder was removed does nothing.
    let mut w = World::new();
    let (task, call) = w.task("Water the plants", "2026-11-21T10:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &at, true);
    let complete = fake.button(&call.to_string(), "Complete");
    let mut removed = World::new();
    removed.state = w.state.clone();
    removed.run(Command::Delete { id: call.into() });
    assert_eq!(r.claim(complete, &removed.state, &at), None);
    // Nor does one after the task was completed elsewhere (it would never reopen it).
    let mut done = World::new();
    done.state = w.state.clone();
    done.run(Command::CompleteTask { id: task });
    assert_eq!(r.claim(complete, &done.state, &at), None);
    // As shown: Complete is claimed once; the toast goes once the completion is queued.
    fake.take();
    let Some(Claim::Complete { alarm, occurrence }) = r.claim(complete, &w.state, &at) else {
        panic!("claimed");
    };
    assert_eq!(alarm.task, task);
    assert!(
        fake.take().is_empty(),
        "nothing happens in Windows between the claim and the queue"
    );
    // A reschedule or a delete that arrives after the claim, before the completion is queued: the
    // check made when it is queued refuses it.
    let mut later = World::new();
    later.state = w.state.clone();
    later.run(Command::UpdateReminder {
        id: call,
        at: ts("2026-11-22T09:00:00Z"),
    });
    assert!(still_as_shown(&w.state, alarm));
    assert!(!still_as_shown(&later.state, alarm));
    assert!(!still_as_shown(&removed.state, alarm));
    assert!(!still_as_shown(&done.state, alarm));
    assert_eq!(r.claim(complete, &w.state, &at), None, "claimed once");
    assert!(!r.finish_complete(occurrence, false, &at));
    assert_eq!(fake.take(), [format!("hide {call}")]);
    assert_eq!(r.claim(complete, &w.state, &at), None);
    // A made-up token never acts.
    let forged = match complete {
        Activation::Complete(o) => Activation::Complete(Occurrence {
            token: o.token + 100,
            ..o
        }),
        other => other,
    };
    assert_eq!(r.claim(forged, &w.state, &at), None);
}

#[test]
fn a_completion_save_error_makes_the_toast_pending_instead_of_dismissing_it() {
    let mut w = World::new();
    let (_, id) = w.task("Read the gas meter", NOW);
    let (mut r, fake) = driver();
    let at = now(NOW);
    r.reconcile(&w.state, &at, true);
    let complete = fake.button(&id.to_string(), "Complete");
    fake.take();
    let Some(Claim::Complete { occurrence, .. }) = r.claim(complete, &w.state, &at) else {
        panic!("claimed");
    };
    assert!(r.finish_complete(occurrence, true, &at));
    assert!(
        fake.take().is_empty(),
        "the failed submit must not dismiss the toast"
    );
    let persisted = Schedule::from_json(&r.schedule().to_json().expect("schedule"));
    assert!(persisted.shown.is_empty());
    let retry = r.reconcile(&w.state, &at, true).next.expect("to retry");
    assert_eq!(retry, ts("2026-11-21T10:00:30Z"));
    r.reconcile(&w.state, &now("2026-11-21T10:00:30Z"), true);
    assert!(fake.take()[0].starts_with(&format!("show {id}")));
    assert_ne!(fake.button(&id.to_string(), "Complete"), complete);
}

#[test]
fn a_missing_record_refuses_the_click_without_consuming_the_live_token() {
    let mut w = World::new();
    let (_, id) = w.task("Read the gas meter", NOW);
    let (mut r, fake) = driver();
    let at = now(NOW);
    r.reconcile(&w.state, &at, true);
    let snooze = fake.button(&id.to_string(), "Snooze 10 min");
    let complete = fake.button(&id.to_string(), "Complete");
    fake.take();
    let record = r.schedule.records.remove(&id).expect("record");
    for action in [snooze, complete] {
        assert_eq!(r.claim(action, &w.state, &at), None);
    }
    assert!(fake.take().is_empty());
    r.schedule.records.insert(id, record);
    assert!(matches!(
        r.claim(snooze, &w.state, &at),
        Some(Claim::Snoozed(_))
    ));
}

#[test]
fn a_toast_token_cannot_follow_a_reparent_or_a_submillisecond_edit() {
    let mut w = World::new();
    let (source, id) = w.task("Source", "2026-11-21T10:00:00.000000001Z");
    let (target, _) = w.task("Target", "2026-11-21T11:00:00Z");
    let original = w.state.clone();
    let at = now("2026-11-21T10:00:01Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &at, true);
    let open = fake.button(&id.to_string(), "Open");
    let snooze = fake.button(&id.to_string(), "Snooze 10 min");
    let complete = fake.button(&id.to_string(), "Complete");
    assert_eq!(r.claim(open, &w.state, &at), Some(Claim::Open(source)));
    w.run(Command::UpdateReminder {
        id,
        at: ts("2026-11-21T10:00:00.000000002Z"),
    });
    assert_eq!(
        r.claim(complete, &w.state, &at),
        None,
        "same millisecond is not the same instant"
    );
    w.state = original.clone();
    w.run(Command::MergeTasks { source, target });
    assert_eq!(w.state.reminders[&id].task, target);
    assert_eq!(w.state.reminders[&id].at, original.reminders[&id].at);
    for action in [open, snooze, complete] {
        assert_eq!(r.claim(action, &w.state, &at), None);
    }
    fake.take();
    r.reconcile(&w.state, &at, true);
    assert!(fake.take().contains(&format!("hide {id}")));
    assert_eq!(
        r.claim(complete, &original, &at),
        None,
        "reconciliation invalidated the token"
    );
}

#[test]
fn a_partial_resync_leaves_the_durable_snooze_alone_until_replay_finishes() {
    let mut w = World::new();
    let (_, id) = w.task("Read the gas meter", NOW);
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    let snooze = fake.button(&id.to_string(), "Snooze 10 min");
    let at = now("2026-11-21T10:01:00Z");
    assert!(matches!(
        r.claim(snooze, &w.state, &at),
        Some(Claim::Snoozed(_))
    ));
    let durable = r.schedule().to_json().expect("json");
    fake.take();
    let due = now("2026-11-21T10:11:00Z");
    for state in [&State::new(), &w.state] {
        assert_eq!(r.reconcile(state, &due, false), Outcome::default());
        assert_eq!(r.schedule().to_json().expect("json"), durable);
        assert!(fake.take().is_empty(), "no toast from a partial view");
    }
    assert!(r.reconcile(&w.state, &due, true).changed);
    assert!(fake.take()[0].starts_with(&format!("show {id}")));
}

#[test]
fn a_toast_windows_couldnt_show_is_tried_again_after_a_back_off() {
    let mut w = World::new();
    let (_, call) = w.task("Call the dentist", "2026-11-21T10:00:00Z");
    let (mut r, fake) = driver();
    fake.fail.set(true);
    let out = r.reconcile(&w.state, &now("2026-11-21T10:00:00Z"), true);
    assert!(r.schedule().shown.is_empty(), "not counted as shown");
    assert_eq!(
        out.next,
        Some(ts("2026-11-21T10:00:30Z")),
        "the first retry"
    );
    assert_eq!(fake.take(), [format!("failed {call}")]);
    // Not before the back-off, and longer after a second failure.
    let out = r.reconcile(&w.state, &now("2026-11-21T10:00:10Z"), true);
    assert!(fake.take().is_empty());
    assert_eq!(out.next, Some(ts("2026-11-21T10:00:30Z")));
    let out = r.reconcile(&w.state, &now("2026-11-21T10:00:30Z"), true);
    assert_eq!(fake.take(), [format!("failed {call}")]);
    assert_eq!(out.next, Some(ts("2026-11-21T10:02:30Z")));
    // Windows shows it again: it is shown, once.
    fake.fail.set(false);
    let out = r.reconcile(&w.state, &now("2026-11-21T10:02:30Z"), true);
    assert!(out.changed);
    assert_eq!(fake.take().len(), 1);
    assert!(r.schedule().shown.iter().any(|s| s.id == call));
    r.reconcile(&w.state, &now("2026-11-21T10:03:00Z"), true);
    assert!(fake.take().is_empty());
}

#[test]
fn the_summary_follows_what_it_covers_and_goes_when_nothing_is_left() {
    let mut w = World::new();
    let (gas_task, _) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
    let (plants_task, plants) = w.task("Water the plants", "2026-11-21T07:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    assert_eq!(
        fake.take(),
        ["show missed | 2 missed reminders | Read the gas meter, Water the plants | Open"]
    );
    // One is completed: the summary is rebuilt, quietly, about the other (which it then opens).
    w.run(Command::CompleteTask { id: gas_task });
    r.reconcile(&w.state, &now("2026-11-21T10:01:00Z"), true);
    let log = fake.take();
    assert!(
        log.contains(
            &"update missed | Missed reminder | Water the plants · Today, 07:00 | Open".to_string()
        ),
        "{log:?}"
    );
    let open = fake.button(MISSED_TAG, "Open");
    assert_eq!(
        r.claim(open, &w.state, &now("2026-11-21T10:01:00Z")),
        Some(Claim::Open(plants_task))
    );
    // The other is removed: the summary goes.
    w.run(Command::Delete { id: plants.into() });
    r.reconcile(&w.state, &now("2026-11-21T10:02:00Z"), true);
    assert!(fake.take().contains(&format!("hide {MISSED_TAG}")));
}

#[test]
fn a_single_missed_reminder_opens_its_task() {
    let mut w = World::new();
    let (_, gas) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
    let missed = alarms(&w.state, ts("2026-11-20T10:00:00Z"));
    let toast = missed_toast(&w.state, &missed, 1, &Now::fixed(NOW)).expect("a toast");
    assert_eq!(toast.title, "Missed reminder");
    assert_eq!(toast.body, "Read the gas meter · Today, 06:00");
    assert!(matches!(toast.launch, Activation::Open(o) if o.id == gas));
    assert_eq!(missed_toast(&w.state, &[], 1, &Now::fixed(NOW)), None);
}

#[test]
fn open_acts_only_for_the_toast_up_and_selects_nothing_otherwise() {
    let mut w = World::new();
    let (task, call) = w.task("Call the dentist", "2026-11-21T10:00:00Z");
    let (mut r, fake) = driver();
    let at = now("2026-11-21T10:00:30Z");
    r.reconcile(&w.state, &at, true);
    let open = fake.button(&call.to_string(), "Open");
    assert_eq!(r.claim(open, &w.state, &at), Some(Claim::Open(task)));
    // Snoozed and shown again: the first toast's Open is stale, even for the same reminder and time.
    let snooze = fake.button(&call.to_string(), "Snooze 10 min");
    assert!(matches!(
        r.claim(snooze, &w.state, &at),
        Some(Claim::Snoozed(_))
    ));
    assert_eq!(r.claim(open, &w.state, &at), None);
    r.reconcile(&w.state, &now("2026-11-21T10:10:30Z"), true);
    let again = fake.button(&call.to_string(), "Open");
    assert_ne!(again, open);
    assert_eq!(r.claim(again, &w.state, &at), Some(Claim::Open(task)));
    assert_eq!(r.claim(open, &w.state, &at), None);
}

#[test]
fn an_individual_success_does_not_reset_a_refused_summarys_back_off() {
    let mut w = World::new();
    let (gas, _) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
    w.task("Clean the windows", "2026-11-21T07:00:00Z");
    let (_, fresh) = w.task("Call the dentist", NOW);
    let (_, next) = w.task("Water the plants", "2026-11-21T10:00:30Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now("2026-11-21T09:59:00Z"), true);
    fake.take();
    w.run(Command::CompleteTask { id: gas });
    *fake.fail_tag.borrow_mut() = Some(MISSED_TAG.into());
    let out = r.reconcile(&w.state, &now(NOW), true);
    let log = fake.take();
    assert!(log.iter().any(|l| l.starts_with(&format!("show {fresh}"))));
    assert_eq!(log.last().expect("quiet rebuild refused"), "failed missed");
    assert_eq!(out.next, Some(ts("2026-11-21T10:00:30Z")));
    r.reconcile(&w.state, &now("2026-11-21T10:00:10Z"), true);
    assert!(fake.take().is_empty(), "the refused rebuild must back off");
    let out = r.reconcile(&w.state, &now("2026-11-21T10:00:30Z"), true);
    let log = fake.take();
    assert!(log[0].starts_with(&format!("show {next}")));
    assert_eq!(log[1], "failed missed");
    assert_eq!(
        out.next,
        Some(ts("2026-11-21T10:02:30Z")),
        "second refusal, not first"
    );
    r.reconcile(&w.state, &now("2026-11-21T10:01:00Z"), true);
    assert!(fake.take().is_empty());
}

#[test]
fn a_toast_windows_reports_failed_later_is_shown_again() {
    let mut w = World::new();
    let (_, call) = w.task("Call the dentist", "2026-11-21T10:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    let launch = fake.button(&call.to_string(), "Open");
    fake.take();
    // Windows says it failed after all: not counted as shown, tried again after the back-off.
    let at = now("2026-11-21T10:00:01Z");
    assert!(r.failed(launch, &at));
    let out = r.reconcile(&w.state, &at, true);
    assert!(r.schedule().shown.is_empty());
    assert_eq!(out.next, Some(ts("2026-11-21T10:00:31Z")));
    assert!(fake.take().is_empty());
    r.reconcile(&w.state, &now("2026-11-21T10:00:31Z"), true);
    assert_eq!(fake.take().len(), 1);
    assert!(r.schedule().shown.iter().any(|s| s.id == call));
    // A report about a toast already replaced does nothing.
    let later = now("2026-11-21T10:00:40Z");
    assert!(!r.failed(launch, &later));
    let retry = fake.button(&call.to_string(), "Open");
    assert!(r.failed(retry, &later));
    let out = r.reconcile(&w.state, &later, true);
    assert_eq!(
        out.next,
        Some(ts("2026-11-21T10:02:40Z")),
        "a second asynchronous failure backs off longer"
    );
    let at = now("2026-11-21T10:02:40Z");
    r.reconcile(&w.state, &at, true);
    assert!(r.failed(fake.button(&call.to_string(), "Open"), &at));
    let out = r.reconcile(&w.state, &at, true);
    assert_eq!(out.next, Some(ts("2026-11-21T10:12:40Z")));
    r.reconcile(&w.state, &now("2026-11-21T10:12:40Z"), true);
    assert!(
        fake.take()
            .last()
            .expect("retried")
            .starts_with(&format!("show {call}"))
    );
}

#[test]
fn a_failed_first_summary_is_pending_after_restart_and_pops_up_again() {
    let mut w = World::new();
    let (_, first) = w.task("First", "2026-11-21T06:00:00Z");
    let (_, second) = w.task("Second", "2026-11-21T07:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    assert_eq!(r.schedule().shown.len(), 2);
    let at = now("2026-11-21T10:00:01Z");
    assert!(r.failed(fake.button(MISSED_TAG, "Open"), &at));
    let out = r.reconcile(&w.state, &at, true);
    assert_eq!(out.next, Some(ts("2026-11-21T10:00:31Z")));
    let pending = Schedule::from_json(&r.schedule().to_json().expect("json"));
    assert!(pending.shown.is_empty());
    assert!(pending.records.contains_key(&first) && pending.records.contains_key(&second));
    fake.take();
    let mut restarted = Reminders::new(pending, Box::new(fake.clone()));
    restarted.reconcile(&w.state, &at, true);
    assert_eq!(
        fake.take(),
        ["show missed | 2 missed reminders | First, Second | Open"]
    );
}

#[test]
fn a_failed_summary_update_only_rolls_back_its_new_members() {
    let mut w = World::new();
    let (_, earlier) = w.task("Earlier", "2026-11-21T06:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    let (_, new) = w.task("New", "2026-11-21T07:00:00Z");
    let at = now("2026-11-21T10:00:01Z");
    r.reconcile(&w.state, &at, true);
    assert!(r.failed(fake.button(MISSED_TAG, "Open"), &at));
    assert!(r.schedule().shown.iter().any(|s| s.id == earlier));
    assert!(!r.schedule().shown.iter().any(|s| s.id == new));
    fake.take();
    let at = now("2026-11-21T10:00:31Z");
    r.reconcile(&w.state, &at, true);
    assert_eq!(
        fake.take(),
        ["show missed | 2 missed reminders | Earlier, New | Open"]
    );
    assert!(r.failed(fake.button(MISSED_TAG, "Open"), &at));
    let out = r.reconcile(&w.state, &at, true);
    assert_eq!(
        out.next,
        Some(ts("2026-11-21T10:02:31Z")),
        "summary's second asynchronous failure"
    );
}

#[test]
fn a_summary_rebuild_windows_refuses_is_withdrawn_and_tried_again() {
    let mut w = World::new();
    let (gas_task, gas) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
    w.task("Water the plants", "2026-11-21T07:00:00Z");
    w.task("Bins out", "2026-11-21T08:00:00Z");
    let (mut r, fake) = driver();
    r.reconcile(&w.state, &now(NOW), true);
    fake.take();
    // One is completed, and Windows refuses the replacement: the old summary still goes.
    w.run(Command::CompleteTask { id: gas_task });
    fake.fail.set(true);
    let out = r.reconcile(&w.state, &now("2026-11-21T10:01:00Z"), true);
    assert_eq!(
        fake.take(),
        [
            format!("hide {gas}"),
            "hide missed".into(),
            "failed missed".into()
        ]
    );
    assert_eq!(out.next, Some(ts("2026-11-21T10:01:30Z")), "tried again");
    // Not before the back-off; then quietly, about the two left.
    r.reconcile(&w.state, &now("2026-11-21T10:01:10Z"), true);
    assert!(fake.take().is_empty());
    fake.fail.set(false);
    let out = r.reconcile(&w.state, &now("2026-11-21T10:01:30Z"), true);
    assert_eq!(
        fake.take(),
        ["update missed | 2 missed reminders | Water the plants, Bins out | Open"]
    );
    assert_eq!(out.next, None, "nothing left to try");
    r.reconcile(&w.state, &now("2026-11-21T10:02:00Z"), true);
    assert!(fake.take().is_empty());
}
