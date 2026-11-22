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
//!   [`MISSED_WINDOW_MS`] is never shown. A toast Windows couldn't show isn't counted as shown: it is
//!   tried again after a back-off ([`RETRY_MS`]).
//! - The next time one is due is when the app looks again ([`Outcome::next`]); a change to the
//!   reminders (a sync, an edit, an agent) plans again at once.
//! - A reminder that is no longer listed (removed, its task completed or deleted, here or elsewhere)
//!   loses its toast, and the summary is rebuilt without it (or withdrawn). So does the old time of a
//!   moved reminder, which starts afresh.
//! - A toast's buttons name the occurrence they were shown for ([`Occurrence`]: the reminder, its time
//!   and a token of this session). A button acts only while that toast is up, only on the reminder as
//!   it was shown, and Snooze and Complete only once.
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
/// How long to wait before trying again after Windows couldn't show a toast: longer after each failure
/// in a row, then the last.
pub const RETRY_MS: [i64; 3] = [30 * 1000, 2 * 60 * 1000, 10 * 60 * 1000];

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
    /// Something due was held back (`can_show` was false).
    pub held: bool,
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
/// starting is still bringing in what changed elsewhere, or Windows couldn't show the last toast),
/// nothing due is shown or counted as shown yet.
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
    // Gone, or moved: its toast goes with it (also when its new time was missed, as the summary never
    // replaces the toast tagged with the reminder). Taking away a toast that isn't up does nothing.
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
    let mut held = false;
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
            } else {
                held = true;
            }
            records.insert(a.reminder, r);
        }
        // Missed by more than the window: never shown, and not kept.
    }
    Plan {
        show,
        missed,
        hide: hide.into_iter().collect(),
        next: next.map(at_ms),
        held,
        schedule: Schedule { records, shown },
    }
}

/// One shown occurrence of a reminder, as its toast's buttons name it: the reminder, its time when the
/// toast was shown, and a token of this session, new for every toast shown. A button acts only on this
/// occurrence (see [`Reminders::claim`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub id: ReminderId,
    /// The reminder's time, in milliseconds since the epoch.
    pub at: i64,
    pub token: u64,
}

/// What a toast's button (or the toast itself) asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Show the window with the reminder's task open.
    Open(Occurrence),
    /// Show it again in ten minutes (this computer only).
    Snooze(Occurrence),
    /// Complete the reminder's task.
    Complete(Occurrence),
    /// The summary of several missed reminders (its token): show the window.
    Missed(u64),
}

impl Activation {
    /// The toast argument that asks for it.
    pub fn arguments(self) -> String {
        let named = |verb: &str, o: Occurrence| format!("{verb}:{}:{}:{}", o.id, o.at, o.token);
        match self {
            Self::Open(o) => named("open", o),
            Self::Snooze(o) => named("snooze", o),
            Self::Complete(o) => named("complete", o),
            Self::Missed(token) => format!("{MISSED_TAG}:{token}"),
        }
    }

