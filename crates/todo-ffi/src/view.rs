//! The view model: everything a screen shows, derived from the engine's optimistic view and the clock.
//!
//! Kotlin only draws these records; it never sees the domain model. The rules match the desktop app's
//! derivation (rooms, progress, due labels, list chips, agent marks), so both clients count and word
//! things the same way. The screenshot fixtures go through exactly the same code.

use crate::labels::{self, DueLabel, Now};
use crate::lists::{self, AgentMark};
use std::collections::{BTreeMap, BTreeSet};
use todo_client::activity::{agent_counts_today, agent_status, select_agent_rows};
use todo_client::room_header;
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::model::{LinkKind, List, Priority, Stamp, Task};
use todo_core::query::{BuiltinList, EvalContext, Query, fold};
use todo_core::time::{Timestamp, add_days, days_between};
use todo_core::{Actor, State};

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

    /// The shared smart list (`todo_client::room_header`), which knows its query and its wording.
    pub fn shared(self) -> room_header::SmartList {
        match self {
            Self::MyDay => room_header::SmartList::MyDay,
            Self::Important => room_header::SmartList::Important,
            Self::Planned => room_header::SmartList::Planned,
            Self::All => room_header::SmartList::All,
            Self::Completed => room_header::SmartList::Completed,
            Self::AgentActivity => room_header::SmartList::AgentActivity,
        }
    }

    /// Its saved query in `todo_core`.
    pub fn builtin(self) -> BuiltinList {
        self.shared().builtin()
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
    /// Explicitly deleted tasks, lists and collections.
    Trash,
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
    /// Whether the shared move-list intent has a destination for this list.
    pub can_move_list: bool,
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

/// An agent active today and what it did (its row under the drawer's Activity).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AgentSummary {
    /// Exact filter key, not the display name.
    pub id: String,
    /// Its letter mark and name.
    pub mark: AgentMark,
    /// "2 added, 1 edited today" or "3 edited today": the drawer has no "Today" heading.
    pub today: String,
}

/// The navigation: smart lists, Activity with today's agents, then collections and loose lists.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Home {
    /// Smart lists.
    pub smart: Vec<NavItem>,
    /// Each collection in display order, then the loose lists if any.
    pub groups: Vec<NavGroup>,
    /// Agents active today, by ID, at most four.
    pub agents: Vec<AgentSummary>,
}

/// The small line above a room's title.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Eyebrow {
    /// Smart lists: "Saturday | 21 Nov 2026 | W47".
    Day {
        /// "Saturday" (the eyebrow lead).
        weekday: String,
        /// "21 Nov 2026".
        date: String,
        /// "W47".
        week: String,
    },
    /// Collections and lists: "Collection | 3 lists".
    Kind {
        /// "Collection", or the list's collection name, or "List" (the eyebrow lead).
        kind: String,
        /// "3 lists", "9 open" (mono caps).
        detail: String,
    },
}

impl From<room_header::Eyebrow> for Eyebrow {
    fn from(e: room_header::Eyebrow) -> Self {
        match e {
            room_header::Eyebrow::Day {
                weekday,
                date,
                week,
            } => Self::Day {
                weekday,
                date,
                week,
            },
            room_header::Eyebrow::Kind { kind, detail } => Self::Kind { kind, detail },
        }
    }
}

/// The summary beside the sun, as every client words it (`todo_client::room_header::Summary`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RoomSummary {
    /// A history room has no score.
    None,
    /// Nothing to measure: "2 finished this month", or "nothing yet".
    Empty {
        /// Tasks in the room finished this calendar month.
        finished_this_month: u32,
    },
    /// Completed: "12 done".
    Done {
        /// Done tasks.
        done: u32,
    },
    /// "3 of 12 done · 9 open".
    Progress {
        /// Done.
        done: u32,
        /// Of.
        total: u32,
        /// Still open.
        open: u32,
    },
    /// My Day with every task done: "12 of 12 done · all clear".
    AllClear {
        /// Done, which is all of them.
        done: u32,
        /// Of.
        total: u32,
    },
}

