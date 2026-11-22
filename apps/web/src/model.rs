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
use jiff::tz::Offset;
use std::collections::BTreeMap;
use todo_client::intent::{self, TaskRoom, quick_add};
use todo_core::ids::{CollectionId, ListId, TaskId};
use todo_core::model::{Collection, LinkKind, List, Priority, Task};
use todo_core::query::{BUILTIN_LISTS, BuiltinList, EvalContext, Query, fold};
use todo_core::recurrence::Rule;
use todo_core::{Actor, Command, State};
use todo_design::Glyph;
use todo_design::hues::{self, SWATCHES};

/// "Now" for the view: the instant, the owner's local date and UTC offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    /// The instant.
    pub at: Timestamp,
    /// The UTC offset of local time, in seconds.
    pub offset_s: i32,
}

impl Now {
    /// The local civil date and time.
    #[must_use]
    pub fn local(&self) -> DateTime {
        local(self.at, self.offset_s)
    }

    /// Today's local date.
    #[must_use]
    pub fn today(&self) -> Date {
        self.local().date()
    }

    /// The query evaluation context.
    #[must_use]
    pub fn eval(&self) -> EvalContext {
        EvalContext::new(self.at, self.today(), self.offset_s)
    }
}

fn local(at: Timestamp, offset_s: i32) -> DateTime {
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
            RoomId::Smart(_) => Self::Other,
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
        }
    }

    /// The room for a path, and the task open in it; unknown paths open My Day.
    #[must_use]
    pub fn from_path(path: &str) -> (RoomId, Option<TaskId>) {
        let rest = path.strip_prefix("/app").unwrap_or(path);
        let segs: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        let (room, used) = match segs.as_slice() {
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

/// The hand-drawn glyph for a list or collection, chosen from words in its name.
#[must_use]
pub fn glyph_for(name: &str) -> Glyph {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["work", "práce", "job", "office work"]) {
        Glyph::Briefcase
    } else if has(&["project", "projekt"]) {
        Glyph::Layers
    } else if has(&["personal", "osobní", "home", "doma", "family", "rodina"]) {
        Glyph::House
    } else if has(&["shop", "errand", "nákup", "groceries", "basket"]) {
        Glyph::Basket
    } else if has(&["health", "zdraví", "doctor", "fitness", "sport"]) {
        Glyph::Heart
    } else if has(&["travel", "trip", "cest", "vacation", "dovolen"]) {
        Glyph::PaperPlane
    } else if has(&["read", "book", "kni", "čten"]) {
        Glyph::Book
    } else if has(&["financ", "money", "peníz", "tax", "dan", "budget", "bill"]) {
        Glyph::Coin
    } else if has(&["code", "app", "dev", "software", "kód"]) {
        Glyph::Code
    } else if has(&["renovat", "repair", "fix", "garden", "zahrad", "oprav"]) {
        Glyph::Wrench
    } else if has(&["plan", "goal", "cíl"]) {
        Glyph::Flag
    } else if has(&["event", "akce", "party", "meeting", "schůz"]) {
        Glyph::CalendarStar
    } else if has(&["office", "admin", "kancel", "paper", "doklad"]) {
        Glyph::Paperclip
    } else if has(&["day", "den", "today", "sun"]) {
        Glyph::Sun
    } else {
        Glyph::Document
    }
}

// ---- visibility --------------------------------------------------------------------------------------

fn list_live(state: &State, l: &List) -> bool {
    l.deleted.is_none()
        && l.collection.is_none_or(|c| {
            state
                .collections
                .get(&c)
                .is_some_and(|c| c.deleted.is_none())
        })
}

fn task_live(state: &State, t: &Task) -> bool {
    t.deleted.is_none()
        && t.merged_into.is_none()
        && state
            .lists
            .get(&t.list)
            .is_some_and(|l| list_live(state, l))
}

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
        .filter(|l| l.collection == collection && list_live(state, l))
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
    /// Agents active this week and what they did today.
    pub agents: Vec<(AgentMark, String)>,
}

