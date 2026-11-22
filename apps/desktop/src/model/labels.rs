//! Words and numbers the views show: dates, times, due labels, recurrence and sizes. English for now.

use jiff::ZonedDateTime;
use jiff::tz::TimeZone;
pub use todo_client::intent::repeat::describe_repeat;
use todo_core::time::{Date, Timestamp, Weekday, days_between};

use super::Now;

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn weekday_index(d: Date) -> usize {
    Weekday::of(d).offset()
}

/// "Saturday".
pub fn weekday(d: Date) -> &'static str {
    WEEKDAYS[weekday_index(d)]
}

fn month(d: Date) -> &'static str {
    MONTHS[usize::try_from(d.month() - 1).unwrap_or(0).min(11)]
}

pub use todo_client::intent::quick_add::{long_date, short_date};

/// "14 Nov" (or "14 Nov 2025" in another year): for creation dates and the like.
pub fn day_month(d: Date, today: Date) -> String {
    if d.year() == today.year() {
        format!("{} {}", d.day(), month(d))
    } else {
        format!("{} {} {}", d.day(), month(d), d.year())
    }
}

/// "W47": the ISO 8601 week.
pub fn iso_week(d: Date) -> String {
    format!("W{}", d.iso_week_date().week())
}

/// The local time of `ts`.
pub fn local(ts: Timestamp, tz: &TimeZone) -> ZonedDateTime {
    ts.to_zoned_datetime(tz.clone())
}

/// "17:05".
pub fn clock(ts: Timestamp, tz: &TimeZone) -> String {
    let z = local(ts, tz);
    format!("{:02}:{:02}", z.hour(), z.minute())
}

/// "Today", "Tomorrow", "Yesterday" or "Mon 23 Nov".
pub fn relative_day(d: Date, today: Date) -> String {
    match days_between(today, d) {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        _ => short_date(d, today),
    }
}

/// "Today, 15:30" or "Mon 23 Nov, 09:00".
pub fn when(ts: Timestamp, now: &Now) -> String {
    let z = local(ts, &now.tz);
    format!(
        "{}, {}",
        relative_day(z.date(), now.today()),
        clock(ts, &now.tz)
    )
}

/// How a row shows its due date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DueLabel {
    None,
    /// Due within the next few hours: accent colour.
    Soon(String),
    Plain(String),
    Overdue(String),
}

/// Reminders within this many minutes make a due-today task "soon".
const SOON_MINUTES: i64 = 3 * 60;

/// A row's due label. `reminder` is the task's next live reminder, if any; when it falls on the due date it
/// gives the time ("Today 17:00", "in 1h 25m").
pub fn due(due: Option<Date>, reminder: Option<Timestamp>, now: &Now) -> DueLabel {
    let Some(due) = due else {
        return DueLabel::None;
    };
    let today = now.today();
    let days = days_between(today, due);
    if days < 0 {
        let n = -days;
        return DueLabel::Overdue(if n == 1 {
            "yesterday".into()
        } else {
            format!("{n}d overdue")
        });
    }
    let at = reminder.filter(|r| local(*r, &now.tz).date() == due);
    match (days, at) {
        (0, Some(at)) => {
            let minutes = (at.as_second() - now.at.as_second()) / 60;
            if (0..=SOON_MINUTES).contains(&minutes) {
                DueLabel::Soon(if minutes < 60 {
                    format!("in {}m", minutes.max(1))
                } else {
                    format!("in {}h {:02}m", minutes / 60, minutes % 60)
                })
            } else {
                DueLabel::Plain(format!("Today {}", clock(at, &now.tz)))
            }
        }
        (_, Some(at)) => DueLabel::Plain(format!(
            "{} {}",
            relative_day(due, today),
            clock(at, &now.tz)
        )),
        (_, None) => DueLabel::Plain(relative_day(due, today)),
    }
}

/// "820 B", "14 KB", "2.4 MB".
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else {
        format!("{:.1} GB", b / KB / KB / KB)
    }
}