impl From<room_header::Summary> for RoomSummary {
    fn from(s: room_header::Summary) -> Self {
        match s {
            room_header::Summary::None => Self::None,
            room_header::Summary::Empty {
                finished_this_month,
            } => Self::Empty {
                finished_this_month: count(finished_this_month),
            },
            room_header::Summary::Done { done } => Self::Done { done: count(done) },
            room_header::Summary::Progress { done, total, open } => Self::Progress {
                done: count(done),
                total: count(total),
                open: count(open),
            },
            room_header::Summary::AllClear { done, total } => Self::AllClear {
                done: count(done),
                total: count(total),
            },
        }
    }
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
    /// Its current list, for guarding a move from this row.
    pub list: String,
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
    /// The update stamp if the last change was by an agent. Pass it back in [`Holding`] to keep the
    /// row's place in Agent activity during completion. No owner device ID crosses the row boundary.
    pub agent_update: Option<AgentUpdate>,
}

/// The agent and exact time of a task's latest update.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AgentUpdate {
    /// Agent ID (not its display label).
    pub agent_id: String,
    /// RFC 3339 timestamp, opaque to the app.
    pub at: String,
}

impl AgentUpdate {
    fn stamp(&self) -> Option<Stamp> {
        Some(Stamp {
            at: self.at.parse().ok()?,
            by: Actor::agent(&self.agent_id).ok()?,
        })
    }
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
    /// The summary beside the sun.
    pub summary: RoomSummary,
    /// The summary as one line ("3 of 12 done · 9 open"), for screen readers.
    pub summary_text: String,
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
    /// The Trash room's rows, from the same replica as the header.
    pub trash: Option<crate::engine::trash::TrashView>,
}

/// Where the app's completion choreography keeps tasks while it plays (as the desktop's `UiState::holding`
/// and `reopening`). A row toggled on screen stays where it was, and counts as it did, until its motion
/// releases it: only then do the rows, the counts, the sun and the finished day change. The old agent
/// update stamp (if any) keeps a completing row in Agent activity; other fields and liveness follow
/// the replica.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct RowPlacement {
    /// Tasks just completed: they keep their place in the open rows and count as open.
    pub holding: Vec<Holding>,
    /// Tasks just reopened: they keep their completed place and count as done.
    pub reopening: Vec<Reopening>,
}

/// A task held open while its completion plays.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Holding {
    /// Task ID.
    pub id: String,
    /// The latest agent update before completion, as given by [`TaskRow::agent_update`].
    pub agent_update: Option<AgentUpdate>,
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
        self.holding.is_empty() && self.reopening.is_empty()
    }
}

/// The state as the rows place it while a choreography plays: each held task still open, and each task
/// being reopened still done, as it was. A held completion also keeps the old agent update stamp.
/// Invalid IDs or stamps are ignored. Only the list and its counts read this; the detail pane always
/// shows the truth.
pub fn placed(state: &State, placement: &RowPlacement) -> State {
    let mut placed = state.clone();
    for h in &placement.holding {
        if let Some(t) = task_id(&h.id).and_then(|id| placed.tasks.get_mut(&id))
            && t.is_done()
        {
            t.completed_at = None;
            if let Some(stamp) = h.agent_update.as_ref().and_then(AgentUpdate::stamp) {
                t.updated = stamp;
            }
        }
    }
    for r in &placement.reopening {
        if let Some(t) = task_id(&r.id).and_then(|id| placed.tasks.get_mut(&id))
            && t.completed_at.is_none()
        {
            t.completed_at = r.completed_at.parse::<Timestamp>().ok();
        }
    }
    placed
}

/// A room, filtered as [`room_filtered`], with the tasks in `placement` kept where they were while their
/// choreography plays: the room of the [`placed`] state.
pub fn room_placed(
    state: &State,
    key: &RoomKey,
    filter_text: &str,
    placement: &RowPlacement,
    now: &Now,
) -> RoomView {
    if placement.is_empty() {
        return room_filtered(state, key, filter_text, now);
    }
    room_filtered(&placed(state, placement), key, filter_text, now)
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

/// What tapping a property row edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FieldEdit {
    /// The due date (a date picker).
    Due,
    /// The priority (four levels).
    Priority,
    /// The reminder (a date and time picker).
    Reminder,
    Repeat,
    Tags,
}

/// A property row in the detail pane.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DetailField {
    /// What a tap edits; `None` for a row that is only shown.
    pub edit: Option<FieldEdit>,
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

