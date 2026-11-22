//! The view model: everything a screen shows, derived from the engine's optimistic view and the clock.
//!
//! Kotlin only draws these records; it never sees the domain model. The rules match the desktop app's
//! derivation (rooms, progress, due labels, list chips, agent marks), so both clients count and word
//! things the same way. The screenshot fixtures go through exactly the same code.

use crate::labels::{self, DueLabel, Now};
use crate::lists::{self, AgentMark};
use std::collections::{BTreeMap, BTreeSet};
use todo_core::State;
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::model::{LinkKind, List, Priority, Stamp, Task};
use todo_core::query::{BUILTIN_LISTS, BuiltinList, EvalContext, Query, fold};
use todo_core::time::{Date, Timestamp, add_days, days_between};

/// A built-in smart list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, uniffi::Enum)]
pub enum SmartList {
    /// Today's plan.
    MyDay,
    /// High priority.
    Important,
    /// With a due date.
    Planned,
    /// Everything open.
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

    fn id(self) -> &'static str {
        match self {
            Self::MyDay => "my_day",
            Self::Important => "important",
            Self::Planned => "planned",
            Self::All => "all",
            Self::Completed => "completed",
            Self::AgentActivity => "agent_activity",
        }
    }

    /// Its saved query in `todo_core`.
    pub fn builtin(self) -> BuiltinList {
        BUILTIN_LISTS
            .iter()
            .copied()
            .find(|b| b.id == self.id())
            .unwrap_or(BUILTIN_LISTS[0])
    }

    fn label(self) -> &'static str {
        match self {
            Self::AgentActivity => "Agent activity",
            other => other.builtin().name,
        }
    }

    /// Its hue's token key.
    fn hue(self) -> &'static str {
        match self {
            Self::MyDay => "my-day",
            Self::Important => "important",
            Self::Planned => "planned",
            Self::All => "all",
            Self::Completed => "completed",
            Self::AgentActivity => "agent-activity",
        }
    }

    /// Its Lucide icon key.
    fn icon(self) -> &'static str {
        match self {
            Self::MyDay => "sun",
            Self::Important => "star",
            Self::Planned => "calendar",
            Self::All => "list",
            Self::Completed => "circle-check",
            Self::AgentActivity => "sparkles",
        }
    }

    fn empty_line(self) -> &'static str {
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

/// Which room a screen shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum RoomKey {
    /// A smart list.
    Smart {
        /// Which.
        list: SmartList,
    },
    /// Every list of a collection.
    Collection {
        /// Its ID.
        id: String,
    },
    /// One list.
    List {
        /// Its ID.
        id: String,
    },
}

impl Default for RoomKey {
    fn default() -> Self {
        Self::Smart {
            list: SmartList::MyDay,
        }
    }
}

/// A coloured list chip: a dot and a name in the list's hue.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Chip {
    /// The text.
    pub label: String,
    /// Swatch key.
    pub hue: String,
}

/// An entry in the navigation (sidebar sheet, room pills).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NavItem {
    /// Where it goes.
    pub key: RoomKey,
    /// Its name.
    pub name: String,
    /// Hue token key.
    pub hue: String,
    /// Hand-drawn glyph key (lists and collections) or `None`.
    pub glyph: Option<String>,
    /// Lucide icon key (smart lists) or `None`.
    pub icon: Option<String>,
    /// Open tasks in it.
    pub open: u32,
}

/// A collection with its lists, or the top-level lists (`collection` is `None`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct NavGroup {
    /// The collection itself, as a room.
    pub collection: Option<NavItem>,
    /// Its lists.
    pub lists: Vec<NavItem>,
}

/// The navigation: smart lists, then collections and lists.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Home {
    /// Smart lists.
    pub smart: Vec<NavItem>,
    /// Top-level lists first, then each collection.
    pub groups: Vec<NavGroup>,
}