/// Builds the sidebar.
#[must_use]
pub fn sidebar(state: &State, now: &Now) -> Sidebar {
    let ctx = now.eval();
    let smart = BUILTIN_LISTS
        .iter()
        .map(|b| NavItem {
            room: RoomId::Smart(b.id),
            name: smart_name(b.id).to_owned(),
            count: b.parse().map_or(0, |q| q.run(state, &ctx).len()),
            hue: smart_hue(b.id),
            glyph: None,
        })
        .collect();
    let mut open_by_list: BTreeMap<ListId, usize> = BTreeMap::new();
    for t in state.tasks.values() {
        if !t.is_done() && task_live(state, t) {
            *open_by_list.entry(t.list).or_default() += 1;
        }
    }
    let item = |l: &List| NavItem {
        room: RoomId::List(l.id),
        name: l.name.clone(),
        count: open_by_list.get(&l.id).copied().unwrap_or(0),
        hue: swatch(l.color.as_deref(), l.id.as_uuid().as_bytes()),
        glyph: Some(glyph_for(&l.name)),
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
                    glyph: Some(glyph_for(&c.name)),
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
        agents: agent_summary(state, now),
    }
}

/// Summarize the creation and latest edit stamps, like the desktop sidebar.
fn agent_summary(state: &State, now: &Now) -> Vec<(AgentMark, String)> {
    let today = now.today();
    let mut agents = BTreeMap::new();
    for task in state.tasks.values().filter(|t| task_live(state, t)) {
        for (stamp, created) in [(&task.created, true), (&task.updated, false)] {
            let day = local(stamp.at, now.offset_s).date();
            let Some(id) = stamp.by.agent_id() else {
                continue;
            };
            if !(0..=7).contains(&(today - day).get_days()) {
                continue;
            }
            let (_, added, edited) = agents.entry(id).or_insert((&stamp.by, 0, 0));
            if day == today {
                if created {
                    *added += 1;
                } else if stamp.at != task.created.at {
                    *edited += 1;
                }
            }
        }
    }
    agents
        .into_values()
        .filter_map(|(actor, added, edited)| {
            let status = match (added, edited) {
                (0, 0) => "idle".to_owned(),
                (a, 0) => format!("{a} added"),
                (0, e) => format!("{e} edited"),
                (a, e) => format!("{a} added, {e} edited"),
            };
            Some((agent_mark(actor)?, status))
        })
        .take(4)
        .collect()
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
    /// Due date.
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn weekday_short(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
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

fn core_weekday(w: todo_core::time::Weekday) -> &'static str {
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

fn month(d: Date) -> &'static str {
    MONTHS[usize::try_from(d.month() - 1).unwrap_or(0).min(11)]
}

/// `Mon 23 Nov` (with the year when it is not this year's).
#[must_use]
pub fn short_date(d: Date, today: Date) -> String {
    if d.year() == today.year() {
        format!("{} {} {}", weekday_short(d.weekday()), d.day(), month(d))
    } else {
        format!("{} {} {}", d.day(), month(d), d.year())
    }
}

/// How a due date reads relative to today.
#[must_use]
pub fn date_preview(text: &str, now: &Now) -> Option<String> {
    quick_add::parse(text, &now.eval())
        .due
        .map(|d| date_label(d, now.today()))
}

/// An empty due-date edit clears the date; invalid text is distinct from clearing it.
pub fn due_edit(text: &str, now: &Now) -> Result<Option<Date>, ()> {
    if text.trim().is_empty() {
        Ok(None)
    } else {
        quick_add::due_text(text, &now.eval()).map(Some).ok_or(())
    }
}

fn date_label(d: Date, today: Date) -> String {
    match (d - today).get_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        _ => short_date(d, today),
    }
}

/// Manual order is only editable from the task's own list, not from an aggregate room.
pub fn reorder_task(state: &State, room: &RoomId, id: TaskId, by: i32) -> Option<Command> {
    let RoomId::List(list) = room else {
        return None;
    };
    (state.tasks.get(&id)?.list == *list).then_some(())?;
    intent::nudge(state, id.into(), by)
}

pub fn due(d: Date, today: Date) -> Due {
    let days = (d - today).get_days();
    let (text, tone) = match days {
        i32::MIN..=-1 => (format!("{}d overdue", -days), DueTone::Overdue),
        0 => ("Today".to_owned(), DueTone::Soon),
        1 => ("Tomorrow".to_owned(), DueTone::Later),
        _ => (short_date(d, today), DueTone::Later),
    };
    Due { text, tone }
}

