//! The Activity room: every confirmed change, newest first, grouped by day, with Undo per change.
//!
//! The pages, the undos asked for, what each row offers and the words are `todo_client::activity`'s,
//! shared with the web app. This adds what only the window knows: which agent the feed is filtered
//! to ([`AgentKey`], kept in the room's key), the selected row, the rows as the view draws them, and
//! their short fades ([`ActivityMotion`]). Pure, so it is unit-tested.

use std::collections::HashMap;
use std::time::Duration;

use gpui::SharedString;
use todo_client::Epoch;
use todo_client::activity::{self, Entry, Feed, UndoAction, Undos};
use todo_core::Actor;
use todo_core::actor::AgentId;
use todo_core::ids::EventId;

use super::motion::{Pace, Track};
use super::{Now, lists};
use crate::theme::{Agent, motion as m, space};

/// How many changes a page holds.
pub const PAGE_SIZE: usize = 20;

/// Older changes are read once the keyboard is this close to the last row read.
const LOAD_AHEAD: usize = 3;

/// An agent's ID as a small `Copy` handle, so a room's key (and a menu item that opens one) stays
/// `Copy` and small. IDs are interned for the life of the process in a table of at most
/// [`MAX_AGENTS`] (each at most 64 bytes, so it never holds more than 16 KiB): an owner has a few
/// agents. An agent past the limit gets no key, so its sidebar row isn't shown.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentKey(u32);

/// How many distinct agent IDs the process keeps keys for.
pub const MAX_AGENTS: usize = 256;

/// The place of `id` in `ids`, adding it while there are fewer than `max`.
fn intern(ids: &mut Vec<&'static str>, id: &str, max: usize) -> Option<usize> {
    if let Some(at) = ids.iter().position(|known| *known == id) {
        return Some(at);
    }
    if ids.len() >= max {
        return None;
    }
    ids.push(Box::leak(id.to_owned().into_boxed_str()));
    Some(ids.len() - 1)
}

fn agent_ids() -> std::sync::MutexGuard<'static, Vec<&'static str>> {
    static IDS: std::sync::OnceLock<std::sync::Mutex<Vec<&'static str>>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl AgentKey {
    /// The key for an agent's ID.
    pub fn of(id: &AgentId) -> Option<Self> {
        let at = intern(&mut agent_ids(), id.as_str(), MAX_AGENTS)?;
        u32::try_from(at).ok().map(Self)
    }

    pub fn as_str(self) -> &'static str {
        usize::try_from(self.0)
            .ok()
            .and_then(|at| agent_ids().get(at).copied())
            .unwrap_or_default()
    }

    /// The agent's ID, for the feed's filter.
    pub fn id(self) -> Option<AgentId> {
        AgentId::new(self.as_str()).ok()
    }

    /// Its display name, as the feed names it ("Codex").
    pub fn name(self) -> String {
        activity::agent_name(self.as_str())
    }
}

impl std::fmt::Debug for AgentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AgentKey").field(&self.as_str()).finish()
    }
}

/// The Activity room's own state, kept by the window while it shows the room.
#[derive(Clone, Debug, Default)]
pub struct ActivityState {
    /// The filter the pages were read with.
    pub agent: Option<AgentKey>,
    pub feed: Feed,
    /// The selected change (by sequence number), which Up and Down move and Enter undoes.
    pub selected: Option<u64>,
    /// The replica's position when the newest page was read: once it moves on, Refresh has news.
    pub revision: Option<(Option<Epoch>, u64)>,
    /// The log epoch the pages belong to; a replaced log starts the feed again.
    pub epoch: Option<Epoch>,
    pub motion: ActivityMotion,
}

impl ActivityState {
    /// A fresh feed for `agent` (nothing read yet).
    pub fn new(agent: Option<AgentKey>) -> Self {
        Self {
            agent,
            ..Self::default()
        }
    }

    /// The changes in the order they're drawn: Up and Down walk these.
    pub fn order(&self) -> Vec<u64> {
        self.feed.entries().map(|e| e.target.seq).collect()
    }