/// A reminder as the detail pane showed it: which one, and when, in milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReminderRef {
    /// Reminder ID.
    pub id: String,
    /// Its time.
    pub at_ms: i64,
}

/// Where a task came from.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SourceLink {
    /// Exact link for removal.
    pub id: String,
    /// Browser destination, only for a parsed HTTP(S) URL with a host.
    pub url: Option<String>,
    /// Subject or page title.
    pub title: String,
    /// "sender · Thu 12 Nov, 10:20" or the web link as text.
    pub subtitle: String,
    /// Lucide icon key (`mail` or `link`).
    pub icon: String,
}

fn browser_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.has_host()).then(|| url.to_string())
}

/// A task's retained creation, latest edit or current completion.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskActivityLine {
    /// An agent's letter and colours, when the actor is an agent.
    pub agent: Option<AgentMark>,
    /// For other actors, a clock or mail icon, or `None` for the owner's `Y`.
    pub icon: Option<String>,
    /// Display name (never a device ID).
    pub who: String,
    /// Action, for example `created this`.
    pub action: String,
    /// Relative time, for example `2 hr ago`.
    pub relative: String,
}

/// Everything the detail pane shows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskDetail {
    /// Task ID.
    pub id: String,
    /// Its current list, for guarding a move from detail.
    pub list: String,
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
    /// The due date, ISO (`2026-11-22`), for the date picker.
    pub due: Option<String>,
    /// It has a repeat rule, which clearing the due date also clears.
    pub repeats: bool,
    pub custom_repeat: crate::engine::organize::CustomRepeat,
    pub repeat_options: Vec<crate::engine::organize::RepeatOption>,
    pub tags: Vec<crate::engine::organize::TagChoice>,
    /// The reminder the Reminder row shows and edits (`intent::reminder::shown`), for its picker; the
    /// edit passes it back, so it changes exactly this reminder, as it was.
    pub reminder: Option<ReminderRef>,
    /// Collection and list chips ("Personal / Admin").
    pub breadcrumb: Vec<Chip>,
    /// "Created 12 Nov / Edited today 08:50".
    pub subtitle: String,
    /// Retained task activity, newest first.
    pub activity: Vec<TaskActivityLine>,
    /// Steps in order.
    pub steps: Vec<StepRow>,
    /// Properties.
    pub fields: Vec<DetailField>,
    /// Sources.
    pub sources: Vec<SourceLink>,
    /// Live file references, available even without a connection.
    pub attachments: Vec<crate::engine::attachments::AttachmentRow>,
}

pub(crate) use todo_client::room_header::{list_live, task_live};

fn priority_marks(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

/// A priority's name, as the desktop's picker words it.
pub fn priority_name(p: Priority) -> &'static str {
    match p {
        Priority::High => "High",
        Priority::Normal => "Normal",
        Priority::Low => "Low",
        Priority::None => "None",
    }
}

