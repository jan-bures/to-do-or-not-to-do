//! Recurrence rules: a small subset of RFC 5545 RRULE.
//!
//! Supported: daily, weekly (with weekdays), monthly by day of month, and yearly, each with an interval
//! and an optional `UNTIL` date. Recurrence follows Microsoft To Do semantics: the next task is created
//! when the current one is completed ("spawn next on completion"), with the next due date computed from
//! the completed task's due date (see [`Recurrence::next_after`]).

use crate::time::{Date, Weekday, add_days};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Largest accepted interval.
pub const MAX_INTERVAL: u32 = 999;

/// Errors from validating or parsing a recurrence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecurrenceError {
    /// The interval is zero or above [`MAX_INTERVAL`].
    #[error("interval must be between 1 and {MAX_INTERVAL}")]
    BadInterval,
    /// A weekly rule has no weekdays.
    #[error("weekly rule needs at least one weekday")]
    NoWeekdays,
    /// The day of month is outside 1..=31.
    #[error("day of month must be between 1 and 31")]
    BadMonthDay,
    /// The RRULE text could not be parsed or uses an unsupported feature.
    #[error("unsupported or malformed RRULE: {0}")]
    Rrule(String),
    /// A rule part that the frequency cannot represent, such as `BYDAY` in a monthly rule. Accepting it
    /// would silently drop the constraint.
    #[error("RRULE part {part} is not supported with FREQ={freq}")]
    PartNotSupported {
        /// The rule part, for example `BYDAY`.
        part: String,
        /// The rule's frequency, for example `MONTHLY`.
        freq: String,
    },
    /// A rule part appears more than once.
    #[error("RRULE part {0} appears more than once")]
    DuplicatePart(String),
}

/// The repeat pattern.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "freq", rename_all = "snake_case")]
pub enum Rule {
    /// Every `interval` days.
    Daily {
        /// Days between occurrences.
        interval: u32,
    },
    /// Every `interval` weeks on the given weekdays (weeks start on Monday).
    Weekly {
        /// Weeks between occurrences.
        interval: u32,
        /// Weekdays on which the task occurs.
        days: BTreeSet<Weekday>,
    },
    /// Every `interval` months on a day of the month. Months that are too short use their last day.
    Monthly {
        /// Months between occurrences.
        interval: u32,
        /// Day of month, 1..=31.
        day: u8,
    },
    /// Every `interval` years on the same month and day (29 February becomes 28 February).
    Yearly {
        /// Years between occurrences.
        interval: u32,
    },
}

/// A recurrence rule with an optional end date.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Recurrence {
    /// The repeat pattern.
    pub rule: Rule,
    /// Last date on which an occurrence may fall (inclusive).
    pub until: Option<Date>,
}

/// Parses an `UNTIL` value: exactly a date (`YYYYMMDD`) or a UTC date-time (`YYYYMMDDTHHMMSSZ`, of which
/// only the date is kept). Anything else, including trailing characters, is rejected.
fn parse_until(v: &str) -> Option<Date> {
    let b = v.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let num = |r: std::ops::Range<usize>| v[r].parse::<u8>().ok();
    match b.len() {
        8 if digits(0..8) => {}
        16 if digits(0..8)
            && b[8].eq_ignore_ascii_case(&b'T')
            && digits(9..15)
            && b[15].eq_ignore_ascii_case(&b'Z') =>
        {
            // Second 60 is a leap second, which RFC 5545 allows.
            if num(9..11)? > 23 || num(11..13)? > 59 || num(13..15)? > 60 {
                return None;
            }
        }
        _ => return None,
    }
    let y = v[..4].parse().ok()?;
    let m = v[4..6].parse().ok()?;
    let d = v[6..8].parse().ok()?;
    Date::new(y, m, d).ok()
}

fn clamp_day(year: i16, month: i8, day: u8) -> Option<Date> {
    let first = Date::new(year, month, 1).ok()?;
    let dim = u8::try_from(first.days_in_month()).ok()?;
    Date::new(year, month, i8::try_from(day.min(dim)).ok()?).ok()
}

impl Recurrence {
    /// Creates a recurrence without an end date.
    #[must_use]
    pub fn new(rule: Rule) -> Self {
        Self { rule, until: None }
    }