    /// Moves the selection `by` rows (from nothing, Down lands on the first and Up on the last).
    /// Returns whether it moved.
    pub fn step(&mut self, by: isize) -> bool {
        let order = self.order();
        let n = order.len();
        if n == 0 {
            return false;
        }
        let at = self
            .selected
            .and_then(|seq| order.iter().position(|&s| s == seq));
        let next = match at {
            None if by > 0 => 0,
            None => n - 1,
            Some(i) => i.saturating_add_signed(by).min(n - 1),
        };
        let to = order[next];
        let moved = self.selected != Some(to);
        self.selected = Some(to);
        moved
    }

    /// The selection is close enough to the end of what has been read to read the next page.
    pub fn near_end(&self) -> bool {
        let order = self.order();
        self.selected
            .and_then(|seq| order.iter().position(|&s| s == seq))
            .is_some_and(|i| i + LOAD_AHEAD >= order.len())
    }

    /// Drops a selection whose change is no longer in the feed (a refresh read other changes).
    pub fn keep_selection(&mut self) {
        if self
            .selected
            .is_some_and(|seq| self.feed.entry(seq).is_none())
        {
            self.selected = None;
        }
    }
}

/// What reached the list in the Activity room: a raw key, or one of the list's bound actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedInput<'a> {
    Key {
        key: &'a str,
        modified: bool,
    },
    /// Down or J.
    Next,
    /// Up or K.
    Previous,
    Escape,
    /// R.
    Refresh,
}

/// What the list does with it in the Activity room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedCommand {
    Step(isize),
    /// Undo the selected change.
    Undo,
    /// Clear the selection.
    Clear,
    Refresh,
}

/// The list's keys in the Activity room, which act only while the list itself has the keyboard. A
/// key pressed on a control inside the feed (Load older activity, Try again) bubbles up to the list,
/// and must never move the selection, refresh or undo the selected change: there it is the control's.
pub fn feed_command(list_focused: bool, input: FeedInput<'_>) -> Option<FeedCommand> {
    if !list_focused {
        return None;
    }
    Some(match input {
        FeedInput::Key {
            key: "enter" | "u",
            modified: false,
        } => FeedCommand::Undo,
        FeedInput::Key { .. } => return None,
        FeedInput::Next => FeedCommand::Step(1),
        FeedInput::Previous => FeedCommand::Step(-1),
        FeedInput::Escape => FeedCommand::Clear,
        FeedInput::Refresh => FeedCommand::Refresh,
    })
}

/// Who made a change, as its mark shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Agent(Agent),
    /// The owner: a neutral `Y`.
    You,
    Mail,
    /// Import and the system.
    Other,
}

impl Mark {
    pub fn of(actor: &Actor) -> Self {
        match actor {
            Actor::Agent { .. } => lists::agent(actor).map_or(Self::Other, Self::Agent),
            Actor::Owner { .. } => Self::You,
            Actor::EmailIngest => Self::Mail,
            Actor::Importer | Actor::System => Self::Other,
        }
    }
}

/// A change as the room draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub seq: u64,
    pub event: EventId,
    /// What happened, in plain words.
    pub what: SharedString,
    pub mark: Mark,
    /// "You", "Codex", "Email ingest".
    pub who: SharedString,
    /// "5 min ago".
    pub when: SharedString,
    /// The local time, "10:35".
    pub clock: SharedString,
    pub action: UndoAction,
    /// Why an undo asked for here was refused, in plain words.
    pub refusal: Option<&'static str>,
    pub selected: bool,
}

/// A day of changes under its heading ("Today", "Yesterday", "Thu 19 Nov").
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityDay {
    pub label: SharedString,
    pub rows: Vec<ActivityRow>,
}