fn priority_marks(p: Priority) -> u8 {
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

fn row(state: &State, t: &Task, today: Date, mode: ChipMode) -> Row {
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
            t.due.map(|d| due(d, today))
        },
        agent: agent_mark(&t.created.by),
    }
}

// ---- rooms -------------------------------------------------------------------------------------------------

/// The header eyebrow: an italic lead and mono details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eyebrow {
    /// `Saturday`, `Collection`, or the collection's name.
    pub lead: String,
    /// `21 NOV 2026`, `W47`; `4 LISTS`; `0 OPEN`.
    pub meta: Vec<String>,
}

/// A room, ready to render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// Which room.
    pub id: RoomId,
    /// Title.
    pub title: String,
    /// Hue key for `data-room-hue`.
    pub hue: &'static str,
    /// Eyebrow.
    pub eyebrow: Eyebrow,
    /// Open rows, in order (for Completed: the completed tasks).
    pub rows: Vec<Row>,
    /// Completed rows, most recent first (not for Completed itself).
    pub completed: Vec<Row>,
    /// Done tasks in the room.
    pub done: usize,
    /// All tasks in the room.
    pub total: usize,
    /// The quick-add placeholder, or `None` if the field is hidden in this room.
    pub add_placeholder: Option<String>,
    /// A query is narrowing the rows, but does not change the room's progress.
    pub filtered: bool,
}

impl Room {
    /// The design-language copy for this empty room or filter.
    pub fn empty_line(&self) -> &'static str {
        if self.filtered {
            return "Nothing matches the filter.";
        }
        match self.id {
            RoomId::Smart("my_day") => "Nothing planned yet.",
            RoomId::Smart("important") => "Nothing pressing.",
            RoomId::Smart("planned") => "Nothing on the calendar.",
            RoomId::Smart("all") => "Nothing open.",
            RoomId::Smart("completed") => "Nothing finished yet.",
            RoomId::Smart("agent_activity") => "No agent changes this week.",
            _ => "Nothing on the shelf yet.",
        }
    }

    /// Open tasks.
    #[must_use]
    pub fn open(&self) -> usize {
        self.total - self.done
    }

    /// My Day with everything done: the dusk scene.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.id == RoomId::Smart("my_day") && self.total > 0 && self.done == self.total
    }

    /// The header summary (`3 of 12 done · 9 open`), as screen readers and tests read it.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn summary(&self) -> String {
        if self.id == RoomId::Smart("completed") {
            return format!("{} done", self.total);
        }
        format!(
            "{} of {} done · {} open",
            self.done,
            self.total,
            self.open()
        )
    }
}

/// ISO week number.
#[must_use]
pub fn iso_week(d: Date) -> i8 {
    d.iso_week_date().week()
}

/// The eyebrow of a dated room: `Saturday`, `21 NOV 2026`, `W47`.
#[must_use]
pub fn day_eyebrow(today: Date) -> Eyebrow {
    Eyebrow {
        lead: weekday_long(today.weekday()).to_owned(),
        meta: vec![
            format!(
                "{} {} {}",
                today.day(),
                month(today).to_uppercase(),
                today.year()
            ),
            format!("W{}", iso_week(today)),
        ],
    }
}

fn sort_done(rows: &mut [(&Task, Row)]) {
    rows.sort_by_key(|r| std::cmp::Reverse(r.0.completed_at));
}

