//! The civil calendar for date pickers. Like the room's ISO week and weekday labels, the app
//! defaults to Monday-first, with English labels. The grid also accepts another first weekday.

use todo_core::time::{Date, Weekday, add_days};

pub const WEEK_START: Weekday = Weekday::Mon;

/// Shown below the calendar when editing the due date of a repeating task.
pub const REPEAT_CLEAR_NOTE: &str = "This task repeats. No date also stops the repeat.";

pub fn next_monday(today: Date) -> Option<Date> {
    add_days(today, 7 - today.weekday().to_monday_zero_offset() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub label: &'static str,
    pub date: Option<Date>,
}

pub fn presets(today: Date) -> Vec<Preset> {
    [
        ("Today", Some(today)),
        ("Tomorrow", add_days(today, 1)),
        ("Next week", next_monday(today)),
    ]
    .into_iter()
    .filter(|(_, date)| date.is_some())
    .map(|(label, date)| Preset { label, date })
    .chain([Preset {
        label: "No date",
        date: None,
    }])
    .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Month {
    /// The day the keyboard is on. Its month is the one shown.
    pub cursor: Date,
    pub first_weekday: Weekday,
}

impl Month {
    pub fn new(cursor: Date) -> Self {
        Self {
            cursor,
            first_weekday: WEEK_START,
        }
    }

    pub fn label(self) -> String {
        self.cursor.strftime("%B %Y").to_string()
    }

    pub fn weekdays(self) -> [&'static str; 7] {
        const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        std::array::from_fn(|i| NAMES[(i + self.first_weekday.offset()) % 7])
    }

    /// Six stable rows; days outside this month have empty cells, not duplicate buttons.
    pub fn rows(self) -> [[Option<Date>; 7]; 6] {
        let first = self.cursor.first_of_month();
        let offset = (Weekday::of(first).offset() + 7 - self.first_weekday.offset()) % 7;
        std::array::from_fn(|row| {
            std::array::from_fn(|col| {
                let day = row * 7 + col;
                let within = day.checked_sub(offset)?;
                (within < usize::from(first.days_in_month() as u8))
                    .then(|| add_days(first, within as i64))
                    .flatten()
            })
        })
    }

    /// Arrows move one or seven days, carrying the focus into the new month when needed.
    pub fn move_days(&mut self, days: i64) {
        if let Some(date) = add_days(self.cursor, days) {
            self.cursor = date;
        }
    }

    /// Page keys and the month buttons clamp the cursor to the last day of a shorter month.
    pub fn shifted(self, months: i32) -> Option<Self> {
        let index = (i32::from(self.cursor.year()) * 12 + i32::from(self.cursor.month()) - 1)
            .checked_add(months)?;
        let year = i16::try_from(index.div_euclid(12)).ok()?;
        let month = (index.rem_euclid(12) + 1) as i8;
        let first = Date::new(year, month, 1).ok()?;
        Some(Self {
            cursor: Date::new(year, month, self.cursor.day().min(first.days_in_month())).ok()?,
            ..self
        })
    }
}

pub fn day_label(date: Date) -> String {
    date.strftime("%A, %-d %B %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        text.parse().expect("fixture date")
    }

    #[test]
    fn week_start_changes_columns_and_labels_together() {
        let monday = Month::new(date("2026-11-21"));
        let sunday = Month {
            first_weekday: Weekday::Sun,
            ..monday
        };
        assert_eq!(
            monday.weekdays(),
            ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        );
        assert_eq!(
            monday.rows()[0],
            [None, None, None, None, None, None, Some(date("2026-11-01"))]
        );
        assert_eq!(monday.rows()[5][0], Some(date("2026-11-30")));
        assert_eq!(sunday.weekdays()[0], "Sun");
        assert_eq!(sunday.rows()[0][0], Some(date("2026-11-01")));
        assert_eq!(sunday.rows()[5], [None; 7]);
        assert_eq!(monday.label(), "November 2026");
        assert_eq!(day_label(monday.cursor), "Saturday, 21 November 2026");
    }

    #[test]
    fn days_and_months_cross_boundaries_and_clamp_month_end() {
        let mut month = Month::new(date("2023-12-31"));
        month.move_days(1);
        assert_eq!(month.cursor, date("2024-01-01"));
        month.move_days(30);
        let feb = month.shifted(1).expect("February");
        assert_eq!(feb.cursor, date("2024-02-29"));
        assert_eq!(feb.rows().iter().flatten().flatten().count(), 29);
        month.move_days(-31);
        assert_eq!(month.cursor, date("2023-12-31"));
        assert!(Month::new(Date::MAX).shifted(1).is_none());
        assert!(Month::new(Date::MIN).shifted(-1).is_none());
    }

    #[test]
    fn next_week_is_the_next_monday_even_on_monday() {
        let monday = date("2026-11-23");
        assert_eq!(presets(monday)[2].date, Some(date("2026-11-30")));
        assert_eq!(presets(date("2026-11-22"))[2].date, Some(monday));
        assert_eq!(presets(monday)[3].date, None);
    }
}
