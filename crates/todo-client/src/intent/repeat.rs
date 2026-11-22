//! Repeat choices shared by clients. The due date (or the owner's local today) anchors a preset.
use todo_core::State;
use todo_core::command::{Command, TaskPatch};
use todo_core::ids::TaskId;
use todo_core::recurrence::{Recurrence, RecurrenceError, Rule};
use todo_core::time::{Date, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
}

impl Preset {
    /// Preserve the intended month-day when the most recent occurrence was clamped to a shorter month.
    pub fn rule_for(self, anchor: Date, current: Option<&Recurrence>) -> Rule {
        if self == Self::Monthly
            && let Some(Recurrence {
                rule: Rule::Monthly { day, .. },
                ..
            }) = current
        {
            return Rule::Monthly {
                interval: 1,
                day: *day,
            };
        }
        self.rule(anchor)
    }

    pub fn rule(self, anchor: Date) -> Rule {
        match self {
            Self::Daily => Rule::Daily { interval: 1 },
            Self::Weekdays => Rule::Weekly {
                interval: 1,
                days: Weekday::ALL[..5].iter().copied().collect(),
            },
            Self::Weekly => Rule::Weekly {
                interval: 1,
                days: [Weekday::of(anchor)].into(),
            },
            Self::Monthly => Rule::Monthly {
                interval: 1,
                day: anchor.day() as u8,
            },
            Self::Yearly => Rule::Yearly { interval: 1 },
        }
    }
}

/// Change just the pattern, preserving an imported end date. Starting an undated task and setting
/// its repeat is a single patch (and hence a single undo); stopping never removes its due date.
pub fn set_repeat(
    state: &State,
    id: TaskId,
    rule: Option<Rule>,
    today: Date,
) -> Result<Option<Command>, RecurrenceError> {
    let Some(t) = state.tasks.get(&id).filter(|_| state.is_live(id.into())) else {
        return Ok(None);
    };
    Ok(patch(t.due, t.recurrence.as_ref(), rule, today)?.map(|p| super::update_task(id, p)))
}

fn patch(
    due: Option<Date>,
    old: Option<&Recurrence>,
    rule: Option<Rule>,
    today: Date,
) -> Result<Option<TaskPatch>, RecurrenceError> {
    let next = rule.map(|rule| Recurrence {
        rule,
        until: old.and_then(|r| r.until),
    });
    let due = match (&next, due) {
        (Some(r), None) => Some(Some(r.first_due(today)?)),
        (Some(r), Some(_)) => {
            r.validate()?;
            None
        }
        (None, _) => None,
    };
    Ok((old != next.as_ref()).then_some(TaskPatch {
        due,
        recurrence: Some(next),
        ..Default::default()
    }))
}

