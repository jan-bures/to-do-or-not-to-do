//! Presentation of civil dates. Callers supply the local date; no wall clock, locale or I/O.

use serde::{Deserialize, Serialize};
use todo_core::time::{Date, days_between};

/// A device's display preference. It does not change date input or wire formats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateFormat {
    #[default]
    DayMonth,
    DayMonthName,
    MonthNameDay,
    Iso,
    NumericUs,
}

impl DateFormat {
    pub const ALL: [Self; 5] = [
        Self::DayMonth,
        Self::DayMonthName,
        Self::MonthNameDay,
        Self::Iso,
        Self::NumericUs,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::DayMonth => "day_month",
            Self::DayMonthName => "day_month_name",
            Self::MonthNameDay => "month_name_day",
            Self::Iso => "iso",
            Self::NumericUs => "numeric_us",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.key() == key)
    }

    pub fn example(self, today: Date) -> String {
        let name = match self {
            Self::DayMonth => "Day.Month",
            Self::DayMonthName => "Day Month name",
            Self::MonthNameDay => "Month name Day (US)",
            Self::Iso => "ISO",
            Self::NumericUs => "Numeric US",
        };
        format!("{name} — {}", self.on(today).date(today))
    }

    pub const fn on(self, today: Date) -> DateLabels {
        DateLabels {
            format: self,
            today,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DateLabels {
    pub format: DateFormat,
    pub today: Date,
}

impl DateLabels {
    pub fn date(self, date: Date) -> String {
        self.render(date, date.year() != self.today.year())
    }

    /// Unambiguous even when read in a different year.
    pub fn full(self, date: Date) -> String {
        self.render(date, true)
    }

    fn render(self, date: Date, full: bool) -> String {
        let (day, month, year) = (date.day(), date.month(), date.year());
        match (self.format, full) {
            (DateFormat::DayMonth, false) => format!("{day:02}.{month:02}."),
            (DateFormat::DayMonth, true) => format!("{day:02}.{month:02}.{year:04}"),
            (DateFormat::DayMonthName, false) => format!("{day} {}", month_name(date)),
            (DateFormat::DayMonthName, true) => format!("{day} {} {year:04}", month_name(date)),
            (DateFormat::MonthNameDay, false) => format!("{} {day}", month_name(date)),
            (DateFormat::MonthNameDay, true) => format!("{} {day}, {year:04}", month_name(date)),
            (DateFormat::Iso, _) => date.to_string(),
            (DateFormat::NumericUs, false) => format!("{month:02}/{day:02}"),
            (DateFormat::NumericUs, true) => format!("{month:02}/{day:02}/{year:04}"),
        }
    }

    pub fn weekday(self, date: Date) -> String {
        format!("{} {}", date.strftime("%a"), self.date(date))
    }

    pub fn relative(self, date: Date) -> String {
        match days_between(self.today, date) {
            0 => "Today".into(),
            1 => "Tomorrow".into(),
            -1 => "Yesterday".into(),
            _ => self.weekday(date),
        }
    }

    pub fn range(self, start: Date, end: Date) -> String {
        if start == end {
            return self.weekday(end);
        }
        if start.year() != end.year() || self.format == DateFormat::Iso {
            return format!("{} – {}", self.full(start), self.full(end));
        }
        if start.month() == end.month() {
            match self.format {
                DateFormat::DayMonth => return format!("{:02}.–{}", start.day(), self.date(end)),
                DateFormat::DayMonthName => return format!("{}–{}", start.day(), self.date(end)),
                DateFormat::MonthNameDay => {
                    let span = format!("{} {}–{}", month_name(start), start.day(), end.day());
                    return if end.year() == self.today.year() {
                        span
                    } else {
                        format!("{span}, {:04}", end.year())
                    };
                }
                DateFormat::Iso | DateFormat::NumericUs => {}
            }
        }
        format!("{}–{}", self.date(start), self.date(end))
    }

    /// A month heading has no day to display. Keep the year to orient month navigation.
    pub fn month(self, date: Date) -> String {
        let (month, year) = (date.month(), date.year());
        match self.format {
            DateFormat::DayMonth => format!("{month:02}.{year:04}"),
            DateFormat::DayMonthName | DateFormat::MonthNameDay => {
                date.strftime("%B %Y").to_string()
            }
            DateFormat::Iso => format!("{year:04}-{month:02}"),
            DateFormat::NumericUs => format!("{month:02}/{year:04}"),
        }
    }
}

fn month_name(date: Date) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[(date.month() - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Date {
        text.parse().expect("date fixture")
    }

    proptest::proptest! {
        #[test]
        fn crossing_a_year_always_identifies_both_years(year in 1i16..9999) {
            let first = Date::new(year, 12, 31).unwrap();
            let last = Date::new(year + 1, 1, 1).unwrap();
            for format in DateFormat::ALL {
                let labels = format.on(first);
                let expected = format!("{} – {}", labels.full(first), labels.full(last));
                proptest::prop_assert_eq!(labels.range(first, last), expected.as_str());
                proptest::prop_assert_eq!(format.on(last).range(first, last), expected);
            }
        }
    }

    #[test]
    fn formats_and_ranges() {
        let first = d("2026-11-23");
        let last = d("2026-11-25");
        for (format, short, full, range, across_month, across_year) in [
            (
                DateFormat::DayMonth,
                "23.11.",
                "23.11.2026",
                "23.–25.11.",
                "23.11.–03.12.",
                "23.11.2026 – 04.01.2027",
            ),
            (
                DateFormat::DayMonthName,
                "23 Nov",
                "23 Nov 2026",
                "23–25 Nov",
                "23 Nov–3 Dec",
                "23 Nov 2026 – 4 Jan 2027",
            ),
            (
                DateFormat::MonthNameDay,
                "Nov 23",
                "Nov 23, 2026",
                "Nov 23–25",
                "Nov 23–Dec 3",
                "Nov 23, 2026 – Jan 4, 2027",
            ),
            (
                DateFormat::Iso,
                "2026-11-23",
                "2026-11-23",
                "2026-11-23 – 2026-11-25",
                "2026-11-23 – 2026-12-03",
                "2026-11-23 – 2027-01-04",
            ),
            (
                DateFormat::NumericUs,
                "11/23",
                "11/23/2026",
                "11/23–11/25",
                "11/23–12/03",
                "11/23/2026 – 01/04/2027",
            ),
        ] {
            let labels = format.on(d("2026-12-31"));
            assert_eq!(labels.date(first), short);
            assert_eq!(labels.full(first), full);
            assert_eq!(format.on(d("2027-01-01")).date(first), full);
            assert_eq!(labels.weekday(first), format!("Mon {short}"));
            assert_eq!(labels.range(first, first), labels.weekday(first));
            assert_eq!(labels.range(first, last), range);
            assert_eq!(labels.range(first, d("2026-12-03")), across_month);
            assert_eq!(labels.range(first, d("2027-01-04")), across_year);
            assert_eq!(labels.relative(d("2027-01-01")), "Tomorrow");
            assert_eq!(labels.relative(d("2026-12-30")), "Yesterday");
            assert_eq!(labels.relative(labels.today), "Today");
            assert_eq!(DateFormat::from_key(format.key()), Some(format));
            assert_eq!(
                serde_json::to_string(&format).unwrap(),
                format!("\"{}\"", format.key())
            );
        }
    }
}
