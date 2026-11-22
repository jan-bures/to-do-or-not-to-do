//! A room's header, derived once for every client: which tasks the room holds, the eyebrow, the progress
//! that places the sun, the summary beside it, the status bar's counts and the empty line.
//!
//! Pure: the replica's state, the room, the instant and the owner's time zone go in, plain values come out.
//! Each client draws them its own way (GPUI, Leptos, Compose), but they count and word them the same.
//!
//! # What the sun measures
//!
//! - **Lists, collections, Important, Planned and All** measure today's work: tasks completed since local
//!   midnight, out of those plus the ones still open. A list with years of finished tasks then doesn't
//!   show a sun that has long set.
//! - **My Day** measures its whole plan: done of everything planned for today.
//! - **Agent activity** measures the tasks agents changed this week: done of all of them.
//! - **Completed** only counts: "12 done", with the sun at the end.
//! - **Activity** shows changes, not tasks, and keeps no score.
//!
//! A task still playing its completion (the client's *holding* set) counts as open until it has left the
//! rows, so the counts and the sun change only then.

use std::collections::BTreeSet;

use jiff::tz::TimeZone;
use todo_core::State;
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::model::Task;
use todo_core::query::{BUILTIN_LISTS, BuiltinList, EvalContext, Query};
use todo_core::time::{Date, Timestamp, Weekday};

use crate::activity;
use crate::intent::confirm::one_line;
use crate::intent::quick_add::long_date;

/// A built-in smart list, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SmartList {
    /// Today's plan.
    MyDay,
    /// High priority.
    Important,
    /// With a due date.
    Planned,
    /// Everything.
    All,
    /// Done.
    Completed,
    /// What agents changed in the last week.
    AgentActivity,
}

impl SmartList {
    /// Sidebar order.
    pub const ALL: [SmartList; 6] = [
        Self::MyDay,
        Self::Important,
        Self::Planned,
        Self::All,
        Self::Completed,
        Self::AgentActivity,
    ];

    /// Its built-in list's ID (`my_day`, `important`, ...).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::MyDay => "my_day",
            Self::Important => "important",
            Self::Planned => "planned",
            Self::All => "all",
            Self::Completed => "completed",
            Self::AgentActivity => "agent_activity",
        }
    }

    /// The smart list with a built-in list's ID.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    /// Its definition in `todo_core` (the saved query and the name).
    #[must_use]
    pub fn builtin(self) -> BuiltinList {
        BUILTIN_LISTS
            .iter()
            .copied()
            .find(|b| b.id == self.id())
            .unwrap_or(BUILTIN_LISTS[0])
    }

    /// The room's title: "My Day", "Important", ...
    #[must_use]
    pub fn name(self) -> &'static str {
        self.builtin().name
    }

    /// The query for the room's Completed section; `None` where the rows already include done tasks.
    #[must_use]
    pub fn done_query(self) -> Option<&'static str> {
        match self {
            Self::MyDay => Some("my_day and done sort:-completed"),
            Self::Important => Some("priority>=high and done sort:-completed"),
            Self::Planned => Some("due:any and done sort:-completed"),
            Self::All => Some("done sort:-completed"),
            Self::Completed | Self::AgentActivity => None,
        }
    }

    /// The empty room's line.
    #[must_use]
    pub fn empty_line(self) -> &'static str {
        match self {
            Self::MyDay => "Nothing planned yet.",
            Self::Important => "Nothing pressing.",
            Self::Planned => "Nothing on the calendar.",
            Self::All => "Nothing open.",
            Self::Completed => "Nothing finished yet.",
            Self::AgentActivity => "No agent changes this week.",
        }
    }
}

/// Which room a header is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoomKey<'a> {
    /// A smart list.
    Smart(SmartList),
    /// Every list of a collection.
    Collection(CollectionId),
    /// One list.
    List(ListId),
    /// Every confirmed change, or one agent's (by its agent ID).
    Activity(Option<&'a str>),
}