/// The small line above a room's title.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Eyebrow {
    /// Smart lists: "Saturday | 21 Nov | W47".
    Day {
        /// "Saturday" (Fraunces italic).
        weekday: String,
        /// "21 Nov".
        date: String,
        /// "W47".
        week: String,
    },
    /// Collections and lists: "Collection | 3 lists".
    Kind {
        /// "Collection", or the list's collection name, or "List" (Fraunces italic).
        kind: String,
        /// "3 lists", "9 open" (mono caps).
        detail: String,
    },
}

/// Done and total steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepCount {
    /// Done.
    pub done: u32,
    /// All.
    pub total: u32,
}

/// One task row.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskRow {
    /// Task ID.
    pub id: String,
    /// Title.
    pub title: String,
    /// Completed.
    pub done: bool,
    /// In today's My Day.
    pub my_day: bool,
    /// The list chip, when the room spans several lists.
    pub chip: Option<Chip>,
    /// Due label (none once done).
    pub due: Option<DueLabel>,
    /// `!` marks, 0–3.
    pub priority: u8,
    /// The agent that last touched it.
    pub agent: Option<AgentMark>,
    /// Steps, if it has any.
    pub steps: Option<StepCount>,
    /// `#tags`.
    pub tags: Vec<String>,
    /// Created from an email.
    pub from_mail: bool,
    /// When it was completed (none while open), exactly: an RFC 3339 timestamp to nanoseconds, opaque to
    /// the app. Reopening a task passes it back in [`Reopening`], so the row keeps its completed place (done
    /// tasks sort by it to the nanosecond) while it leaves it.
    pub completed_at: Option<String>,
    /// When and by whom it was last changed, exactly: opaque to the app. Toggling a task changes it (to
    /// the owner, now), so the app passes it back in [`LastChange`] and the row keeps its place in a room
    /// that selects or sorts by it (Agent activity) while its choreography plays.
    pub updated: String,
}

/// What the finished day hands over to: tomorrow's first task and the rest of the week.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Tomorrow {
    /// "Sunday".
    pub weekday: String,
    /// The first task.
    pub first: TaskRow,
    /// "Sun 22 Nov, 09:00".
    pub first_when: String,
    /// More of tomorrow.
    pub also: Vec<TaskRow>,
    /// The rest of the week.
    pub week: Vec<TaskRow>,
    /// How many more there are.
    pub more: u32,
}

/// A room: header, landscape inputs and rows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RoomView {
    /// Which room (a deleted list falls back to My Day).
    pub key: RoomKey,
    /// "My Day", "Personal".
    pub title: String,
    /// Hue token key of the room.
    pub hue: String,
    /// The line above the title.
    pub eyebrow: Eyebrow,
    /// Done, for the sun and "3 of 12 done".
    pub done: u32,
    /// Total, for the sun.
    pub total: u32,
    /// Still open.
    pub open: u32,
    /// My Day with everything done: the dusk scene.
    pub finished: bool,
    /// Open rows, in order.
    pub tasks: Vec<TaskRow>,
    /// Done rows, newest first.
    pub completed: Vec<TaskRow>,
    /// Finished this month (the empty list's summary).
    pub finished_this_month: u32,
    /// The empty state's line.
    pub empty_line: String,
    /// The composer's placeholder.
    pub add_placeholder: String,
    /// The finished day's hand-over (never while filtered).
    pub tomorrow: Option<Tomorrow>,
    /// A filter is applied: the rows (not the progress) are only the matching ones.
    pub filtered: bool,
    /// The finished day (not filtered): the next day's name ("Sunday") for *Plan Sunday*, even when
    /// nothing is due then and there is no hand-over.
    pub plan_weekday: Option<String>,
}

/// Where the app's completion choreography keeps tasks while it plays (as the desktop's `UiState::holding`
/// and `reopening`). A row toggled on screen stays where it was, and counts as it did, until its motion
/// releases it: only then do the rows, the counts, the sun and the finished day change.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct RowPlacement {
    /// Tasks just completed: they keep their place in the open rows and count as open.
    pub holding: Vec<String>,
    /// Tasks just reopened: they keep their completed place and count as done.
    pub reopening: Vec<Reopening>,
    /// Tasks toggled on screen, with their last change as it was before: a room that selects or sorts by
    /// it (Agent activity) keeps their rows, in order, where they were.
    pub last_changes: Vec<LastChange>,
}

