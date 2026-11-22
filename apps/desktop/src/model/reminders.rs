//! When this computer shows a reminder: which Windows toasts to show or take away, and when to look
//! again, from the synced reminders (`todo_client::intent::reminder::alarms`) and what was shown
//! before.
//!
//! Pure: the toasts go through a [`ToastSink`], which the app backs with WinRT
//! (`platform::toast`) and the tests with a fake, so no test ever shows a real toast. What was shown and
//! snoozed is kept per pairing ([`Schedule`], `reminders.json`): reminder IDs and times only, never
//! task content.
//!
//! - A reminder whose time has come is shown once for that time ([`Shown`]): as its own toast if it is
//!   at most [`GRACE_MS`] late, or collapsed with the other late ones into one summary toast if it is
//!   staler (the app was closed, the computer asleep, or it synced in late). One older than
//!   [`MISSED_WINDOW_MS`] is never shown.
//! - The next time one is due is when the app looks again ([`Plan::next`]); a change to the reminders
//!   (a sync, an edit, an agent) plans again at once.
//! - A reminder that is no longer listed (removed, its task completed or deleted, here or elsewhere)
//!   loses its toast. So does the old time of a moved reminder, which starts afresh.
//! - Snooze is this computer's own ([`Reminders::snooze`]): the toast goes and the reminder shows again
//!   [`SNOOZE_MS`] later. Nothing is synced, as on Android.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use todo_client::intent::reminder::{Alarm, alarms};
use todo_core::State;
use todo_core::ids::{ReminderId, TaskId};
use todo_core::time::Timestamp;

use super::{Now, labels};

/// How late a reminder may be found and still show as itself. Later than that, it was missed: it goes
/// into the summary toast instead.
pub const GRACE_MS: i64 = 5 * 60 * 1000;
/// How far back a missed reminder is still mentioned, once.
pub const MISSED_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// What Snooze puts a reminder off by.
pub const SNOOZE_MS: i64 = 10 * 60 * 1000;

/// The toast group every reminder toast is in, and the summary's tag.
pub const GROUP: &str = "reminders";
pub const MISSED_TAG: &str = "missed";

fn ms(ts: Timestamp) -> i64 {
    ts.as_millisecond()
}

fn at_ms(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// What this computer keeps for one listed reminder: the time it was set for when this was written (one
/// moved since starts afresh) and a snooze, which is this computer's own and never synced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub task: TaskId,
    /// The reminder's time, in milliseconds since the epoch.
    pub at: i64,
    /// Snoozed until, in milliseconds since the epoch.
    pub snooze: Option<i64>,
}

impl Record {
    fn of(a: &Alarm) -> Self {
        Self {
            task: a.task,
            at: ms(a.at),
            snooze: None,
        }
    }

    /// When it shows next: the snooze, else the reminder's own time.
    pub fn fire_at(self) -> i64 {
        self.snooze.unwrap_or(self.at)
    }
}

/// One occurrence of a reminder that has been shown: the reminder and the time it showed for (its own,
/// or a snooze's). Kept for [`MISSED_WINDOW_MS`] whatever becomes of the reminder, so one removed and
/// brought back by Undo, or synced away and back, never shows the same occurrence twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Shown {
    pub id: ReminderId,
    pub at: i64,
}

/// What this computer has done: what it keeps, by reminder, and the occurrences it has shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    pub records: BTreeMap<ReminderId, Record>,
    pub shown: BTreeSet<Shown>,
}

#[derive(Serialize, Deserialize)]
struct RecordFile {
    id: ReminderId,
    task: TaskId,
    at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snooze: Option<i64>,
}

#[derive(Serialize, Deserialize, Default)]
struct ScheduleFile {
    #[serde(default)]
    records: Vec<RecordFile>,
    #[serde(default)]
    shown: Vec<Shown>,
}

impl Schedule {
    /// The schedule as written to `reminders.json`.
    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        let file = ScheduleFile {
            records: (self.records.iter())
                .map(|(&id, r)| RecordFile {
                    id,
                    task: r.task,
                    at: r.at,
                    snooze: r.snooze,
                })
                .collect(),
            shown: self.shown.iter().copied().collect(),
        };
        serde_json::to_vec(&file)
    }

    /// Reads [`to_json`](Self::to_json)'s schedule. Anything unreadable counts as none: what is due
    /// then shows (once) again.
    pub fn from_json(bytes: &[u8]) -> Self {
        let file: ScheduleFile = serde_json::from_slice(bytes).unwrap_or_default();
        Self {
            records: (file.records.into_iter())
                .map(|r| {
                    let record = Record {
                        task: r.task,
                        at: r.at,
                        snooze: r.snooze,
                    };
                    (r.id, record)
                })
                .collect(),
            shown: file.shown.into_iter().collect(),
        }
    }
}

