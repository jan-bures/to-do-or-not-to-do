//! Time types and helpers.
//!
//! The core uses [`jiff`] rather than `time` or `chrono`. It is pure Rust, builds for wasm32 and Android
//! without a clock or a time-zone database (`default-features = false`), has first-class civil-date
//! arithmetic (month-end clamping, weekdays) that recurrence needs, and supports serde. The core never
//! reads the current time: callers pass `now`, and the owner's local "today" is computed by the caller
//! (the owner's time zone lives in settings) and passed in an [`EvalContext`](crate::query::EvalContext).

pub use jiff::Timestamp;
pub use jiff::civil::Date;
use jiff::{Span, tz::TimeZone};
use serde::{Deserialize, Serialize};

/// Day of the week, Monday first (ISO 8601).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// All weekdays, Monday first.
    pub const ALL: [Weekday; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    /// Zero-based offset from Monday.
    #[must_use]
    pub fn offset(self) -> usize {
        Self::ALL.iter().position(|d| *d == self).unwrap_or(0)
    }

    /// The two-letter RRULE code (`MO`, `TU`, ...).
    #[must_use]
    pub fn rrule_code(self) -> &'static str {
        ["MO", "TU", "WE", "TH", "FR", "SA", "SU"][self.offset()]
    }

    /// Parses a two-letter RRULE code.
    #[must_use]
    pub fn from_rrule_code(code: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.rrule_code() == code)
    }

    /// Weekday of a civil date.
    #[must_use]
    pub fn of(date: Date) -> Self {
        let off = usize::try_from(date.weekday().to_monday_zero_offset()).unwrap_or(0);
        Self::ALL[off.min(6)]
    }
}

/// UTC calendar date of a timestamp.
#[must_use]
pub fn utc_date(ts: Timestamp) -> Date {
    TimeZone::UTC.to_datetime(ts).date()
}

/// Adds (or subtracts) whole days to a date. Returns `None` on overflow of the supported range.
#[must_use]
pub fn add_days(date: Date, days: i64) -> Option<Date> {
    date.checked_add(Span::new().try_days(days).ok()?).ok()
}

/// Whole days from `from` to `to` (negative if `to` is earlier).
#[must_use]
pub fn days_between(from: Date, to: Date) -> i64 {
    from.until(to).map_or(0, |s| i64::from(s.get_days()))
}

/// Shifts a timestamp by whole days of 86 400 seconds.
#[must_use]
pub fn shift_days(ts: Timestamp, days: i64) -> Option<Timestamp> {
    Timestamp::from_second(ts.as_second().checked_add(days.checked_mul(86_400)?)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_roundtrip() {
        let d = Date::constant(2026, 3, 2); // a Monday
        assert_eq!(Weekday::of(d), Weekday::Mon);
        for w in Weekday::ALL {
            assert_eq!(Weekday::from_rrule_code(w.rrule_code()), Some(w));
        }
    }

    #[test]
    fn day_math() {
        let d = Date::constant(2026, 2, 27);
        assert_eq!(add_days(d, 2), Some(Date::constant(2026, 3, 1)));
        assert_eq!(days_between(d, Date::constant(2026, 3, 1)), 2);
        assert_eq!(days_between(Date::constant(2026, 3, 1), d), -2);
    }
}