/// A task's last change as it was before a toggle ([`TaskRow::updated`]).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LastChange {
    /// Task ID.
    pub id: String,
    /// Exactly as [`TaskRow::updated`] gave it.
    pub updated: String,
}

/// A task being reopened, with when it had been completed ([`TaskRow::completed_at`]).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Reopening {
    /// Task ID.
    pub id: String,
    /// When it had been completed, exactly as [`TaskRow::completed_at`] gave it.
    pub completed_at: String,
}

impl RowPlacement {
    /// Nothing held back.
    pub fn is_empty(&self) -> bool {
        self.holding.is_empty() && self.reopening.is_empty() && self.last_changes.is_empty()
    }
}

/// [`TaskRow::updated`]: the stamp, as JSON.
fn stamp_token(s: &Stamp) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The state as the rows place it while a choreography plays: each held task still open, and each task
/// being reopened still done, as it was. IDs or times that don't parse, and IDs that name no task, are
/// ignored. Only the list and its counts read this; the detail pane always shows the truth.
pub fn placed(state: &State, placement: &RowPlacement) -> State {
    let mut placed = state.clone();
    for id in &placement.holding {
        if let Some(t) = task_id(id).and_then(|id| placed.tasks.get_mut(&id)) {
            t.completed_at = None;
        }
    }
    for r in &placement.reopening {
        if let Some(t) = task_id(&r.id).and_then(|id| placed.tasks.get_mut(&id))
            && t.completed_at.is_none()
        {
            t.completed_at = r.completed_at.parse::<Timestamp>().ok();
        }
    }
    for c in &placement.last_changes {
        if let Some(t) = task_id(&c.id).and_then(|id| placed.tasks.get_mut(&id))
            && let Ok(s) = serde_json::from_str::<Stamp>(&c.updated)
        {
            t.updated = s;
        }
    }
    placed
}

fn task_id(id: &str) -> Option<TaskId> {
    id.parse().ok()
}

/// A step in the detail pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepRow {
    /// Step ID.
    pub id: String,
    /// Title.
    pub title: String,
    /// Done.
    pub done: bool,
}

/// A property row in the detail pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DetailField {
    /// Lucide icon key.
    pub icon: String,
    /// "Due".
    pub label: String,
    /// "Sun 22 Nov, 09:00" or "–".
    pub value: String,
    /// The value is a date or number (mono).
    pub mono: bool,
    /// The value is empty.
    pub empty: bool,
}

/// Where a task came from.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SourceLink {
    /// Subject or page title.
    pub title: String,
    /// "sender · Thu 12 Nov, 10:20" or the host.
    pub subtitle: String,
    /// Lucide icon key (`mail` or `link`).
    pub icon: String,
}

/// Everything the detail pane shows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskDetail {
    /// Task ID.
    pub id: String,
    /// Title.
    pub title: String,
    /// Notes.
    pub notes: String,
    /// Completed.
    pub done: bool,
    /// In today's My Day.
    pub my_day: bool,
    /// `!` marks.
    pub priority: u8,
    /// Collection and list chips ("Personal / Admin").
    pub breadcrumb: Vec<Chip>,
    /// "Created 12 Nov / Edited today 08:50".
    pub subtitle: String,
    /// Steps in order.
    pub steps: Vec<StepRow>,
    /// Properties.
    pub fields: Vec<DetailField>,
    /// Sources.
    pub sources: Vec<SourceLink>,
}