/// A name as one line of text: every control character (line breaks and tabs included) and the Unicode
/// line and paragraph separators become a space. Names come from the server and other clients, and a row,
/// a chip, a breadcrumb or the sidebar draws them on one line (GPUI's one-line shaping can't take a line
/// break). The data itself is left as it is.
pub fn one_line(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::describe_repeat as recurrence;
    use super::*;
    use std::collections::BTreeSet;
    use todo_core::recurrence::{Recurrence, Rule};

    fn now() -> Now {
        Now::fixed("2026-11-21T10:35:00Z")
    }

    fn d(s: &str) -> Date {
        s.parse().expect("date")
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn dates_read_naturally() {
        let today = d("2026-11-21");
        assert_eq!(weekday(today), "Saturday");
        assert_eq!(long_date(today), "21 Nov 2026");
        assert_eq!(iso_week(today), "W47");
        assert_eq!(iso_week(d("2021-01-01")), "W53");
        assert_eq!(short_date(d("2026-11-23"), today), "Mon 23 Nov");
        assert_eq!(short_date(d("2027-01-04"), today), "Mon 4 Jan 2027");
        assert_eq!(relative_day(d("2026-11-22"), today), "Tomorrow");
        assert_eq!(relative_day(d("2026-11-20"), today), "Yesterday");
    }

    #[test]
    fn due_labels_follow_the_design() {
        let n = now();
        assert_eq!(due(None, None, &n), DueLabel::None);
        assert_eq!(
            due(Some(d("2026-11-19")), None, &n),
            DueLabel::Overdue("2d overdue".into())
        );
        assert_eq!(
            due(Some(d("2026-11-20")), None, &n),
            DueLabel::Overdue("yesterday".into())
        );
        assert_eq!(
            due(Some(d("2026-11-21")), None, &n),
            DueLabel::Plain("Today".into())
        );
        // A reminder later today but within three hours makes it "soon".
        assert_eq!(
            due(Some(d("2026-11-21")), Some(ts("2026-11-21T12:00:00Z")), &n),
            DueLabel::Soon("in 1h 25m".into())
        );
        assert_eq!(
            due(Some(d("2026-11-21")), Some(ts("2026-11-21T10:50:00Z")), &n),
            DueLabel::Soon("in 15m".into())
        );
        assert_eq!(
            due(Some(d("2026-11-21")), Some(ts("2026-11-21T17:00:00Z")), &n),
            DueLabel::Plain("Today 17:00".into())
        );
        // A reminder that has passed shows the time, not a countdown.
        assert_eq!(
            due(Some(d("2026-11-21")), Some(ts("2026-11-21T08:00:00Z")), &n),
            DueLabel::Plain("Today 08:00".into())
        );
        assert_eq!(
            due(Some(d("2026-11-22")), None, &n),
            DueLabel::Plain("Tomorrow".into())
        );
        // A reminder on another day than the due date is not the due time.
        assert_eq!(
            due(Some(d("2026-11-23")), Some(ts("2026-11-22T09:00:00Z")), &n),
            DueLabel::Plain("Mon 23 Nov".into())
        );
        assert_eq!(when(ts("2026-11-21T15:30:00Z"), &n), "Today, 15:30");
    }

    #[test]
    fn recurrence_and_sizes() {
        let weekly = |interval, days: &[Weekday]| Recurrence {
            rule: Rule::Weekly {
                interval,
                days: days.iter().copied().collect::<BTreeSet<_>>(),
            },
            ..Recurrence::new(Rule::Daily { interval: 1 })
        };
        assert_eq!(recurrence(&weekly(1, &[Weekday::Mon])), "Every Monday");
        assert_eq!(
            recurrence(&weekly(2, &[Weekday::Mon, Weekday::Thu])),
            "Every 2 weeks on Mon, Thu"
        );
        assert_eq!(
            recurrence(&weekly(
                1,
                &[
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri
                ]
            )),
            "Every weekday"
        );
        assert_eq!(
            recurrence(&Recurrence::new(Rule::Monthly {
                interval: 1,
                day: 22
            })),
            "Every month on the 22nd"
        );
        assert_eq!(
            recurrence(&Recurrence::new(Rule::Daily { interval: 3 })),
            "Every 3 days"
        );
        assert_eq!(size(820), "820 B");
        assert_eq!(size(14_000), "14 KB");
        assert_eq!(size(2_500_000), "2.4 MB");
    }

    #[test]
    fn names_are_drawn_on_one_line() {
        assert_eq!(one_line("Plan\nthe\r\nweek"), "Plan the  week");
        assert_eq!(one_line("a\tb\u{0}c\u{2028}d\u{85}e"), "a b c d e");
        assert_eq!(one_line("Koupit rohlíky"), "Koupit rohlíky");
    }
}