/// The priority for `marks` `!` marks (0 to 3), as the detail pane's picker sends it.
pub fn priority_of(marks: u8) -> Option<Priority> {
    Some(match marks {
        0 => Priority::None,
        1 => Priority::Low,
        2 => Priority::Normal,
        3 => Priority::High,
        _ => return None,
    })
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
        list: task.list.to_string(),
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
            .map(|t| format!("#{}", labels::one_line(&t.name)))
            .collect(),
        from_mail: ix.mail.contains(&task.id),
        completed_at: task.completed_at.map(|c| c.to_string()),
        agent_update: task.updated.by.agent_id().map(|agent| AgentUpdate {
            agent_id: agent.to_string(),
            at: task.updated.at.to_string(),
        }),
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

/// The shared room key for a resolved room; `None` for an ID that doesn't parse.
fn shared_key(key: &RoomKey) -> Option<room_header::RoomKey<'static>> {
    Some(match key {
        RoomKey::Smart { list } => room_header::RoomKey::Smart(list.shared()),
        RoomKey::Collection { id } => room_header::RoomKey::Collection(parse_collection(id)?),
        RoomKey::List { id } => room_header::RoomKey::List(parse_list(id)?),
        RoomKey::Trash => room_header::RoomKey::Trash,
    })
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
        RoomKey::Smart { .. } | RoomKey::Trash => key.clone(),
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
    let ctx = now.eval();
    // Held tasks are already open in the placed state (`room_placed`), so nothing more is held here.
    let holding = BTreeSet::new();
    let shared =
        shared_key(&key).unwrap_or(room_header::RoomKey::Smart(room_header::SmartList::MyDay));
    let tasks = room_header::room_tasks(state, shared, &holding, &ctx);
    let header = room_header::header_of(state, shared, &tasks, &holding, &now.local_time());
    let room_header::RoomTasks { open, done } = tasks;

    let filtered = !filter_text.trim().is_empty() && !shared.is_history();
    let open = filter(state, filter_text, &ctx, open);
    let done = filter(state, filter_text, &ctx, done);

    let (title, hue, chips, placeholder) = match &key {
        RoomKey::Trash => (
            todo_client::trash::TITLE.into(),
            "agent-activity".into(),
            false,
            String::new(),
        ),
        RoomKey::Smart { list } => (
            list.builtin().name.to_owned(),
            list.hue().to_owned(),
            true,
            "Add a task…".to_owned(),
        ),
        RoomKey::Collection { id } => {
            let col = parse_collection(id).and_then(|c| state.collections.get(&c));
            let name = col.map(|c| c.name.clone()).unwrap_or_default();
            (
                name.clone(),
                col.map_or("lagoon", |c| {
                    lists::swatch(c.color.as_deref(), c.id.as_uuid().as_bytes())
                })
                .to_owned(),
                true,
                format!("Add to {name}…"),
            )
        }
        RoomKey::List { id } => {
            let list = parse_list(id).and_then(|l| state.lists.get(&l));
            let name = list.map(|l| l.name.clone()).unwrap_or_default();
            (
                name.clone(),
                list.map_or("lagoon", |l| {
                    lists::swatch(l.color.as_deref(), l.id.as_uuid().as_bytes())
                })
                .to_owned(),
                false,
                format!("Add to {name}…"),
            )
        }
    };

    let finished = header.finished_day();
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
        eyebrow: header.eyebrow.clone().into(),
        hue,
        done: count(header.progress_done),
        total: count(header.progress_total),
        open: count(header.open()),
        summary: header.summary.into(),
        summary_text: header.summary.text(),
        finished,
        tasks: open
            .iter()
            .map(|t| row(state, &ix, t, now, chips))
            .collect(),
        completed: done
            .iter()
            .map(|t| row(state, &ix, t, now, chips))
            .collect(),
        finished_this_month: count(header.finished_this_month),
        empty_line: header.empty_line(filtered).to_owned(),
        add_placeholder: placeholder,
        tomorrow,
        filtered,
        plan_weekday,
        trash: (key == RoomKey::Trash)
            .then(|| crate::engine::trash::TrashView::from_state(state, now.at)),
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
            can_move_list: false,
            key: RoomKey::Smart { list: *s },
            name: s.label().into(),
            hue: s.hue().into(),
            glyph: None,
            icon: Some(s.icon().into()),
            open: match s {
                SmartList::Completed => 0,
                _ => count(room_header::smart_list_count(state, s.builtin(), &ctx)),
            },
        })
        .collect();
    let counts = open_counts(state);
    let list_item = |l: &List| NavItem {
        can_move_list: todo_client::intent::can_move_list(state, l.id),
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
                can_move_list: false,
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
    let top: Vec<_> = ordered
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
    Home {
        smart,
        groups,
        agents: agents(state, now),
    }
}

/// Agents who added or last edited a task today, with the shared sidebar status.
fn agents(state: &State, now: &Now) -> Vec<AgentSummary> {
    let counts = agent_counts_today(state, |at| now.is_today(at));
    select_agent_rows(counts)
        .filter_map(|(actor, added, edited)| {
            Some(AgentSummary {
                id: actor.agent_id()?.as_str().into(),
                mark: lists::agent(actor)?,
                today: agent_status(added, edited),
            })
        })
        .collect()
}