pub(crate) fn list_live(state: &State, id: ListId) -> bool {
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

pub(crate) fn task_live(state: &State, t: &Task) -> bool {
    t.deleted.is_none() && t.merged_into.is_none() && list_live(state, t.list)
}

fn priority_marks(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Lookups built once per derivation.
struct Index {
    /// Next live reminder per task (the first on or after today, else the last).
    reminder: BTreeMap<TaskId, Timestamp>,
    steps: BTreeMap<TaskId, StepCount>,
    mail: BTreeSet<TaskId>,
}

impl Index {
    fn new(state: &State, now: &Now) -> Self {
        let today = now.today();
        let mut reminder: BTreeMap<TaskId, Timestamp> = BTreeMap::new();
        let mut all: Vec<_> = state
            .reminders
            .values()
            .filter(|r| r.deleted.is_none())
            .collect();
        all.sort_by_key(|r| r.at);
        // Ascending, so a past reminder is replaced by every later one: the result is the first one on
        // or after today, else the last.
        for r in &all {
            let keep = reminder
                .get(&r.task)
                .is_some_and(|prev| now.date_of(*prev) >= today);
            if !keep {
                reminder.insert(r.task, r.at);
            }
        }
        let mut steps: BTreeMap<TaskId, StepCount> = BTreeMap::new();
        for s in state.steps.values().filter(|s| s.deleted.is_none()) {
            let c = steps
                .entry(s.task)
                .or_insert(StepCount { done: 0, total: 0 });
            c.total += 1;
            if s.completed_at.is_some() {
                c.done += 1;
            }
        }
        let mail = state
            .links
            .values()
            .filter(|l| l.deleted.is_none() && matches!(l.kind, LinkKind::Email(_)))
            .map(|l| l.task)
            .collect();
        Self {
            reminder,
            steps,
            mail,
        }
    }
}

fn list_chip(state: &State, list: ListId) -> Option<Chip> {
    let l = state.lists.get(&list)?;
    Some(Chip {
        label: l.name.clone(),
        hue: lists::swatch(l.color.as_deref(), l.id.as_uuid().as_bytes()).into(),
    })
}

fn row(state: &State, ix: &Index, task: &Task, now: &Now, chip: bool) -> TaskRow {
    TaskRow {
        id: task.id.to_string(),
        title: task.title.clone(),
        done: task.is_done(),
        my_day: task.my_day == Some(now.today()),
        chip: if chip {
            list_chip(state, task.list)
        } else {
            None
        },
        due: if task.is_done() {
            None
        } else {
            labels::due(task.due, ix.reminder.get(&task.id).copied(), now)
        },
        priority: priority_marks(task.priority),
        agent: lists::agent(&task.updated.by).or_else(|| lists::agent(&task.created.by)),
        steps: ix.steps.get(&task.id).copied(),
        tags: task
            .tags
            .iter()
            .filter_map(|t| state.tags.get(t))
            .filter(|t| t.deleted.is_none())
            .map(|t| format!("#{}", t.name))
            .collect(),
        from_mail: ix.mail.contains(&task.id),
        completed_at: task.completed_at.map(|c| c.to_string()),
        updated: stamp_token(&task.updated),
    }
}

fn parse_list(id: &str) -> Option<ListId> {
    id.parse().ok()
}

fn parse_collection(id: &str) -> Option<CollectionId> {
    id.parse().ok()
}

/// Lists in navigation order: top-level lists, then each collection's (collections by position).
pub(crate) fn ordered_lists(state: &State) -> Vec<&List> {
    let mut cols: Vec<_> = state
        .collections
        .values()
        .filter(|c| c.deleted.is_none())
        .collect();
    cols.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    let mut out = Vec::new();
    let mut push = |c: Option<CollectionId>| {
        let mut ls: Vec<&List> = state
            .lists
            .values()
            .filter(|l| l.collection == c && l.deleted.is_none())
            .collect();
        ls.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
        out.extend(ls);
    };
    push(None);
    for c in cols {
        push(Some(c.id));
    }
    out
}

/// The room's tasks: (open in order, done newest first).
fn room_tasks<'a>(state: &'a State, key: &RoomKey, now: &Now) -> (Vec<&'a Task>, Vec<&'a Task>) {
    let ctx = now.eval();
    match key {
        RoomKey::Smart { list } => {
            let Ok(q) = list.builtin().parse() else {
                return (Vec::new(), Vec::new());
            };
            let open = q.run(state, &ctx);
            // The rows a smart list hides once done, for its progress and Completed section.
            let done_query = match list {
                SmartList::MyDay => Some("my_day and done sort:-completed"),
                SmartList::Important => Some("priority>=high and done sort:-completed"),
                SmartList::Planned => Some("due:any and done sort:-completed"),
                SmartList::All => Some("done sort:-completed"),
                SmartList::Completed | SmartList::AgentActivity => None,
            };
            let done = done_query
                .and_then(|s| Query::parse(s).ok())
                .map(|q| q.run(state, &ctx))
                .unwrap_or_default();
            (open, done)
        }
        RoomKey::Collection { .. } | RoomKey::List { .. } => {
            let in_room = |t: &Task| match key {
                RoomKey::List { id } => parse_list(id) == Some(t.list),
                RoomKey::Collection { id } => {
                    state.lists.get(&t.list).and_then(|l| l.collection) == parse_collection(id)
                }
                RoomKey::Smart { .. } => false,
            };
            let list_pos = |t: &Task| state.lists.get(&t.list).map(|l| l.position.clone());
            let (mut open, mut done): (Vec<&Task>, Vec<&Task>) = state
                .tasks
                .values()
                .filter(|t| in_room(t) && task_live(state, t))
                .partition(|t| !t.is_done());
            open.sort_by(|a, b| {
                (list_pos(a), &a.position, a.id).cmp(&(list_pos(b), &b.position, b.id))
            });
            done.sort_by(|a, b| b.completed_at.cmp(&a.completed_at).then(a.id.cmp(&b.id)));
            (open, done)
        }
    }
}

fn completed_on(now: &Now, t: &Task, pred: impl Fn(Date) -> bool) -> bool {
    t.completed_at.is_some_and(|c| pred(now.date_of(c)))
}

/// The room, falling back to My Day when its list or collection is gone.
fn resolve(state: &State, key: &RoomKey) -> RoomKey {
    match key {
        RoomKey::List { id } if parse_list(id).is_some_and(|l| list_live(state, l)) => key.clone(),
        RoomKey::Collection { id }
            if parse_collection(id)
                .and_then(|c| state.collections.get(&c))
                .is_some_and(|c| c.deleted.is_none()) =>
        {
            key.clone()
        }
        RoomKey::Smart { .. } => key.clone(),
        _ => RoomKey::default(),
    }
}

/// Derives a room.
pub fn room(state: &State, key: &RoomKey, now: &Now) -> RoomView {
    room_filtered(state, key, "", now)
}

/// The filter, as on the desktop: the query language when it parses (`tag:docs`, `due<=friday`), else
/// every word must appear in the title (case and accents folded).
fn filter<'a>(state: &State, text: &str, ctx: &EvalContext, tasks: Vec<&'a Task>) -> Vec<&'a Task> {
    let text = text.trim();
    if text.is_empty() {
        return tasks;
    }
    if let Ok(q) = Query::parse(text) {
        return tasks
            .into_iter()
            .filter(|t| q.matches(state, t, ctx))
            .collect();
    }
    let folded = fold(text);
    let words: Vec<&str> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    tasks
        .into_iter()
        .filter(|t| {
            let title = fold(&t.title);
            words.iter().all(|w| title.contains(w))
        })
        .collect()
}