/// Builds a room.
#[must_use]
pub fn room(state: &State, now: &Now, id: &RoomId, filter: &str) -> Room {
    let today = now.today();
    let ctx = now.eval();
    let live = |t: &&Task| task_live(state, t);
    let (title, hue, eyebrow, scope, mode, add): (String, &'static str, Eyebrow, Vec<&Task>, _, _) =
        match id {
            RoomId::Smart(s) => {
                let in_scope: Box<dyn Fn(&Task) -> bool> = match *s {
                    "my_day" => Box::new(move |t: &Task| t.my_day == Some(today)),
                    "important" => Box::new(|t: &Task| t.priority == Priority::High),
                    "planned" => Box::new(|t: &Task| t.due.is_some()),
                    "completed" => Box::new(Task::is_done),
                    "all" => Box::new(|_: &Task| true),
                    other => {
                        let q = builtin(other).and_then(|b| b.parse().ok());
                        Box::new(move |t: &Task| {
                            q.as_ref().is_some_and(|q| q.matches(state, t, &ctx))
                        })
                    }
                };
                let scope: Vec<&Task> = state
                    .tasks
                    .values()
                    .filter(live)
                    .filter(|t| in_scope(t))
                    .collect();
                let eyebrow = if *s == "my_day" {
                    day_eyebrow(today)
                } else {
                    Eyebrow {
                        lead: "Smart list".to_owned(),
                        meta: vec![format!("{} tasks", scope.len()).to_uppercase()],
                    }
                };
                let add = (*s != "completed").then(|| "Add a task…".to_owned());
                (
                    smart_name(s).to_owned(),
                    smart_hue(s),
                    eyebrow,
                    scope,
                    ChipMode::Full,
                    add,
                )
            }
            RoomId::Collection(c) => {
                let coll = state.collections.get(c);
                let lists = lists_in(state, Some(*c));
                let scope: Vec<&Task> = state
                    .tasks
                    .values()
                    .filter(live)
                    .filter(|t| lists.iter().any(|l| l.id == t.list))
                    .collect();
                let n = lists.len();
                (
                    coll.map_or_else(String::new, |c| c.name.clone()),
                    coll.map_or("teal", |c| {
                        swatch(c.color.as_deref(), c.id.as_uuid().as_bytes())
                    }),
                    Eyebrow {
                        lead: "Collection".to_owned(),
                        meta: vec![format!("{n} {}", if n == 1 { "LIST" } else { "LISTS" })],
                    },
                    scope,
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
                let list = state.lists.get(l).filter(|l| list_live(state, l));
                let scope: Vec<&Task> = state
                    .tasks
                    .values()
                    .filter(live)
                    .filter(|t| t.list == *l)
                    .collect();
                let coll = list
                    .and_then(|l| l.collection)
                    .and_then(|c| state.collections.get(&c));
                let open = scope.iter().filter(|t| !t.is_done()).count();
                (
                    list.map_or_else(String::new, |l| l.name.clone()),
                    list.map_or("teal", |l| {
                        swatch(l.color.as_deref(), l.id.as_uuid().as_bytes())
                    }),
                    Eyebrow {
                        lead: coll.map_or_else(|| "List".to_owned(), |c| c.name.clone()),
                        meta: vec![format!("{open} OPEN")],
                    },
                    scope,
                    ChipMode::Hidden,
                    list.map(|_| "Add a task…".to_owned()),
                )
            }
        };
    let done = scope.iter().filter(|t| t.is_done()).count();
    let total = scope.len();
    let filter = filter.trim();
    let filtered = !filter.is_empty();
    let scope = if filtered {
        filter_tasks(state, &ctx, scope, filter)
    } else {
        scope
    };
    // Open rows: smart lists in their query's order, lists and collections in manual order.
    let order: Option<Query> = match id {
        RoomId::Smart(s) => builtin(s).and_then(|b| b.parse().ok()),
        _ => None,
    };
    let mut open: Vec<&Task> = scope.iter().copied().filter(|t| !t.is_done()).collect();
    match (&order, id) {
        (Some(q), _) => open.sort_by_cached_key(|t| q.order_key(state, t)),
        (None, RoomId::Collection(_)) => {
            let rank: BTreeMap<ListId, usize> = match id {
                RoomId::Collection(c) => lists_in(state, Some(*c))
                    .iter()
                    .enumerate()
                    .map(|(i, l)| (l.id, i))
                    .collect(),
                _ => BTreeMap::new(),
            };
            open.sort_by(|a, b| {
                rank.get(&a.list)
                    .cmp(&rank.get(&b.list))
                    .then(a.position.cmp(&b.position))
            });
        }
        _ => open.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id))),
    }
    let mut done_rows: Vec<(&Task, Row)> = scope
        .iter()
        .copied()
        .filter(|t| t.is_done())
        .map(|t| (t, row(state, t, today, mode)))
        .collect();
    sort_done(&mut done_rows);
    let done_rows: Vec<Row> = done_rows.into_iter().map(|(_, r)| r).collect();
    let (rows, completed) = if *id == RoomId::Smart("completed") {
        (done_rows, Vec::new())
    } else {
        (
            open.into_iter()
                .map(|t| row(state, t, today, mode))
                .collect(),
            done_rows,
        )
    };
    Room {
        id: id.clone(),
        title,
        hue,
        eyebrow,
        rows,
        completed,
        done,
        total,
        add_placeholder: add,
        filtered,
    }
}

