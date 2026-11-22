//! The view model: everything the main window shows, derived from the sync engine's state.
//!
//! [`derive`] turns a `todo_core::State` (the client's optimistic view), the window's own UI state
//! ([`UiState`]: the selected room and task, the filter, ...), the sync status and the clock into a
//! [`Shell`]. The views only draw a `Shell`; they never look at the domain model. The `--screenshot`
//! fixtures build a `State` with invented data and go through exactly the same derivation.
//!
//! Nothing here touches GPUI windows, the network or the disk, so it is all unit-tested.

mod derive;
pub mod draft;
pub mod files;
pub mod fit;
pub mod hints;
pub mod labels;
pub mod layout;
pub mod lists;
pub mod motion;
pub use todo_client::intent::{self as commands, quick_add};

use std::collections::{BTreeMap, BTreeSet};

use gpui::SharedString;
use jiff::tz::TimeZone;
use todo_client::Connection;
use todo_core::State;
use todo_core::ids::{AttachmentId, CollectionId, LinkId, ListId, StepId, TaskId};
use todo_core::query::{BUILTIN_LISTS, BuiltinList, EvalContext};
use todo_core::time::{Date, Timestamp};

pub use derive::{derive, my_day_progress};

use crate::theme::{Agent, Glyph, Hue, Icon, hues};

/// The current time and the owner's time zone. Fixtures and tests use a fixed one.
#[derive(Clone, Debug, PartialEq)]
pub struct Now {
    pub at: Timestamp,
    pub tz: TimeZone,
}

impl Now {
    /// The system clock and time zone.
    pub fn system() -> Self {
        Self {
            at: Timestamp::now(),
            tz: TimeZone::system(),
        }
    }

    /// A fixed time in UTC, `"2026-11-21T10:35:00Z"`. For fixtures and tests (the input is a constant).
    pub fn fixed(at: &str) -> Self {
        Self {
            at: at.parse().unwrap_or(Timestamp::UNIX_EPOCH),
            tz: TimeZone::UTC,
        }
    }

    /// The owner's local date.
    pub fn today(&self) -> Date {
        self.at.to_zoned_datetime(self.tz.clone()).date()
    }

    /// The query evaluation context: local today and the current UTC offset.
    pub fn eval(&self) -> EvalContext {
        let offset = self.tz.to_offset(self.at).seconds();
        EvalContext::new(self.at, self.today(), offset)
    }

    /// The local date of `ts`.
    pub fn date_of(&self, ts: Timestamp) -> Date {
        ts.to_zoned_datetime(self.tz.clone()).date()
    }
}

/// The built-in smart lists, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Smart {
    MyDay,
    Important,
    Planned,
    All,
    Completed,
    AgentActivity,
}

impl Smart {
    pub const ALL: [Smart; 6] = [
        Self::MyDay,
        Self::Important,
        Self::Planned,
        Self::All,
        Self::Completed,
        Self::AgentActivity,
    ];

    /// Its definition in `todo_core` (the saved query).
    pub fn builtin(self) -> BuiltinList {
        let id = match self {
            Self::MyDay => "my_day",
            Self::Important => "important",
            Self::Planned => "planned",
            Self::All => "all",
            Self::Completed => "completed",
            Self::AgentActivity => "agent_activity",
        };
        BUILTIN_LISTS
            .iter()
            .copied()
            .find(|b| b.id == id)
            .unwrap_or(BUILTIN_LISTS[0])
    }

    /// The sidebar label (shorter than the built-in name for agent activity).
    pub fn label(self) -> &'static str {
        match self {
            Self::AgentActivity => "Agent activity",
            other => other.builtin().name,
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::MyDay => Icon::Sun,
            Self::Important => Icon::Star,
            Self::Planned => Icon::Calendar,
            Self::All => Icon::List,
            Self::Completed => Icon::CircleCheck,
            Self::AgentActivity => Icon::Sparkles,
        }
    }

    pub fn hue(self) -> Hue {
        match self {
            Self::MyDay => hues::MY_DAY,
            Self::Important => hues::IMPORTANT,
            Self::Planned => hues::PLANNED,
            Self::All => hues::ALL,
            Self::Completed => hues::COMPLETED,
            Self::AgentActivity => hues::AGENT_ACTIVITY,
        }
    }

    /// The query for the room's "Completed" section; `None` where the rows already include done tasks.
    pub fn done_query(self) -> Option<&'static str> {
        match self {
            Self::MyDay => Some("my_day and done sort:-completed"),
            Self::Important => Some("priority>=high and done sort:-completed"),
            Self::Planned => Some("due:any and done sort:-completed"),
            Self::All => Some("done sort:-completed"),
            Self::Completed | Self::AgentActivity => None,
        }
    }

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