/// What to do to bring the toasts in line with the reminders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Due now (at most [`GRACE_MS`] late): one toast each.
    pub show: Vec<Alarm>,
    /// Missed (staler, within [`MISSED_WINDOW_MS`]): one summary toast for all of them.
    pub missed: Vec<Alarm>,
    /// Toasts to take away, by reminder.
    pub hide: Vec<ReminderId>,
    /// When the next reminder is due, if any: look again then.
    pub next: Option<Timestamp>,
    /// What to keep afterwards.
    pub schedule: Schedule,
}

/// The earliest reminder time to list: the window's start, or an older record's (a snoozed reminder
/// from long ago must still be listed, or its snooze would be forgotten).
pub fn since(before: &Schedule, now: Timestamp) -> Timestamp {
    let oldest = before.records.values().map(|r| r.at).min();
    at_ms(oldest.map_or(ms(now) - MISSED_WINDOW_MS, |o| {
        o.min(ms(now) - MISSED_WINDOW_MS)
    }))
}

/// The scheduling policy (see the module's documentation), for the reminders listed now (`alarms`,
/// earliest first) and what was done `before`. While `can_show` is false (the first catch-up after
/// starting is still bringing in what changed elsewhere), nothing due is shown or counted as shown yet.
pub fn plan(alarms: &[Alarm], before: &Schedule, now: Timestamp, can_show: bool) -> Plan {
    let now = ms(now);
    let window_start = now - MISSED_WINDOW_MS;
    let mut shown: BTreeSet<Shown> = (before.shown.iter())
        .filter(|s| s.at >= window_start)
        .copied()
        .collect();
    let mut hide: BTreeSet<ReminderId> = BTreeSet::new();
    // Aged out: no longer followed, so a toast still up goes now.
    for old in before.shown.difference(&shown) {
        if !shown.iter().any(|s| s.id == old.id) {
            hide.insert(old.id);
        }
    }
    // Gone, or moved: its toast goes with it.
    for (id, old) in &before.records {
        let same = alarms
            .iter()
            .any(|a| a.reminder == *id && ms(a.at) == old.at);
        if !same && before.shown.iter().any(|s| s.id == *id) {
            hide.insert(*id);
        }
    }
    let mut records = BTreeMap::new();
    let (mut show, mut missed) = (Vec::new(), Vec::new());
    let mut next: Option<i64> = None;
    for a in alarms {
        let r = (before.records.get(&a.reminder))
            .filter(|r| r.at == ms(a.at))
            .copied()
            .unwrap_or_else(|| Record::of(a));
        let fire = r.fire_at();
        if fire > now {
            next = Some(next.map_or(fire, |n| n.min(fire)));
            records.insert(a.reminder, r);
        } else if shown.contains(&Shown {
            id: a.reminder,
            at: fire,
        }) {
            records.insert(a.reminder, r);
        } else if fire >= window_start {
            // Not shown yet while it can't be: kept as it is for a pass that can.
            if can_show {
                if now - fire <= GRACE_MS {
                    show.push(*a);
                } else {
                    missed.push(*a);
                }
                shown.insert(Shown {
                    id: a.reminder,
                    at: fire,
                });
            }
            records.insert(a.reminder, r);
        }
        // Missed by more than the window: never shown, and not kept.
    }
    // A toast shown now is never also taken away.
    for a in show.iter().chain(&missed) {
        hide.remove(&a.reminder);
    }
    Plan {
        show,
        missed,
        hide: hide.into_iter().collect(),
        next: next.map(at_ms),
        schedule: Schedule { records, shown },
    }
}

/// The schedule with reminder `id` snoozed: it shows again [`SNOOZE_MS`] from `now`. `None` when there
/// is no record of it (it has gone meanwhile).
pub fn snooze(schedule: &Schedule, id: ReminderId, now: Timestamp) -> Option<Schedule> {
    let record = schedule.records.get(&id)?;
    let mut next = schedule.clone();
    next.records.insert(
        id,
        Record {
            snooze: Some(ms(now) + SNOOZE_MS),
            ..*record
        },
    );
    Some(next)
}

/// What a toast's button (or the toast itself) asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Show the window with the reminder's task open.
    Open(ReminderId),
    /// Show it again in ten minutes (this computer only).
    Snooze(ReminderId),
    /// Complete the reminder's task.
    Complete(ReminderId),
    /// The summary of missed reminders: show the window.
    Missed,
}