/// Like Desktop, keep free-text search useful while entering an incomplete query.
fn filter_tasks<'a>(
    state: &State,
    ctx: &EvalContext,
    mut tasks: Vec<&'a Task>,
    text: &str,
) -> Vec<&'a Task> {
    if let Ok(query) = Query::parse(text) {
        let matcher = query.matcher(state, ctx);
        tasks.retain(|task| matcher.matches(task));
    } else {
        let folded = fold(text);
        let words: Vec<_> = folded
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect();
        tasks.retain(|task| {
            let title = fold(&task.title);
            words.iter().all(|word| title.contains(word))
        });
    }
    tasks
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

/// Tomorrow's plan, shown when My Day is finished. A completing row keeps its place until it exits.
#[must_use]
pub fn tomorrow(state: &State, now: &Now, leaving: impl Fn(TaskId) -> bool) -> Tomorrow {
    let today = now.today();
    let tomorrow = today.tomorrow().unwrap_or(today);
    let mut upcoming: Vec<&Task> = state
        .tasks
        .values()
        .filter(|t| {
            task_live(state, t)
                && (!t.is_done() || leaving(t.id))
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
            row(state, t, today, ChipMode::Full),
            short_date(tomorrow, today),
        )
    });
    let also = next
        .map(|t| row(state, t, today, ChipMode::ListOnly))
        .collect();
    let week_all: Vec<&&Task> = upcoming
        .iter()
        .filter(|t| t.due.is_some_and(|d| d > tomorrow && d <= week_end))
        .collect();
    let week = week_all
        .iter()
        .take(3)
        .map(|t| row(state, t, today, ChipMode::ListOnly))
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
        /// Title or URL.
        title: String,
        /// The URL (`http` or `https` only).
        href: String,
    },
}

/// An attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentView {
    /// The attachment ID (for the download URL).
    pub id: String,
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
    /// The next reminder.
    pub reminder: Option<String>,
    /// The repeat rule.
    pub repeat: String,
    /// Tag names.
    pub tags: Vec<String>,
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
    pub activity: Vec<Activity>,
}

/// One line of a task's activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    /// The agent's mark, or `None` for the owner (a neutral `Y` mark) and the system.
    pub mark: Option<AgentMark>,
    /// `You`, `Codex`, `Email`.
    pub who: String,
    /// `created this`.
    pub what: String,
    /// `today 09:12`.
    pub when: String,
}

/// How an actor is named in the interface.
#[must_use]
pub fn actor_name(actor: &Actor) -> String {
    match actor {
        Actor::Owner { .. } => "You".to_owned(),
        Actor::Agent { agent_id } => agent_display(agent_id.as_str()),
        Actor::Importer => "Import".to_owned(),
        Actor::EmailIngest => "Email".to_owned(),
        Actor::System => "System".to_owned(),
    }
}

/// An agent's display name (`claude` → `Claude`, `pi` → `Pi`).
#[must_use]
pub fn agent_display(id: &str) -> String {
    let mut c = id.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn clock(dt: DateTime) -> String {
    format!("{:02}:{:02}", dt.hour(), dt.minute())
}

fn when(at: Timestamp, now: &Now) -> String {
    let dt = local(at, now.offset_s);
    let today = now.today();
    let d = dt.date();
    if d == today {
        format!("today {}", clock(dt))
    } else if Some(d) == today.yesterday().ok() {
        format!("yesterday {}", clock(dt))
    } else if d.year() == today.year() {
        format!("{} {}", d.day(), month(d))
    } else {
        format!("{} {} {}", d.day(), month(d), d.year())
    }
}

/// `Sat 21 Nov, 17:00` or `Today, 15:30`.
fn long_when(at: Timestamp, now: &Now) -> String {
    let dt = local(at, now.offset_s);
    let today = now.today();
    let day = match (dt.date() - today).get_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        _ => short_date(dt.date(), today),
    };
    format!("{day}, {}", clock(dt))
}