/// The rows to draw, by local day, newest first. `undos` are the undos asked for (the store keeps
/// them, across rooms and filters); `flags` is the replica's view of an entry: its confirmed journal
/// flags and whether the outbox holds an undo of it (see `activity::undo_action`).
pub fn days(
    state: &ActivityState,
    undos: &Undos,
    now: &Now,
    flags: impl Fn(&Entry) -> (Option<(bool, bool)>, bool),
) -> Vec<ActivityDay> {
    let today = now.today();
    state
        .feed
        .days(|at| now.date_of(at))
        .into_iter()
        .map(|(day, seqs)| ActivityDay {
            label: activity::day_label(day, today).into(),
            rows: seqs
                .into_iter()
                .filter_map(|seq| state.feed.entry(seq))
                .map(|entry| {
                    let (confirmed, pending) = flags(entry);
                    let group = undos.group_outcome(entry.group);
                    ActivityRow {
                        seq: entry.target.seq,
                        event: entry.target.id,
                        what: super::labels::one_line(&entry.what).into(),
                        mark: Mark::of(&entry.actor),
                        who: entry.actor_name().into(),
                        when: activity::relative_time(entry.at, now.at).into(),
                        clock: super::labels::clock(entry.at, &now.tz).into(),
                        action: activity::undo_action(entry, confirmed, pending, group),
                        refusal: undos.refusal(entry.target.id),
                        selected: state.selected == Some(entry.target.seq),
                    }
                })
                .collect(),
        })
        .collect()
}

/// The room's short fades: rows a page brings in fade in and rise into place (`ROW_ENTER`,
/// `ENTER`), a refusal's line fades in beside its change (`TOAST_IN`), and the selection's wash
/// crossfades between rows (`HOVER`). A whole page enters together, never staggered row by row. With
/// reduced motion nothing moves and the fades are at most `REDUCED_FADE`.
#[derive(Clone, Debug, Default)]
pub struct ActivityMotion {
    rows: HashMap<u64, Track>,
    notes: HashMap<u64, Track>,
    selection: HashMap<u64, Track>,
    reduced: bool,
}

/// How a row is drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedLook {
    pub opacity: f32,
    /// How far below its place it is drawn, in pixels.
    pub rise: f32,
    /// The selection wash, 0 to 1.
    pub selected: f32,
    /// The refusal line's opacity.
    pub note: f32,
}

fn span(ms: u32, fade: bool, pace: Pace) -> Duration {
    let ms = if pace.reduced {
        m::reduced(ms, fade)
    } else {
        ms
    };
    Duration::from_millis(u64::from(ms))
}

impl ActivityMotion {
    /// Rows a page has just brought in.
    pub fn enter(&mut self, seqs: impl IntoIterator<Item = u64>, now: Duration, pace: Pace) {
        self.reduced = pace.reduced;
        let span = span(m::ROW_ENTER, true, pace);
        for seq in seqs {
            self.rows
                .insert(seq, Track::new(0., 1., now, span, m::ENTER));
        }
    }

    /// A refusal has just been said beside `seq`.
    pub fn note(&mut self, seq: u64, now: Duration, pace: Pace) {
        self.notes.insert(
            seq,
            Track::new(0., 1., now, span(m::TOAST_IN, true, pace), m::ENTER),
        );
    }

    /// The selection moved from `from` to `to`.
    pub fn select(&mut self, from: Option<u64>, to: Option<u64>, now: Duration, pace: Pace) {
        if from == to {
            return;
        }
        let span = span(m::HOVER, true, pace);
        for (seq, on) in [(from, 0.), (to, 1.)] {
            let Some(seq) = seq else { continue };
            let track = self
                .selection
                .entry(seq)
                .or_insert_with(|| Track::rest(1. - on));
            track.aim(on, now, span, m::STANDARD);
        }
    }

    pub fn look(&self, seq: u64, selected: bool, now: Duration) -> FeedLook {
        let enter = self.rows.get(&seq).map_or(1., |t| t.at(now));
        let rise = if self.reduced {
            0.
        } else {
            (1. - enter) * space::S1
        };
        FeedLook {
            opacity: enter,
            rise,
            selected: self
                .selection
                .get(&seq)
                .map_or(f32::from(selected), |t| t.at(now)),
            note: self.notes.get(&seq).map_or(1., |t| t.at(now)),
        }
    }

