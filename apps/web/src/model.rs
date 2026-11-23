//! The view-model: what each screen shows, derived from the replica's [`State`] and the clock. Pure
//! Rust with no browser calls, so it is unit-tested on the host (`cargo test`).
//!
//! | Function | Screen part |
//! |---|---|
//! | [`sidebar`] | smart lists, collections and their lists, with counts |
//! | [`room`] | a room's header (eyebrow, title, summary, sun progress) and its rows |
//! | [`detail`] | the task detail pane |
//!
//! Colours are never computed here: rooms and lists carry a hue *key* (`my-day`, `teal`, ...) that
//! the page puts in `data-room-hue` / `data-list-hue`, and `tokens.css` maps it to the palette.

use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Weekday};
use jiff::tz::{Offset, TimeZone};
use leptos::prelude::{Get, Memo, RwSignal, With};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use todo_client::activity::{agent_counts_today, agent_status, select_agent_rows};
use todo_client::date_format::{DateFormat, DateLabels, date_time};
use todo_client::intent::{
    self, TaskRoom,
    organize::{actionable_task, is_actionable, list_live},
    quick_add,
};
use todo_client::room_header::{self, LocalTime, RoomHeader, SmartList};
use todo_core::ids::{AttachmentId, CollectionId, LinkId, ListId, TaskId};
use todo_core::model::{Collection, LinkKind, List, Priority, Stamp, Task};
use todo_core::query::{BUILTIN_LISTS, BuiltinList, EvalContext, Query, fold};
use todo_core::{Actor, State};
use todo_design::Glyph;
use todo_design::hues::{self, SWATCHES};

/// "Now" for the view: the instant, the owner's local date and UTC offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    /// The instant.
    pub at: Timestamp,
    /// The UTC offset of local time, in seconds.
    pub offset_s: i32,
    /// Use the browser's offset at each instant rather than a fixture's fixed offset.
    pub browser_time: bool,
    pub date_format: DateFormat,
}

impl Now {
    /// The local civil date and time.
    #[must_use]
    pub fn local(&self) -> DateTime {
        self.civil_time_at(self.at)
    }

    /// The owner's wall time at `at`, with that instant's UTC offset.
    #[must_use]
    pub fn civil_time_at(&self, at: Timestamp) -> DateTime {
        self.local_time().civil_time_at(at)
    }

    /// Today's local date.
    #[must_use]
    pub fn today(&self) -> Date {
        self.local().date()
    }

    /// The query evaluation context.
    #[must_use]
    pub fn eval(&self) -> EvalContext {
        self.local_time().eval()
    }

    /// The clock as the shared room header takes it.
    #[must_use]
    pub fn local_time(&self) -> LocalTime {
        #[cfg(target_arch = "wasm32")]
        if self.browser_time {
            return LocalTime::with_offsets(self.at, crate::platform::utc_offset)
                .with_date_format(self.date_format);
        }
        let off = Offset::from_seconds(self.offset_s).unwrap_or(Offset::UTC);
        LocalTime::new(self.at, TimeZone::fixed(off)).with_date_format(self.date_format)
    }

    pub fn dates(&self) -> DateLabels {
        self.date_format.on(self.today())
    }
}

pub(crate) fn local(at: Timestamp, offset_s: i32) -> DateTime {
    let off = Offset::from_seconds(offset_s).unwrap_or(Offset::UTC);
    off.to_datetime(at)
}

// ---- rooms ---------------------------------------------------------------------------------------

/// What the main pane shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RoomId {
    /// A built-in smart list, by its ID (`my_day`, `important`, ...).
    Smart(&'static str),
    /// A collection: every list in it.
    Collection(CollectionId),
    /// One list.
    List(ListId),
    /// History, optionally narrowed to a single agent.
    Activity(Option<todo_core::actor::AgentId>),
    /// Explicitly deleted entities.
    Trash,
}

impl Default for RoomId {
    fn default() -> Self {
        RoomId::Smart("my_day")
    }
}

impl From<&RoomId> for TaskRoom {
    fn from(room: &RoomId) -> Self {
        match *room {
            RoomId::Smart("my_day") => Self::MyDay,
            RoomId::Smart("important") => Self::Important,
            RoomId::Smart("planned") => Self::Planned,
            RoomId::Smart(_) | RoomId::Activity(_) | RoomId::Trash => Self::Other,
            RoomId::Collection(id) => Self::Collection(id),
            RoomId::List(id) => Self::List(id),
        }
    }
}

impl RoomId {
    /// The app's path for this room (`/app`, `/app/important`, `/app/list/<id>`, ...).
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            RoomId::Smart("my_day") => "/app".to_owned(),
            RoomId::Smart(id) => format!("/app/{}", id.replace('_', "-")),
            RoomId::Collection(c) => format!("/app/collection/{c}"),
            RoomId::List(l) => format!("/app/list/{l}"),
            RoomId::Activity(None) => "/app/activity".to_owned(),
            RoomId::Trash => "/app/trash".to_owned(),
            RoomId::Activity(Some(agent)) => format!("/app/activity/agent/{agent}"),
        }
    }

    /// The room for a path, and the task open in it; unknown paths open My Day.
    #[must_use]
    pub fn from_path(path: &str) -> (RoomId, Option<TaskId>) {
        let rest = path.strip_prefix("/app").unwrap_or(path);
        let segs: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        let (room, used) = match segs.as_slice() {
            ["activity", "agent", agent] => {
                return (
                    RoomId::Activity(todo_core::actor::AgentId::new(*agent).ok()),
                    None,
                );
            }
            ["activity", ..] => return (RoomId::Activity(None), None),
            ["trash", ..] => return (RoomId::Trash, None),
            ["collection", id, ..] => (id.parse().map(RoomId::Collection).ok(), 2),
            ["list", id, ..] => (id.parse().map(RoomId::List).ok(), 2),
            ["task", ..] | [] => (Some(RoomId::default()), 0),
            [smart, ..] => (
                BUILTIN_LISTS
                    .iter()
                    .find(|b| b.id.replace('_', "-") == *smart)
                    .map(|b| RoomId::Smart(b.id)),
                1,
            ),
        };
        let Some(room) = room else {
            return (RoomId::default(), None);
        };
        let task = match &segs[used..] {
            ["task", id] => id.parse().ok(),
            _ => None,
        };
        (room, task)
    }

    /// Whether this room is the activity feed rather than a task list.
    pub fn is_activity(&self) -> bool {
        matches!(self, Self::Activity(_))
    }

    pub fn is_history(&self) -> bool {
        self.is_activity() || *self == Self::Trash
    }
}

fn builtin(id: &str) -> Option<&'static BuiltinList> {
    BUILTIN_LISTS.iter().find(|b| b.id == id)
}

/// The hue key of a smart list.
#[must_use]
pub fn smart_hue(id: &str) -> &'static str {
    match id {
        "my_day" => "my-day",
        "important" => "important",
        "planned" => "planned",
        "all" => "all",
        "completed" => "completed",
        _ => "agent-activity",
    }
}

/// The interface icon of a smart list.
#[must_use]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn smart_icon(id: &str) -> todo_design::Icon {
    use todo_design::Icon;
    match id {
        "my_day" => Icon::Sun,
        "important" => Icon::Star,
        "planned" => Icon::Calendar,
        "all" => Icon::List,
        "completed" => Icon::CircleCheck,
        _ => Icon::Sparkles,
    }
}

/// Short name of a smart list (the sidebar and pills drop "(7 days)").
#[must_use]
pub fn smart_name(id: &str) -> &'static str {
    match id {
        "agent_activity" => "Agent activity",
        other => builtin(other).map_or("My Day", |b| b.name),
    }
}

// ---- hues and glyphs -------------------------------------------------------------------------------