/// Derives a room showing only the tasks that match `filter` (see [`filter`]). The progress (sun and
/// counts) is the whole room's; the finished day's hand-over is left out while filtering.
pub fn room_filtered(state: &State, key: &RoomKey, filter_text: &str, now: &Now) -> RoomView {
    let key = resolve(state, key);
    let ix = Index::new(state, now);
    let today = now.today();
    let (open, done) = room_tasks(state, &key, now);

    let (progress_done, progress_total) = match key {
        RoomKey::Smart {
            list: SmartList::MyDay,
        } => (done.len(), open.len() + done.len()),
        RoomKey::Smart {
            list: SmartList::AgentActivity | SmartList::Completed,
        } => (0, open.len()),
        _ => {
            let today_done = done
                .iter()
                .filter(|t| completed_on(now, t, |d| d == today))
                .count();
            (today_done, today_done + open.len())
        }
    };
    let finished_this_month = done
        .iter()
        .filter(|t| {
            completed_on(now, t, |d| {
                d.year() == today.year() && d.month() == today.month()
            })
        })
        .count();

    let open_all = open.len();
    let filtered = !filter_text.trim().is_empty();
    let ctx = now.eval();
    let open = filter(state, filter_text, &ctx, open);
    let done = filter(state, filter_text, &ctx, done);

    let (title, eyebrow, hue, chips, empty_line, placeholder) = match &key {
        RoomKey::Smart { list } => (
            list.builtin().name.to_owned(),
            Eyebrow::Day {
                weekday: labels::weekday(today).into(),
                date: labels::day_month(today, today),
                week: labels::iso_week(today),
            },
            list.hue().to_owned(),
            true,
            list.empty_line().to_owned(),
            "Add a task…".to_owned(),
        ),
        RoomKey::Collection { id } => {
            let col = parse_collection(id).and_then(|c| state.collections.get(&c));
            let name = col.map(|c| c.name.clone()).unwrap_or_default();
            let n = state
                .lists
                .values()
                .filter(|l| col.is_some_and(|c| l.collection == Some(c.id)) && l.deleted.is_none())
                .count();
            (
                name.clone(),
                Eyebrow::Kind {
                    kind: "Collection".into(),
                    detail: format!("{n} {}", if n == 1 { "list" } else { "lists" }),
                },
                col.map_or("lagoon", |c| {
                    lists::swatch(c.color.as_deref(), c.id.as_uuid().as_bytes())
                })
                .to_owned(),
                true,
                "Nothing here yet.".to_owned(),
                format!("Add to {name}…"),
            )
        }
        RoomKey::List { id } => {
            let list = parse_list(id).and_then(|l| state.lists.get(&l));
            let name = list.map(|l| l.name.clone()).unwrap_or_default();
            let kind = list
                .and_then(|l| l.collection)
                .and_then(|c| state.collections.get(&c))
                .map_or_else(|| "List".to_owned(), |c| c.name.clone());
            (
                name.clone(),
                Eyebrow::Kind {
                    kind,
                    detail: format!("{open_all} open"),
                },
                list.map_or("lagoon", |l| {
                    lists::swatch(l.color.as_deref(), l.id.as_uuid().as_bytes())
                })
                .to_owned(),
                false,
                "Nothing here yet.".to_owned(),
                format!("Add to {name}…"),
            )
        }
    };

    let finished = matches!(
        key,
        RoomKey::Smart {
            list: SmartList::MyDay
        }
    ) && progress_total > 0
        && progress_done >= progress_total;
    let plan_weekday = (finished && !filtered)
        .then(|| add_days(today, 1))
        .flatten()
        .map(|d| labels::weekday(d).to_owned());
    let tomorrow = if finished && !filtered {
        tomorrow(state, &ix, now)
    } else {
        None
    };
    RoomView {
        title,
        eyebrow,
        hue,
        done: count(progress_done),
        total: count(progress_total),
        open: count(open_all),
        finished,
        tasks: open
            .iter()
            .map(|t| row(state, &ix, t, now, chips))
            .collect(),
        completed: done
            .iter()
            .map(|t| row(state, &ix, t, now, chips))
            .collect(),
        finished_this_month: count(finished_this_month),
        empty_line: if filtered {
            "Nothing matches the filter.".to_owned()
        } else {
            empty_line
        },
        add_placeholder: placeholder,
        tomorrow,
        filtered,
        plan_weekday,
        key,
    }
}