    /// Something still fades: the view asks for the next frame.
    pub fn moving(&self, now: Duration) -> bool {
        (self.rows.values())
            .chain(self.notes.values())
            .chain(self.selection.values())
            .any(|t| t.moving(now))
    }

    /// Forgets what has finished, so a row at rest costs nothing.
    pub fn prune(&mut self, now: Duration) {
        for map in [&mut self.rows, &mut self.notes] {
            map.retain(|_, t| t.moving(now));
        }
        self.selection
            .retain(|_, t| t.moving(now) || t.target() > 0.);
    }

    /// Draws everything at rest (the screenshot fixtures).
    pub fn settle(&mut self) {
        self.rows.clear();
        self.notes.clear();
        self.selection.clear();
    }
}

/// What the status bar's left side says in the room: how many changes are shown, and how many of
/// them are from today.
pub fn counts(state: &ActivityState, now: &Now) -> (usize, usize) {
    let today = now.today();
    let shown = state.feed.len();
    let on_today = state
        .feed
        .entries()
        .filter(|e| now.date_of(e.at) == today)
        .count();
    (shown, on_today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_client::activity::{ActivityItem, ActivityPage, UndoOutcome};
    use todo_client::protocol::LogEvent;
    use todo_core::Event;
    use todo_core::event::EventPayload;
    use todo_core::time::Timestamp;

    fn item(seq: u64, actor: Actor, at: &str, undone: bool) -> ActivityItem {
        let id = EventId::from_uuid(uuid::Uuid::from_u128(u128::from(seq)));
        ActivityItem {
            event: LogEvent {
                event: Event {
                    id,
                    seq: Some(seq),
                    group: id,
                    actor,
                    at: at.parse().unwrap_or(Timestamp::UNIX_EPOCH),
                    undoes: None,
                    forced: false,
                    payload: EventPayload::Updated {
                        target: todo_core::ids::TaskId::from_uuid(uuid::Uuid::from_u128(1)).into(),
                        changes: Vec::new(),
                    },
                },
                command_id: id.as_uuid(),
                command_idx: 0,
                command_len: Some(1),
            },
            summary: format!("Change\n{seq}"),
            approved_by: None,
            undone,
            can_undo: !undone,
        }
    }

    fn loaded(items: Vec<ActivityItem>, next: Option<u64>) -> ActivityState {
        let mut state = ActivityState::new(None);
        let r = state.feed.begin(None).expect("a read");
        state.feed.finish(
            r,
            None,
            Ok(ActivityPage {
                items,
                next_before: next,
            }),
        );
        state
    }

    fn agent(id: &str) -> Actor {
        Actor::agent(id).unwrap_or(Actor::System)
    }

    #[test]
    fn the_lists_keys_act_only_while_the_list_has_the_keyboard() {
        let key = |key| FeedInput::Key {
            key,
            modified: false,
        };
        for input in [
            key("enter"),
            key("u"),
            FeedInput::Next,
            FeedInput::Previous,
            FeedInput::Escape,
            FeedInput::Refresh,
        ] {
            // Enter on Load older activity or Try again bubbles up from the button: nothing here.
            assert_eq!(feed_command(false, input), None, "{input:?}");
            assert!(feed_command(true, input).is_some(), "{input:?}");
        }
        assert_eq!(feed_command(true, key("enter")), Some(FeedCommand::Undo));
        assert_eq!(feed_command(true, key("u")), Some(FeedCommand::Undo));
        assert_eq!(
            feed_command(
                true,
                FeedInput::Key {
                    key: "enter",
                    modified: true
                }
            ),
            None
        );
        assert_eq!(feed_command(true, key("space")), None);
        assert_eq!(
            feed_command(true, FeedInput::Next),
            Some(FeedCommand::Step(1))
        );
        assert_eq!(
            feed_command(true, FeedInput::Previous),
            Some(FeedCommand::Step(-1))
        );
        assert_eq!(
            feed_command(true, FeedInput::Escape),
            Some(FeedCommand::Clear)
        );
        assert_eq!(
            feed_command(true, FeedInput::Refresh),
            Some(FeedCommand::Refresh)
        );
    }

    #[test]
    fn the_agent_table_is_bounded() {
        let mut ids = Vec::new();
        assert_eq!(intern(&mut ids, "codex", 2), Some(0));
        assert_eq!(intern(&mut ids, "pi", 2), Some(1));
        assert_eq!(
            intern(&mut ids, "codex", 2),
            Some(0),
            "a known agent keeps its key"
        );
        assert_eq!(
            intern(&mut ids, "claude", 2),
            None,
            "full: no key, nothing added"
        );
        assert_eq!(ids, ["codex", "pi"]);
        assert_eq!(intern(&mut ids, "pi", 2), Some(1));
    }

    #[test]
    fn agent_keys_round_trip_and_stay_copy() {
        let id = AgentId::new("claude-code_2").expect("an ID");
        let key = AgentKey::of(&id).expect("a key");
        let copy = key;
        assert_eq!(copy.id(), Some(id.clone()));
        assert_eq!(key.as_str(), "claude-code_2");
        assert_eq!(key.name(), "Claude-code_2");
        let long = AgentId::new("a".repeat(64)).expect("the longest ID");
        assert_eq!(AgentKey::of(&long).and_then(|k| k.id()), Some(long));
        assert_eq!(AgentKey::of(&id), Some(key), "the same agent, the same key");
        assert_ne!(AgentKey::of(&AgentId::new("pi").expect("an ID")), Some(key));
    }

    #[test]
    fn rows_group_by_local_day_with_the_shared_words() {
        let now = Now::fixed("2026-11-21T10:35:00Z");
        let mut state = loaded(
            vec![
                item(9, agent("codex"), "2026-11-21T10:30:00Z", false),
                item(
                    8,
                    Actor::owner("desktop").expect("owner"),
                    "2026-11-21T08:00:00Z",
                    false,
                ),
                item(7, Actor::EmailIngest, "2026-11-20T17:00:00Z", true),
                item(6, Actor::System, "2026-11-19T17:00:00Z", false),
            ],
            Some(6),
        );
        state.selected = Some(8);
        let days = days(&state, &Undos::default(), &now, |_| (None, false));
        let labels: Vec<_> = days.iter().map(|d| d.label.to_string()).collect();
        assert_eq!(labels, ["Today", "Yesterday", "Thu 19 Nov"]);
        let first = &days[0].rows[0];
        assert_eq!(first.what, "Change 9", "drawn on one line");
        assert_eq!(first.who, "Codex");
        assert_eq!(first.when, "5 min ago");
        assert_eq!(first.clock, "10:30");
        assert!(matches!(first.mark, Mark::Agent(Agent::Codex)));
        assert_eq!(first.action, UndoAction::Undo);
        assert!(!first.selected && days[0].rows[1].selected);
        assert_eq!(days[0].rows[1].who, "You");
        assert_eq!(days[0].rows[1].mark, Mark::You);
        assert_eq!(days[1].rows[0].mark, Mark::Mail);
        assert_eq!(days[1].rows[0].action, UndoAction::Undone);
        assert_eq!(days[2].rows[0].mark, Mark::Other);
        assert_eq!(counts(&state, &now), (4, 2));
    }

    #[test]
    fn pending_and_refused_undos_show_on_their_rows() {
        let now = Now::fixed("2026-11-21T10:35:00Z");
        let state = loaded(
            vec![
                item(9, agent("codex"), "2026-11-21T10:30:00Z", false),
                item(8, agent("claude"), "2026-11-21T10:00:00Z", false),
            ],
            None,
        );
        let (nine, eight) = (
            state.feed.entry(9).expect("9").clone(),
            state.feed.entry(8).expect("8").clone(),
        );
        let mut undos = Undos::default();
        undos.record(
            nine.target.id,
            nine.group,
            UndoOutcome::Pending(uuid::Uuid::from_u128(5)),
        );
        let why = todo_client::failure::undo_message("undo_conflict");
        undos.record(eight.target.id, eight.group, UndoOutcome::Refused(why));
        let rows: Vec<_> = days(&state, &undos, &now, |_| (None, false))
            .into_iter()
            .flat_map(|d| d.rows)
            .collect();
        assert_eq!(rows[0].action, UndoAction::Undoing);
        assert_eq!(rows[0].refusal, None);
        assert_eq!(
            rows[1].action,
            UndoAction::Undo,
            "a refusal can be tried again"
        );
        assert_eq!(
            rows[1].refusal,
            Some("Couldn't undo: the item was changed again.")
        );
        // The outbox's own record counts too, before this window asked.
        let rows: Vec<_> = days(&state, &undos, &now, |e| (None, e.target.seq == 8))
            .into_iter()
            .flat_map(|d| d.rows)
            .collect();
        assert_eq!(rows[1].action, UndoAction::Undoing);
    }

    #[test]
    fn the_keyboard_walks_the_rows_and_reads_ahead_near_the_end() {
        let items = (1..=6)
            .rev()
            .map(|seq| item(seq, agent("pi"), "2026-11-21T10:00:00Z", false))
            .collect();
        let mut state = loaded(items, Some(1));
        assert!(!state.near_end());
        assert!(state.step(1));
        assert_eq!(state.selected, Some(6));
        assert!(!state.step(-1), "stays on the first row");
        state.step(2);
        assert_eq!(state.selected, Some(4));
        assert!(!state.near_end());
        state.step(1);
        assert!(state.near_end());
        state.step(10);
        assert_eq!(state.selected, Some(1));
        let mut empty = ActivityState::new(None);
        assert!(!empty.step(1));
        let mut from_nothing = loaded(
            vec![item(2, agent("pi"), "2026-11-21T10:00:00Z", false)],
            None,
        );
        from_nothing.step(-1);
        assert_eq!(from_nothing.selected, Some(2));
        from_nothing.selected = Some(99);
        from_nothing.keep_selection();
        assert_eq!(from_nothing.selected, None);
    }

    #[test]
    fn rows_fade_in_together_and_settle_without_frames() {
        let ms = |n: u32| Duration::from_millis(u64::from(n));
        let mut motion = ActivityMotion::default();
        motion.enter([3, 4], ms(0), Pace::FULL);
        let half = motion.look(3, false, ms(m::ROW_ENTER / 2));
        assert!(half.opacity > 0. && half.opacity < 1.);
        assert!(half.rise > 0. && half.rise < space::S1);
        assert_eq!(
            motion.look(4, false, ms(m::ROW_ENTER / 2)),
            half,
            "no stagger"
        );
        assert!(motion.moving(ms(m::ROW_ENTER / 2)));
        assert!(!motion.moving(ms(m::ROW_ENTER)));
        motion.select(None, Some(3), ms(0), Pace::FULL);
        let wash = motion.look(3, true, ms(m::HOVER / 2)).selected;
        assert!(wash > 0. && wash < 1.);
        motion.select(Some(3), Some(4), ms(m::HOVER), Pace::FULL);
        assert!(motion.look(3, false, ms(m::HOVER * 3 / 2)).selected < 1.);
        motion.note(4, ms(0), Pace::FULL);
        assert!(motion.look(4, false, ms(1)).note < 1.);
        motion.prune(ms(10_000));
        assert!(!motion.moving(ms(10_000)));
        assert_eq!(motion.look(4, true, ms(10_000)).selected, 1.);
        assert_eq!(motion.look(3, false, ms(10_000)).selected, 0.);
        // Reduced motion: nothing rises, and the fade is short.
        let mut reduced = ActivityMotion::default();
        reduced.enter([1], ms(0), Pace::REDUCED);
        assert_eq!(reduced.look(1, false, ms(1)).rise, 0.);
        assert!(!reduced.moving(ms(m::REDUCED_FADE)));
        motion.enter([5], ms(0), Pace::FULL);
        motion.settle();
        assert_eq!(motion.look(5, false, ms(0)).opacity, 1.);
    }
}
