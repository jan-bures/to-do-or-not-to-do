//! Repeat picker and its uncommitted custom schedule. Only a preset or Apply writes a command.
use super::{MenuAction, MenuEntry, MenuItem, item, with};
use crate::theme::Icon;
use todo_client::intent::repeat::{
    Preset, RepeatDraft, UNDATED_NOTE, Unit, describe_repeat, error_text,
};
use todo_core::{
    ids::TaskId,
    model::Task,
    recurrence::{MAX_INTERVAL, Recurrence, Rule},
    time::{Date, Weekday},
};

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
    fields: RepeatDraft,
    typing: bool,
}
impl Draft {
    pub fn new(task: &Task, today: Date) -> Self {
        Self {
            fields: RepeatDraft::new(task.due, task.recurrence.as_ref(), today),
            typing: false,
        }
    }
    pub fn rule(self) -> Rule {
        self.fields.rule()
    }
    pub fn edit(&mut self, a: Action) {
        match a {
            Action::Step(delta) => {
                self.fields.interval = self
                    .fields
                    .interval
                    .saturating_add_signed(delta)
                    .clamp(1, MAX_INTERVAL)
            }
            Action::Unit(unit) => self.fields.unit = unit,
            Action::Day(day) => self.fields.days[day.offset()] = !self.fields.days[day.offset()],
            _ => return,
        }
        self.typing = false;
    }
    /// First digit replaces the interval; Backspace removes the last digit. Zero means an empty or
    /// invalid interval and cannot be applied, just like a weekly rule with no days.
    pub fn type_key(&mut self, key: &str) -> bool {
        match key {
            "backspace" => self.fields.interval /= 10,
            _ if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
                let base = if self.typing { self.fields.interval } else { 0 };
                let n = base * 10 + u32::from(key.as_bytes()[0] - b'0');
                if n <= MAX_INTERVAL {
                    self.fields.interval = n;
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
    let mut starts = None;
    if let Some(d) = draft {
        entries.push(MenuEntry::Heading("Custom repeat".into()));
        entries.push(MenuEntry::Controls {
            label: "Interval · type or step",
            value: Some(d.fields.interval.to_string()),
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
            .map(|(name, unit)| choice(t.id, name, name, Action::Unit(unit), d.fields.unit == unit))
            .collect(),
        });
        if d.fields.unit == Unit::Week {
            entries.push(MenuEntry::Controls {
                label: "On weekdays",
                value: None,
                buttons: Weekday::ALL
                    .into_iter()
                    .zip(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"])
                    .map(|(day, name)| {
                        choice(
                            t.id,
                            name,
                            name,
                            Action::Day(day),
                            d.fields.days[day.offset()],
                        )
                    })
                    .collect(),
            });
        }
        entries.push(MenuEntry::Separator);
        let r = Recurrence {
            rule: d.rule(),
            until: t.recurrence.as_ref().and_then(|r| r.until),
        };
        let valid = if t.due.is_none() {
            r.first_due(today).map(|date| starts = Some(date))
        } else {
            r.validate()
        };
        match valid {
            Ok(()) => {
                entries.push(MenuEntry::Note(describe_repeat(&r).into()));
                entries.push(item(
                    "repeat-apply",
                    Icon::Check,
                    "Apply repeat",
                    MenuAction::Repeat(t.id, Action::Apply),
                ));
            }
            Err(e) => entries.push(MenuEntry::Note(error_text(e).into())),
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
            ("monthly", "Monthly".into(), Preset::Monthly),
            ("yearly", "Yearly".into(), Preset::Yearly),
        ] {
            let rule = preset.rule_for(anchor, t.recurrence.as_ref());
            let label = if let Rule::Monthly { day, .. } = rule {
                format!("Monthly on day {day}")
            } else {
                label
            };
            let on = t.recurrence.as_ref().is_some_and(|r| r.rule == rule);
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
            starts
                .map(|date| format!("Due will be {}.", crate::model::labels::long_date(date)))
                .unwrap_or_else(|| UNDATED_NOTE.into())
                .into(),
        ));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    #[test]
    fn a_clamped_month_end_stays_on_day_31_in_the_picker_and_command() {
        let f = fixtures::my_day();
        let mut t = f.state.tasks[&f.ui.task.expect("task")].clone();
        let april_end = Date::constant(2025, 4, 30);
        t.due = Some(april_end);
        let rule = Rule::Monthly {
            interval: 1,
            day: 31,
        };
        t.recurrence = Some(Recurrence::new(rule.clone()));
        let menu = entries(&t, april_end, None);
        let monthly = super::super::items(&menu)
            .find(|i| i.key == "monthly")
            .expect("monthly");
        assert!(monthly.checked);
        assert_eq!(monthly.label, "Monthly on day 31");
        assert_eq!(
            Preset::Monthly.rule_for(april_end, t.recurrence.as_ref()),
            rule
        );
    }
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