/// OKLCH hue of an sRGB hex colour (`#rrggbb`), if it has any chroma.
fn hex_hue(hex: &str) -> Option<f64> {
    let rgb = todo_design::Rgb::from_hex(hex)?;
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb.r), lin(rgb.g), lin(rgb.b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let a = 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s;
    if a.hypot(bb) < 0.02 {
        return None;
    }
    Some(bb.atan2(a).to_degrees().rem_euclid(360.0))
}

fn nearest_swatch(hue: f64) -> &'static str {
    SWATCHES
        .iter()
        .min_by(|x, y| {
            let d = |h: f64| {
                let d = (h - hue).rem_euclid(360.0);
                d.min(360.0 - d)
            };
            d(x.degrees).total_cmp(&d(y.degrees))
        })
        .map_or("teal", |s| s.key)
}

/// The swatch key for an entity's colour: a swatch key as is, a hex colour by nearest hue, otherwise
/// a stable pick from the ID.
#[must_use]
pub fn swatch(color: Option<&str>, seed: &[u8; 16]) -> &'static str {
    if let Some(c) = color {
        if let Some(s) = SWATCHES.iter().find(|s| s.key == c) {
            return s.key;
        }
        if let Some(h) = hex_hue(c) {
            return nearest_swatch(h);
        }
    }
    hues::swatch_for_seed(seed).key
}

// ---- visibility --------------------------------------------------------------------------------------

/// Live collections, in order.
#[must_use]
pub fn collections(state: &State) -> Vec<&Collection> {
    let mut v: Vec<&Collection> = state
        .collections
        .values()
        .filter(|c| c.deleted.is_none())
        .collect();
    v.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    v
}

/// Live lists of a collection (or the top-level ones), in order.
#[must_use]
pub fn lists_in(state: &State, collection: Option<CollectionId>) -> Vec<&List> {
    let mut v: Vec<&List> = state
        .lists
        .values()
        .filter(|l| l.collection == collection && list_live(state, l.id))
        .collect();
    v.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    v
}

// ---- sidebar -------------------------------------------------------------------------------------------

/// One sidebar entry.
#[derive(Clone, Debug, PartialEq)]
pub struct NavItem {
    /// Where it leads.
    pub room: RoomId,
    /// Label.
    pub name: String,
    /// Open tasks (completed tasks for Completed).
    pub count: usize,
    /// Hue key.
    pub hue: &'static str,
    /// For a list or collection: its glyph. Smart lists use an interface icon.
    pub glyph: Option<Glyph>,
}

/// A collection with its lists.
#[derive(Clone, Debug, PartialEq)]
pub struct NavGroup {
    /// The collection itself.
    pub head: NavItem,
    /// Its lists.
    pub lists: Vec<NavItem>,
}

/// The sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct Sidebar {
    /// The built-in smart lists.
    pub smart: Vec<NavItem>,
    /// Collections and their lists.
    pub groups: Vec<NavGroup>,
    /// Lists outside any collection.
    pub loose: Vec<NavItem>,
    /// Agents seen today and what they did.
    pub agents: Vec<(AgentMark, String)>,
    /// Live tasks whose latest change today was made by an agent.
    pub agent_updates_today: usize,
}

/// Builds the sidebar. `date_of` resolves a stamp with the local offset at its own instant.
#[must_use]
pub fn sidebar(state: &State, now: &Now, date_of: impl Fn(Timestamp) -> Date) -> Sidebar {
    let ctx = now.eval();
    let is_today = |at| date_of(at) == ctx.today;
    let smart = BUILTIN_LISTS
        .iter()
        .map(|b| NavItem {
            room: RoomId::Smart(b.id),
            name: smart_name(b.id).to_owned(),
            count: room_header::smart_list_count(state, *b, &ctx),
            hue: smart_hue(b.id),
            glyph: None,
        })
        .collect();
    let mut open_by_list: BTreeMap<ListId, usize> = BTreeMap::new();
    let mut agent_updates_today = 0;
    for t in state.tasks.values().filter(|t| is_actionable(state, t)) {
        if !t.is_done() {
            *open_by_list.entry(t.list).or_default() += 1;
        }
        if t.updated.by.is_agent() && is_today(t.updated.at) {
            agent_updates_today += 1;
        }
    }
    let item = |l: &List| NavItem {
        room: RoomId::List(l.id),
        name: l.name.clone(),
        count: open_by_list.get(&l.id).copied().unwrap_or(0),
        hue: swatch(l.color.as_deref(), l.id.as_uuid().as_bytes()),
        glyph: Some(Glyph::for_list(&fold(&l.name))),
    };
    let groups = collections(state)
        .into_iter()
        .map(|c| {
            let lists: Vec<NavItem> = lists_in(state, Some(c.id)).into_iter().map(item).collect();
            NavGroup {
                head: NavItem {
                    room: RoomId::Collection(c.id),
                    name: c.name.clone(),
                    count: lists.iter().map(|l| l.count).sum(),
                    hue: swatch(c.color.as_deref(), c.id.as_uuid().as_bytes()),
                    glyph: Some(Glyph::for_collection(&fold(&c.name))),
                },
                lists,
            }
        })
        .collect();
    let loose = lists_in(state, None).into_iter().map(item).collect();
    Sidebar {
        smart,
        groups,
        loose,
        agents: select_agent_rows(agent_counts_today(state, is_today))
            .filter_map(|(actor, added, edited)| {
                Some((agent_mark(actor)?, agent_status(added, edited)))
            })
            .collect(),
        agent_updates_today,
    }
}

// ---- rows ------------------------------------------------------------------------------------------------

/// How a due date reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DueTone {
    /// Past: a danger badge.
    Overdue,
    /// Today: accent.
    Soon,
    /// Later.
    Later,
}

/// A due date as a row shows it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Due {
    /// Text (`2d overdue`, `Today`, `Tomorrow`, `Mon 23 Nov`).
    pub text: String,
    /// Tone.
    pub tone: DueTone,
}

/// An agent's letter mark.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AgentMark {
    /// The letter (`C`, `X`, `π`).
    pub letter: String,
    /// The agent's key for `data-agent` (`claude`, `codex`, `pi`), or `None` for an unknown agent.
    pub key: Option<&'static str>,
    /// The agent's ID, for the accessible name.
    pub name: String,
}

/// A list chip on a row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chip {
    /// `Work · Finance`, or just the list name.
    pub label: String,
    /// The list's hue key.
    pub hue: &'static str,
}

/// One task row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    /// The task.
    pub id: TaskId,
    /// Title.
    pub title: String,
    /// Completed.
    pub done: bool,
    /// The list chip (not shown in the list's own room).
    pub chip: Option<Chip>,
    /// `#tag` names.
    pub tags: Vec<String>,
    /// Created from an email.
    pub email: bool,
    /// Steps done and total, when it has steps.
    pub steps: Option<(usize, usize)>,
    /// Priority as a count of `!` (0–3).
    pub priority: u8,
    /// Due date (or a start–due span in a timeline-enabled list).
    pub due: Option<Due>,
    /// The agent that created it.
    pub agent: Option<AgentMark>,
}

/// Agent letter mark for an actor, if it is an agent.
#[must_use]
pub fn agent_mark(actor: &Actor) -> Option<AgentMark> {
    let id = actor.agent_id()?.as_str();
    let (letter, key) = match id {
        "claude" => ("C".to_owned(), Some("claude")),
        "codex" => ("X".to_owned(), Some("codex")),
        "pi" => ("π".to_owned(), Some("pi")),
        other => (
            other
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default(),
            None,
        ),
    };
    Some(AgentMark {
        letter,
        key,
        name: id.to_owned(),
    })
}