fn tomorrow(state: &State, ix: &Index, now: &Now) -> Option<Tomorrow> {
    let today = now.today();
    let tomorrow = add_days(today, 1)?;
    let mut soon: Vec<&Task> = state
        .tasks
        .values()
        .filter(|t| !t.is_done() && task_live(state, t))
        .filter(|t| {
            t.due
                .is_some_and(|d| (1..=7).contains(&days_between(today, d)))
        })
        .collect();
    soon.sort_by(|a, b| {
        (
            a.due,
            std::cmp::Reverse(a.priority),
            ix.reminder.get(&a.id),
            &a.position,
        )
            .cmp(&(
                b.due,
                std::cmp::Reverse(b.priority),
                ix.reminder.get(&b.id),
                &b.position,
            ))
    });
    let (tmrw, week): (Vec<&Task>, Vec<&Task>) =
        soon.into_iter().partition(|t| t.due == Some(tomorrow));
    let first = tmrw.first()?;
    let first_when = match ix.reminder.get(&first.id) {
        Some(at) if now.date_of(*at) == tomorrow => {
            format!(
                "{}, {}",
                labels::short_date(tomorrow, today),
                now.clock(*at)
            )
        }
        _ => labels::short_date(tomorrow, today),
    };
    const ALSO: usize = 2;
    const WEEK: usize = 3;
    let more = tmrw.len().saturating_sub(1 + ALSO) + week.len().saturating_sub(WEEK);
    Some(Tomorrow {
        weekday: labels::weekday(tomorrow).into(),
        first: row(state, ix, first, now, true),
        first_when,
        also: tmrw
            .iter()
            .skip(1)
            .take(ALSO)
            .map(|t| row(state, ix, t, now, true))
            .collect(),
        week: week
            .iter()
            .take(WEEK)
            .map(|t| row(state, ix, t, now, true))
            .collect(),
        more: count(more),
    })
}