/// What the main pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoomKey {
    Smart(Smart),
    Collection(CollectionId),
    List(ListId),
}

impl RoomKey {
    /// A deleted room takes the owner back to My Day.
    pub fn resolved(self, state: &State) -> Self {
        match self {
            Self::List(l) if !state.is_live(l.into()) => Self::default(),
            Self::Collection(c) if !state.is_live(c.into()) => Self::default(),
            key => key,
        }
    }
}

impl From<RoomKey> for commands::TaskRoom {
    fn from(room: RoomKey) -> Self {
        match room {
            RoomKey::Smart(Smart::MyDay) => Self::MyDay,
            RoomKey::Smart(Smart::Important) => Self::Important,
            RoomKey::Smart(Smart::Planned) => Self::Planned,
            RoomKey::Smart(_) => Self::Other,
            RoomKey::Collection(id) => Self::Collection(id),
            RoomKey::List(id) => Self::List(id),
        }
    }
}

impl Default for RoomKey {
    fn default() -> Self {
        Self::Smart(Smart::MyDay)
    }
}

/// The window's own state, which the data doesn't know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiState {
    pub room: RoomKey,
    /// The selected task (shown in the detail pane).
    pub task: Option<TaskId>,
    /// Inline filter text (query language, or plain words).
    pub filter: String,
    /// The Completed section is open.
    pub show_completed: bool,
    /// Tasks just completed that keep their place in the open rows while the completion plays. They count
    /// as open until they're released (`motion::Release`), so the counts and the sun change only then.
    pub holding: BTreeSet<TaskId>,
    /// Tasks just reopened that keep their completed place (with when they were completed) while they
    /// leave it. They count as done until they're released.
    pub reopening: BTreeMap<TaskId, Timestamp>,
    /// Collection IDs whose child lists are hidden in the sidebar.
    pub collapsed_collections: BTreeSet<CollectionId>,
    /// Device-local preferred widths of the main window's side panes.
    pub panes: layout::PaneWidths,
}

impl UiState {
    /// Changes only sidebar visibility, even if the current room is a list in this collection.
    pub fn toggle_collection(&mut self, id: CollectionId) {
        if !self.collapsed_collections.remove(&id) {
            self.collapsed_collections.insert(id);
        }
    }

    /// Navigates to a room, revealing its sidebar row if needed. Returns whether visibility changed.
    pub fn select_room(&mut self, state: &State, room: RoomKey) -> bool {
        if self.room != room {
            self.room = room;
            self.task = None;
            self.show_completed = false;
        }
        if let RoomKey::List(id) = room
            && let Some(list) = state.lists.get(&id)
            && let Some(collection) = list.collection
            && state.is_live(id.into())
        {
            self.collapsed_collections.remove(&collection)
        } else {
            false
        }
    }

    /// Drops deleted and unknown collections once the replica has data. Returns whether any were dropped.
    pub fn prune_collapsed_collections(&mut self, state: &State) -> bool {
        let before = self.collapsed_collections.len();
        self.collapsed_collections
            .retain(|&id| state.is_live(id.into()));
        self.collapsed_collections.len() != before
    }
}

/// What the sidebar footer says about sync.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncInfo {
    pub paired: bool,
    pub connection: Connection,
    pub pending: usize,
    pub failed: usize,
    /// The server rejected the device token: it needs pairing again.
    pub unauthorized: bool,
    /// When the client was last fully in sync.
    pub synced_at: Option<Timestamp>,
}

impl SyncInfo {
    /// No session (fixtures that show the app before pairing).
    pub fn signed_out() -> Self {
        Self {
            paired: false,
            connection: Connection::Offline,
            pending: 0,
            failed: 0,
            unauthorized: false,
            synced_at: None,
        }
    }
}

/// The line above the room title.
#[derive(Clone, Debug, PartialEq)]
pub enum Eyebrow {
    /// Smart lists: the weekday in italics, then the date and the week.
    Day {
        weekday: SharedString,
        date: SharedString,
        week: SharedString,
    },
    /// Collections and lists: an italic kind, then a mono detail.
    Kind {
        kind: SharedString,
        detail: SharedString,
    },
}

/// A list as a chip shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ListRef {
    pub collection: Option<SharedString>,
    pub name: SharedString,
    pub hue: Hue,
}

/// How a row's chip names its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chips {
    /// Inside a list: no chip.
    None,
    /// Inside a collection: the list.
    List,
    /// Smart lists: "Collection · List".
    CollectionAndList,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Due {
    None,
    /// Soon: accent colour.
    Soon(SharedString),
    Plain(SharedString),
    Overdue(SharedString),
}

impl Due {
    pub fn text(&self) -> Option<&SharedString> {
        match self {
            Self::None => None,
            Self::Soon(s) | Self::Plain(s) | Self::Overdue(s) => Some(s),
        }
    }
}

