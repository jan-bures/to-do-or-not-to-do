//! Repeat picker and its uncommitted custom schedule. Only a preset or Apply writes a command.
use super::{MenuAction, MenuEntry, MenuItem, item, with};
use crate::theme::Icon;
use todo_client::intent::repeat::{Preset, describe_repeat};
use todo_core::{
    ids::TaskId,
    model::Task,
    recurrence::{MAX_INTERVAL, Recurrence, Rule},
    time::{Date, Weekday},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Preset(Option<Preset>),
    Custom,
    Step(i32),
    Unit(Unit),
    Day(Weekday),
    Apply,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draft {
    interval: u32,
    typing: bool,
    unit: Unit,
    days: [bool; 7],
    month_day: u8,
}
impl Draft {
    pub fn new(task: &Task, today: Date) -> Self {
        let anchor = task.due.unwrap_or(today);
        let mut d = Self {
            interval: 1,
            typing: false,
            unit: Unit::Day,
            days: [false; 7],
            month_day: anchor.day() as u8,
        };
        d.days[Weekday::of(anchor).offset()] = true;
        if let Some(r) = &task.recurrence {
            (d.unit, d.interval) = match &r.rule {
                Rule::Daily { interval } => (Unit::Day, *interval),
                Rule::Weekly { interval, days } => {
                    d.days = Weekday::ALL.map(|day| days.contains(&day));
                    (Unit::Week, *interval)
                }
                Rule::Monthly { interval, day } => {
                    d.month_day = *day;
                    (Unit::Month, *interval)
                }
                Rule::Yearly { interval } => (Unit::Year, *interval),
            };
        }
        d
    }
    pub fn rule(self) -> Rule {
        match self.unit {
            Unit::Day => Rule::Daily {
                interval: self.interval,
            },
            Unit::Year => Rule::Yearly {
                interval: self.interval,
            },
            Unit::Month => Rule::Monthly {
                interval: self.interval,
                day: self.month_day,
            },
            Unit::Week => Rule::Weekly {
                interval: self.interval,
                days: Weekday::ALL
                    .into_iter()
                    .filter(|d| self.days[d.offset()])
                    .collect(),
            },
        }
    }
    pub fn edit(&mut self, a: Action) {
        match a {
            Action::Step(delta) => {
                self.interval = self
                    .interval
                    .saturating_add_signed(delta)
                    .clamp(1, MAX_INTERVAL)
            }
            Action::Unit(unit) => self.unit = unit,
            Action::Day(day) => self.days[day.offset()] = !self.days[day.offset()],
            _ => return,
        }
        self.typing = false;
    }
    /// First digit replaces the interval; Backspace removes the last digit. Zero means an empty or
    /// invalid interval and cannot be applied, just like a weekly rule with no days.
    pub fn type_key(&mut self, key: &str) -> bool {
        match key {
            "backspace" => self.interval /= 10,
            _ if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
                let base = if self.typing { self.interval } else { 0 };
                let n = base * 10 + u32::from(key.as_bytes()[0] - b'0');
                if n <= MAX_INTERVAL {
                    self.interval = n;
                }
            }
            _ => return false,
        }
        self.typing = true;
        true
    }
}

fn choice(
    task: TaskId,
    key: &'static str,
    label: impl Into<gpui::SharedString>,
    a: Action,
    checked: bool,
) -> MenuItem {
    MenuItem {
        key: key.into(),
        label: label.into(),
        action: MenuAction::Repeat(task, a),
        checked,
        icon: Icon::Repeat,
        glyph: None,
        dot: None,
        danger: false,
        hint: None,
        more: None,
    }
}