/// Open tasks per list (live tasks only).
fn open_counts(state: &State) -> BTreeMap<ListId, usize> {
    let mut m = BTreeMap::new();
    for t in state.tasks.values() {
        if !t.is_done() && task_live(state, t) {
            *m.entry(t.list).or_insert(0) += 1;
        }
    }
    m
}

/// The navigation.
pub fn home(state: &State, now: &Now) -> Home {
    let ctx = now.eval();
    let smart = SmartList::ALL
        .iter()
        .map(|s| NavItem {
            key: RoomKey::Smart { list: *s },
            name: s.label().into(),
            hue: s.hue().into(),
            glyph: None,
            icon: Some(s.icon().into()),
            open: match s {
                SmartList::Completed => 0,
                _ => count(
                    s.builtin()
                        .parse()
                        .map(|q| q.run(state, &ctx).len())
                        .unwrap_or(0),
                ),
            },
        })
        .collect();
    let counts = open_counts(state);
    let list_item = |l: &List| NavItem {
        key: RoomKey::List {
            id: l.id.to_string(),
        },
        name: l.name.clone(),
        hue: lists::swatch(l.color.as_deref(), l.id.as_uuid().as_bytes()).into(),
        glyph: Some(lists::list_glyph(&l.name).into()),
        icon: None,
        open: count(counts.get(&l.id).copied().unwrap_or(0)),
    };
    let ordered = ordered_lists(state);
    let mut groups = Vec::new();
    let top: Vec<NavItem> = ordered
        .iter()
        .filter(|l| l.collection.is_none())
        .map(|l| list_item(l))
        .collect();
    if !top.is_empty() {
        groups.push(NavGroup {
            collection: None,
            lists: top,
        });
    }
    let mut cols: Vec<_> = state
        .collections
        .values()
        .filter(|c| c.deleted.is_none())
        .collect();
    cols.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    for c in cols {
        let lists_in: Vec<NavItem> = ordered
            .iter()
            .filter(|l| l.collection == Some(c.id))
            .map(|l| list_item(l))
            .collect();
        let open = lists_in.iter().map(|l| l.open).sum();
        groups.push(NavGroup {
            collection: Some(NavItem {
                key: RoomKey::Collection {
                    id: c.id.to_string(),
                },
                name: c.name.clone(),
                hue: lists::swatch(c.color.as_deref(), c.id.as_uuid().as_bytes()).into(),
                glyph: Some(lists::collection_glyph(&c.name).into()),
                icon: None,
                open,
            }),
            lists: lists_in,
        });
    }
    Home { smart, groups }
}