fn weekday_long(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

#[cfg(target_arch = "wasm32")]
pub fn core_weekday(w: todo_core::time::Weekday) -> &'static str {
    use todo_core::time::Weekday as W;
    match w {
        W::Mon => "Monday",
        W::Tue => "Tuesday",
        W::Wed => "Wednesday",
        W::Thu => "Thursday",
        W::Fri => "Friday",
        W::Sat => "Saturday",
        W::Sun => "Sunday",
    }
}

/// The parsed date displayed in the quick-add chip, using the shared client format.
#[cfg(test)]
pub fn date_preview(text: &str, now: &Now) -> Option<String> {
    let eval = now.eval();
    quick_add::parse(text, &eval)
        .due
        .map(|d| now.dates().weekday(d))
}

/// An empty due-date edit clears the date; invalid text is distinct from clearing it.
pub fn due_edit(text: &str, now: &Now) -> Result<Option<Date>, ()> {
    if text.trim().is_empty() {
        Ok(None)
    } else {
        quick_add::due_text(text, &now.eval()).map(Some).ok_or(())
    }
}

fn date_label(d: Date, dates: DateLabels) -> String {
    match (d - dates.today).get_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        _ => dates.weekday(d),
    }
}

pub fn due(d: Date, dates: DateLabels) -> Due {
    let days = (d - dates.today).get_days();
    let (text, tone) = match days {
        i32::MIN..=-1 => (format!("{}d overdue", -days), DueTone::Overdue),
        0 => ("Today".to_owned(), DueTone::Soon),
        1 => ("Tomorrow".to_owned(), DueTone::Later),
        _ => (dates.weekday(d), DueTone::Later),
    };
    Due { text, tone }
}

pub(crate) fn priority_marks(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

/// Which chip a row shows inside a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChipMode {
    /// Collection and list.
    Full,
    /// The list only (inside a collection).
    ListOnly,
    /// None (the list's own room).
    Hidden,
}

#[cfg(target_arch = "wasm32")]
pub fn board_row(state: &State, id: TaskId, dates: DateLabels) -> Option<Row> {
    let task = state.tasks.get(&id)?;
    let mut row = row(state, task, dates, ChipMode::Hidden);
    row.agent = agent_mark(&task.updated.by).or(row.agent);
    Some(row)
}

fn row(state: &State, t: &Task, dates: DateLabels, mode: ChipMode) -> Row {
    let chip = state.lists.get(&t.list).and_then(|l| {
        let hue = swatch(l.color.as_deref(), l.id.as_uuid().as_bytes());
        match mode {
            ChipMode::Hidden => None,
            ChipMode::ListOnly => Some(Chip {
                label: l.name.clone(),
                hue,
            }),
            ChipMode::Full => {
                let coll = l
                    .collection
                    .and_then(|c| state.collections.get(&c))
                    .map(|c| c.name.as_str());
                Some(Chip {
                    label: coll.map_or_else(|| l.name.clone(), |c| format!("{c} · {}", l.name)),
                    hue,
                })
            }
        }
    });
    let mut tags: Vec<String> = t
        .tags
        .iter()
        .filter_map(|id| state.tags.get(id))
        .filter(|tag| tag.deleted.is_none())
        .map(|tag| tag.name.clone())
        .collect();
    tags.sort();
    let steps: Vec<_> = state
        .steps
        .values()
        .filter(|s| s.task == t.id && s.deleted.is_none())
        .collect();
    let email = state
        .links
        .values()
        .any(|l| l.task == t.id && l.deleted.is_none() && matches!(l.kind, LinkKind::Email(_)));
    Row {
        id: t.id,
        title: t.title.clone(),
        done: t.is_done(),
        chip,
        tags,
        email,
        steps: (!steps.is_empty()).then(|| {
            (
                steps.iter().filter(|s| s.completed_at.is_some()).count(),
                steps.len(),
            )
        }),
        priority: priority_marks(t.priority),
        due: if t.is_done() {
            None
        } else {
            t.due.map(|d| {
                let mut label = due(d, dates);
                if label.tone != DueTone::Overdue
                    && state
                        .lists
                        .get(&t.list)
                        .is_some_and(|l| l.features.timeline)
                    && let Some(start) = t.start_date
                {
                    label.text = dates.range(start, d);
                }
                label
            })
        },
        agent: agent_mark(&t.created.by),
    }
}

// ---- rooms -------------------------------------------------------------------------------------------------

/// A room, ready to render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// Which room.
    pub id: RoomId,
    /// Title.
    pub title: String,
    /// Hue key for `data-room-hue`.
    pub hue: &'static str,
    /// The header: eyebrow, the sun's progress, the summary and the status bar's counts, as every client
    /// derives them (`todo_client::room_header`).
    pub header: RoomHeader,
    /// Open rows, in order (for Completed: the completed tasks).
    pub rows: Vec<Row>,
    /// Completed rows, most recent first (not for Completed itself).
    pub completed: Vec<Row>,
    /// The quick-add placeholder, or `None` if the field is hidden in this room.
    pub add_placeholder: Option<String>,
    /// A query is narrowing the rows, but does not change the room's progress.
    pub filtered: bool,
}

impl Room {
    /// The design-language copy for this empty room or filter.
    pub fn empty_line(&self) -> &'static str {
        self.header.empty_line(self.filtered)
    }

    /// My Day with everything done: the dusk scene.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.header.finished_day()
    }

    /// The header summary (`3 of 12 done · 9 open`), as screen readers and tests read it.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn summary(&self) -> String {
        self.header.summary.text()
    }
}

/// The shared room key for a room.
#[must_use]
pub fn shared_key(id: &RoomId) -> room_header::RoomKey<'_> {
    match id {
        RoomId::Smart(s) => {
            room_header::RoomKey::Smart(SmartList::from_id(s).unwrap_or(SmartList::MyDay))
        }
        RoomId::Collection(c) => room_header::RoomKey::Collection(*c),
        RoomId::List(l) => room_header::RoomKey::List(*l),
        RoomId::Trash => room_header::RoomKey::Trash,
        RoomId::Activity(agent) => {
            room_header::RoomKey::Activity(agent.as_ref().map(|a| a.as_str()))
        }
    }
}

/// Keep room membership and the header independent of the filter signal. Editing filter text
/// searches only the already derived sections, not the whole replica for the room scope again.
pub fn room_memos(
    state: RwSignal<Arc<State>>,
    now: RwSignal<Now>,
    id: RwSignal<RoomId>,
    holds: Memo<BTreeMap<TaskId, Stamp>>,
    text: RwSignal<String>,
) -> (Memo<Arc<Room>>, Memo<Room>) {
    let unfiltered = Memo::new(move |_| {
        let now = now.get();
        let id = id.get();
        holds.with(|updates| {
            let held = updates.keys().copied().collect();
            state.with(|s| Arc::new(unfiltered_room(s, &now, &id, &held, updates)))
        })
    });
    let filtered = Memo::new(move |_| {
        unfiltered.with(|room| {
            text.with(|text| {
                holds
                    .with(|updates| state.with(|s| filter_room(room, s, &now.get(), text, updates)))
            })
        })
    });
    (unfiltered, filtered)
}

#[cfg(test)]
fn room(state: &State, now: &Now, id: &RoomId, filter: &str, holding: &BTreeSet<TaskId>) -> Room {
    let unfiltered = unfiltered_room(state, now, id, holding, &BTreeMap::new());
    filter_room(&unfiltered, state, now, filter, &BTreeMap::new())
}

