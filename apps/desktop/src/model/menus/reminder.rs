//! The reminder picker: presets (Later today, Tomorrow morning, Next week), a custom day and time, and
//! Remove. Only a preset, Set reminder or Remove writes a command, through the shared
//! `todo_client::intent::reminder` (which reminder the row edits, and an edit on one that changed
//! meanwhile writes nothing).
//!
//! Times are the owner's local ones. A custom time the clocks skip (spring forward) saves the time
//! the gap moves it to, and one that happens twice (fall back) saves the first, as on Android; the
//! picker says so before it is set.

use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};
use todo_core::ids::TaskId;
use todo_core::time::{Timestamp, Weekday, add_days};

use super::{MenuAction, MenuEntry, MenuItem, danger, item, with};
use crate::model::{Now, labels};
use crate::theme::Icon;

/// The hour a morning preset (Tomorrow morning, Next week) and a custom day on a due date start at.
pub const MORNING_HOUR: i8 = 9;
/// Later today is at least this many hours away, then rounded up to the whole hour.
const LATER_HOURS: i64 = 2;
/// What the custom time's buttons step by, in minutes.
const STEP_SMALL: i32 = 15;
const STEP_LARGE: i32 = 60;

/// Under the picker when the time picked has passed.
pub const PAST: &str = "Pick a time that hasn’t passed.";
/// Under the picker while the time typed isn't one.
pub const BAD_TIME: &str = "Type an hour from 00 to 23 and a minute from 00 to 59.";
/// In the status bar when the reminder changed while the picker was open.
pub const CHANGED: &str =
    "The reminder changed while the picker was open. Check it, then try again.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    LaterToday,
    TomorrowMorning,
    NextWeek,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::LaterToday, Self::TomorrowMorning, Self::NextWeek];

    fn key(self) -> &'static str {
        match self {
            Self::LaterToday => "later-today",
            Self::TomorrowMorning => "tomorrow-morning",
            Self::NextWeek => "next-week",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::LaterToday => "Later today",
            Self::TomorrowMorning => "Tomorrow morning",
            Self::NextWeek => "Next week",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::LaterToday => Icon::Clock,
            Self::TomorrowMorning => Icon::Sun,
            Self::NextWeek => Icon::Calendar,
        }
    }

    /// When it reminds, from `now`: Later today is two hours on, rounded up to the hour (and only while
    /// that is still today); Tomorrow morning is 09:00 tomorrow; Next week is 09:00 next Monday.
    pub fn at(self, now: &Now) -> Option<Timestamp> {
        let local = now.at.to_zoned_datetime(now.tz.clone());
        let today = local.date();
        match self {
            Self::LaterToday => {
                let later = local
                    .checked_add(jiff::Span::new().hours(LATER_HOURS))
                    .ok()?;
                let whole = whole_hour(later.datetime(), jiff::RoundMode::Ceil)?;
                (whole.date() == today).then(|| resolve(whole, &now.tz).0)
            }
            Self::TomorrowMorning => {
                let day = add_days(today, 1)?;
                Some(resolve(day.at(MORNING_HOUR, 0, 0, 0), &now.tz).0)
            }
            Self::NextWeek => {
                let ahead = 7 - i64::try_from(Weekday::of(today).offset()).unwrap_or(0);
                let monday = add_days(today, ahead)?;
                Some(resolve(monday.at(MORNING_HOUR, 0, 0, 0), &now.tz).0)
            }
        }
    }
}

/// `dt` rounded to the whole hour, up or down (`mode`).
fn whole_hour(dt: DateTime, mode: jiff::RoundMode) -> Option<DateTime> {
    let round = jiff::civil::DateTimeRound::new()
        .smallest(jiff::Unit::Hour)
        .mode(mode);
    dt.round(round).ok()
}

/// How the clocks changed a local time: not at all, skipped it (it saves the time the gap moves it to),
/// or doubled it (it saves the first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clocks {
    Plain,
    Skipped,
    Twice,
}

/// The instant a local day and time stands for in `tz`, and how the clocks changed it.
pub fn resolve(local: DateTime, tz: &TimeZone) -> (Timestamp, Clocks) {
    let ambiguous = tz.to_ambiguous_zoned_datetime(local);
    let clocks = match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => Clocks::Plain,
        AmbiguousOffset::Gap { .. } => Clocks::Skipped,
        AmbiguousOffset::Fold { .. } => Clocks::Twice,
    };
    // "Compatible": a gap moves on by its length, an overlap takes the earlier offset.
    let at = ambiguous
        .compatible()
        .map(|z| z.timestamp())
        .unwrap_or_else(|_| Timestamp::UNIX_EPOCH);
    (at, clocks)
}