    /// What a toast's argument asks for; `None` for anything else (an empty body click, a stale or
    /// foreign argument).
    pub fn parse(arguments: &str) -> Option<Self> {
        let mut parts = arguments.split(':');
        let verb = parts.next()?;
        if verb == MISSED_TAG {
            let token = parts.next()?.parse().ok()?;
            return parts.next().is_none().then_some(Self::Missed(token));
        }
        let occurrence = Occurrence {
            id: parts.next()?.parse().ok()?,
            at: parts.next()?.parse().ok()?,
            token: parts.next()?.parse().ok()?,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(match verb {
            "open" => Self::Open(occurrence),
            "snooze" => Self::Snooze(occurrence),
            "complete" => Self::Complete(occurrence),
            _ => return None,
        })
    }

    /// The token of the toast it came from.
    pub fn token(self) -> u64 {
        match self {
            Self::Open(o) | Self::Snooze(o) | Self::Complete(o) => o.token,
            Self::Missed(token) => token,
        }
    }
}

/// A toast to show: what it says and what its buttons ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// Replaces a toast with the same tag; [`ToastSink::hide`] takes it away.
    pub tag: String,
    pub title: String,
    pub body: String,
    /// What clicking the toast itself asks for. Its token also names the toast when Windows reports
    /// that it failed after all ([`Reminders::failed`]).
    pub launch: Activation,
    /// Its buttons, in order: the label and what it asks for.
    pub actions: Vec<(&'static str, Activation)>,
    /// An update of a toast already shown (the summary, rebuilt): straight to the action center,
    /// without popping up or sounding again.
    pub quiet: bool,
}

/// Why a toast wasn't shown.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the toast couldn't be shown: {0}")]
pub struct NotShown(pub String);

/// Where toasts go: Windows (`platform::toast`), or a fake in tests.
pub trait ToastSink {
    /// Shows a toast, replacing one with the same tag. An error means it isn't up.
    fn show(&mut self, toast: &Toast) -> Result<(), NotShown>;
    /// Takes the toast with `tag` away, if it is up.
    fn hide(&mut self, tag: &str);
    /// Takes every reminder toast away (quitting, signing out).
    fn clear(&mut self);
}

/// The toast for a reminder that is due: its task's title, when and where, with Open, Snooze 10 min
/// and Complete, all naming `occurrence`.
pub fn reminder_toast(state: &State, alarm: &Alarm, occurrence: Occurrence, now: &Now) -> Toast {
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
    Toast {
        tag: alarm.reminder.to_string(),
        title,
        body,
        launch: Activation::Open(occurrence),
        actions: vec![
            ("Open", Activation::Open(occurrence)),
            ("Snooze 10 min", Activation::Snooze(occurrence)),
            ("Complete", Activation::Complete(occurrence)),
        ],
        quiet: false,
    }
}

/// The one toast for reminders that were missed: how many, and the first tasks' titles. A single one
/// opens its task; several show the window. Only Open: nothing in it changes anything.
pub fn missed_toast(state: &State, missed: &[Alarm], token: u64, now: &Now) -> Option<Toast> {
    let first = missed.first()?;
    let title_of = |a: &Alarm| {
        (state.tasks.get(&a.task)).map_or_else(|| "A task".into(), |t| labels::one_line(&t.title))
    };
    if missed.len() == 1 {
        let open = Activation::Open(Occurrence {
            id: first.reminder,
            at: ms(first.at),
            token,
        });
        return Some(Toast {
            tag: MISSED_TAG.into(),
            title: "Missed reminder".into(),
            body: format!("{} · {}", title_of(first), labels::when(first.at, now)),
            launch: open,
            actions: vec![("Open", open)],
            quiet: false,
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
        launch: Activation::Missed(token),
        actions: vec![("Open", Activation::Missed(token))],
        quiet: false,
    })
}

/// Whether the reminder of `occurrence` is still as its toast showed it in `state`: live, at the same
/// time, on `task`, which is open. What a toast's Complete checks again at the moment it queues the
/// completion (`SyncClient::submit_tracked_if`), so a sync that moved, removed or completed it
/// meanwhile wins.
pub fn still_as_shown(state: &State, occurrence: Occurrence, task: TaskId) -> bool {
    let Some(r) = state.reminders.get(&occurrence.id) else {
        return false;
    };
    let open = state
        .tasks
        .get(&task)
        .is_some_and(|t| !t.is_done() && t.merged_into.is_none());
    r.task == task
        && ms(r.at) == occurrence.at
        && open
        && state.is_live(occurrence.id.into())
        && state.is_live(task.into())
}

/// What a pass did: whether the schedule changed (and should be saved), when to look again, and
/// whether something due was held back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub next: Option<Timestamp>,
    pub held: bool,
}

/// What a button may do, once [`Reminders::claim`] has checked it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Show this task (Open).
    Open(TaskId),
    /// Snooze was done.
    Snoozed(Outcome),
    /// Complete this task, if [`still_as_shown`] when it is queued; then [`Reminders::dismiss`].
    Complete(Occurrence, TaskId),
}

/// Keeps the toasts in line with the reminders ([`plan`] decides, the sink does it) and holds what was
/// done. Log lines are counts only, at debug level: never a title.
#[allow(missing_debug_implementations)] // Holds the sink.
pub struct Reminders {
    schedule: Schedule,
    sink: Box<dyn ToastSink>,
    /// The reminder toasts up in this session, by reminder: what their buttons may act on.
    live: BTreeMap<ReminderId, Occurrence>,
    /// What the summary should cover (reminders counted as shown and still listed as they were),
    /// what the summary up now covers, and its token.
    summary_want: Vec<Alarm>,
    summary_up: Vec<Alarm>,
    summary_token: Option<u64>,
    next_token: u64,
    /// Toasts Windows couldn't show, in a row, and when to try again.
    failures: usize,
    retry_at: Option<i64>,
}