/// Plain English, including an optional inclusive end date.
pub fn describe_repeat(r: &Recurrence) -> String {
    let every = |n: u32, unit: &str| {
        if n == 1 {
            format!("Every {unit}")
        } else {
            format!("Every {n} {unit}s")
        }
    };
    let mut text = match &r.rule {
        Rule::Daily { interval } => every(*interval, "day"),
        Rule::Weekly { interval, days } => {
            let names = [
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ];
            if *interval == 1 && days.iter().copied().eq(Weekday::ALL[..5].iter().copied()) {
                "Every weekday".into()
            } else if *interval == 1
                && let Some(d) = days.first().filter(|_| days.len() == 1)
            {
                format!("Every {}", names[d.offset()])
            } else {
                format!(
                    "{} on {}",
                    every(*interval, "week"),
                    days.iter()
                        .map(|d| &names[d.offset()][..3])
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Rule::Monthly { interval, day } => {
            let suffix = match (day % 10, day) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            };
            format!("{} on the {day}{suffix}", every(*interval, "month"))
        }
        Rule::Yearly { interval } => every(*interval, "year"),
    };
    if let Some(date) = r.until {
        text.push_str(&format!(", until {}", super::quick_add::long_date(date)));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    const MON: Date = Date::constant(2025, 3, 31);
    #[test]
    fn presets_read_as_their_anchored_rules() {
        for (preset, words) in [
            (Preset::Daily, "Every day"),
            (Preset::Weekdays, "Every weekday"),
            (Preset::Weekly, "Every Monday"),
            (Preset::Monthly, "Every month on the 31st"),
            (Preset::Yearly, "Every year"),
        ] {
            assert_eq!(describe_repeat(&Recurrence::new(preset.rule(MON))), words);
        }
        let rule = Rule::Weekly {
            interval: 2,
            days: [Weekday::Thu, Weekday::Mon].into(),
        };
        assert_eq!(
            describe_repeat(&Recurrence::new(rule)),
            "Every 2 weeks on Mon, Thu"
        );
        let r = Recurrence {
            rule: Rule::Weekly {
                interval: 1,
                days: [
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Sun,
                ]
                .into(),
            },
            until: Some(MON),
        };
        assert_eq!(
            describe_repeat(&r),
            "Every week on Mon, Tue, Wed, Thu, Sun, until 31 Mar 2025"
        );
    }
    #[test]
    fn first_due_matches_the_schedule() {
        let sat = Date::constant(2025, 3, 29);
        let tue = Date::constant(2025, 4, 1);
        for (today, rule, expected) in [
            (sat, Preset::Weekdays.rule(sat), MON),
            (MON, Preset::Weekdays.rule(MON), MON),
            (
                tue,
                Rule::Weekly {
                    interval: 2,
                    days: [Weekday::Mon, Weekday::Thu].into(),
                },
                Date::constant(2025, 4, 3),
            ),
            (
                Date::constant(2025, 4, 30),
                Rule::Monthly {
                    interval: 1,
                    day: 31,
                },
                Date::constant(2025, 4, 30),
            ),
        ] {
            assert_eq!(
                patch(None, None, Some(rule), today).unwrap().unwrap().due,
                Some(Some(expected))
            );
        }
    }
    #[test]
    fn month_ordinals_and_year_intervals() {
        for (day, suffix) in [
            (1, "1st"),
            (3, "3rd"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
        ] {
            assert_eq!(
                describe_repeat(&Recurrence::new(Rule::Monthly { interval: 1, day })),
                format!("Every month on the {suffix}")
            );
        }
        assert_eq!(
            describe_repeat(&Recurrence::new(Rule::Yearly { interval: 3 })),
            "Every 3 years"
        );
    }
    #[test]
    fn patch_is_atomic_and_preserves_unedited_fields() {
        let first = patch(None, None, Some(Preset::Daily.rule(MON)), MON)
            .unwrap()
            .unwrap();
        assert_eq!(first.due, Some(Some(MON)));
        let old = Recurrence {
            rule: Preset::Daily.rule(MON),
            until: Some(MON),
        };
        assert!(
            patch(Some(MON), Some(&old), Some(old.rule.clone()), MON)
                .unwrap()
                .is_none()
        );
        let edited = patch(Some(MON), Some(&old), Some(Preset::Monthly.rule(MON)), MON)
            .unwrap()
            .unwrap();
        assert_eq!(edited.recurrence.flatten().unwrap().until, Some(MON));
        assert_eq!(edited.due, None);
        let clear = patch(Some(MON), Some(&old), None, MON).unwrap().unwrap();
        assert_eq!(clear.recurrence, Some(None));
        assert_eq!(clear.due, None);
        assert!(patch(None, None, None, MON).unwrap().is_none());
        assert_eq!(
            patch(
                None,
                None,
                Some(Rule::Weekly {
                    interval: 1,
                    days: Default::default()
                }),
                MON
            ),
            Err(RecurrenceError::NoWeekdays)
        );
        for interval in [0, 1000] {
            assert_eq!(
                patch(None, None, Some(Rule::Yearly { interval }), MON),
                Err(RecurrenceError::BadInterval)
            );
        }
    }
}