    /// Checks interval, weekday and day-of-month ranges.
    pub fn validate(&self) -> Result<(), RecurrenceError> {
        let interval = match &self.rule {
            Rule::Daily { interval } | Rule::Yearly { interval } => *interval,
            Rule::Weekly { interval, days } => {
                if days.is_empty() {
                    return Err(RecurrenceError::NoWeekdays);
                }
                *interval
            }
            Rule::Monthly { interval, day } => {
                if !(1..=31).contains(day) {
                    return Err(RecurrenceError::BadMonthDay);
                }
                *interval
            }
        };
        if (1..=MAX_INTERVAL).contains(&interval) {
            Ok(())
        } else {
            Err(RecurrenceError::BadInterval)
        }
    }

    /// The next occurrence strictly after `from`, or `None` if the rule has ended.
    ///
    /// For a task, `from` is its due date. The result does not depend on today's date, so completing an
    /// overdue task yields the next date after the old due date, which may itself be in the past.
    #[must_use]
    pub fn next_after(&self, from: Date) -> Option<Date> {
        let next = match &self.rule {
            Rule::Daily { interval } => add_days(from, i64::from(*interval))?,
            Rule::Weekly { interval, days } => {
                let wd = from.weekday().to_monday_zero_offset();
                let wd = usize::try_from(wd).ok()?;
                let later = days.iter().map(|d| d.offset()).filter(|&o| o > wd).min();
                match later {
                    Some(o) => add_days(from, i64::try_from(o - wd).ok()?)?,
                    None => {
                        let first = days.iter().map(|d| d.offset()).min()?;
                        let week_start = add_days(from, -i64::try_from(wd).ok()?)?;
                        let jump = 7 * i64::from(*interval) + i64::try_from(first).ok()?;
                        add_days(week_start, jump)?
                    }
                }
            }
            Rule::Monthly { interval, day } => {
                let this_month = clamp_day(from.year(), from.month(), *day)?;
                if this_month > from {
                    this_month
                } else {
                    let total = i64::from(from.year()) * 12
                        + i64::from(from.month() - 1)
                        + i64::from(*interval);
                    let year = i16::try_from(total.div_euclid(12)).ok()?;
                    let month = i8::try_from(total.rem_euclid(12) + 1).ok()?;
                    clamp_day(year, month, *day)?
                }
            }
            Rule::Yearly { interval } => {
                let year = from.year().checked_add(i16::try_from(*interval).ok()?)?;
                clamp_day(year, from.month(), u8::try_from(from.day()).ok()?)?
            }
        };
        match self.until {
            Some(u) if next > u => None,
            _ => Some(next),
        }
    }

    /// Formats the rule as an RRULE string such as `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE`.
    #[must_use]
    pub fn to_rrule(&self) -> String {
        let mut s = match &self.rule {
            Rule::Daily { interval } => format!("FREQ=DAILY;INTERVAL={interval}"),
            Rule::Weekly { interval, days } => {
                let codes: Vec<_> = days.iter().map(|d| d.rrule_code()).collect();
                format!("FREQ=WEEKLY;INTERVAL={interval};BYDAY={}", codes.join(","))
            }
            Rule::Monthly { interval, day } => {
                format!("FREQ=MONTHLY;INTERVAL={interval};BYMONTHDAY={day}")
            }
            Rule::Yearly { interval } => format!("FREQ=YEARLY;INTERVAL={interval}"),
        };
        if let Some(u) = self.until {
            s.push_str(&format!(
                ";UNTIL={:04}{:02}{:02}",
                u.year(),
                u.month(),
                u.day()
            ));
        }
        s
    }