fn unfiltered_room(
    state: &State,
    now: &Now,
    id: &RoomId,
    holding: &BTreeSet<TaskId>,
    updates: &BTreeMap<TaskId, Stamp>,
) -> Room {
    let time = now.local_time();
    let ctx = time.eval();
    let shared = shared_key(id);
    let tasks = room_header::room_tasks_with_updates(state, shared, holding, updates, &ctx);
    let header = room_header::header_of(state, shared, &tasks, holding, &time);
    let (title, hue, mode, add) = match id {
        RoomId::Trash => (
            todo_client::trash::TITLE.into(),
            "agent-activity",
            ChipMode::Hidden,
            None,
        ),
        RoomId::Activity(_) => (
            "Activity".to_owned(),
            "agent-activity",
            ChipMode::Hidden,
            None,
        ),
        RoomId::Smart(s) => (
            smart_name(s).to_owned(),
            smart_hue(s),
            ChipMode::Full,
            (*s != "completed").then(|| "Add a task…".to_owned()),
        ),
        RoomId::Collection(c) => {
            let coll = state.collections.get(c);
            let lists = lists_in(state, Some(*c));
            (
                coll.map_or_else(String::new, |c| c.name.clone()),
                coll.map_or("teal", |c| {
                    swatch(c.color.as_deref(), c.id.as_uuid().as_bytes())
                }),
                ChipMode::ListOnly,
                coll.filter(|c| c.deleted.is_none()).map(|_| {
                    lists.first().map_or_else(
                        || "Add a task…".to_owned(),
                        |l| format!("Add to {}…", l.name),
                    )
                }),
            )
        }
        RoomId::List(l) => {
            let list = state.lists.get(l).filter(|l| list_live(state, l.id));

            (
                list.map_or_else(String::new, |l| l.name.clone()),
                list.map_or("teal", |l| {
                    swatch(l.color.as_deref(), l.id.as_uuid().as_bytes())
                }),
                ChipMode::Hidden,
                list.map(|_| "Add a task…".to_owned()),
            )
        }
    };
    let to_rows = |tasks: Vec<&Task>| {
        tasks
            .into_iter()
            .map(|t| row(state, t, now.dates(), mode))
            .collect()
    };
    Room {
        id: id.clone(),
        title,
        hue,
        header,
        rows: to_rows(tasks.open),
        completed: to_rows(tasks.done),
        add_placeholder: add,
        filtered: false,
    }
}

fn filter_room(
    room: &Room,
    state: &State,
    now: &Now,
    text: &str,
    updates: &BTreeMap<TaskId, Stamp>,
) -> Room {
    let text = text.trim();
    if text.is_empty() || room.id.is_history() {
        return room.clone();
    }
    with_task_filter(state, now.eval(), text, Some(updates), |matches| {
        let matches = |row: &&Row| state.tasks.get(&row.id).is_some_and(matches);
        Room {
            id: room.id.clone(),
            title: room.title.clone(),
            hue: room.hue,
            header: room.header.clone(),
            rows: room.rows.iter().filter(matches).cloned().collect(),
            completed: room.completed.iter().filter(matches).cloned().collect(),
            add_placeholder: room.add_placeholder.clone(),
            filtered: true,
        }
    })
}

fn with_task_filter<R>(
    state: &State,
    ctx: EvalContext,
    text: &str,
    updates: Option<&BTreeMap<TaskId, Stamp>>,
    run: impl FnOnce(&dyn Fn(&Task) -> bool) -> R,
) -> R {
    let query = Query::parse(text);
    let matcher = query.as_ref().ok().map(|q| q.matcher(state, &ctx));
    // Like Desktop, keep free-text search useful while entering an incomplete query.
    let folded = matcher.is_none().then(|| fold(text));
    run(&|task| {
        if let Some(matcher) = &matcher {
            room_header::matches_with_held_update(
                matcher,
                task,
                updates.and_then(|m| m.get(&task.id)),
            )
        } else {
            let title = fold(&task.title);
            folded
                .as_deref()
                .unwrap_or_default()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .all(|word| title.contains(word))
        }
    })
}

#[cfg(any(target_arch = "wasm32", test))]
pub fn timeline_room(
    state: &State,
    room: &RoomId,
    now: Now,
    text: &str,
    holding: &BTreeMap<TaskId, Stamp>,
) -> Option<todo_client::timeline::Timeline> {
    let held = holding.keys().copied().collect();
    let ctx = now.eval();
    let mut timeline = todo_client::timeline::room_with_held(state, shared_key(room), &ctx, &held)?;
    timeline.unscheduled.retain(|id| !state.tasks[id].is_done());
    if !text.trim().is_empty() {
        with_task_filter(state, ctx, text.trim(), Some(holding), |matches| {
            timeline.lanes.retain(|l| matches(&state.tasks[&l.task]));
            timeline.unscheduled.retain(|id| matches(&state.tasks[id]));
        });
    }
    Some(timeline)
}

// ---- tomorrow (the finished day) ---------------------------------------------------------------------------

/// What the finished My Day hands over to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tomorrow {
    /// Tomorrow's weekday (`Sunday`).
    pub weekday: String,
    /// The first task due tomorrow, with its date text.
    pub first: Option<(Row, String)>,
    /// Other tasks due tomorrow.
    pub also: Vec<Row>,
    /// Tasks due later this week.
    pub week: Vec<Row>,
    /// How many more are due this week beyond `week`.
    pub more: usize,
}

impl Tomorrow {
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.first
            .iter()
            .map(|(row, _)| row)
            .chain(&self.also)
            .chain(&self.week)
    }
}

/// Tomorrow's plan, shown when My Day is finished. Local exits stay in place until they finish.
#[must_use]
pub fn tomorrow(
    state: &State,
    now: &Now,
    completing: impl Fn(TaskId) -> bool,
    departing: impl Fn(TaskId) -> bool,
) -> Tomorrow {
    let today = now.today();
    let tomorrow = today.tomorrow().unwrap_or(today);
    let mut upcoming: Vec<&Task> = state
        .tasks
        .values()
        .filter(|t| {
            (is_actionable(state, t) || departing(t.id))
                && (!t.is_done() || completing(t.id))
                && t.due.is_some_and(|d| d > today)
        })
        .collect();
    upcoming.sort_by(|a, b| a.due.cmp(&b.due).then(a.position.cmp(&b.position)));
    let week_end = today
        .checked_add(jiff::Span::new().days(7))
        .unwrap_or(today);
    let mut next = upcoming.iter().filter(|t| t.due == Some(tomorrow));
    let first = next.next().map(|t| {
        (
            row(state, t, now.dates(), ChipMode::Full),
            now.dates().weekday(tomorrow),
        )
    });
    let also = next
        .map(|t| row(state, t, now.dates(), ChipMode::ListOnly))
        .collect();
    let week_all: Vec<&&Task> = upcoming
        .iter()
        .filter(|t| t.due.is_some_and(|d| d > tomorrow && d <= week_end))
        .collect();
    let week = week_all
        .iter()
        .take(3)
        .map(|t| row(state, t, now.dates(), ChipMode::ListOnly))
        .collect();
    Tomorrow {
        weekday: weekday_long(tomorrow.weekday()).to_owned(),
        first,
        also,
        week,
        more: week_all.len().saturating_sub(3),
    }
}

// ---- detail ---------------------------------------------------------------------------------------------------

/// A step in the detail pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepView {
    /// The step.
    pub id: todo_core::ids::StepId,
    /// Title.
    pub title: String,
    /// Done.
    pub done: bool,
}

/// A source of the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// An email: subject and `sender · received`.
    Email {
        /// Subject.
        subject: String,
        /// `dana@example.com · Fri 20 Nov, 16:48`.
        meta: String,
        /// A web link to the message, if any.
        href: Option<String>,
    },
    /// A web page.
    Web {
        /// The link to remove.
        id: LinkId,
        /// Title or URL.
        title: String,
        /// The URL (`http` or `https` only).
        href: String,
    },
}