/// What the custom page holds: a day and a time, stepped with the buttons or typed (digits make the
/// time: `9` is 09:00, `930` 09:30, `1430` 14:30).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draft {
    day: Date,
    /// Minutes since midnight.
    minute: i32,
    /// Digits typed so far, and how many.
    typed: [u8; 4],
    len: u8,
}

/// What the picker's items do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Preset(Preset),
    /// Turns the picker into its custom page.
    Custom,
    /// Moves the custom day by this many days.
    Day(i32),
    /// Moves the custom time by this many minutes.
    Minutes(i32),
    /// Sets the custom day and time.
    Apply,
    Remove,
}

impl Draft {
    /// Where the custom page starts: the reminder's own time; else, for a task due on a later day,
    /// 09:00 that day; else the next whole hour.
    pub fn new(at: Option<Timestamp>, due: Option<Date>, now: &Now) -> Self {
        let local = |ts: Timestamp| ts.to_zoned_datetime(now.tz.clone()).datetime();
        let start = match (at, due) {
            (Some(at), _) => local(at),
            (None, Some(due)) if due > now.today() => due.at(MORNING_HOUR, 0, 0, 0),
            _ => {
                let n = local(now.at);
                whole_hour(n, jiff::RoundMode::Trunc)
                    .and_then(|h| h.checked_add(jiff::Span::new().hours(1)).ok())
                    .unwrap_or(n)
            }
        };
        Self {
            day: start.date(),
            minute: i32::from(start.hour()) * 60 + i32::from(start.minute()),
            typed: [0; 4],
            len: 0,
        }
    }

    /// The day and time, unless what's typed isn't a time (yet).
    pub fn local(self) -> Option<DateTime> {
        let (h, m) = match self.typed_time() {
            Some(t) => t?,
            None => (self.minute / 60, self.minute % 60),
        };
        let (h, m) = (i8::try_from(h).ok()?, i8::try_from(m).ok()?);
        Some(self.day.at(h, m, 0, 0))
    }

    /// The typed time: `None` when nothing is typed, `Some(None)` when it isn't a time.
    fn typed_time(self) -> Option<Option<(i32, i32)>> {
        if self.len == 0 {
            return None;
        }
        let d: Vec<i32> = self.typed[..usize::from(self.len)]
            .iter()
            .map(|&b| i32::from(b))
            .collect();
        let (h, m) = match d.as_slice() {
            [h] => (*h, 0),
            [a, b] => (a * 10 + b, 0),
            [h, a, b] => (*h, a * 10 + b),
            [a, b, c, e] => (a * 10 + b, c * 10 + e),
            _ => return Some(None),
        };
        Some((h < 24 && m < 60).then_some((h, m)))
    }

    /// The buttons: they take in what's typed first (if it's a time), then step.
    pub fn edit(&mut self, a: Action, today: Date) {
        if let Some(Some((h, m))) = self.typed_time() {
            self.minute = h * 60 + m;
        }
        self.len = 0;
        match a {
            Action::Day(by) => {
                self.day = add_days(self.day, i64::from(by)).unwrap_or(self.day);
            }
            Action::Minutes(by) => {
                let total = self.minute + by;
                let days = total.div_euclid(24 * 60);
                self.minute = total.rem_euclid(24 * 60);
                self.day = add_days(self.day, i64::from(days)).unwrap_or(self.day);
            }
            _ => return,
        }
        // Never before today: the past can't be set anyway.
        if self.day < today {
            self.day = today;
        }
    }