/// Derives the detail pane, or `None` if the task is gone.
pub fn detail(state: &State, id: &str, now: &Now) -> Option<TaskDetail> {
    let id: TaskId = id.parse().ok()?;
    let task = state.tasks.get(&id).filter(|t| task_live(state, t))?;
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

    let mut fields = vec![
        match task.due {
            Some(d) => DetailField {
                edit: Some(FieldEdit::Due),
                icon: "calendar".into(),
                label: "Due".into(),
                value: labels::short_date(d, today),
                mono: true,
                empty: false,
            },
            None => DetailField {
                edit: Some(FieldEdit::Due),
                icon: "calendar".into(),
                label: "Due".into(),
                value: "–".into(),
                mono: false,
                empty: true,
            },
        },
        DetailField {
            edit: Some(FieldEdit::Priority),
            icon: "flag".into(),
            label: "Priority".into(),
            value: if task.priority == Priority::None {
                "–".into()
            } else {
                priority_name(task.priority).into()
            },
            mono: false,
            empty: task.priority == Priority::None,
        },
    ];
    let shown = todo_client::intent::reminder::shown(state, id, &now.eval());
    let reminder = shown.map(|r| r.at);
    fields.push(DetailField {
        edit: Some(FieldEdit::Reminder),
        icon: "bell".into(),
        label: "Reminder".into(),
        value: reminder.map_or_else(|| "–".into(), |at| labels::when(at, now)),
        mono: reminder.is_some(),
        empty: reminder.is_none(),
    });
    fields.push(DetailField {
        edit: Some(FieldEdit::Repeat),
        icon: "repeat".into(),
        label: "Repeat".into(),
        value: task
            .recurrence
            .as_ref()
            .map(todo_client::intent::repeat::describe_repeat)
            .unwrap_or_else(|| "Does not repeat".into()),
        mono: false,
        empty: task.recurrence.is_none(),
    });
    let tags: Vec<String> = task
        .tags
        .iter()
        .filter_map(|t| state.tags.get(t))
        .filter(|t| t.deleted.is_none())
        .map(|t| format!("#{}", labels::one_line(&t.name)))
        .collect();
    fields.push(DetailField {
        edit: Some(FieldEdit::Tags),
        icon: "hash".into(),
        label: "Tags".into(),
        value: if tags.is_empty() {
            "–".into()
        } else {
            tags.join(" ")
        },
        mono: true,
        empty: tags.is_empty(),
    });
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
                id: l.id.to_string(),
                url: m.web_link.as_deref().and_then(browser_url),
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
                id: l.id.to_string(),
                title: title.clone().unwrap_or_else(|| url.clone()),
                subtitle: url.clone(),
                url: browser_url(url),
                icon: "link".into(),
            },
        })
        .collect();

    let mut files: Vec<_> = state
        .attachments
        .values()
        .filter(|a| a.task == id && a.deleted.is_none())
        .collect();
    files.sort_by_key(|a| (a.created.at, a.id));
    let attachments = files.into_iter().map(Into::into).collect();

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
        list: task.list.to_string(),
        title: task.title.clone(),
        notes: task.notes.clone(),
        done: task.is_done(),
        my_day: task.my_day == Some(today),
        priority: priority_marks(task.priority),
        due: task.due.map(|d| d.to_string()),
        repeats: task.recurrence.is_some(),
        custom_repeat: crate::engine::organize::draft(task, today),
        repeat_options: crate::engine::organize::presets(task, today),
        tags: todo_client::intent::tags(state)
            .into_iter()
            .map(|t| crate::engine::organize::TagChoice {
                id: t.id.to_string(),
                name: labels::one_line(&t.name),
                hue: lists::swatch(t.color.as_deref(), t.id.as_uuid().as_bytes()).into(),
                selected: task.tags.contains(&t.id),
            })
            .collect(),
        reminder: shown.map(|r| ReminderRef {
            id: r.id.to_string(),
            at_ms: r.at.as_millisecond(),
        }),
        breadcrumb,
        subtitle,
        activity: todo_client::task_activity::lines(state, task.id, now.at)
            .into_iter()
            .map(|line| TaskActivityLine {
                agent: line.actor.as_ref().and_then(lists::agent),
                icon: match line.actor {
                    Some(Actor::Owner { .. } | Actor::Agent { .. }) => None,
                    Some(Actor::EmailIngest) => Some("mail".into()),
                    _ => Some("clock".into()),
                },
                who: line.who,
                action: line.action.into(),
                relative: line.when,
            })
            .collect(),
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
        attachments,
    })
}