impl RoomKey<'_> {
    /// The Activity room, filtered or not.
    #[must_use]
    pub fn is_activity(self) -> bool {
        matches!(self, Self::Activity(_))
    }
}

/// The owner's clock: the instant and the time zone that says where local midnight is.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalTime {
    /// The instant.
    pub at: Timestamp,
    /// The owner's time zone (a fixed offset where the platform gives only that).
    pub tz: TimeZone,
}

impl LocalTime {
    /// The instant in `tz`.
    #[must_use]
    pub fn new(at: Timestamp, tz: TimeZone) -> Self {
        Self { at, tz }
    }

    /// The owner's local date.
    #[must_use]
    pub fn today(&self) -> Date {
        self.date_of(self.at)
    }

    /// The local date of `ts`.
    #[must_use]
    pub fn date_of(&self, ts: Timestamp) -> Date {
        ts.to_zoned_datetime(self.tz.clone()).date()
    }

    /// The query evaluation context: local today and the current UTC offset.
    #[must_use]
    pub fn eval(&self) -> EvalContext {
        let offset = self.tz.to_offset(self.at).seconds();
        EvalContext::new(self.at, self.today(), offset)
    }
}

/// A live list: not deleted, and not in a deleted collection.
#[must_use]
pub fn list_live(state: &State, id: ListId) -> bool {
    state.lists.get(&id).is_some_and(|l| {
        l.deleted.is_none()
            && l.collection.is_none_or(|c| {
                state
                    .collections
                    .get(&c)
                    .is_some_and(|c| c.deleted.is_none())
            })
    })
}

/// A live task: not deleted or merged, in a live list. `State::is_live` without its clones.
#[must_use]
pub fn task_live(state: &State, t: &Task) -> bool {
    t.deleted.is_none() && t.merged_into.is_none() && list_live(state, t.list)
}

/// A room's tasks, unfiltered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomTasks<'s> {
    /// The rows, in order: open tasks, and held ones (just completed) where they were.
    pub open: Vec<&'s Task>,
    /// The Completed section, most recent first. Completed and Agent activity keep their done tasks in
    /// `open`, since their rows include them.
    pub done: Vec<&'s Task>,
}

/// The tasks a room holds. Smart lists come in their query's order, lists and collections in manual order
/// (collections list by list). A held task (completed, still playing its completion) stays in the rows
/// where it was as if it were open.
#[must_use]
pub fn room_tasks<'s>(
    state: &'s State,
    key: RoomKey<'_>,
    holding: &BTreeSet<TaskId>,
    ctx: &EvalContext,
) -> RoomTasks<'s> {
    let held = |t: &Task| holding.contains(&t.id) && t.is_done();
    match key {
        RoomKey::Smart(smart) => {
            let Ok(q) = smart.builtin().parse() else {
                return RoomTasks::default();
            };
            let m = q.matcher(state, ctx);
            let mut open: Vec<&Task> = state
                .tasks
                .values()
                .filter(|t| {
                    m.matches(t)
                        || (held(t) && task_live(state, t) && {
                            // Still in the room as if it were open, until the completion has played.
                            let mut as_open = (*t).clone();
                            as_open.completed_at = None;
                            q.matches(state, &as_open, ctx)
                        })
                })
                .collect();
            open.sort_by_cached_key(|t| q.order_key(state, t));
            let done = smart
                .done_query()
                .and_then(|s| Query::parse(s).ok())
                .map(|dq| {
                    dq.run(state, ctx)
                        .into_iter()
                        .filter(|t| !holding.contains(&t.id))
                        .collect()
                })
                .unwrap_or_default();
            RoomTasks { open, done }
        }
        RoomKey::Activity(_) => RoomTasks::default(),
        RoomKey::List(_) | RoomKey::Collection(_) => {
            let in_room = |t: &Task| match key {
                RoomKey::List(l) => t.list == l,
                RoomKey::Collection(c) => {
                    state.lists.get(&t.list).and_then(|l| l.collection) == Some(c)
                }
                RoomKey::Smart(_) | RoomKey::Activity(_) => false,
            };
            let list_pos = |t: &Task| state.lists.get(&t.list).map(|l| l.position.clone());
            let mut open = Vec::new();
            let mut done = Vec::new();
            for t in state.tasks.values() {
                if !in_room(t) || !task_live(state, t) {
                    continue;
                }
                if !t.is_done() || held(t) {
                    open.push(t);
                } else {
                    done.push(t);
                }
            }
            open.sort_by(|a, b| {
                (list_pos(a), &a.position, a.id).cmp(&(list_pos(b), &b.position, b.id))
            });
            done.sort_by(|a, b| b.completed_at.cmp(&a.completed_at).then(a.id.cmp(&b.id)));
            RoomTasks { open, done }
        }
    }
}