    /// A key typed on the custom page: digits type the time, Backspace takes one back. Returns whether
    /// it was taken.
    pub fn type_key(&mut self, key: &str) -> bool {
        match key {
            "backspace" if self.len > 0 => self.len -= 1,
            _ if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
                if usize::from(self.len) == self.typed.len() {
                    self.len = 0;
                }
                self.typed[usize::from(self.len)] = key.as_bytes()[0] - b'0';
                self.len += 1;
            }
            _ => return false,
        }
        true
    }

    /// The time as the page shows it: typed digits as they stand (`14:3_`), else the time.
    fn time_label(self) -> String {
        if self.len == 0 {
            return format!("{:02}:{:02}", self.minute / 60, self.minute % 60);
        }
        let digits: String = self.typed[..usize::from(self.len)]
            .iter()
            .map(|d| char::from(b'0' + d))
            .collect();
        match digits.len() {
            1 | 2 => format!("{digits}:__"),
            3 => format!("{}:{}", &digits[..1], &digits[1..]),
            _ => format!("{}:{}", &digits[..2], &digits[2..]),
        }
    }
}

fn choice(
    task: TaskId,
    key: &'static str,
    label: impl Into<gpui::SharedString>,
    a: Action,
) -> MenuItem {
    MenuItem {
        key: key.into(),
        label: label.into(),
        action: MenuAction::Reminder(task, a),
        checked: false,
        icon: Icon::Clock,
        glyph: None,
        dot: None,
        danger: false,
        hint: None,
        more: None,
    }
}

/// What the custom page will set, or why it can't: the instant and what to say about it.
pub fn preview(draft: Draft, now: &Now) -> Result<(Timestamp, String), &'static str> {
    let local = draft.local().ok_or(BAD_TIME)?;
    let (at, clocks) = resolve(local, &now.tz);
    if at <= now.at {
        return Err(PAST);
    }
    let asked = format!("{:02}:{:02}", local.hour(), local.minute());
    let saved = labels::clock(at, &now.tz);
    let when = labels::when(at, now);
    Ok((
        at,
        match clocks {
            Clocks::Plain => format!("Reminds you {when}."),
            Clocks::Skipped => format!(
                "{asked} doesn’t exist that day: the clocks go forward. Reminds you {when}."
            ),
            Clocks::Twice => {
                format!(
                    "{asked} happens twice that day: the clocks go back. Saves the first, {saved}."
                )
            }
        },
    ))
}