/// A task row.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub id: TaskId,
    pub title: SharedString,
    pub list: ListRef,
    pub tags: Vec<SharedString>,
    /// (done, total) steps, when it has steps.
    pub steps: Option<(u32, u32)>,
    pub agent: Option<Agent>,
    /// 0 to 3 exclamation marks.
    pub priority: u8,
    pub due: Due,
    pub from_mail: bool,
    pub done: bool,
}

/// The room the main pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub key: RoomKey,
    pub title: SharedString,
    pub eyebrow: Eyebrow,
    pub hue: Hue,
    pub chips: Chips,
    /// Header progress: done of total (see `derive` for what counts).
    pub done: u32,
    pub total: u32,
    /// The data has no estimates yet, so this stays `None` outside fixtures that show the layout.
    pub estimate: Option<SharedString>,
    /// Open tasks (plus tasks finishing their completion), in order.
    pub tasks: Vec<TaskRow>,
    /// The Completed section's rows (filled only when it is open).
    pub completed: Vec<TaskRow>,
    /// How many tasks the Completed section holds.
    pub completed_count: u32,
    /// For the empty header: tasks finished this month.
    pub finished_this_month: u32,
    /// For the status bar: tasks completed since local midnight, unfiltered.
    pub done_today: u32,
    pub empty_line: SharedString,
    /// The quieter line under `empty_line`: how to add a task here.
    pub empty_hint: SharedString,
    pub quick_add_placeholder: SharedString,
    /// My Day: finishing every task turns the header into the dusk scene.
    pub finishes_day: bool,
    /// What the finished day hands over to.
    pub tomorrow: Option<Tomorrow>,
    /// The finished day (not filtered): the next day's name ("Sunday") for *Plan Sunday*, even when
    /// nothing is due then and there is no hand-over.
    pub plan_weekday: Option<SharedString>,
    /// A filter is narrowing the rows.
    pub filtered: bool,
}

impl Room {
    /// My Day with every task done.
    pub fn finished_day(&self) -> bool {
        self.finishes_day && self.total > 0 && self.done >= self.total
    }

    /// What the finished day shows instead of rows: tomorrow's hand-over, unless the Completed section is
    /// open (then its rows show) or there is nothing to hand over.
    pub fn handover(&self) -> Option<&Tomorrow> {
        self.tomorrow
            .as_ref()
            .filter(|_| self.finished_day() && self.completed.is_empty())
    }

    /// The finished day with nothing due tomorrow: no hand-over, but planning the next day is still
    /// offered ("Nothing is due on Sunday yet." and *Plan Sunday*). The next day's name, or `None` when
    /// there's a hand-over, the Completed section is open, or a filter is narrowing the room.
    pub fn plan_only(&self) -> Option<&SharedString> {
        self.plan_weekday
            .as_ref()
            .filter(|_| self.tomorrow.is_none() && self.finished_day() && self.completed.is_empty())
    }

    /// (done, total, open) for the header.
    pub fn summary(&self) -> (u32, u32, u32) {
        (self.done, self.total, self.total.saturating_sub(self.done))
    }
}

/// The finished day's hand-over: tomorrow's first task, the rest of tomorrow and the rest of the week.
#[derive(Clone, Debug, PartialEq)]
pub struct Tomorrow {
    pub weekday: SharedString,
    pub first: TaskRow,
    pub first_when: SharedString,
    pub also: Vec<TaskRow>,
    pub week: Vec<TaskRow>,
    pub more: u32,
}

impl Tomorrow {
    /// Every task the hand-over shows, top to bottom.
    pub fn tasks(&self) -> impl Iterator<Item = &TaskRow> {
        std::iter::once(&self.first)
            .chain(self.also.iter())
            .chain(self.week.iter())
    }