impl Activation {
    /// The toast argument that asks for it.
    pub fn arguments(self) -> String {
        match self {
            Self::Open(id) => format!("open:{id}"),
            Self::Snooze(id) => format!("snooze:{id}"),
            Self::Complete(id) => format!("complete:{id}"),
            Self::Missed => MISSED_TAG.into(),
        }
    }

    /// What a toast's argument asks for; `None` for anything else (an empty body click, a stale or
    /// foreign argument).
    pub fn parse(arguments: &str) -> Option<Self> {
        if arguments == MISSED_TAG {
            return Some(Self::Missed);
        }
        let (verb, id) = arguments.split_once(':')?;
        let id: ReminderId = id.parse().ok()?;
        Some(match verb {
            "open" => Self::Open(id),
            "snooze" => Self::Snooze(id),
            "complete" => Self::Complete(id),
            _ => return None,
        })
    }
}

/// A toast to show: what it says and what its buttons ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// Replaces a toast with the same tag; [`ToastSink::hide`] takes it away.
    pub tag: String,
    pub title: String,
    pub body: String,
    /// What clicking the toast itself asks for.
    pub launch: Activation,
    /// Its buttons, in order: the label and what it asks for.
    pub actions: Vec<(&'static str, Activation)>,
}

/// Where toasts go: Windows (`platform::toast`), or a fake in tests.
pub trait ToastSink {
    /// Shows a toast, replacing one with the same tag.
    fn show(&mut self, toast: &Toast);
    /// Takes the toast with `tag` away, if it is up.
    fn hide(&mut self, tag: &str);
    /// Takes every reminder toast away (quitting, signing out).
    fn clear(&mut self);
}

/// The toast for a reminder that is due: its task's title, when and where, with Open, Snooze 10 min
/// and Complete.
pub fn reminder_toast(state: &State, alarm: &Alarm, now: &Now) -> Toast {
    let task = state.tasks.get(&alarm.task);
    let title = task.map_or_else(|| "Reminder".into(), |t| labels::one_line(&t.title));
    let list = task
        .and_then(|t| state.lists.get(&t.list))
        .map(|l| labels::one_line(&l.name));
    let when = labels::when(alarm.at, now);
    let body = match list {
        Some(list) => format!("{when} · {list}"),
        None => when,
    };
    let id = alarm.reminder;
    Toast {
        tag: id.to_string(),
        title,
        body,
        launch: Activation::Open(id),
        actions: vec![
            ("Open", Activation::Open(id)),
            ("Snooze 10 min", Activation::Snooze(id)),
            ("Complete", Activation::Complete(id)),
        ],
    }
}

/// The one toast for reminders that were missed: how many, and the first tasks' titles. A single one
/// opens its task; several show the window.
pub fn missed_toast(state: &State, missed: &[Alarm], now: &Now) -> Option<Toast> {
    let first = missed.first()?;
    let title_of = |a: &Alarm| {
        (state.tasks.get(&a.task)).map_or_else(|| "A task".into(), |t| labels::one_line(&t.title))
    };
    if missed.len() == 1 {
        return Some(Toast {
            tag: MISSED_TAG.into(),
            title: "Missed reminder".into(),
            body: format!("{} · {}", title_of(first), labels::when(first.at, now)),
            launch: Activation::Open(first.reminder),
            actions: vec![("Open", Activation::Open(first.reminder))],
        });
    }
    const NAMED: usize = 2;
    let mut body: Vec<String> = missed.iter().take(NAMED).map(title_of).collect();
    if missed.len() > NAMED {
        body.push(format!("and {} more", missed.len() - NAMED));
    }
    Some(Toast {
        tag: MISSED_TAG.into(),
        title: format!("{} missed reminders", missed.len()),
        body: body.join(", "),
        launch: Activation::Missed,
        actions: vec![("Open", Activation::Missed)],
    })
}

/// What a pass did: whether the schedule changed (and should be saved), and when to look again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub next: Option<Timestamp>,
}

/// Keeps the toasts in line with the reminders ([`plan`] decides, the sink does it) and holds what was
/// done. Log lines are counts only, at debug level: never a title.
#[allow(missing_debug_implementations)] // Holds the sink.
pub struct Reminders {
    schedule: Schedule,
    sink: Box<dyn ToastSink>,
}

impl Reminders {
    pub fn new(schedule: Schedule, sink: Box<dyn ToastSink>) -> Self {
        Self { schedule, sink }
    }