pub fn entries(t: &Task, today: Date, draft: Option<Draft>) -> Vec<MenuEntry> {
    let anchor = t.due.unwrap_or(today);
    let mut entries = vec![];
    if let Some(d) = draft {
        entries.push(MenuEntry::Heading("Custom repeat".into()));
        entries.push(MenuEntry::Controls {
            label: "Interval · type or step",
            value: Some(d.interval.to_string()),
            buttons: vec![
                choice(t.id, "less", "−", Action::Step(-1), false),
                choice(t.id, "more", "+", Action::Step(1), false),
            ],
        });
        entries.push(MenuEntry::Controls {
            label: "Unit",
            value: None,
            buttons: [
                ("Day", Unit::Day),
                ("Week", Unit::Week),
                ("Month", Unit::Month),
                ("Year", Unit::Year),
            ]
            .into_iter()
            .map(|(name, unit)| choice(t.id, name, name, Action::Unit(unit), d.unit == unit))
            .collect(),
        });
        if d.unit == Unit::Week {
            entries.push(MenuEntry::Controls {
                label: "On weekdays",
                value: None,
                buttons: Weekday::ALL
                    .into_iter()
                    .zip(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"])
                    .map(|(day, name)| {
                        choice(t.id, name, name, Action::Day(day), d.days[day.offset()])
                    })
                    .collect(),
            });
        }
        entries.push(MenuEntry::Separator);
        let r = Recurrence {
            rule: d.rule(),
            until: t.recurrence.as_ref().and_then(|r| r.until),
        };
        match r.validate() {
            Ok(()) => {
                entries.push(MenuEntry::Note(describe_repeat(&r).into()));
                entries.push(item(
                    "repeat-apply",
                    Icon::Check,
                    "Apply repeat",
                    MenuAction::Repeat(t.id, Action::Apply),
                ));
            }
            Err(e) => entries.push(MenuEntry::Note(match e {
                todo_core::recurrence::RecurrenceError::NoWeekdays => {
                    "Choose at least one weekday.".into()
                }
                _ => format!("Choose an interval from 1 to {MAX_INTERVAL}.").into(),
            })),
        }
    } else {
        entries.push(MenuEntry::Heading("Repeat".into()));
        let mut matched = false;
        for (key, label, preset) in [
            ("daily", "Daily".to_owned(), Preset::Daily),
            ("weekdays", "Weekdays".into(), Preset::Weekdays),
            (
                "weekly",
                format!("Weekly on {}", crate::model::labels::weekday(anchor)),
                Preset::Weekly,
            ),
            (
                "monthly",
                format!("Monthly on day {}", anchor.day()),
                Preset::Monthly,
            ),
            ("yearly", "Yearly".into(), Preset::Yearly),
        ] {
            let on = t
                .recurrence
                .as_ref()
                .is_some_and(|r| r.rule == preset.rule(anchor));
            matched |= on;
            entries.push(MenuEntry::Item(choice(
                t.id,
                key,
                label,
                Action::Preset(Some(preset)),
                on,
            )));
        }
        entries.push(with(
            item(
                "custom",
                Icon::Settings,
                "Custom…",
                MenuAction::Repeat(t.id, Action::Custom),
            ),
            |i| MenuItem {
                checked: t.recurrence.is_some() && !matched,
                ..i
            },
        ));
        entries.push(MenuEntry::Separator);
        entries.push(with(
            item(
                "stop-repeat",
                Icon::X,
                "Don’t repeat",
                MenuAction::Repeat(t.id, Action::Preset(None)),
            ),
            |i| MenuItem {
                checked: t.recurrence.is_none(),
                ..i
            },
        ));
    }
    if t.due.is_none() {
        entries.push(MenuEntry::Note(
            "Choosing a repeat also sets Due to today.".into(),
        ));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    #[test]
    fn custom_is_not_applied_until_valid_and_keeps_the_imported_month_day() {
        let f = fixtures::my_day();
        let mut t = f.state.tasks[&f.ui.task.expect("task")].clone();
        t.recurrence = Some(Recurrence::new(Rule::Monthly {
            interval: 8,
            day: 31,
        }));
        let mut d = Draft::new(&t, f.now.today());
        assert_eq!(
            Some(d.rule()),
            t.recurrence.as_ref().map(|r| r.rule.clone())
        );
        d.type_key("2");
        d.type_key("5");
        d.type_key("backspace");
        d.edit(Action::Unit(Unit::Week));
        let day = Weekday::of(t.due.unwrap_or(f.now.today()));
        d.edit(Action::Day(day));
        let can_apply = |d| {
            super::super::items(&entries(&t, f.now.today(), Some(d)))
                .any(|i| i.action == MenuAction::Repeat(t.id, Action::Apply))
        };
        assert!(!can_apply(d));
        d.edit(Action::Day(Weekday::Thu));
        assert!(can_apply(d));
        assert_eq!(
            d.rule(),
            Rule::Weekly {
                interval: 2,
                days: [Weekday::Thu].into()
            }
        );
        d.type_key("0");
        assert!(!can_apply(d));
    }
}