    /// The hand-over's child (see `ShellView::tomorrow`) that shows task `id`: the first section label,
    /// tomorrow's first task, then each section's label followed by its tasks. Children enter in this
    /// order while dusk falls (`motion::DuskLook::handover`).
    pub fn child(&self, id: TaskId) -> Option<usize> {
        if self.first.id == id {
            return Some(1);
        }
        let mut child = 2;
        for section in [&self.also, &self.week] {
            if section.is_empty() {
                continue;
            }
            child += 1;
            if let Some(i) = section.iter().position(|t| t.id == id) {
                return Some(child + i);
            }
            child += section.len();
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavItem {
    pub smart: Smart,
    pub count: Option<u32>,
    /// "3 new" pip instead of a count.
    pub pip: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListNav {
    pub id: ListId,
    pub list: ListRef,
    pub glyph: Glyph,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectionNav {
    pub id: CollectionId,
    pub glyph: Glyph,
    pub name: SharedString,
    pub hue: Hue,
    pub count: u32,
    pub collapsed: bool,
    /// The collection itself is selected, or it hides the selected list.
    pub selected: bool,
    pub lists: Vec<ListNav>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    pub agent: Agent,
    pub name: SharedString,
    pub status: SharedString,
}

/// The footer's sync line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Problem,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sidebar {
    pub smart: Vec<NavItem>,
    pub collections: Vec<CollectionNav>,
    /// Lists outside any collection.
    pub lists: Vec<ListNav>,
    pub agents: Vec<AgentRow>,
    pub selected: RoomKey,
    pub sync: SharedString,
    pub tone: Tone,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepRow {
    pub id: StepId,
    pub title: SharedString,
    pub done: bool,
}

/// How a detail field's value is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Dates and times: mono.
    Date,
    Words,
    Tags,
    /// Not set: a quiet dash.
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub icon: Icon,
    pub label: &'static str,
    pub value: SharedString,
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkRow {
    pub id: LinkId,
    pub title: SharedString,
    pub detail: SharedString,
    /// What opening it opens.
    pub url: Option<SharedString>,
    pub email: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentRow {
    pub id: AttachmentId,
    /// The stored file name, exactly: Save and Open, and whether it can be opened, go by it.
    pub name: SharedString,
    /// The name as the pane draws it, on one line (`labels::one_line`).
    pub label: SharedString,
    pub detail: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub task: TaskId,
    pub list: ListRef,
    pub title: SharedString,
    pub subtitle: SharedString,
    pub done: bool,
    pub my_day: bool,
    pub important: bool,
    pub steps: Vec<StepRow>,
    pub notes: SharedString,
    pub fields: Vec<Field>,
    pub links: Vec<LinkRow>,
    pub attachments: Vec<AttachmentRow>,
}

/// Everything one window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub room: Room,
    pub sidebar: Sidebar,
    pub detail: Option<Detail>,
    /// Index of the selected row among [`Shell::rows`].
    pub selected_task: Option<usize>,
}

impl Shell {
    /// The rows keyboard navigation walks: open tasks, then the Completed section's when it is open.
    pub fn rows(&self) -> impl Iterator<Item = &TaskRow> {
        self.room.tasks.iter().chain(self.room.completed.iter())
    }

    pub fn row(&self, i: usize) -> Option<&TaskRow> {
        self.rows().nth(i)
    }

    pub fn row_count(&self) -> usize {
        self.room.tasks.len() + self.room.completed.len()
    }

    /// The tasks J and K walk, in order: the rows, or on the finished day (while it hands over to
    /// tomorrow) the hand-over's tasks.
    #[cfg(test)]
    pub fn nav_tasks(&self) -> Vec<TaskId> {
        self.nav_tasks_shown(|_| true)
    }

    /// [`Shell::nav_tasks`] that can be seen: while dusk falls, only the hand-over's children that have
    /// begun to enter (`entered(child)`, see [`Tomorrow::child`]), so J, K and Space never reach a task
    /// that isn't there yet.
    pub fn nav_tasks_shown(&self, entered: impl Fn(usize) -> bool) -> Vec<TaskId> {
        match self.room.handover() {
            Some(next) => next
                .tasks()
                .filter(|t| next.child(t.id).is_some_and(&entered))
                .map(|t| t.id)
                .collect(),
            None => self.rows().map(|r| r.id).collect(),
        }
    }

    /// Whether a row action still targets a row in the live room, not a snapshot or stale hitbox.
    pub fn has_row_in(&self, room: RoomKey, id: TaskId) -> bool {
        self.room.key == room && self.rows().any(|row| row.id == id)
    }

    /// Space can reach a visible row, or a hand-over task once its child has begun to enter. It can
    /// also reach the open detail's task when that has no row (after completing it with Completed
    /// collapsed, including the task that finished the day), so it does what the status bar offers.
    /// A closing detail is only a paint snapshot.
    pub fn reachable(
        &self,
        id: TaskId,
        row_visible: impl FnOnce() -> bool,
        entered: impl FnOnce(usize) -> bool,
    ) -> bool {
        if let Some(child) = self.room.handover().and_then(|next| next.child(id)) {
            entered(child) && row_visible()
        } else if self.has_row_in(self.room.key, id) {
            row_visible()
        } else {
            self.detail.as_ref().is_some_and(|d| d.task == id)
        }
    }

    /// The detail to paint and whether it may receive input. An outgoing detail never does.
    pub fn detail_layer<'a>(&'a self, closing: Option<&'a Detail>) -> Option<(&'a Detail, bool)> {
        match self.detail.as_ref() {
            Some(detail) => Some((detail, true)),
            None => closing.map(|detail| (detail, false)),
        }
    }
}