    /// Parses the RRULE subset produced by [`to_rrule`](Self::to_rrule) (an optional `RRULE:` prefix is
    /// accepted; `UNTIL` must be exactly a date, `YYYYMMDD`, or a UTC date-time, `YYYYMMDDTHHMMSSZ`, of
    /// which only the date is kept).
    ///
    /// Unknown and repeated parts are rejected, and so are parts the frequency cannot represent
    /// (`BYDAY` outside weekly rules, `BYMONTHDAY` outside monthly rules), so a rule is never silently
    /// widened.
    pub fn from_rrule(text: &str) -> Result<Self, RecurrenceError> {
        let err = |m: &str| RecurrenceError::Rrule(m.to_owned());
        let text = text.trim();
        let text = text
            .get(..6)
            .filter(|p| p.eq_ignore_ascii_case("RRULE:"))
            .map_or(text, |_| &text[6..]);
        let (mut freq, mut interval, mut days, mut month_day, mut until) = (
            None::<String>,
            None::<u32>,
            None::<BTreeSet<Weekday>>,
            None::<u8>,
            None::<Date>,
        );
        let mut seen: Vec<String> = Vec::new();
        for part in text.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').ok_or_else(|| err("missing `=`"))?;
            let k = k.trim().to_ascii_uppercase();
            if seen.contains(&k) {
                return Err(RecurrenceError::DuplicatePart(k));
            }
            match k.as_str() {
                "FREQ" => freq = Some(v.to_ascii_uppercase()),
                "INTERVAL" => interval = Some(v.parse().map_err(|_| err("bad INTERVAL"))?),
                "BYDAY" => {
                    let mut set = BTreeSet::new();
                    for code in v.split(',') {
                        let day = Weekday::from_rrule_code(&code.to_ascii_uppercase())
                            .ok_or_else(|| err("bad BYDAY"))?;
                        if !set.insert(day) {
                            return Err(err("repeated weekday in BYDAY"));
                        }
                    }
                    days = Some(set);
                }
                "BYMONTHDAY" => month_day = Some(v.parse().map_err(|_| err("bad BYMONTHDAY"))?),
                "UNTIL" => until = Some(parse_until(v).ok_or_else(|| err("bad UNTIL"))?),
                other => return Err(RecurrenceError::Rrule(format!("unsupported part {other}"))),
            }
            seen.push(k);
        }
        let freq = freq.ok_or_else(|| err("missing FREQ"))?;
        let reject = |part: &str| RecurrenceError::PartNotSupported {
            part: part.to_owned(),
            freq: freq.clone(),
        };
        let interval = interval.unwrap_or(1);
        let rule = match freq.as_str() {
            "DAILY" | "YEARLY" => {
                if days.is_some() {
                    return Err(reject("BYDAY"));
                }
                if month_day.is_some() {
                    return Err(reject("BYMONTHDAY"));
                }
                if freq == "DAILY" {
                    Rule::Daily { interval }
                } else {
                    Rule::Yearly { interval }
                }
            }
            "WEEKLY" => {
                if month_day.is_some() {
                    return Err(reject("BYMONTHDAY"));
                }
                Rule::Weekly {
                    interval,
                    days: days.unwrap_or_default(),
                }
            }
            "MONTHLY" => {
                if days.is_some() {
                    return Err(reject("BYDAY"));
                }
                Rule::Monthly {
                    interval,
                    day: month_day.ok_or_else(|| err("MONTHLY needs BYMONTHDAY"))?,
                }
            }
            _ => return Err(err("unsupported FREQ")),
        };
        let r = Self { rule, until };
        r.validate()?;
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i16, m: i8, day: i8) -> Date {
        Date::constant(y, m, day)
    }

    fn weekly(interval: u32, days: &[Weekday]) -> Recurrence {
        Recurrence::new(Rule::Weekly {
            interval,
            days: days.iter().copied().collect(),
        })
    }

    #[test]
    fn daily() {
        let r = Recurrence::new(Rule::Daily { interval: 3 });
        assert_eq!(r.next_after(d(2026, 2, 27)), Some(d(2026, 3, 2)));
    }

    #[test]
    fn weekly_same_week_and_next_week() {
        let r = weekly(1, &[Weekday::Mon, Weekday::Wed]);
        assert_eq!(r.next_after(d(2026, 3, 2)), Some(d(2026, 3, 4))); // Mon -> Wed
        assert_eq!(r.next_after(d(2026, 3, 4)), Some(d(2026, 3, 9))); // Wed -> next Mon
        let biweekly = weekly(2, &[Weekday::Fri]);
        assert_eq!(biweekly.next_after(d(2026, 3, 6)), Some(d(2026, 3, 20)));
    }

    #[test]
    fn monthly_clamps_and_rolls() {
        let r = Recurrence::new(Rule::Monthly {
            interval: 1,
            day: 31,
        });
        assert_eq!(r.next_after(d(2026, 1, 31)), Some(d(2026, 2, 28)));
        assert_eq!(r.next_after(d(2026, 2, 28)), Some(d(2026, 3, 31)));
        let early = Recurrence::new(Rule::Monthly {
            interval: 1,
            day: 15,
        });
        assert_eq!(early.next_after(d(2026, 3, 10)), Some(d(2026, 3, 15)));
        let q = Recurrence::new(Rule::Monthly {
            interval: 3,
            day: 1,
        });
        assert_eq!(q.next_after(d(2026, 11, 1)), Some(d(2027, 2, 1)));
    }

    #[test]
    fn yearly_leap_day() {
        let r = Recurrence::new(Rule::Yearly { interval: 1 });
        assert_eq!(r.next_after(d(2028, 2, 29)), Some(d(2029, 2, 28)));
    }

    #[test]
    fn until_ends_the_series() {
        let mut r = Recurrence::new(Rule::Daily { interval: 1 });
        r.until = Some(d(2026, 3, 2));
        assert_eq!(r.next_after(d(2026, 3, 1)), Some(d(2026, 3, 2)));
        assert_eq!(r.next_after(d(2026, 3, 2)), None);
    }

    #[test]
    fn validation() {
        assert_eq!(
            Recurrence::new(Rule::Daily { interval: 0 }).validate(),
            Err(RecurrenceError::BadInterval)
        );
        assert_eq!(weekly(1, &[]).validate(), Err(RecurrenceError::NoWeekdays));
        assert_eq!(
            Recurrence::new(Rule::Monthly {
                interval: 1,
                day: 32
            })
            .validate(),
            Err(RecurrenceError::BadMonthDay)
        );
    }

    #[test]
    fn rrule_roundtrip() {
        let mut r = weekly(2, &[Weekday::Mon, Weekday::Fri]);
        r.until = Some(d(2027, 1, 31));
        let text = r.to_rrule();
        assert_eq!(text, "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;UNTIL=20270131");
        assert_eq!(Recurrence::from_rrule(&text).unwrap(), r);
        assert_eq!(
            Recurrence::from_rrule("RRULE:FREQ=MONTHLY;BYMONTHDAY=5").unwrap(),
            Recurrence::new(Rule::Monthly {
                interval: 1,
                day: 5
            })
        );
        assert!(Recurrence::from_rrule("FREQ=HOURLY").is_err());
        assert!(Recurrence::from_rrule("FREQ=MONTHLY").is_err());
        assert!(Recurrence::from_rrule("FREQ=DAILY;COUNT=3").is_err());
    }

    #[test]
    fn rrule_rejects_parts_the_frequency_cannot_represent() {
        let not_supported = |text: &str, part: &str, freq: &str| {
            assert_eq!(
                Recurrence::from_rrule(text),
                Err(RecurrenceError::PartNotSupported {
                    part: part.into(),
                    freq: freq.into()
                }),
                "{text}"
            );
        };
        not_supported("FREQ=MONTHLY;BYMONTHDAY=15;BYDAY=MO", "BYDAY", "MONTHLY");
        not_supported("FREQ=DAILY;BYDAY=MO", "BYDAY", "DAILY");
        not_supported("FREQ=DAILY;BYMONTHDAY=3", "BYMONTHDAY", "DAILY");
        not_supported("FREQ=WEEKLY;BYDAY=MO;BYMONTHDAY=3", "BYMONTHDAY", "WEEKLY");
        not_supported("FREQ=YEARLY;BYMONTHDAY=1", "BYMONTHDAY", "YEARLY");
        assert_eq!(
            Recurrence::from_rrule("FREQ=DAILY;INTERVAL=2;interval=3"),
            Err(RecurrenceError::DuplicatePart("INTERVAL".into()))
        );
        assert_eq!(
            Recurrence::from_rrule("FREQ=WEEKLY;BYDAY=MO;BYDAY=TU"),
            Err(RecurrenceError::DuplicatePart("BYDAY".into()))
        );
        assert!(Recurrence::from_rrule("FREQ=WEEKLY;BYDAY=MO,MO").is_err());
        assert!(Recurrence::from_rrule("FREQ=WEEKLY;BYDAY=MO;WKST=SU").is_err());
        assert!(Recurrence::from_rrule("rrule:FREQ=DAILY").is_ok());
    }

    #[test]
    fn rrule_until_must_be_exactly_a_date_or_utc_date_time() {
        let until = |text: &str| Recurrence::from_rrule(text).map(|r| r.until);
        let end = Some(Date::constant(2026, 12, 31));
        assert_eq!(until("FREQ=DAILY;UNTIL=20261231"), Ok(end));
        assert_eq!(until("FREQ=DAILY;UNTIL=20261231T235959Z"), Ok(end));
        assert_eq!(until("FREQ=DAILY;UNTIL=20261231t000000z"), Ok(end));
        for bad in [
            "20261231junk",
            "20261231T",
            "20261231T235959",
            "20261231T235959Zjunk",
            "20261231T2359590",
            "20261231X235959Z",
            "20261231T245959Z",
            "20261231T236059Z",
            "20261231T235961Z",
            "2026123",
            "202612310",
            "2026-12-31",
            "20261331",
            "+0261231",
            "２0261231",
        ] {
            assert!(
                Recurrence::from_rrule(&format!("FREQ=DAILY;UNTIL={bad}")).is_err(),
                "{bad}"
            );
        }
    }
}