    /// What has been done, to save.
    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// Brings the toasts in line with `state`'s reminders at `now`.
    pub fn reconcile(&mut self, state: &State, now: &Now, can_show: bool) -> Outcome {
        let listed = alarms(state, since(&self.schedule, now.at));
        let plan = plan(&listed, &self.schedule, now.at, can_show);
        for id in &plan.hide {
            self.sink.hide(&id.to_string());
        }
        for a in &plan.show {
            self.sink.show(&reminder_toast(state, a, now));
        }
        if let Some(toast) = missed_toast(state, &plan.missed, now) {
            self.sink.show(&toast);
        }
        if !(plan.show.is_empty() && plan.missed.is_empty() && plan.hide.is_empty()) {
            tracing::debug!(
                shown = plan.show.len(),
                missed = plan.missed.len(),
                hidden = plan.hide.len(),
                "reminders"
            );
        }
        let changed = plan.schedule != self.schedule;
        self.schedule = plan.schedule;
        Outcome {
            changed,
            next: plan.next,
        }
    }

    /// Snoozes a reminder: its toast goes, and it shows again in [`SNOOZE_MS`]. Then plans again, for
    /// when to look next. Nothing happens to a reminder that has gone meanwhile but its toast going.
    pub fn snooze(&mut self, id: ReminderId, state: &State, now: &Now) -> Outcome {
        self.sink.hide(&id.to_string());
        if let Some(schedule) = snooze(&self.schedule, id, now.at) {
            self.schedule = schedule;
        }
        let outcome = self.reconcile(state, now, true);
        Outcome {
            changed: true,
            ..outcome
        }
    }

    /// Takes a reminder's toast away (its task was completed from it).
    pub fn dismiss(&mut self, id: ReminderId) {
        self.sink.hide(&id.to_string());
    }

    /// Takes every reminder toast away (quitting, signing out).
    pub fn clear(&mut self) {
        self.sink.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
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
        assert!(held.schedule.shown.is_empty());
        let after = plan(&[due], &held.schedule, ts("2026-11-21T10:00:30Z"), true);
        assert_eq!(after.show, [due]);
    }

    #[test]
    fn snooze_shows_it_again_ten_minutes_later_on_this_computer_only() {
        let r = alarm(1, "2026-11-21T10:00:00Z");
        let shown = plan(&[r], &Schedule::default(), ts(NOW), true);
        assert_eq!(shown.show, [r]);
        let snoozed =
            snooze(&shown.schedule, r.reminder, ts("2026-11-21T10:02:00Z")).expect("kept");
        let record = snoozed.records[&r.reminder];
        assert_eq!(record.at, ms(r.at), "the reminder itself is unchanged");
        assert_eq!(record.fire_at(), ms(ts("2026-11-21T10:12:00Z")));
        let p = plan(&[r], &snoozed, ts("2026-11-21T10:02:00Z"), true);
        assert!(p.show.is_empty() && p.hide.is_empty());
        assert_eq!(p.next, Some(ts("2026-11-21T10:12:00Z")));
        let fired = plan(&[r], &p.schedule, ts("2026-11-21T10:12:00Z"), true);
        assert_eq!(fired.show, [r]);
        // Moved elsewhere meanwhile: the snooze goes with the old time.
        let moved = Alarm {
            at: ts("2026-11-21T15:00:00Z"),
            ..r
        };
        let p = plan(&[moved], &snoozed, ts("2026-11-21T10:03:00Z"), true);
        assert_eq!(p.next, Some(moved.at));
        // A reminder that has gone can't be snoozed.
        assert_eq!(snooze(&Schedule::default(), r.reminder, ts(NOW)), None);
        // A snoozed reminder from long ago is still listed, so its snooze isn't forgotten.
        let old = Schedule {
            records: [(
                r.reminder,
                Record {
                    task: r.task,
                    at: ms(ts("2026-11-18T10:00:00Z")),
                    snooze: Some(ms(ts("2026-11-21T10:05:00Z"))),
                },
            )]
            .into(),
            shown: BTreeSet::new(),
        };
        assert_eq!(since(&old, ts(NOW)), ts("2026-11-18T10:00:00Z"));
        assert_eq!(
            since(&Schedule::default(), ts(NOW)),
            ts("2026-11-20T10:00:00Z")
        );
    }

    #[test]
    fn toast_arguments_round_trip_and_nothing_else_parses() {
        let r = id(1);
        for a in [
            Activation::Open(r),
            Activation::Snooze(r),
            Activation::Complete(r),
            Activation::Missed,
        ] {
            assert_eq!(Activation::parse(&a.arguments()), Some(a));
        }
        for junk in ["", "open", "open:", "open:nope", "delete:{r}", "missed:1"] {
            assert_eq!(Activation::parse(junk), None, "{junk}");
        }
        assert_eq!(Activation::parse(&format!("delete:{r}")), None);
    }