/// An attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentView {
    /// The attachment ID (for download and removal).
    pub id: AttachmentId,
    /// File name.
    pub name: String,
    /// `1.2 MB`.
    pub size: String,
}

/// The detail pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    /// The task.
    pub id: TaskId,
    /// Title.
    pub title: String,
    /// Done.
    pub done: bool,
    /// The list chip (`Work`) and the list name (`Planning`).
    pub crumb: (Option<Chip>, String),
    /// `Created 14 Nov / Edited today 09:12`.
    pub stamps: String,
    /// Steps, in order.
    pub steps: Vec<StepView>,
    /// Notes.
    pub notes: String,
    /// Due date, shown relative to the local clock when appropriate.
    pub due: Option<(Date, String)>,
    /// The reminder shown and edited by the detail row.
    pub reminder: Option<intent::reminder::Seen>,
    /// The repeat rule.
    pub repeat: String,
    /// Attached tags in display order.
    pub tags: Vec<Chip>,
    /// Email and web sources.
    pub sources: Vec<Source>,
    /// Attachments.
    pub attachments: Vec<AttachmentView>,
    /// In My Day today.
    pub my_day: bool,
    /// The task's priority; high is also shown as Important.
    pub priority: Priority,
    /// The agent that created it.
    pub agent: Option<AgentMark>,
    /// What happened to it, newest first.
    pub activity: Vec<todo_client::task_activity::Line>,
}

/// An agent's display name (`claude` → `Claude`, `pi` → `Pi`).
#[must_use]
pub fn agent_display(id: &str) -> String {
    todo_client::activity::agent_name(id)
}

fn clock(dt: DateTime) -> String {
    format!("{:02}:{:02}", dt.hour(), dt.minute())
}

fn when(at: Timestamp, now: &Now) -> String {
    let dt = now.civil_time_at(at);
    let today = now.today();
    let d = dt.date();
    if d == today {
        date_time("today", &clock(dt))
    } else if Some(d) == today.yesterday().ok() {
        date_time("yesterday", &clock(dt))
    } else {
        now.dates().full(d)
    }
}

/// `Sat 21.11. 17:00` or `Today, 15:30`.
fn long_when(at: Timestamp, now: &Now) -> String {
    local_date_time(now.civil_time_at(at), now.dates())
}

/// A civil date and time, relative to today.
pub(crate) fn local_date_time(dt: DateTime, dates: DateLabels) -> String {
    let day = date_label(dt.date(), dates);
    date_time(&day, &clock(dt))
}

/// The repeat rule in words.
#[must_use]
pub fn repeat_text(rule: Option<&todo_core::recurrence::Recurrence>, dates: DateLabels) -> String {
    rule.map_or_else(
        || "Does not repeat".to_owned(),
        |r| intent::repeat::describe_repeat(r, dates),
    )
}

fn size_text(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} kB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