/// Derives the detail pane, or `None` if the task is gone.
pub fn detail(state: &State, id: &str, now: &Now) -> Option<TaskDetail> {
    let id: TaskId = id.parse().ok()?;
    let task = state.tasks.get(&id).filter(|t| task_live(state, t))?;
    let ix = Index::new(state, now);
    let today = now.today();
    let mut subtitle = format!(
        "Created {}",
        labels::day_month(now.date_of(task.created.at), today)
    );
    if let Some(a) = lists::agent(&task.created.by) {
        subtitle = format!("{subtitle} by {}", a.name);
    }
    if task.updated.at != task.created.at {
        let day = now.date_of(task.updated.at);
        let when = if day == today {
            "today".to_owned()
        } else {
            labels::day_month(day, today)
        };
        subtitle = format!("{subtitle} / Edited {when} {}", now.clock(task.updated.at));
    }
    let mut steps: Vec<_> = state
        .steps
        .values()
        .filter(|s| s.task == id && s.deleted.is_none())
        .collect();
    steps.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));

    let mut fields = vec![match task.due {
        Some(d) => DetailField {
            icon: "calendar".into(),
            label: "Due".into(),
            value: labels::short_date(d, today),
            mono: true,
            empty: false,
        },
        None => DetailField {
            icon: "calendar".into(),
            label: "Due".into(),
            value: "–".into(),
            mono: false,
            empty: true,
        },
    }];
    if let Some(at) = ix.reminder.get(&id) {
        fields.push(DetailField {
            icon: "bell".into(),
            label: "Reminder".into(),
            value: labels::when(*at, now),
            mono: true,
            empty: false,
        });
    }
    fields.push(DetailField {
        icon: "repeat".into(),
        label: "Repeat".into(),
        value: if task.recurrence.is_some() {
            "Repeats".into()
        } else {
            "Does not repeat".into()
        },
        mono: false,
        empty: task.recurrence.is_none(),
    });
    let tags: Vec<String> = task
        .tags
        .iter()
        .filter_map(|t| state.tags.get(t))
        .filter(|t| t.deleted.is_none())
        .map(|t| format!("#{}", t.name))
        .collect();
    if !tags.is_empty() {
        fields.push(DetailField {
            icon: "hash".into(),
            label: "Tags".into(),
            value: tags.join(" "),
            mono: true,
            empty: false,
        });
    }
    let mut links: Vec<_> = state
        .links
        .values()
        .filter(|l| l.task == id && l.deleted.is_none())
        .collect();
    links.sort_by_key(|l| (l.created.at, l.id));
    let sources = links
        .into_iter()
        .map(|l| match &l.kind {
            LinkKind::Email(m) => SourceLink {
                title: m.subject.clone(),
                subtitle: format!(
                    "{} · {}, {}",
                    m.sender,
                    labels::short_date(now.date_of(m.received_at), today),
                    now.clock(m.received_at)
                ),
                icon: "mail".into(),
            },
            LinkKind::Web { url, title } => SourceLink {
                title: title.clone().unwrap_or_else(|| url.clone()),
                subtitle: url::Url::parse(url)
                    .ok()
                    .and_then(|u| {
                        u.host_str()
                            .map(|h| h.trim_start_matches("www.").to_owned())
                    })
                    .unwrap_or_default(),
                icon: "link".into(),
            },
        })
        .collect();

    let list = state.lists.get(&task.list);
    let mut breadcrumb = Vec::new();
    if let Some(c) = list
        .and_then(|l| l.collection)
        .and_then(|c| state.collections.get(&c))
    {
        breadcrumb.push(Chip {
            label: c.name.clone(),
            hue: lists::swatch(c.color.as_deref(), c.id.as_uuid().as_bytes()).into(),
        });
    }
    breadcrumb.extend(list_chip(state, task.list));
    Some(TaskDetail {
        id: id.to_string(),
        title: task.title.clone(),
        notes: task.notes.clone(),
        done: task.is_done(),
        my_day: task.my_day == Some(today),
        priority: priority_marks(task.priority),
        breadcrumb,
        subtitle,
        steps: steps
            .iter()
            .map(|s| StepRow {
                id: s.id.to_string(),
                title: s.title.clone(),
                done: s.completed_at.is_some(),
            })
            .collect(),
        fields,
        sources,
    })
}
