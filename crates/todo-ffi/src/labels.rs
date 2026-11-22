//! Words and numbers the views show: dates, times and due labels. English for now, the same wording as
//! the desktop app.

use jiff::ZonedDateTime;
use jiff::tz::{Offset, TimeZone};
use todo_core::query::EvalContext;
use todo_core::time::{Date, Timestamp, Weekday, days_between};

/// The current time and the owner's UTC offset, from the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LocalNow {
    /// Milliseconds since the Unix epoch.
    pub epoch_ms: i64,
    /// The device's current UTC offset in seconds (Prague in winter: 3600).
    pub utc_offset_secs: i32,
}

/// [`LocalNow`] resolved: a timestamp and a fixed-offset zone.
#[derive(Clone, Debug)]
pub struct Now {
    /// The time.
    pub at: Timestamp,
    tz: TimeZone,
}

impl Now {
    /// Resolves a [`LocalNow`]; out-of-range values fall back to the epoch and UTC.
    pub fn new(now: LocalNow) -> Self {
        let at = Timestamp::from_millisecond(now.epoch_ms).unwrap_or(Timestamp::UNIX_EPOCH);
        let offset = Offset::from_seconds(now.utc_offset_secs).unwrap_or(Offset::UTC);
        Self {
            at,
            tz: TimeZone::fixed(offset),
        }
    }

    fn zoned(&self, ts: Timestamp) -> ZonedDateTime {
        ts.to_zoned_datetime(self.tz.clone())
    }

    /// The owner's local date.
    pub fn today(&self) -> Date {
        self.date_of(self.at)
    }

    /// The local date of `ts`.
    pub fn date_of(&self, ts: Timestamp) -> Date {
        self.zoned(ts).date()
    }

    /// The query evaluation context: local today and the UTC offset.
    pub fn eval(&self) -> EvalContext {
        let offset = self.tz.to_offset(self.at).seconds();
        EvalContext::new(self.at, self.today(), offset)
    }

    /// "17:05".
    pub fn clock(&self, ts: Timestamp) -> String {
        let z = self.zoned(ts);
        format!("{:02}:{:02}", z.hour(), z.minute())
    }
}

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

/// "Saturday".
pub fn weekday(d: Date) -> &'static str {
    WEEKDAYS[Weekday::of(d).offset()]
}

fn month(d: Date) -> &'static str {
    MONTHS[usize::try_from(d.month() - 1).unwrap_or(0).min(11)]
}

/// "21 Nov".
pub fn day_month(d: Date, today: Date) -> String {
    if d.year() == today.year() {
        format!("{} {}", d.day(), month(d))
    } else {
        format!("{} {} {}", d.day(), month(d), d.year())
    }
}

pub use todo_client::intent::quick_add::short_date;

/// "W47": the ISO 8601 week.
pub fn iso_week(d: Date) -> String {
    format!("W{}", d.iso_week_date().week())
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

/// A name on one line: control characters (line breaks, tabs) and the line and paragraph separators
/// become a space, as the desktop's `labels::one_line` does. The data keeps them.
pub use todo_client::intent::confirm::one_line;

/// "Today, 15:30" or "Mon 23 Nov, 09:00".
pub fn when(ts: Timestamp, now: &Now) -> String {
    format!(
        "{}, {}",
        relative_day(now.date_of(ts), now.today()),
        now.clock(ts)
    )
}

/// How a row shows its due date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DueTone {
    /// Neutral metadata.
    Plain,
    /// Due within a few hours: accent.
    Soon,
    /// Past due: the danger badge.
    Overdue,
}

/// A row's due label.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DueLabel {
    /// "Tomorrow", "in 1h 25m", "2d overdue", ...
    pub text: String,
    /// How to colour it.
    pub tone: DueTone,
}

/// Reminders within this many minutes make a due-today task "soon".
const SOON_MINUTES: i64 = 3 * 60;

/// A row's due label. `reminder` is the task's next live reminder; when it falls on the due date it gives
/// the time ("Today 17:00", "in 1h 25m").
pub fn due(due: Option<Date>, reminder: Option<Timestamp>, now: &Now) -> Option<DueLabel> {
    let due = due?;
    let today = now.today();
    let days = days_between(today, due);
    let label = |text: String, tone| Some(DueLabel { text, tone });
    if days < 0 {
        let n = -days;
        return label(
            if n == 1 {
                "yesterday".into()
            } else {
                format!("{n}d overdue")
            },
            DueTone::Overdue,
        );
    }
    let at = reminder.filter(|r| now.date_of(*r) == due);
    match (days, at) {
        (0, Some(at)) => {
            let minutes = (at.as_second() - now.at.as_second()) / 60;
            if (0..=SOON_MINUTES).contains(&minutes) {
                label(
                    if minutes < 60 {
                        format!("in {}m", minutes.max(1))
                    } else {
                        format!("in {}h {:02}m", minutes / 60, minutes % 60)
                    },
                    DueTone::Soon,
                )
            } else {
                label(format!("Today {}", now.clock(at)), DueTone::Plain)
            }
        }
        (_, Some(at)) => label(
            format!("{} {}", relative_day(due, today), now.clock(at)),
            DueTone::Plain,
        ),
        (_, None) => label(relative_day(due, today), DueTone::Plain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(s: &str) -> Now {
        let at: Timestamp = s.parse().unwrap();
        Now::new(LocalNow {
            epoch_ms: at.as_millisecond(),
            utc_offset_secs: 3600,
        })
    }

    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }

    #[test]
    fn dates_read_like_the_mockups() {
        let n = now("2026-11-21T09:35:00Z");
        assert_eq!(n.today(), date("2026-11-21"));
        assert_eq!(weekday(n.today()), "Saturday");
        assert_eq!(iso_week(n.today()), "W47");
        assert_eq!(short_date(date("2026-11-23"), n.today()), "Mon 23 Nov");
        assert_eq!(short_date(date("2027-01-04"), n.today()), "Mon 4 Jan 2027");
        assert_eq!(relative_day(date("2026-11-22"), n.today()), "Tomorrow");
    }

    #[test]
    fn the_offset_moves_midnight() {
        // 23:30 UTC is already the next day at UTC+1.
        let n = now("2026-11-21T23:30:00Z");
        assert_eq!(n.today(), date("2026-11-22"));
        assert_eq!(n.clock(n.at), "00:30");
    }

    #[test]
    fn due_labels() {
        let n = now("2026-11-21T09:35:00Z"); // 10:35 local
        let today = n.today();
        let d = |s| due(Some(date(s)), None, &n).unwrap();
        assert_eq!(d("2026-11-19").text, "2d overdue");
        assert_eq!(d("2026-11-19").tone, DueTone::Overdue);
        assert_eq!(d("2026-11-20").text, "yesterday");
        assert_eq!(d("2026-11-21").text, "Today");
        assert_eq!(d("2026-11-23").text, "Mon 23 Nov");
        // A reminder on the due day gives the time; within three hours it counts down.
        let soon: Timestamp = "2026-11-21T11:00:00Z".parse().unwrap();
        let l = due(Some(today), Some(soon), &n).unwrap();
        assert_eq!((l.text.as_str(), l.tone), ("in 1h 25m", DueTone::Soon));
        let later: Timestamp = "2026-11-21T16:00:00Z".parse().unwrap();
        assert_eq!(
            due(Some(today), Some(later), &n).unwrap().text,
            "Today 17:00"
        );
        assert_eq!(due(None, None, &n), None);
    }
}