    #[test]
    fn an_unreadable_schedule_counts_as_none() {
        assert_eq!(Schedule::from_json(b"{not json"), Schedule::default());
        assert_eq!(Schedule::from_json(b"{}"), Schedule::default());
    }

    /// A sink that records what it was asked to do, instead of showing anything.
    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Vec<String>>>);

    impl ToastSink for Fake {
        fn show(&mut self, toast: &Toast) {
            let actions: Vec<_> = toast.actions.iter().map(|(l, _)| *l).collect();
            self.0.borrow_mut().push(format!(
                "show {} | {} | {} | {}",
                toast.tag,
                toast.title,
                toast.body,
                actions.join(", ")
            ));
        }
        fn hide(&mut self, tag: &str) {
            self.0.borrow_mut().push(format!("hide {tag}"));
        }
        fn clear(&mut self) {
            self.0.borrow_mut().push("clear".into());
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

    #[test]
    fn the_driver_shows_snoozes_and_takes_toasts_away_through_the_sink() {
        let mut w = World::new();
        let (dentist, call) = w.task("Call the dentist", "2026-11-21T10:30:00Z");
        let (_, gas) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
        let (_, plants) = w.task("Water the plants", "2026-11-21T07:00:00Z");
        let (_, bins) = w.task("Bins out", "2026-11-21T08:00:00Z");
        let fake = Fake::default();
        let log = fake.0.clone();
        let mut r = Reminders::new(Schedule::default(), Box::new(fake));
        let now = |at: &str| Now::fixed(at);
        // Starting at 10:00: the three missed ones in one summary, the dentist looked at again at 10:30.
        let out = r.reconcile(&w.state, &now(NOW), true);
        assert!(out.changed);
        assert_eq!(out.next, Some(ts("2026-11-21T10:30:00Z")));
        assert_eq!(
            log.borrow().as_slice(),
            [
                "show missed | 3 missed reminders | Read the gas meter, Water the plants, and 1 more | Open"
            ]
        );
        assert!(r.schedule().shown.iter().any(|s| s.id == gas));
        assert!(r.schedule().shown.iter().any(|s| s.id == plants));
        assert!(r.schedule().shown.iter().any(|s| s.id == bins));
        log.borrow_mut().clear();
        // Nothing new: nothing happens.
        let out = r.reconcile(&w.state, &now("2026-11-21T10:10:00Z"), true);
        assert!(!out.changed && log.borrow().is_empty());
        // At 10:30 the dentist's own toast, with its three buttons.
        r.reconcile(&w.state, &now("2026-11-21T10:30:00Z"), true);
        assert_eq!(
            log.borrow().as_slice(),
            [format!(
                "show {call} | Call the dentist | Today, 10:30 · Errands | Open, Snooze 10 min, Complete"
            )]
        );
        log.borrow_mut().clear();
        // Snoozed: the toast goes, and it comes back ten minutes later.
        let out = r.snooze(call, &w.state, &now("2026-11-21T10:31:00Z"));
        assert!(out.changed);
        assert_eq!(out.next, Some(ts("2026-11-21T10:41:00Z")));
        assert_eq!(log.borrow().as_slice(), [format!("hide {call}")]);
        log.borrow_mut().clear();
        r.reconcile(&w.state, &now("2026-11-21T10:41:00Z"), true);
        assert_eq!(log.borrow().len(), 1);
        assert!(log.borrow()[0].starts_with(&format!("show {call} | Call the dentist")));
        log.borrow_mut().clear();
        // Completed (here or elsewhere): its toast goes on the next pass.
        w.run(Command::CompleteTask { id: dentist });
        let out = r.reconcile(&w.state, &now("2026-11-21T10:42:00Z"), true);
        assert!(out.changed);
        assert_eq!(log.borrow().as_slice(), [format!("hide {call}")]);
        log.borrow_mut().clear();
        r.clear();
        assert_eq!(log.borrow().as_slice(), ["clear"]);
    }

    #[test]
    fn a_single_missed_reminder_opens_its_task() {
        let mut w = World::new();
        let (_, gas) = w.task("Read the gas meter", "2026-11-21T06:00:00Z");
        let missed = alarms(&w.state, ts("2026-11-20T10:00:00Z"));
        let toast = missed_toast(&w.state, &missed, &Now::fixed(NOW)).expect("a toast");
        assert_eq!(toast.title, "Missed reminder");
        assert_eq!(toast.body, "Read the gas meter · Today, 06:00");
        assert_eq!(toast.launch, Activation::Open(gas));
        assert_eq!(missed_toast(&w.state, &[], &Now::fixed(NOW)), None);
    }
}