impl Reminders {
    pub fn new(schedule: Schedule, sink: Box<dyn ToastSink>) -> Self {
        Self {
            schedule,
            sink,
            live: BTreeMap::new(),
            summary_want: Vec::new(),
            summary_up: Vec::new(),
            summary_token: None,
            next_token: 0,
            failures: 0,
            retry_at: None,
        }
    }

    /// What has been done, to save.
    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    fn hide(&mut self, id: ReminderId) {
        self.live.remove(&id);
        self.sink.hide(&id.to_string());
    }

    /// Windows couldn't show something: try again after a back-off, longer each time in a row.
    fn back_off(&mut self, now: i64) {
        let wait = RETRY_MS[self.failures.min(RETRY_MS.len() - 1)];
        self.failures += 1;
        self.retry_at = Some(now + wait);
    }

    /// Brings the toasts in line with `state`'s reminders at `now`.
    pub fn reconcile(&mut self, state: &State, now: &Now, can_show: bool) -> Outcome {
        let now_ms = ms(now.at);
        let listed = alarms(state, since(&self.schedule, now.at));
        let backing_off = self.retry_at.is_some_and(|at| now_ms < at);
        let plan = plan(&listed, &self.schedule, now.at, can_show && !backing_off);
        for &id in &plan.hide {
            self.hide(id);
        }
        let mut schedule = plan.schedule;
        let mut failed: Vec<Alarm> = Vec::new();
        let mut delivered = false;
        for a in &plan.show {
            // Once one couldn't be shown, the rest of the batch isn't tried: it would fail alike.
            if !failed.is_empty() {
                failed.push(*a);
                continue;
            }
            let occurrence = Occurrence {
                id: a.reminder,
                at: ms(a.at),
                token: self.token(),
            };
            match self.sink.show(&reminder_toast(state, a, occurrence, now)) {
                Ok(()) => {
                    delivered = true;
                    self.live.insert(a.reminder, occurrence);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "a reminder toast wasn't shown");
                    failed.push(*a);
                }
            }
        }
        // The summary should cover what it covered and is still listed as it was, and what was
        // missed now. When that differs from what is up, the old one is withdrawn and the new one
        // shown (quietly unless it brings news); if that fails, it's tried again after a back-off.
        let still = |a: &Alarm| {
            listed
                .iter()
                .any(|l| l.reminder == a.reminder && l.at == a.at)
        };
        self.summary_want.retain(still);
        let mut want = self.summary_want.clone();
        want.extend(plan.missed.iter().copied());
        let may_try = failed.is_empty() && !backing_off;
        if want != self.summary_up && (may_try || want.is_empty()) {
            if !self.summary_up.is_empty() || self.summary_token.is_some() {
                self.sink.hide(MISSED_TAG);
                self.summary_up.clear();
                self.summary_token = None;
            }
            let token = self.token();
            if let Some(toast) = missed_toast(state, &want, token, now) {
                let toast = Toast {
                    quiet: plan.missed.is_empty(),
                    ..toast
                };
                match self.sink.show(&toast) {
                    Ok(()) => {
                        delivered = true;
                        self.summary_up = want.clone();
                        self.summary_token = Some(token);
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "the missed reminders' toast wasn't shown");
                        failed.extend(plan.missed.iter().copied());
                        want.retain(|a| !plan.missed.contains(a));
                    }
                }
            }
        } else if !plan.missed.is_empty() {
            // Not tried now: they stay due, uncounted, for the pass that tries.
            failed.extend(plan.missed.iter().copied());
            want.retain(|a| !plan.missed.contains(a));
        }
        self.summary_want = want;
        // What Windows didn't show isn't shown: it's tried again after a back-off.
        for a in &failed {
            if let Some(r) = schedule.records.get(&a.reminder) {
                schedule.shown.remove(&Shown {
                    id: a.reminder,
                    at: r.fire_at(),
                });
            }
        }
        let summary_pending = self.summary_want != self.summary_up;
        if !failed.is_empty() || (summary_pending && !backing_off && !delivered) {
            if !backing_off {
                self.back_off(now_ms);
            }
        } else if delivered && !summary_pending {
            self.failures = 0;
            self.retry_at = None;
        }
        if !(plan.show.is_empty() && plan.missed.is_empty() && plan.hide.is_empty()) {
            tracing::debug!(
                shown = plan.show.len().saturating_sub(failed.len()),
                missed = plan.missed.len(),
                hidden = plan.hide.len(),
                failed = failed.len(),
                "reminders"
            );
        }
        let changed = schedule != self.schedule;
        self.schedule = schedule;
        let waiting = plan.held || !failed.is_empty() || summary_pending;
        let retry = self.retry_at.filter(|_| waiting);
        let next = match (plan.next, retry) {
            (Some(a), Some(b)) => Some(a.min(at_ms(b))),
            (a, b) => a.or(b.map(at_ms)),
        };
        Outcome {
            changed,
            next,
            held: plan.held,
        }
    }

    /// Windows reported, after showing it, that the toast with this launch failed: it isn't up. Its
    /// occurrence isn't counted as shown any more and is tried again after a back-off. A report about
    /// a toast that has since been replaced does nothing.
    pub fn failed(&mut self, launch: Activation, state: &State, now: &Now) -> Option<Outcome> {
        let token = launch.token();
        if self.summary_token == Some(token) {
            self.summary_token = None;
            self.summary_up.clear();
        } else {
            let (&id, occurrence) = self.live.iter().find(|(_, o)| o.token == token)?;
            let occurrence = *occurrence;
            self.live.remove(&id);
            if let Some(r) = self
                .schedule
                .records
                .get(&id)
                .filter(|r| r.at == occurrence.at)
            {
                let fire = r.fire_at();
                self.schedule.shown.remove(&Shown { id, at: fire });
            }
        }
        self.back_off(ms(now.at));
        let outcome = self.reconcile(state, now, true);
        Some(Outcome {
            changed: true,
            ..outcome
        })
    }

    /// Checks what a toast's button asks for, and claims it: only while its toast is up (the token it
    /// was shown with), only for the reminder as it was shown (still live, at the same time, its task
    /// still open), and Snooze and Complete only once. `None` when it no longer stands: nothing is
    /// done then (a stale Open may still bring the window forward, but selects nothing).
    pub fn claim(&mut self, activation: Activation, state: &State, now: &Now) -> Option<Claim> {
        let as_shown = |o: Occurrence| {
            let task = state.reminders.get(&o.id)?.task;
            still_as_shown(state, o, task).then_some(task)
        };
        match activation {
            Activation::Open(o) => {
                let up = self.live.get(&o.id) == Some(&o)
                    || (self.summary_token == Some(o.token)
                        && self.summary_up.iter().any(|a| a.reminder == o.id));
                if up {
                    as_shown(o).map(Claim::Open)
                } else {
                    None
                }
            }
            Activation::Missed(_) => None,
            Activation::Snooze(o) | Activation::Complete(o) => {
                if self.live.get(&o.id) != Some(&o) {
                    return None;
                }
                let task = as_shown(o)?;
                // Once: its buttons may do nothing more.
                self.live.remove(&o.id);
                if matches!(activation, Activation::Complete(_)) {
                    return Some(Claim::Complete(o, task));
                }
                self.sink.hide(&o.id.to_string());
                let record = self.schedule.records.get(&o.id).filter(|r| r.at == o.at)?;
                self.schedule.records.insert(
                    o.id,
                    Record {
                        snooze: Some(ms(now.at) + SNOOZE_MS),
                        ..*record
                    },
                );
                let outcome = self.reconcile(state, now, true);
                Some(Claim::Snoozed(Outcome {
                    changed: true,
                    ..outcome
                }))
            }
        }
    }

    /// Takes a reminder's toast away (its task was completed from it).
    pub fn dismiss(&mut self, id: ReminderId) {
        self.hide(id);
    }

    /// Takes every reminder toast away (quitting, signing out).
    pub fn clear(&mut self) {
        self.live.clear();
        self.summary_want.clear();
        self.summary_up.clear();
        self.summary_token = None;
        self.sink.clear();
    }
}

#[cfg(test)]
mod tests;