/// Whether reopening a task undoes completing it. Not for a recurring task with a due date: its
/// completion also created the next occurrence, which reopening would leave behind. The app offers
/// no Undo then, as the desktop app does; the server's undo of the whole command needs the
/// confirmed event, which the toast can't wait for.
#[must_use]
pub fn reopen_undoes_completion(state: &State, id: TaskId) -> bool {
    state.tasks.get(&id).is_none_or(|t| {
        t.recurrence
            .as_ref()
            .zip(t.due)
            .is_none_or(|(r, due)| r.next_after(due).is_none())
    })
}

/// The repeat rule in words.
#[must_use]
pub fn repeat_text(rule: Option<&todo_core::recurrence::Recurrence>) -> String {
    let Some(r) = rule else {
        return "Does not repeat".to_owned();
    };
    let every = |n: u32, one: &str, many: &str| {
        if n == 1 {
            format!("Every {one}")
        } else {
            format!("Every {n} {many}")
        }
    };
    match &r.rule {
        Rule::Daily { interval } => every(*interval, "day", "days"),
        Rule::Weekly { interval, days } => {
            let names: Vec<&str> = days.iter().map(|d| core_weekday(*d)).collect();
            let base = every(*interval, "week", "weeks");
            if *interval == 1 && names.len() == 1 {
                format!("Every {}", names[0])
            } else {
                format!("{base} on {}", names.join(", "))
            }
        }
        Rule::Monthly { interval, day } => {
            format!("{} on day {day}", every(*interval, "month", "months"))
        }
        Rule::Yearly { interval } => every(*interval, "year", "years"),
    }
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
    let t = state.tasks.get(&id).filter(|t| task_live(state, t))?;
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
    let mut reminders: Vec<_> = state
        .reminders
        .values()
        .filter(|r| r.task == id && r.deleted.is_none())
        .collect();
    reminders.sort_by_key(|r| r.at);
    let reminder = reminders
        .iter()
        .find(|r| r.at >= now.at)
        .or(reminders.last())
        .map(|r| long_when(r.at, now));
    let mut tags: Vec<String> = t
        .tags
        .iter()
        .filter_map(|tid| state.tags.get(tid))
        .filter(|tag| tag.deleted.is_none())
        .map(|tag| tag.name.clone())
        .collect();
    tags.sort();
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
            id: a.id.to_string(),
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
        stamps: format!("Created {}{edited}", when(t.created.at, now)),
        steps: steps
            .into_iter()
            .map(|s| StepView {
                id: s.id,
                title: s.title.clone(),
                done: s.completed_at.is_some(),
            })
            .collect(),
        notes: t.notes.clone(),
        due: t.due.map(|d| (d, date_label(d, today))),
        reminder,
        repeat: repeat_text(t.recurrence.as_ref()),
        tags,
        sources,
        attachments,
        my_day: t.my_day == Some(today),
        priority: t.priority,
        agent: agent_mark(&t.created.by),
        activity: activity(t, now),
    })
}

