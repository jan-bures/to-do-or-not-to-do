//! The reminder picker: presets (Later today, Tomorrow morning, Next week), a custom day and time, and
//! Remove. Only a preset, Set reminder or Remove writes a command, through the shared
//! `todo_client::intent::reminder` (which reminder the row edits, and an edit on one that changed
//! meanwhile writes nothing).
//!
//! Times are the owner's local ones. A custom time the clocks skip (spring forward) saves the time
//! the gap moves it to, and one that happens twice (fall back) saves the first, as on Android; the
//! picker says so before it is set.

use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
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

    /// When it reminds, from `now`: Later today is the first whole hour at least two hours on (and only
    /// while that is still today; when the clocks go back, the hour that comes twice is taken at its
    /// first instant that far on, never an hour early); Tomorrow morning is 09:00 tomorrow; Next week
    /// is 09:00 next Monday.
    pub fn at(self, now: &Now) -> Option<Timestamp> {
        let local = now.at.to_zoned_datetime(now.tz.clone());
        let today = local.date();
        match self {
            Self::LaterToday => {
                let later = local
                    .checked_add(jiff::Span::new().hours(LATER_HOURS))
                    .ok()?;
                let threshold = later.timestamp();
                let whole = whole_hour(later.datetime(), jiff::RoundMode::Ceil)?;
                if whole.date() != today {
                    return None;
                }
                let ambiguous = now.tz.to_ambiguous_zoned_datetime(whole);
                [ambiguous.clone().earlier(), ambiguous.later()]
                    .into_iter()
                    .filter_map(Result::ok)
                    .map(|z| z.timestamp())
                    .filter(|at| *at >= threshold)
                    .min()
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
    /// A preset, at the instant its row showed: that instant is what is saved, even if the picker
    /// stayed open past a whole hour (in milliseconds since the epoch, which keeps menu items small).
    Preset(Preset, i64),
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

/// Preset instants and their local offsets, captured on opening. A redraw or Enter uses the same
/// choices even after the hour or the time zone changes. The custom page still uses the live clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presets([Option<(Timestamp, Offset)>; Preset::ALL.len()]);

impl Presets {
    pub fn new(now: &Now) -> Self {
        Self(Preset::ALL.map(|p| p.at(now).map(|at| (at, now.tz.to_offset(at)))))
    }
}

/// The picker for `task`, whose row showed the reminder at `seen` (or none): its presets and Remove,
/// or (`draft`) the custom page.
pub fn entries(
    task: TaskId,
    seen: Option<Timestamp>,
    draft: Option<Draft>,
    presets: &Presets,
    now: &Now,
) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    let Some(d) = draft else {
        entries.push(MenuEntry::Heading("Reminder".into()));
        for (p, resolved) in Preset::ALL.into_iter().zip(presets.0) {
            let Some((at, offset)) = resolved else {
                continue;
            };
            let zone = TimeZone::fixed(offset);
            let clock = labels::clock(at, &zone);
            let hint = if p == Preset::LaterToday {
                clock
            } else {
                let day = at.to_zoned_datetime(zone).date();
                todo_client::date_format::date_time(
                    labels::weekday(day).get(..3).unwrap_or_default(),
                    &clock,
                )
            };
            entries.push(MenuEntry::Item(MenuItem {
                icon: p.icon(),
                hint: Some(hint.into()),
                checked: seen == Some(at),
                ..choice(
                    task,
                    p.key(),
                    p.label(),
                    Action::Preset(p, at.as_millisecond()),
                )
            }));
        }
        let custom = item(
            "custom",
            Icon::Settings,
            "Pick a day and time…",
            MenuAction::Reminder(task, Action::Custom),
        );
        let preset = presets.0.iter().flatten().any(|&(at, _)| seen == Some(at));
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
    entries.push(MenuEntry::Heading("Pick a day and time".into()));
    entries.push(MenuEntry::Controls {
        label: "Day",
        value: Some(now.dates().relative(d.day)),
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

    fn entries(
        task: TaskId,
        seen: Option<Timestamp>,
        draft: Option<Draft>,
        now: &Now,
    ) -> Vec<MenuEntry> {
        super::entries(task, seen, draft, &Presets::new(now), now)
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
    fn the_presets_page_keeps_the_instants_it_opened_with_also_for_the_keyboard() {
        use crate::model::menus::chosen;
        let opened = Now::fixed("2026-11-21T10:59:00Z");
        let presets = Presets::new(&opened);
        let drawn = super::entries(task(), None, None, &presets, &opened);
        // Enter reads the page after both the hour and the system time zone have changed.
        let later = Now {
            at: ts(NOW_LATE),
            date_format: Default::default(),
            tz: TimeZone::get("America/New_York").expect("bundled tz"),
        };
        let entered = super::entries(task(), None, None, &presets, &later);
        assert_eq!(drawn, entered);
        assert_eq!(
            chosen(&entered, Some("tomorrow-morning")),
            Some(MenuAction::Reminder(
                task(),
                Action::Preset(
                    Preset::TomorrowMorning,
                    ts("2026-11-22T09:00:00Z").as_millisecond()
                )
            ))
        );
        let action = chosen(&entered, Some("later-today")).expect("Enter on Later today");
        assert_eq!(
            action,
            MenuAction::Reminder(
                task(),
                Action::Preset(
                    Preset::LaterToday,
                    ts("2026-11-21T13:00:00Z").as_millisecond()
                )
            )
        );
        // The custom page checks against the time now.
        let d = Draft::new(
            Some(ts("2026-11-21T11:00:00Z")),
            None,
            &Now::fixed(NOW_LATE),
        );
        let late = Now::fixed(NOW_LATE);
        let custom = super::entries(task(), None, Some(d), &presets, &late);
        assert!(items(&custom).all(|i| i.key != "reminder-apply"));
    }

    const NOW_LATE: &str = "2026-11-21T11:01:00Z";

    #[test]
    fn later_today_is_never_early_when_the_clocks_go_back() {
        let tz = TimeZone::get("America/New_York").expect("bundled tz");
        // Midnight EDT on 1 November 2026: two hours on is 01:00 EST, the second 01:00 that night.
        let n = Now {
            at: ts("2026-11-01T04:00:00Z"),
            date_format: Default::default(),
            tz: tz.clone(),
        };
        assert_eq!(Preset::LaterToday.at(&n), Some(ts("2026-11-01T06:00:00Z")));
        // Half an hour later, two hours on is 01:30 EST, and the next whole hour 02:00 EST.
        let n = Now {
            at: ts("2026-11-01T04:30:00Z"),
            date_format: Default::default(),
            tz: tz.clone(),
        };
        assert_eq!(Preset::LaterToday.at(&n), Some(ts("2026-11-01T07:00:00Z")));
        // At least two hours, whatever the clocks do, at every quarter hour of that day.
        for q in 0..96 {
            let at = Timestamp::from_second(ts("2026-11-01T04:00:00Z").as_second() + q * 900)
                .expect("ts");
            let n = Now {
                at,
                tz: tz.clone(),
                date_format: Default::default(),
            };
            if let Some(later) = Preset::LaterToday.at(&n) {
                let gap = later.as_second() - at.as_second();
                assert!((7200..7200 + 3600).contains(&gap), "{at}: {later}");
            }
        }
    }

    #[test]
    fn a_preset_saves_the_instant_its_row_showed() {
        let n = now();
        let menu = entries(task(), None, None, &n);
        for p in Preset::ALL {
            let item = items(&menu).find(|i| i.key == p.key()).expect("offered");
            let MenuAction::Reminder(_, Action::Preset(_, at)) = item.action else {
                panic!("a preset");
            };
            let at = Timestamp::from_millisecond(at).expect("an instant");
            assert_eq!(Some(at), p.at(&n));
            assert_eq!(
                item.hint.as_ref().map(|h| h.to_string()),
                Some(match p {
                    Preset::LaterToday => labels::clock(at, &n.tz),
                    _ => format!(
                        "{}, {}",
                        &labels::weekday(n.date_of(at))[..3],
                        labels::clock(at, &n.tz)
                    ),
                })
            );
        }
        // Opened at 10:59, chosen at 11:01: still 13:00, as shown, not the 14:00 that is now.
        let opened = Now::fixed("2026-11-21T10:59:00Z");
        let menu = entries(task(), None, None, &opened);
        let item = items(&menu)
            .find(|i| i.key == "later-today")
            .expect("offered");
        assert_eq!(
            item.action,
            MenuAction::Reminder(
                task(),
                Action::Preset(
                    Preset::LaterToday,
                    ts("2026-11-21T13:00:00Z").as_millisecond()
                )
            )
        );
        assert_eq!(
            Preset::LaterToday.at(&Now::fixed("2026-11-21T11:01:00Z")),
            Some(ts("2026-11-21T14:00:00Z"))
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
            Some("Sun, 09:00".into())
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
            date_format: Default::default(),
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
            date_format: Default::default(),
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