/// The detail pane for a task, if it is live.
#[must_use]
pub fn detail(state: &State, now: &Now, id: TaskId) -> Option<Detail> {
    let t = actionable_task(state, id)?;
    let today = now.today();
    let list = state.lists.get(&t.list)?;
    let coll = list.collection.and_then(|c| state.collections.get(&c));
    let crumb_chip = coll.map(|c| Chip {
        label: c.name.clone(),
        hue: swatch(c.color.as_deref(), c.id.as_uuid().as_bytes()),
    });
    let mut steps: Vec<_> = state
        .steps
        .values()
        .filter(|s| s.task == id && s.deleted.is_none())
        .collect();
    steps.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    let reminder = intent::reminder::Seen::of(state, id, &now.eval());
    let mut tags: Vec<_> = t
        .tags
        .iter()
        .filter_map(|tid| state.tags.get(tid))
        .filter(|tag| tag.deleted.is_none())
        .collect();
    tags.sort_by_cached_key(|tag| (fold(&tag.name), tag.id));
    let tags = tags
        .into_iter()
        .map(|tag| Chip {
            label: tag.name.clone(),
            hue: swatch(tag.color.as_deref(), tag.id.as_uuid().as_bytes()),
        })
        .collect();
    let sources = state
        .links
        .values()
        .filter(|l| l.task == id && l.deleted.is_none())
        .filter_map(|l| match &l.kind {
            LinkKind::Email(e) => Some(Source::Email {
                subject: e.subject.clone(),
                meta: format!("{} · {}", e.sender, long_when(e.received_at, now)),
                href: e
                    .web_link
                    .clone()
                    .filter(|u| u.starts_with("https://") || u.starts_with("http://")),
            }),
            LinkKind::Web { url, title } => {
                (url.starts_with("https://") || url.starts_with("http://")).then(|| Source::Web {
                    id: l.id,
                    title: title.clone().unwrap_or_else(|| url.clone()),
                    href: url.clone(),
                })
            }
        })
        .collect();
    let attachments = state
        .attachments
        .values()
        .filter(|a| a.task == id && a.deleted.is_none())
        .map(|a| AttachmentView {
            id: a.id,
            name: a.file_name.clone(),
            size: size_text(a.size_bytes),
        })
        .collect();
    let edited = if t.updated.at > t.created.at {
        format!(" / Edited {}", when(t.updated.at, now))
    } else {
        String::new()
    };
    Some(Detail {
        id,
        title: t.title.clone(),
        done: t.is_done(),
        crumb: (crumb_chip, list.name.clone()),
        stamps: {
            let dt = now.civil_time_at(t.created.at);
            let stamp = date_time(&now.dates().full(dt.date()), &clock(dt));
            format!("Created {stamp}{edited}")
        },
        steps: steps
            .into_iter()
            .map(|s| StepView {
                id: s.id,
                title: s.title.clone(),
                done: s.completed_at.is_some(),
            })
            .collect(),
        notes: t.notes.clone(),
        due: t.due.map(|d| (d, date_label(d, now.dates()))),
        reminder,
        repeat: repeat_text(t.recurrence.as_ref(), now.dates()),
        tags,
        sources,
        attachments,
        my_day: t.my_day == Some(today),
        priority: t.priority,
        agent: agent_mark(&t.created.by),
        activity: todo_client::task_activity::lines(state, id, now.at),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::fixtures;
    use todo_core::Command;

    fn now() -> Now {
        fixtures::now()
    }

    #[test]
    fn rooms_round_trip_through_paths() {
        let s = fixtures::state();
        let list = *s.lists.keys().next().unwrap();
        let coll = *s.collections.keys().next().unwrap();
        for r in [
            RoomId::Smart("my_day"),
            RoomId::Smart("agent_activity"),
            RoomId::Smart("completed"),
            RoomId::Activity(None),
            RoomId::Trash,
            RoomId::Activity(Some(
                todo_core::actor::AgentId::new("sample_agent").unwrap(),
            )),
            RoomId::List(list),
            RoomId::Collection(coll),
        ] {
            assert_eq!(
                RoomId::from_path(&r.path()),
                (r.clone(), None),
                "{}",
                r.path()
            );
        }
        let task = *s.tasks.keys().next().unwrap();
        let p = format!("{}/task/{task}", RoomId::List(list).path());
        assert_eq!(RoomId::from_path(&p), (RoomId::List(list), Some(task)));
        let p = format!("{}/task/{task}", RoomId::Smart("my_day").path());
        assert_eq!(RoomId::from_path(&p), (RoomId::Smart("my_day"), Some(task)));
        let p = format!("{}/task/{task}", RoomId::Smart("planned").path());
        assert_eq!(
            RoomId::from_path(&p),
            (RoomId::Smart("planned"), Some(task))
        );
        assert_eq!(RoomId::from_path("/app/nope"), (RoomId::default(), None));
        assert_eq!(
            RoomId::from_path("/app/list/not-a-uuid"),
            (RoomId::default(), None)
        );
    }

    #[test]
    fn malformed_activity_routes_open_unfiltered_activity() {
        for path in [
            "/app/activity/agent/INVALID!",
            "/app/activity/agent/",
            "/app/activity/agent/writer/extra",
            "/app/activity/list/extra",
        ] {
            assert_eq!(
                RoomId::from_path(path),
                (RoomId::Activity(None), None),
                "{path}"
            );
        }
    }

    #[test]
    fn web_routes_map_to_shared_task_rooms() {
        let collection = CollectionId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
        let list = ListId::from_uuid(todo_core::ids::uuid_v7(2, [0; 10]));
        for (path, expected) in [
            ("/app".into(), TaskRoom::MyDay),
            ("/app/important".into(), TaskRoom::Important),
            ("/app/planned".into(), TaskRoom::Planned),
            ("/app/all".into(), TaskRoom::Other),
            ("/app/completed".into(), TaskRoom::Other),
            ("/app/agent-activity".into(), TaskRoom::Other),
            (
                format!("/app/collection/{collection}"),
                TaskRoom::Collection(collection),
            ),
            (format!("/app/list/{list}"), TaskRoom::List(list)),
        ] {
            let (room, _) = RoomId::from_path(&path);
            assert_eq!(TaskRoom::from(&room), expected, "{path}");
        }
        assert_eq!(TaskRoom::from(&RoomId::Smart("unknown")), TaskRoom::Other);
    }

    #[test]
    fn quick_add_from_a_web_route_matches_the_shared_intent() {
        use todo_client::intent;
        use todo_core::command::{Command, Placement};

        let state = State::new();
        let route = RoomId::from_path("/app").0;
        let clock = now();
        assert!(
            room(&state, &clock, &route, "", &BTreeSet::new())
                .add_placeholder
                .is_some()
        );
        let ids = [0, 1].map(|n| todo_core::ids::uuid_v7(1_790_000_000_000 + n, [0; 10]));
        let build = |target: TaskRoom| {
            let mut ids = ids.into_iter();
            intent::quick_add_task(
                &state,
                target,
                "Pay the invoice by fri !!",
                &clock.eval(),
                || ids.next().expect("ID"),
            )
        };
        let result = build((&route).into());
        assert_eq!(result, build(TaskRoom::MyDay));
        let [
            Command::CreateList { color: Some(_), .. },
            Command::CreateTask(task),
        ] = result.1.as_slice()
        else {
            panic!("quick add on a fresh account creates a coloured default list then a task");
        };
        assert_eq!(task.title, "Pay the invoice");
        assert_eq!(task.due, Some("2026-11-27".parse().expect("date")));
        assert_eq!(task.priority, Priority::Normal);
        assert_eq!(task.placement, Placement::First);
    }

    #[test]
    fn date_preview_and_due_editor_have_distinct_rules() {
        let n = now();
        assert_eq!(
            date_preview("Mail it zítra !!", &n).as_deref(),
            Some("Sun 22.11.")
        );
        assert_eq!(
            date_preview("Mail it today", &n).as_deref(),
            Some("Sat 21.11.")
        );
        assert_eq!(
            date_preview("Mail it 2027-01-04", &n).as_deref(),
            Some("Mon 04.01.2027")
        );
        for text in ["tomorrow", r"See you \tomorrow", "Read on 2026-11-20"] {
            assert_eq!(date_preview(text, &n), None, "{text}");
        }
        assert_eq!(due_edit("  ", &n), Ok(None));
        assert_eq!(due_edit("not a date", &n), Err(()));
        assert_eq!(due_edit("fri", &n), Ok(Some("2026-11-27".parse().unwrap())));
        let past = due_edit("2026-11-20", &n).unwrap();
        let mut s = fixtures::state();
        let id = fixtures::task(1);
        assert!(s.tasks[&id].recurrence.is_some());
        let owner = Actor::owner("web").unwrap();
        for date in [past, None] {
            let cmd = intent::set_due(&s, id, date).unwrap();
            for event in todo_core::decide(&s, &owner, &cmd, n.at).unwrap() {
                todo_core::apply(&mut s, &event).unwrap();
            }
            assert_eq!(detail(&s, &n, id).unwrap().due.map(|d| d.0), date);
        }
        assert!(s.tasks[&id].recurrence.is_none());
    }

    #[test]
    fn restoring_a_merge_source_leaves_the_my_day_badge_equal_to_the_room() {
        let mut s = fixtures::state();
        let n = now();
        let id = RoomId::Smart("my_day");
        let (source, target) = (fixtures::task(3), fixtures::task(6));
        for task in [source, target] {
            s.tasks.get_mut(&task).unwrap().my_day = Some(n.today());
        }
        let before = room(&s, &n, &id, "", &BTreeSet::new()).header.open();
        let owner = Actor::owner("web").unwrap();
        for command in [
            Command::MergeTasks { source, target },
            Command::Restore { id: source.into() },
        ] {
            for event in todo_core::decide(&s, &owner, &command, n.at).unwrap() {
                todo_core::apply(&mut s, &event).unwrap();
            }
        }
        let after = room(&s, &n, &id, "", &BTreeSet::new());
        assert_eq!(after.header.open(), before - 1);
        let sidebar = sidebar(&s, &n, |at| n.local_time().date_of(at));
        let badge = sidebar.smart.iter().find(|item| item.room == id).unwrap();
        assert_eq!(badge.count, after.header.open());
    }

    #[test]
    fn my_day_counts_match_the_mockup_story() {
        let s = fixtures::state();
        let r = room(&s, &now(), &RoomId::Smart("my_day"), "", &BTreeSet::new());
        assert_eq!((r.header.progress_done, r.header.progress_total), (3, 12));
        assert_eq!(r.rows.len(), 9);
        assert_eq!(r.summary(), "3 of 12 done · 9 open");
        assert_eq!(r.header.eyebrow.text(), "Saturday · 21.11. · W47");
        assert_eq!(
            r.header.status.map(|s| s.text()).as_deref(),
            Some("9 open · 3 done today")
        );
        assert!(!r.finished());
        assert!(r.add_placeholder.is_some());
    }

    #[test]
    fn typing_a_filter_reuses_the_unfiltered_room_and_header_in_a_large_replica() {
        use leptos::prelude::{Owner, Set};
        Owner::new().with(|| {
            let mut state = fixtures::state();
            let mut archived = state.tasks[&fixtures::task(11)].clone();
            archived.my_day = None;
            for n in 100..5100 {
                archived.id = fixtures::task(n);
                state.tasks.insert(archived.id, archived.clone());
            }
            let state = RwSignal::new(Arc::new(state));
            let now = RwSignal::new(now());
            let id = RwSignal::new(RoomId::default());
            let text = RwSignal::new(String::new());
            let (base, view) = room_memos(state, now, id, Memo::new(|_| BTreeMap::new()), text);
            let cached = base.get();
            for input in ["tag:d", "tag:docs", "done ondrej", "nonexistent", ""] {
                text.set(input.to_owned());
                let visible = view.get();
                assert_eq!(visible.header, cached.header);
                assert!(
                    Arc::ptr_eq(&base.get(), &cached),
                    "filter {input:?} rebuilt the room"
                );
            }
            id.set(RoomId::Smart("important"));
            assert!(
                !Arc::ptr_eq(&base.get(), &cached),
                "navigation invalidates membership"
            );
        });
    }

    #[test]
    fn a_room_filter_uses_queries_and_folded_text_without_changing_progress() {
        let s = fixtures::state();
        let filter = |text| room(&s, &now(), &RoomId::default(), text, &BTreeSet::new());
        assert_eq!(filter("tag:docs").rows[0].id, fixtures::task(1));
        assert_eq!(filter("KVULI STK").rows[0].id, fixtures::task(4));
        assert_eq!(filter("(kvuli").rows[0].id, fixtures::task(4));
        let done = filter("done ondrej");
        let ondrej = fixtures::task(11);
        assert_eq!(done.completed[0].id, ondrej);
        assert!(done.rows.is_empty());
        let held = room(
            &s,
            &now(),
            &RoomId::default(),
            "done ondrej",
            &BTreeSet::from([ondrej]),
        );
        assert!(held.completed.is_empty());
        assert_eq!(
            held.rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![ondrej]
        );
        assert_eq!(
            (held.header.progress_done, held.header.progress_total),
            (2, 12)
        );
        assert_eq!(
            (done.header.progress_done, done.header.progress_total),
            (3, 12)
        );
        let no_match = filter("tag:unknown");
        assert!(no_match.rows.is_empty() && no_match.completed.is_empty());
        assert_eq!(no_match.empty_line(), "Nothing matches the filter.");
        assert!(!filter("  ").filtered);
        assert!(
            filter("standing desk").rows.is_empty(),
            "other rooms are not searched"
        );
    }

    #[test]
    fn empty_smart_lists_use_the_desktop_copy() {
        let s = State::new();
        for (id, copy) in [
            ("my_day", "Nothing planned yet."),
            ("important", "Nothing pressing."),
            ("planned", "Nothing on the calendar."),
            ("all", "Nothing open."),
            ("completed", "Nothing finished yet."),
            ("agent_activity", "No agent changes this week."),
        ] {
            let empty = room(&s, &now(), &RoomId::Smart(id), "", &BTreeSet::new());
            assert!(empty.rows.is_empty());
            assert_eq!(empty.empty_line(), copy, "{id}");
        }
    }

    #[test]
    fn overdue_and_today_rows_read_right() {
        let dates = now().dates();
        let d = |n: i64| dates.today.checked_add(jiff::Span::new().days(n)).unwrap();
        assert_eq!(
            due(d(-2), dates),
            Due {
                text: "2d overdue".into(),
                tone: DueTone::Overdue
            }
        );
        assert_eq!(due(d(0), dates).tone, DueTone::Soon);
        assert_eq!(due(d(1), dates).text, "Tomorrow");
        assert_eq!(due(d(2), dates).text, "Mon 23.11.");
        assert_eq!(
            dates.weekday(Date::new(2027, 1, 4).unwrap()),
            "Mon 04.01.2027"
        );
    }

    #[test]
    fn chips_follow_the_room() {
        let s = fixtures::state();
        let n = now();
        let my_day = room(&s, &n, &RoomId::Smart("my_day"), "", &BTreeSet::new());
        let vat = my_day
            .rows
            .iter()
            .find(|r| r.title == "Pay the December supplier invoices")
            .unwrap();
        assert_eq!(vat.chip.as_ref().unwrap().label, "Work · Finance");
        assert_eq!(vat.priority, 3);
        assert_eq!(vat.due.as_ref().unwrap().tone, DueTone::Overdue);
        let work = collections(&s)
            .into_iter()
            .find(|c| c.name == "Work")
            .unwrap();
        let w = room(&s, &n, &RoomId::Collection(work.id), "", &BTreeSet::new());
        let vat = w
            .rows
            .iter()
            .find(|r| r.title == "Pay the December supplier invoices")
            .unwrap();
        assert_eq!(vat.chip.as_ref().unwrap().label, "Finance");
        let list = s.lists.values().find(|l| l.name == "Finance").unwrap();
        let f = room(&s, &n, &RoomId::List(list.id), "", &BTreeSet::new());
        assert!(f.rows.iter().all(|r| r.chip.is_none()));
    }

    #[test]
    fn the_sidebar_counts_open_tasks() {
        let s = fixtures::state();
        let n = now();
        let sb = sidebar(&s, &n, |at| local(at, n.offset_s).date());
        let my_day = &sb.smart[0];
        assert_eq!((my_day.name.as_str(), my_day.count), ("My Day", 9));
        for g in &sb.groups {
            assert_eq!(g.head.count, g.lists.iter().map(|l| l.count).sum::<usize>());
        }
        assert_eq!(sb.agent_updates_today, 2);
        assert_eq!(
            sb.agents
                .iter()
                .map(|(mark, status)| (mark.name.as_str(), status.as_str()))
                .collect::<Vec<_>>(),
            [("claude", "1 edited today"), ("codex", "1 edited today")]
        );
    }

    fn set_stamps(task: &mut Task, agent: &str, at: Timestamp) {
        task.created = todo_core::model::Stamp {
            at,
            by: Actor::agent(agent).unwrap(),
        };
        task.updated = task.created.clone();
    }

    #[test]
    fn agent_rows_count_added_and_edited_but_not_creation_as_an_edit() {
        let mut s = fixtures::state();
        s.tasks
            .retain(|id, _| *id == fixtures::task(3) || *id == fixtures::task(4));
        let n = now();
        for task in s.tasks.values_mut() {
            set_stamps(task, "claude", n.at);
        }
        let date_of = |at| local(at, n.offset_s).date();
        let rows = sidebar(&s, &n, date_of).agents;
        assert_eq!(rows.len(), 1);
        assert_eq!((&*rows[0].0.name, &*rows[0].1), ("claude", "2 added today"));

        s.tasks.get_mut(&fixtures::task(4)).unwrap().updated.at =
            n.at.checked_add(jiff::SignedDuration::from_secs(1))
                .unwrap();
        let rows = sidebar(&s, &n, date_of).agents;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "2 added, 1 edited today");
    }

    #[test]
    fn agent_rows_skip_quiet_agents_before_the_four_row_cap() {
        let n = now();
        let s = fixtures::agent_rows_state();
        let sb = sidebar(&s, &n, |at| local(at, n.offset_s).date());
        assert_eq!(
            sb.agents
                .iter()
                .map(|(mark, _)| mark.name.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex", "helper-for-release-notes", "reviewer"],
        );
    }

    #[test]
    fn only_live_tasks_count_as_agent_activity() {
        let n = now();
        let mut s = fixtures::state();
        let id = fixtures::task(3);
        s.tasks.retain(|task_id, _| *task_id == id);
        set_stamps(s.tasks.get_mut(&id).unwrap(), "claude", n.at);
        let list = s.tasks[&id].list;
        let collection = s.lists[&list].collection.unwrap();
        let deletion = todo_core::model::Tombstone {
            at: n.at,
            by: Actor::owner("web").unwrap(),
        };
        let build = |state: &State| sidebar(state, &n, |at| local(at, n.offset_s).date());
        assert_eq!(build(&s).agents.len(), 1);
        for entity in [id.into(), list.into(), collection.into()] {
            s.set_tombstone(entity, Some(deletion.clone())).unwrap();
            let sb = build(&s);
            assert!(sb.agents.is_empty());
            assert_eq!(sb.agent_updates_today, 0);
            s.set_tombstone(entity, None).unwrap();
        }
        s.tasks.get_mut(&id).unwrap().merged_into = Some(fixtures::task(4));
        assert!(build(&s).agents.is_empty());
    }

    #[test]
    fn sidebar_uses_the_offset_at_each_stamp_across_local_midnight() {
        let n = Now {
            at: "2026-11-01T12:00:00Z".parse().unwrap(),
            offset_s: -5 * 3600,
            browser_time: false,
            date_format: DateFormat::default(),
        };
        let clock_change: Timestamp = "2026-11-01T06:00:00Z".parse().unwrap();
        let date_of = |at| {
            local(
                at,
                if at < clock_change {
                    -4 * 3600
                } else {
                    n.offset_s
                },
            )
            .date()
        };
        let mut s = fixtures::state();
        s.tasks
            .retain(|id, _| *id == fixtures::task(3) || *id == fixtures::task(4));
        for (id, agent, at) in [
            (3, "claude", "2026-11-01T03:59:59Z"),
            (4, "codex", "2026-11-01T04:00:00Z"),
        ] {
            set_stamps(
                s.tasks.get_mut(&fixtures::task(id)).unwrap(),
                agent,
                at.parse().unwrap(),
            );
        }
        let sb = sidebar(&s, &n, date_of);
        assert_eq!(sb.agent_updates_today, 1);
        assert_eq!(sb.agents.len(), 1);
        assert_eq!(
            (&*sb.agents[0].0.name, &*sb.agents[0].1),
            ("codex", "1 added today")
        );
    }

    #[test]
    fn a_finished_day_hands_over_to_tomorrow() {
        let s = fixtures::finished_state();
        let n = now();
        let r = room(&s, &n, &RoomId::Smart("my_day"), "", &BTreeSet::new());
        assert!(r.finished(), "{}", r.summary());
        let t = tomorrow(&s, &n, |_| false, |_| false);
        assert_eq!(t.weekday, "Sunday");
        assert!(t.first.is_some());
        assert_eq!(r.completed.len(), 12);
        assert!(r.add_placeholder.is_some());
    }

    #[test]
    fn later_tasks_do_not_take_the_place_of_tomorrows_first_task() {
        let t = tomorrow(
            &fixtures::finished_without_tomorrow(),
            &now(),
            |_| false,
            |_| false,
        );
        assert_eq!(t.weekday, "Sunday");
        assert!(t.first.is_none() && t.also.is_empty());
        assert_eq!(t.week[0].title, "Prepare the November board update");
        assert!(t.week.iter().any(|row| row.id == fixtures::task(21)));
    }

    #[test]
    fn a_completing_handover_task_keeps_its_slot_until_exit() {
        let mut s = fixtures::finished_state();
        let n = now();
        let t = tomorrow(&s, &n, |_| false, |_| false);
        let id = t.first.unwrap().0.id;
        let cmd = intent::set_done(&s, id, true).unwrap();
        let owner = Actor::owner("web").unwrap();
        for event in todo_core::decide(&s, &owner, &cmd, n.at).unwrap() {
            todo_core::apply(&mut s, &event).unwrap();
        }
        let t = tomorrow(&s, &n, |leaving| leaving == id, |_| false);
        let first = t.rows().next().unwrap();
        assert_eq!(first.id, id);
        assert!(first.done);
        assert!(
            tomorrow(&s, &n, |_| false, |_| false)
                .rows()
                .all(|row| row.id != id)
        );
    }

    #[test]
    fn detail_shows_steps_sources_and_stamps() {
        let s = fixtures::state();
        let n = now();
        let t = s
            .tasks
            .values()
            .find(|t| t.title == "Prepare the November board update")
            .unwrap();
        let d = detail(&s, &n, t.id).unwrap();
        assert_eq!(d.steps.len(), 5);
        assert_eq!(d.steps.iter().filter(|s| s.done).count(), 2);
        assert_eq!(d.crumb.1, "Planning");
        assert_eq!(d.repeat, "Every Monday");
        assert!(d.stamps.starts_with("Created 14.11.2026"), "{}", d.stamps);
        assert!(matches!(d.sources.first(), Some(Source::Email { .. })));
        assert!(d.my_day);
        assert_eq!(d.activity.last().unwrap().action, "created this");
        assert_eq!(d.activity.first().unwrap().who, "You");
        let codex = s
            .tasks
            .values()
            .find(|t| t.title == "Review the API latency report")
            .unwrap();
        let d = detail(&s, &n, codex.id).unwrap();
        assert_eq!(d.activity.last().unwrap().who, "Codex");
        assert_eq!(d.agent.unwrap().letter, "X");
    }

    #[test]
    fn swatches_come_from_keys_hex_or_the_id() {
        let id = todo_core::ids::uuid_v7(1_790_000_000_000, [7; 10]);
        let seed = id.as_bytes();
        assert_eq!(swatch(Some("teal"), seed), "teal");
        assert_eq!(swatch(Some("#e05a7a"), seed), "rose");
        assert_eq!(swatch(Some("#3366ff"), seed), "cornflower");
        assert_eq!(swatch(None, seed), hues::swatch_for_seed(seed).key);
    }

    #[test]
    fn detail_tag_chips_sort_by_folded_name_and_skip_deleted_tags() {
        let mut s = fixtures::state();
        let n = now();
        let task = fixtures::task(3);
        let actor = Actor::owner("web").unwrap();
        for (index, name) in [(1, "Zebra"), (2, "čaj"), (3, "ábel"), (4, "Beans")] {
            let tag = todo_core::ids::TagId::from_uuid(todo_core::ids::uuid_v7(
                1_790_000_000_000,
                [index; 10],
            ));
            let create = intent::create_tag(&s, tag, name).unwrap();
            let attach = todo_core::Command::AddTag { task, tag };
            for cmd in [create, attach] {
                for event in todo_core::decide(&s, &actor, &cmd, n.at).unwrap() {
                    todo_core::apply(&mut s, &event).unwrap();
                }
            }
            if name == "Beans" {
                for event in todo_core::decide(&s, &actor, &intent::delete(tag), n.at).unwrap() {
                    todo_core::apply(&mut s, &event).unwrap();
                }
            }
        }
        let d = detail(&s, &n, task).unwrap();
        assert_eq!(
            d.tags.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
            ["ábel", "čaj", "ops", "Zebra"]
        );
    }

    #[test]
    fn detail_shows_the_first_reminder_from_local_today() {
        let mut s = fixtures::state();
        let n = now();
        let task = fixtures::task(1); // Already has an afternoon reminder.
        let id = todo_core::ids::ReminderId::from_uuid(uuid::Uuid::from_u128(42));
        let at = "2026-11-20T23:30:00Z".parse().unwrap(); // 00:30 today in the fixture zone.
        let actor = Actor::owner("web").unwrap();
        for event in
            todo_core::decide(&s, &actor, &Command::AddReminder { id, task, at }, n.at).unwrap()
        {
            todo_core::apply(&mut s, &event).unwrap();
        }
        assert_eq!(
            detail(&s, &n, task).unwrap().reminder,
            Some(intent::reminder::Seen { id, at })
        );
    }

    #[test]
    fn detail_web_source_keeps_its_link_id_but_never_offers_non_web_urls() {
        let mut s = fixtures::state();
        let n = now();
        let task = fixtures::task(22);
        let link = s.links.values().find(|l| l.task == task).unwrap();
        let id = link.id;
        let d = detail(&s, &n, task).unwrap();
        assert!(
            matches!(d.sources.as_slice(), [Source::Web { id: link, title, href }] if *link == id && title == "Train schedule" && href == "https://example.com/trains")
        );
        for url in ["javascript:void(0)", "data:text/plain,example"] {
            s.links.get_mut(&id).unwrap().kind = LinkKind::Web {
                url: url.into(),
                title: None,
            };
            assert!(detail(&s, &n, task).unwrap().sources.is_empty());
        }
    }

    #[test]
    fn repeat_rules_read_as_words() {
        let dates = now().dates();
        assert_eq!(repeat_text(None, dates), "Does not repeat");
        use todo_core::recurrence::Recurrence;
        for pattern in [
            "FREQ=WEEKLY;BYDAY=MO",
            "FREQ=DAILY;INTERVAL=2",
            "FREQ=MONTHLY;BYMONTHDAY=31;UNTIL=20261231",
        ] {
            let r = Recurrence::from_rrule(pattern).unwrap();
            assert_eq!(
                repeat_text(Some(&r), dates),
                intent::repeat::describe_repeat(&r, dates)
            );
        }
    }
}