/// The line above the room's title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Eyebrow {
    /// Smart lists: the weekday in italics, then the date and the week, `Saturday | 21 Nov 2026 | W47`.
    Day {
        /// "Saturday".
        weekday: String,
        /// "21 Nov 2026".
        date: String,
        /// "W47".
        week: String,
    },
    /// Collections, lists and Activity: an italic kind, then a detail, `Collection | 4 lists`,
    /// `Work | 9 open`, `History | All changes`.
    Kind {
        /// "Collection", the list's collection or "List", "History".
        kind: String,
        /// "4 lists", "9 open", "All changes", "Agent: Claude".
        detail: String,
    },
}

impl Eyebrow {
    /// The italic lead.
    #[must_use]
    pub fn lead(&self) -> &str {
        match self {
            Self::Day { weekday, .. } => weekday,
            Self::Kind { kind, .. } => kind,
        }
    }

    /// The details after the lead, in order. Clients set them in mono caps (the token's transform).
    #[must_use]
    pub fn details(&self) -> Vec<&str> {
        match self {
            Self::Day { date, week, .. } => vec![date, week],
            Self::Kind { detail, .. } => vec![detail],
        }
    }

    /// As one line, `Saturday · 21 Nov 2026 · W47`, for tests and screen readers.
    #[must_use]
    pub fn text(&self) -> String {
        std::iter::once(self.lead())
            .chain(self.details())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// "Saturday".
#[must_use]
pub fn weekday(d: Date) -> &'static str {
    WEEKDAYS[Weekday::of(d).offset()]
}

/// "W47": the ISO week.
#[must_use]
pub fn iso_week(d: Date) -> String {
    format!("W{}", d.iso_week_date().week())
}

/// The eyebrow of a dated room: `Saturday`, `21 Nov 2026`, `W47`.
#[must_use]
pub fn day_eyebrow(today: Date) -> Eyebrow {
    Eyebrow::Day {
        weekday: weekday(today).to_owned(),
        date: long_date(today),
        week: iso_week(today),
    }
}

/// The eyebrow's detail for Activity: every change, or one agent's.
#[must_use]
pub fn activity_scope(agent: Option<&str>) -> String {
    agent.map_or_else(
        || "All changes".to_owned(),
        |agent| format!("Agent: {}", activity::agent_name(agent)),
    )
}

/// The summary beside the sun.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Summary {
    /// Activity: changes aren't scored, the feed says what happened.
    None,
    /// Nothing to measure: `2 finished this month`, or `nothing yet`.
    Empty {
        /// Tasks in the room finished this calendar month.
        finished_this_month: usize,
    },
    /// Completed: `12 done`.
    Done {
        /// Done tasks.
        done: usize,
    },
    /// `3 of 12 done · 9 open`.
    Progress {
        /// Done.
        done: usize,
        /// Of.
        total: usize,
        /// Still open (`total - done`).
        open: usize,
    },
    /// My Day with every task done: `12 of 12 done · all clear`.
    AllClear {
        /// Done, which is all of them.
        done: usize,
        /// Of.
        total: usize,
    },
}