fn activity(t: &Task, now: &Now) -> Vec<Activity> {
    let item = |by: &Actor, what: &str, at: Timestamp| Activity {
        mark: agent_mark(by),
        who: actor_name(by),
        what: what.to_owned(),
        when: when(at, now),
    };
    let mut v = Vec::new();
    if let Some(done) = t.completed_at {
        v.push(item(&t.updated.by, "completed this", done));
    }
    if t.updated.at > t.created.at && Some(t.updated.at) != t.completed_at {
        v.push(item(&t.updated.by, "edited this", t.updated.at));
    }
    let created = if t.created.by == Actor::EmailIngest {
        "created this from an email"
    } else {
        "created this"
    };
    v.push(item(&t.created.by, created, t.created.at));
    v
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::fixtures;

    fn now() -> Now {
        fixtures::now()
    }

    #[test]
    fn completing_a_recurring_task_is_not_undone_by_reopening_it() {
        let s = fixtures::state();
        let owner = Actor::owner("web").unwrap();
        let run = |s: &mut State, cmd: todo_core::Command| {
            for e in todo_core::decide(s, &owner, &cmd, now().at).unwrap() {
                todo_core::apply(s, &e).unwrap();
            }
        };
        // Task 1 repeats weekly and has a due date: completing it creates the next occurrence, and
        // reopening leaves that behind, so the app must not offer reopening as Undo.
        let recurring = fixtures::task(1);
        assert!(s.tasks[&recurring].recurrence.is_some());
        assert!(!reopen_undoes_completion(&s, recurring));
        let mut t = s.clone();
        run(&mut t, todo_core::Command::CompleteTask { id: recurring });
        run(&mut t, todo_core::Command::UncompleteTask { id: recurring });
        let live = |s: &State| s.tasks.values().filter(|t| t.deleted.is_none()).count();
        assert_eq!(live(&t), live(&s) + 1, "an extra occurrence remains");

        // A plain task: reopening restores it exactly.
        let plain = fixtures::task(2);
        assert!(s.tasks[&plain].recurrence.is_none());
        assert!(reopen_undoes_completion(&s, plain));
        let mut t = s.clone();
        run(&mut t, todo_core::Command::CompleteTask { id: plain });
        run(&mut t, todo_core::Command::UncompleteTask { id: plain });
        assert_eq!(live(&t), live(&s));
        assert!(t.tasks[&plain].completed_at.is_none());
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
        assert!(room(&state, &clock, &route, "").add_placeholder.is_some());
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
            Some("Tomorrow")
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
    fn task_reorder_is_scoped_to_its_own_list() {
        let mut s = fixtures::state();
        let n = now();
        let a = fixtures::task(3);
        let b = fixtures::task(6);
        let list = RoomId::List(s.tasks[&a].list);
        assert!(reorder_task(&s, &RoomId::default(), a, 1).is_none());
        assert!(reorder_task(&s, &RoomId::List(s.tasks[&fixtures::task(1)].list), a, 1).is_none());
        assert!(reorder_task(&s, &list, a, -1).is_none());
        let cmd = reorder_task(&s, &list, a, 1).unwrap();
        let owner = Actor::owner("web").unwrap();
        for event in todo_core::decide(&s, &owner, &cmd, n.at).unwrap() {
            todo_core::apply(&mut s, &event).unwrap();
        }
        assert_eq!(
            room(&s, &n, &list, "")
                .rows
                .iter()
                .take(2)
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            [b, a]
        );
    }

    #[test]
    fn my_day_counts_match_the_mockup_story() {
        let s = fixtures::state();
        let r = room(&s, &now(), &RoomId::Smart("my_day"), "");
        assert_eq!((r.done, r.total), (3, 12));
        assert_eq!(r.rows.len(), 9);
        assert_eq!(r.summary(), "3 of 12 done · 9 open");
        assert_eq!(r.eyebrow.lead, "Saturday");
        assert_eq!(r.eyebrow.meta, vec!["21 NOV 2026", "W47"]);
        assert!(!r.finished());
        assert!(r.add_placeholder.is_some());
    }

    #[test]
    fn a_room_filter_uses_queries_and_folded_text_without_changing_progress() {
        let s = fixtures::state();
        let filter = |text| room(&s, &now(), &RoomId::default(), text);
        assert_eq!(filter("tag:docs").rows[0].id, fixtures::task(1));
        assert_eq!(filter("KVULI STK").rows[0].id, fixtures::task(4));
        assert_eq!(filter("(kvuli").rows[0].id, fixtures::task(4));
        let done = filter("done ondrej");
        assert_eq!(done.completed[0].id, fixtures::task(11));
        assert!(done.rows.is_empty());
        assert_eq!((done.done, done.total), (3, 12));
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
            let empty = room(&s, &now(), &RoomId::Smart(id), "");
            assert!(empty.rows.is_empty());
            assert_eq!(empty.empty_line(), copy, "{id}");
        }
    }

    #[test]
    fn overdue_and_today_rows_read_right() {
        let today = now().today();
        let d = |n: i64| today.checked_add(jiff::Span::new().days(n)).unwrap();
        assert_eq!(
            due(d(-2), today),
            Due {
                text: "2d overdue".into(),
                tone: DueTone::Overdue
            }
        );
        assert_eq!(due(d(0), today).tone, DueTone::Soon);
        assert_eq!(due(d(1), today).text, "Tomorrow");
        assert_eq!(due(d(2), today).text, "Mon 23 Nov");
        assert_eq!(
            short_date(Date::new(2027, 1, 4).unwrap(), today),
            "4 Jan 2027"
        );
    }

    #[test]
    fn chips_follow_the_room() {
        let s = fixtures::state();
        let n = now();
        let my_day = room(&s, &n, &RoomId::Smart("my_day"), "");
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
        let w = room(&s, &n, &RoomId::Collection(work.id), "");
        let vat = w
            .rows
            .iter()
            .find(|r| r.title == "Pay the December supplier invoices")
            .unwrap();
        assert_eq!(vat.chip.as_ref().unwrap().label, "Finance");
        let list = s.lists.values().find(|l| l.name == "Finance").unwrap();
        let f = room(&s, &n, &RoomId::List(list.id), "");
        assert!(f.rows.iter().all(|r| r.chip.is_none()));
    }

    #[test]
    fn the_sidebar_counts_open_tasks() {
        let s = fixtures::state();
        let sb = sidebar(&s, &now());
        let my_day = &sb.smart[0];
        assert_eq!((my_day.name.as_str(), my_day.count), ("My Day", 9));
        for g in &sb.groups {
            assert_eq!(g.head.count, g.lists.iter().map(|l| l.count).sum::<usize>());
        }
        assert_eq!(
            sb.agents
                .iter()
                .map(|(mark, _)| mark.name.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex", "pi"]
        );
    }

    #[test]
    fn a_finished_day_hands_over_to_tomorrow() {
        let s = fixtures::finished_state();
        let n = now();
        let r = room(&s, &n, &RoomId::Smart("my_day"), "");
        assert!(r.finished(), "{} of {}", r.done, r.total);
        let t = tomorrow(&s, &n, |_| false);
        assert_eq!(t.weekday, "Sunday");
        assert!(t.first.is_some());
        assert_eq!(r.completed.len(), 12);
        assert!(r.add_placeholder.is_some());
    }

    #[test]
    fn later_tasks_do_not_take_the_place_of_tomorrows_first_task() {
        let t = tomorrow(&fixtures::finished_without_tomorrow(), &now(), |_| false);
        assert_eq!(t.weekday, "Sunday");
        assert!(t.first.is_none() && t.also.is_empty());
        assert_eq!(t.week[0].title, "Prepare the November board update");
        assert!(t.week.iter().any(|row| row.id == fixtures::task(21)));
    }

    #[test]
    fn a_completing_handover_task_keeps_its_slot_until_exit() {
        let mut s = fixtures::finished_state();
        let n = now();
        let t = tomorrow(&s, &n, |_| false);
        let id = t.first.unwrap().0.id;
        let cmd = intent::set_done(&s, id, true).unwrap();
        let owner = Actor::owner("web").unwrap();
        for event in todo_core::decide(&s, &owner, &cmd, n.at).unwrap() {
            todo_core::apply(&mut s, &event).unwrap();
        }
        let t = tomorrow(&s, &n, |leaving| leaving == id);
        let first = t.rows().next().unwrap();
        assert_eq!(first.id, id);
        assert!(first.done);
        assert!(tomorrow(&s, &n, |_| false).rows().all(|row| row.id != id));
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
        assert!(d.stamps.starts_with("Created 14 Nov"), "{}", d.stamps);
        assert!(matches!(d.sources.first(), Some(Source::Email { .. })));
        assert!(d.my_day);
        assert_eq!(d.activity.last().unwrap().what, "created this");
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
        assert_eq!(glyph_for("Nákupy a pochůzky"), Glyph::Basket);
        assert_eq!(glyph_for("Reading list"), Glyph::Book);
    }

    #[test]
    fn repeat_rules_read_as_words() {
        assert_eq!(repeat_text(None), "Does not repeat");
        let r = todo_core::recurrence::Recurrence::from_rrule("FREQ=WEEKLY;BYDAY=MO").unwrap();
        assert_eq!(repeat_text(Some(&r)), "Every Monday");
        let r = todo_core::recurrence::Recurrence::from_rrule("FREQ=DAILY;INTERVAL=2").unwrap();
        assert_eq!(repeat_text(Some(&r)), "Every 2 days");
    }
}
