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

fn weekday_index(d: Date) -> usize {
    Weekday::of(d).offset()
}

/// "Saturday".
pub fn weekday(d: Date) -> &'static str {
    WEEKDAYS[weekday_index(d)]
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

/// "Today, 15:30" or "Mon 23 Nov, 09:00".
pub fn when(ts: Timestamp, now: &Now) -> String {
    let z = local(ts, &now.tz);
    format!("{}, {}", now.dates().relative(z.date()), clock(ts, &now.tz))
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
            now.dates().relative(due),
            clock(at, &now.tz)
        )),
        (_, None) => DueLabel::Plain(now.dates().relative(due)),
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
pub use todo_client::intent::confirm::one_line;

#[cfg(test)]
mod tests {
    fn recurrence(r: &Recurrence) -> String {
        describe_repeat(r, now().dates())
    }
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
        let dates = now().dates();
        assert_eq!(dates.full(today), "21.11.2026");
        assert_eq!(dates.weekday(d("2026-11-23")), "Mon 23.11.");
        assert_eq!(dates.weekday(d("2027-01-04")), "Mon 04.01.2027");
        assert_eq!(dates.relative(d("2026-11-22")), "Tomorrow");
        assert_eq!(dates.relative(d("2026-11-20")), "Yesterday");
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
            DueLabel::Plain("Mon 23.11.".into())
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