impl Summary {
    /// As one line, for tests and screen readers.
    #[must_use]
    pub fn text(&self) -> String {
        match *self {
            Self::None => String::new(),
            Self::Empty {
                finished_this_month: 0,
            } => "nothing yet".to_owned(),
            Self::Empty {
                finished_this_month: n,
            } => format!("{n} finished this month"),
            Self::Done { done } => format!("{done} done"),
            Self::Progress { done, total, open } => {
                format!("{done} of {total} done · {open} open")
            }
            Self::AllClear { done, total } => format!("{done} of {total} done · all clear"),
        }
    }
}

/// The status bar's counts, `2 open · 0 done today`: two figures, each with its words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusCounts {
    /// The first figure and its words: `(2, "open")`.
    pub first: (usize, &'static str),
    /// The second figure and its words: `(0, "done today")`.
    pub second: (usize, &'static str),
}

impl StatusCounts {
    /// A task room's: open tasks and those completed since local midnight.
    #[must_use]
    pub fn tasks(open: usize, done_today: usize) -> Self {
        Self {
            first: (open, "open"),
            second: (done_today, "done today"),
        }
    }

    /// Activity's: the changes shown, and today's among them.
    #[must_use]
    pub fn activity(shown: usize, today: usize) -> Self {
        Self {
            first: (shown, "shown"),
            second: (today, "today"),
        }
    }

    /// As one line: `2 open · 0 done today`.
    #[must_use]
    pub fn text(&self) -> String {
        format!(
            "{} {} · {} {}",
            self.first.0, self.first.1, self.second.0, self.second.1
        )
    }
}

/// The line an empty room shows when a filter matches nothing.
pub const NOTHING_MATCHES: &str = "Nothing matches the filter.";
/// The line an empty list or collection shows.
pub const NOTHING_HERE: &str = "Nothing here yet.";

/// A room's header, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomHeader {
    /// The line above the title.
    pub eyebrow: Eyebrow,
    /// Where the sun stands: done of total. Unfiltered.
    pub progress_done: usize,
    /// See [`progress_done`](Self::progress_done); never less than it.
    pub progress_total: usize,
    /// The summary beside the sun.
    pub summary: Summary,
    /// The status bar's counts; `None` in Activity, whose counts come from the feed
    /// ([`StatusCounts::activity`]).
    pub status: Option<StatusCounts>,
    /// Tasks in the room completed since local midnight.
    pub done_today: usize,
    /// Tasks in the room completed this calendar month (the empty room's summary).
    pub finished_this_month: usize,
    /// My Day: finishing every task turns the header into the dusk scene.
    pub finishes_day: bool,
    /// The empty room's line, without a filter.
    pub empty_line: &'static str,
}

impl RoomHeader {
    /// Still open: total less done.
    #[must_use]
    pub fn open(&self) -> usize {
        self.progress_total - self.progress_done
    }

    /// My Day with every task done: the dusk scene.
    #[must_use]
    pub fn finished_day(&self) -> bool {
        self.finishes_day && self.progress_total > 0 && self.progress_done >= self.progress_total
    }

    /// The empty room's line, or [`NOTHING_MATCHES`] while a filter narrows the room.
    #[must_use]
    pub fn empty_line(&self, filtered: bool) -> &'static str {
        if filtered {
            NOTHING_MATCHES
        } else {
            self.empty_line
        }
    }
}

/// Derives a room's header. `holding` names the tasks still playing their completion, which count as
/// open (and stay in the rows) until they're released.
#[must_use]
pub fn room_header(
    state: &State,
    key: RoomKey<'_>,
    holding: &BTreeSet<TaskId>,
    now: &LocalTime,
) -> RoomHeader {
    let tasks = room_tasks(state, key, holding, &now.eval());
    header_of(state, key, &tasks, holding, now)
}