/// The picker for `task`, whose row showed the reminder at `seen` (or none): its presets and Remove,
/// or (`draft`) the custom page.
pub fn entries(
    task: TaskId,
    seen: Option<Timestamp>,
    draft: Option<Draft>,
    now: &Now,
) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    let Some(d) = draft else {
        entries.push(MenuEntry::Heading("Reminder".into()));
        for p in Preset::ALL {
            let Some(at) = p.at(now) else {
                continue;
            };
            let hint = match p {
                Preset::LaterToday => labels::clock(at, &now.tz),
                _ => format!(
                    "{} {}",
                    labels::weekday(now.date_of(at))
                        .get(..3)
                        .unwrap_or_default(),
                    labels::clock(at, &now.tz)
                ),
            };
            entries.push(MenuEntry::Item(MenuItem {
                icon: p.icon(),
                hint: Some(hint.into()),
                checked: seen == Some(at),
                ..choice(task, p.key(), p.label(), Action::Preset(p))
            }));
        }
        let custom = item(
            "custom",
            Icon::Settings,
            "Pick a day and time…",
            MenuAction::Reminder(task, Action::Custom),
        );
        let preset = Preset::ALL.iter().any(|p| p.at(now) == seen);
        entries.push(with(custom, |i| MenuItem {
            checked: seen.is_some() && !preset,
            ..i
        }));
        if seen.is_some() {
            entries.push(MenuEntry::Separator);
            entries.push(danger(
                "remove-reminder",
                Icon::X,
                "Remove reminder",
                MenuAction::Reminder(task, Action::Remove),
            ));
        }
        return entries;
    };
    let today = now.today();
    entries.push(MenuEntry::Heading("Pick a day and time".into()));
    entries.push(MenuEntry::Controls {
        label: "Day",
        value: Some(labels::relative_day(d.day, today)),
        buttons: vec![
            choice(task, "week-less", "−7", Action::Day(-7)),
            choice(task, "day-less", "−1", Action::Day(-1)),
            choice(task, "day-more", "+1", Action::Day(1)),
            choice(task, "week-more", "+7", Action::Day(7)),
        ],
    });
    entries.push(MenuEntry::Controls {
        label: "Time · type or step",
        value: Some(d.time_label()),
        buttons: vec![
            choice(task, "hour-less", "−1h", Action::Minutes(-STEP_LARGE)),
            choice(task, "minutes-less", "−15", Action::Minutes(-STEP_SMALL)),
            choice(task, "minutes-more", "+15", Action::Minutes(STEP_SMALL)),
            choice(task, "hour-more", "+1h", Action::Minutes(STEP_LARGE)),
        ],
    });
    entries.push(MenuEntry::Separator);
    match preview(d, now) {
        Ok((_, said)) => {
            entries.push(MenuEntry::Note(said.into()));
            entries.push(item(
                "reminder-apply",
                Icon::Check,
                "Set reminder",
                MenuAction::Reminder(task, Action::Apply),
            ));
        }
        Err(why) => entries.push(MenuEntry::Note(why.into())),
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::menus::items;

    /// Saturday 21 November 2026, 10:35 (UTC, as the fixtures).
    fn now() -> Now {
        Now::fixed("2026-11-21T10:35:00Z")
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    fn task() -> TaskId {
        TaskId::from_uuid(uuid::Uuid::from_u128(7))
    }

    #[test]
    fn presets_are_later_today_tomorrow_morning_and_next_monday() {
        let n = now();
        assert_eq!(Preset::LaterToday.at(&n), Some(ts("2026-11-21T13:00:00Z")));
        assert_eq!(
            Preset::TomorrowMorning.at(&n),
            Some(ts("2026-11-22T09:00:00Z"))
        );
        assert_eq!(Preset::NextWeek.at(&n), Some(ts("2026-11-23T09:00:00Z")));
        // A whole hour stays as it is.
        assert_eq!(
            Preset::LaterToday.at(&Now::fixed("2026-11-21T10:00:00Z")),
            Some(ts("2026-11-21T12:00:00Z"))
        );
        // Late in the evening there is no "later today" left.
        assert_eq!(
            Preset::LaterToday.at(&Now::fixed("2026-11-21T21:30:00Z")),
            None
        );
        let menu = entries(task(), None, None, &Now::fixed("2026-11-21T21:30:00Z"));
        assert!(items(&menu).all(|i| i.key != "later-today"));
        // On a Monday, next week is the Monday after.
        assert_eq!(
            Preset::NextWeek.at(&Now::fixed("2026-11-23T08:00:00Z")),
            Some(ts("2026-11-30T09:00:00Z"))
        );
    }

    #[test]
    fn the_picker_ticks_the_current_choice_and_offers_remove_only_when_set() {
        let n = now();
        let none = entries(task(), None, None, &n);
        let keys: Vec<_> = items(&none).map(|i| i.key.to_string()).collect();
        assert_eq!(
            keys,
            ["later-today", "tomorrow-morning", "next-week", "custom"]
        );
        assert!(items(&none).all(|i| !i.checked));
        let tomorrow = Preset::TomorrowMorning.at(&n);
        let set = entries(task(), tomorrow, None, &n);
        let checked: Vec<_> = items(&set).filter(|i| i.checked).map(|i| &i.key).collect();
        assert_eq!(checked, ["tomorrow-morning"]);
        assert!(items(&set).any(|i| i.key == "remove-reminder" && i.danger));
        // A time no preset gives: Custom is the current choice.
        let other = entries(task(), Some(ts("2026-11-25T16:20:00Z")), None, &n);
        let checked: Vec<_> = items(&other)
            .filter(|i| i.checked)
            .map(|i| &i.key)
            .collect();
        assert_eq!(checked, ["custom"]);
        assert_eq!(
            items(&set)
                .find(|i| i.key == "tomorrow-morning")
                .and_then(|i| i.hint.clone()),
            Some("Sun 09:00".into())
        );
    }

    #[test]
    fn the_custom_page_starts_where_it_makes_sense() {
        let n = now();
        let at = |d: Draft| d.local().map(|l| resolve(l, &n.tz).0);
        // The reminder's own time.
        let own = ts("2026-11-24T16:20:00Z");
        assert_eq!(at(Draft::new(Some(own), None, &n)), Some(own));
        // A task due later: 09:00 that day.
        let due = Date::constant(2026, 11, 27);
        assert_eq!(
            at(Draft::new(None, Some(due), &n)),
            Some(ts("2026-11-27T09:00:00Z"))
        );
        // Otherwise (or due today) the next whole hour.
        assert_eq!(
            at(Draft::new(None, Some(n.today()), &n)),
            Some(ts("2026-11-21T11:00:00Z"))
        );
        assert_eq!(
            at(Draft::new(None, None, &Now::fixed("2026-11-21T23:10:00Z"))),
            Some(ts("2026-11-22T00:00:00Z"))
        );
    }

    #[test]
    fn steps_and_typing_set_the_day_and_time_and_the_past_cant_be_set() {
        let n = now();
        let today = n.today();
        let mut d = Draft::new(None, None, &n);
        d.edit(Action::Day(1), today);
        d.edit(Action::Minutes(-STEP_LARGE), today);
        d.edit(Action::Minutes(STEP_SMALL), today);
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-22T10:15:00Z")));
        // Typing makes the time.
        for k in ["1", "4", "3", "0"] {
            assert!(d.type_key(k));
        }
        assert_eq!(d.time_label(), "14:30");
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-22T14:30:00Z")));
        assert!(d.type_key("backspace"));
        assert_eq!(d.time_label(), "1:43");
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-22T01:43:00Z")));
        for _ in 0..3 {
            d.type_key("backspace");
        }
        assert_eq!(d.time_label(), "10:15");
        assert!(!d.type_key("x"));
        // A time that isn't one can't be set; a fifth digit starts again.
        for k in ["2", "9", "7", "5"] {
            d.type_key(k);
        }
        assert_eq!(d.time_label(), "29:75");
        assert_eq!(preview(d, &n), Err(BAD_TIME));
        let menu = entries(task(), None, Some(d), &n);
        assert!(items(&menu).all(|i| i.key != "reminder-apply"));
        d.type_key("9");
        assert_eq!(d.time_label(), "9:__");
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-22T09:00:00Z")));
        d.type_key("9");
        assert_eq!(preview(d, &n), Err(BAD_TIME));
        // Stepping takes nothing invalid in, and never goes before today.
        d.edit(Action::Day(-7), today);
        assert_eq!(preview(d, &n), Err(PAST));
        d.edit(Action::Minutes(STEP_LARGE), today);
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-21T11:15:00Z")));
        // A valid typed time is taken in before a step.
        for k in ["1", "6"] {
            d.type_key(k);
        }
        d.edit(Action::Minutes(STEP_SMALL), today);
        assert_eq!(preview(d, &n).map(|p| p.0), Ok(ts("2026-11-21T16:15:00Z")));
        // Stepping over midnight moves the day.
        let mut late = Draft::new(Some(ts("2026-11-21T23:45:00Z")), None, &n);
        late.edit(Action::Minutes(STEP_SMALL), today);
        assert_eq!(
            preview(late, &n).map(|p| p.0),
            Ok(ts("2026-11-22T00:00:00Z"))
        );
        let menu = entries(task(), None, Some(late), &n);
        assert!(items(&menu).any(|i| i.key == "reminder-apply"));
    }

    #[test]
    fn a_time_the_clocks_change_says_which_time_is_saved() {
        let tz = TimeZone::get("Europe/Prague").expect("bundled tz");
        let n = Now {
            at: ts("2026-03-20T10:00:00Z"),
            tz: tz.clone(),
        };
        // 29 March 2026, 02:30 doesn't exist in Prague: it saves 03:30 (CEST).
        let mut skipped = Draft::new(None, Some(Date::constant(2026, 3, 29)), &n);
        for k in ["0", "2", "3", "0"] {
            skipped.type_key(k);
        }
        let (at, said) = preview(skipped, &n).expect("settable");
        assert_eq!(at, ts("2026-03-29T01:30:00Z"));
        assert!(said.starts_with("02:30 doesn’t exist that day"), "{said}");
        // 25 October 2026, 02:30 happens twice: the first (CEST) is saved.
        let n = Now {
            at: ts("2026-10-20T10:00:00Z"),
            tz,
        };
        let mut twice = Draft::new(None, Some(Date::constant(2026, 10, 25)), &n);
        for k in ["0", "2", "3", "0"] {
            twice.type_key(k);
        }
        let (at, said) = preview(twice, &n).expect("settable");
        assert_eq!(at, ts("2026-10-25T00:30:00Z"));
        assert!(said.contains("happens twice"), "{said}");
        let (_, plain) = preview(Draft::new(None, None, &n), &n).expect("settable");
        assert_eq!(plain, "Reminds you Today, 13:00.");
    }
}