/// [`room_header`] for tasks already gathered with [`room_tasks`] (with the same `holding`), for a client
/// that draws the rows too.
#[must_use]
pub fn header_of(
    state: &State,
    key: RoomKey<'_>,
    tasks: &RoomTasks<'_>,
    holding: &BTreeSet<TaskId>,
    now: &LocalTime,
) -> RoomHeader {
    let today = now.today();
    let (open, done) = (&tasks.open, &tasks.done);
    // A task still playing its completion counts as open: the counts (and the sun) change when it has
    // left the rows.
    let counted_done = |t: &Task| t.is_done() && !holding.contains(&t.id);
    let finished_on = |t: &Task, pred: &dyn Fn(Date) -> bool| {
        counted_done(t) && t.completed_at.is_some_and(|c| pred(now.date_of(c)))
    };
    let all = || open.iter().chain(done.iter());
    let done_today = all().filter(|t| finished_on(t, &|d| d == today)).count();
    let finished_this_month = all()
        .filter(|t| {
            finished_on(t, &|d| {
                d.year() == today.year() && d.month() == today.month()
            })
        })
        .count();
    let still_open = open.iter().filter(|t| !counted_done(t)).count();

    let (progress_done, progress_total) = match key {
        RoomKey::Activity(_) => (0, 0),
        RoomKey::Smart(SmartList::MyDay) => {
            let d = open.iter().filter(|t| counted_done(t)).count() + done.len();
            (d, open.len() + done.len())
        }
        RoomKey::Smart(SmartList::AgentActivity) => {
            (open.iter().filter(|t| counted_done(t)).count(), open.len())
        }
        RoomKey::Smart(SmartList::Completed) => {
            let d = open.iter().filter(|t| counted_done(t)).count();
            (d, d)
        }
        _ => (done_today, done_today + still_open),
    };

    let finishes_day = key == RoomKey::Smart(SmartList::MyDay);
    let summary = match key {
        RoomKey::Activity(_) => Summary::None,
        _ if progress_total == 0 => Summary::Empty {
            finished_this_month,
        },
        RoomKey::Smart(SmartList::Completed) => Summary::Done {
            done: progress_done,
        },
        _ if finishes_day && progress_done >= progress_total => Summary::AllClear {
            done: progress_done,
            total: progress_total,
        },
        _ => Summary::Progress {
            done: progress_done,
            total: progress_total,
            open: progress_total - progress_done,
        },
    };

    let (eyebrow, empty_line) = match key {
        RoomKey::Smart(s) => (day_eyebrow(today), s.empty_line()),
        RoomKey::Collection(c) => {
            let n = state
                .lists
                .values()
                .filter(|l| l.collection == Some(c) && l.deleted.is_none())
                .count();
            (
                Eyebrow::Kind {
                    kind: "Collection".to_owned(),
                    detail: format!("{n} {}", if n == 1 { "list" } else { "lists" }),
                },
                NOTHING_HERE,
            )
        }
        RoomKey::List(l) => {
            let kind = state
                .lists
                .get(&l)
                .and_then(|l| l.collection)
                .and_then(|c| state.collections.get(&c))
                .map_or_else(|| "List".to_owned(), |c| one_line(&c.name));
            (
                Eyebrow::Kind {
                    kind,
                    detail: format!("{} open", progress_total - progress_done),
                },
                NOTHING_HERE,
            )
        }
        RoomKey::Activity(agent) => (
            Eyebrow::Kind {
                kind: "History".to_owned(),
                detail: activity_scope(agent),
            },
            activity::empty_line(agent.is_some()),
        ),
    };

    RoomHeader {
        eyebrow,
        progress_done,
        progress_total,
        summary,
        status: (!key.is_activity())
            .then(|| StatusCounts::tasks(progress_total - progress_done, done_today)),
        done_today,
        finished_this_month,
        finishes_day,
        empty_line,
    }
}

#[cfg(test)]
mod tests;
