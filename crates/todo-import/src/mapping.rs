//! Field conversions from Graph values to `todo-core` values.
//!
//! [`verify`](crate::verify) uses the same conversions to compute what the bundle should contain, so
//! each one is small, documented and unit-tested here.

use crate::graph::DateTimeTz;
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use serde::Deserialize;
use std::collections::BTreeSet;
use todo_core::decide::{MAX_NAME, MAX_NOTES, MAX_TITLE};
use todo_core::model::Priority;
use todo_core::recurrence::{Recurrence, Rule};
use todo_core::time::Weekday;

/// Title used for a task or step whose title is empty.
pub const UNTITLED: &str = "(untitled)";

/// A value could not be converted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MapError(pub String);

/// Parses an ISO 8601 instant such as `2026-01-31T10:20:30.1234567Z`.
pub fn instant(s: &str) -> Result<Timestamp, MapError> {
    s.parse::<Timestamp>()
        .map_err(|e| MapError(format!("bad timestamp: {e}")))
}

/// Resolves a Graph time zone name: `UTC` or an IANA name. (Windows names are not supported; the
/// exports seen so far use `UTC` throughout.)
pub fn zone(name: &str) -> Result<TimeZone, MapError> {
    if name.eq_ignore_ascii_case("utc") {
        return Ok(TimeZone::UTC);
    }
    TimeZone::get(name).map_err(|_| MapError(format!("unsupported time zone `{name}`")))
}

/// The instant a Graph `dateTimeTimeZone` denotes.
pub fn date_time(v: &DateTimeTz) -> Result<Timestamp, MapError> {
    let dt: DateTime = v
        .date_time
        .parse()
        .map_err(|e| MapError(format!("bad dateTime: {e}")))?;
    let tz = zone(&v.time_zone)?;
    tz.to_timestamp(dt)
        .map_err(|e| MapError(format!("bad dateTime: {e}")))
}

/// The due date of a Graph `dueDateTime`.
///
/// To Do stores a due date as midnight in the owner's time zone and Graph returns it converted to
/// (usually) UTC, so for owners east of UTC it reads as 22:00 or 23:00 on the previous day. The date is
/// therefore taken in the owner's time zone, `owner`.
pub fn due_date(v: &DateTimeTz, owner: &TimeZone) -> Result<Date, MapError> {
    Ok(owner.to_datetime(date_time(v)?).date())
}

/// Task notes from a Graph body: line endings normalised to `\n`, trailing whitespace removed, and a
/// body of only whitespace treated as empty. HTML bodies are converted to plain text.
#[must_use]
pub fn notes(content: &str, content_type: &str) -> String {
    let text = if content_type.eq_ignore_ascii_case("html") {
        html_to_text(content)
    } else {
        content.to_owned()
    };
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.trim_end().to_owned()
}

/// A minimal HTML to text conversion for task bodies: line breaks for `<br>`, `</p>` and `</div>`,
/// tags dropped, the common entities decoded.
#[must_use]
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_ascii_lowercase();
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        let closing = tag.starts_with('/');
        if name == "br" || (closing && matches!(name, "p" | "div" | "li" | "tr")) {
            out.push('\n');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// A title for a task or step: trimmed, [`UNTITLED`] when empty, and cut to the domain's limit.
#[must_use]
pub fn title(raw: Option<&str>) -> String {
    clip(raw.unwrap_or(""), MAX_TITLE)
}

/// A name for a collection, list or tag: trimmed, [`UNTITLED`] when empty, cut to the limit.
#[must_use]
pub fn name(raw: &str) -> String {
    clip(raw, MAX_NAME)
}

fn clip(raw: &str, max: usize) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return UNTITLED.to_owned();
    }
    if t.chars().count() <= max {
        t.to_owned()
    } else {
        t.chars()
            .take(max)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }
}

/// True if notes fit the domain's limit.
#[must_use]
pub fn notes_fit(notes: &str) -> bool {
    notes.len() <= MAX_NOTES
}

/// Graph importance to priority: `high` is high, `low` is low, and `normal` (To Do's default for
/// every task) is no priority.
#[must_use]
pub fn priority(importance: &str) -> Priority {
    match importance {
        "high" => Priority::High,
        "low" => Priority::Low,
        _ => Priority::None,
    }
}

/// A list theme colour name from the cache to a colour. Known To Do theme names map to approximate
/// hex values; unknown names are kept verbatim (they are short identifiers).
#[must_use]
pub fn theme_color(name: &str) -> Option<String> {
    let hex = match name {
        "dark_blue" => "#2564cf",
        "blue" => "#4a7dd8",
        "light_blue" => "#5c9fe0",
        "purple" => "#8764b8",
        "light_purple" => "#a98bd6",
        "pink" => "#c74f97",
        "light_pink" => "#e0749f",
        "red" => "#d13438",
        "dark_red" => "#a4262c",
        "light_red" => "#e86a6e",
        "orange" => "#ca5010",
        "light_orange" => "#e8875a",
        "yellow" => "#c19c00",
        "green" => "#107c41",
        "dark_green" => "#0b5a2e",
        "light_green" => "#4e9f3d",
        "teal" => "#038387",
        "light_teal" => "#2fa7aa",
        "grey" | "gray" => "#69797e",
        "dark_grey" | "dark_gray" => "#4a5459",
        "light_grey" | "light_gray" => "#8e9a9f",
        other => {
            let other = other.trim();
            return (!other.is_empty() && other.len() <= todo_core::decide::MAX_COLOR)
                .then(|| other.to_owned());
        }
    };
    Some(hex.to_owned())
}

/// A file name without directories, for an attachment.
#[must_use]
pub fn file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() {
        "attachment".to_owned()
    } else {
        base.to_owned()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pattern {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    interval: Option<u32>,
    #[serde(default)]
    days_of_week: Vec<String>,
    #[serde(default)]
    day_of_month: Option<u8>,
    #[serde(default)]
    month: Option<i8>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Range {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    end_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Patterned {
    pattern: Pattern,
    #[serde(default)]
    range: Option<Range>,
}

fn weekday(name: &str) -> Option<Weekday> {
    Some(match name.to_ascii_lowercase().as_str() {
        "monday" => Weekday::Mon,
        "tuesday" => Weekday::Tue,
        "wednesday" => Weekday::Wed,
        "thursday" => Weekday::Thu,
        "friday" => Weekday::Fri,
        "saturday" => Weekday::Sat,
        "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// Maps a Graph `patternedRecurrence` to the `todo-core` subset.
///
/// Supported: `daily`, `weekly` (with days), `absoluteMonthly` (day of month) and `absoluteYearly`
/// when its month and day are the due date's, each with an interval, and a range of `noEnd` or
/// `endDate`. Everything else (`relativeMonthly`, `relativeYearly`, a `numbered` range, a yearly rule on
/// another day than the due date, or a rule without a due date) is an error with the reason, so the
/// caller can report it instead of silently dropping or widening it.
pub fn recurrence(raw: &serde_json::Value, due: Option<Date>) -> Result<Recurrence, MapError> {
    let p: Patterned = serde_json::from_value(raw.clone())
        .map_err(|e| MapError(format!("unreadable recurrence: {e}")))?;
    let due = due.ok_or_else(|| MapError("recurrence without a due date".into()))?;
    let interval = p.pattern.interval.unwrap_or(1);
    let rule = match p.pattern.kind.as_str() {
        "daily" => Rule::Daily { interval },
        "weekly" => {
            let mut days = BTreeSet::new();
            for d in &p.pattern.days_of_week {
                days.insert(weekday(d).ok_or_else(|| MapError(format!("unknown weekday `{d}`")))?);
            }
            if days.is_empty() {
                days.insert(Weekday::of(due));
            }
            Rule::Weekly { interval, days }
        }
        "absoluteMonthly" => Rule::Monthly {
            interval,
            day: p
                .pattern
                .day_of_month
                .filter(|d| *d > 0)
                .unwrap_or_else(|| u8::try_from(due.day()).unwrap_or(1)),
        },
        "absoluteYearly" => {
            let same_day = p.pattern.month.is_none_or(|m| m == due.month())
                && p.pattern
                    .day_of_month
                    .is_none_or(|d| i8::try_from(d).ok() == Some(due.day()));
            if !same_day {
                return Err(MapError(
                    "yearly rule on another day than the due date".into(),
                ));
            }
            Rule::Yearly { interval }
        }
        other => return Err(MapError(format!("pattern type `{other}` is not supported"))),
    };
    let until = match p.range {
        None => None,
        Some(r) => match r.kind.as_str() {
            "" | "noEnd" => None,
            "endDate" => Some(
                r.end_date
                    .as_deref()
                    .ok_or_else(|| MapError("endDate range without an end date".into()))?
                    .parse::<Date>()
                    .map_err(|e| MapError(format!("bad end date: {e}")))?,
            ),
            other => return Err(MapError(format!("range type `{other}` is not supported"))),
        },
    };
    let r = Recurrence { rule, until };
    r.validate().map_err(|e| MapError(e.to_string()))?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(y: i16, m: i8, day: i8) -> Date {
        Date::constant(y, m, day)
    }

    #[test]
    fn due_dates_are_taken_in_the_owner_zone() {
        let berlin = TimeZone::get("Europe/Berlin").unwrap();
        let v = DateTimeTz {
            date_time: "2026-01-31T23:00:00.0000000".into(),
            time_zone: "UTC".into(),
        };
        assert_eq!(due_date(&v, &berlin).unwrap(), d(2026, 2, 1));
        assert_eq!(due_date(&v, &TimeZone::UTC).unwrap(), d(2026, 1, 31));
        let summer = DateTimeTz {
            date_time: "2026-06-30T22:00:00.0000000".into(),
            time_zone: "UTC".into(),
        };
        assert_eq!(due_date(&summer, &berlin).unwrap(), d(2026, 7, 1));
    }

    #[test]
    fn instants_accept_seven_fraction_digits() {
        let t = instant("2024-05-06T07:08:09.1234567Z").unwrap();
        assert_eq!(t.subsec_nanosecond(), 123_456_700);
    }

    #[test]
    fn notes_are_normalised() {
        assert_eq!(notes("\r\n", "text"), "");
        assert_eq!(notes("a\r\nb  \r\n", "text"), "a\nb");
        assert_eq!(
            notes("<p>Hi &amp; bye</p><div>x<br/>y</div>", "html"),
            "Hi & bye\nx\ny"
        );
    }

    #[test]
    fn titles_and_names() {
        assert_eq!(title(Some("  a ")), "a");
        assert_eq!(title(None), UNTITLED);
        assert_eq!(title(Some(&"x".repeat(600))).chars().count(), MAX_TITLE);
        assert_eq!(file_name("dir\\sub/report.pdf"), "report.pdf");
        assert_eq!(file_name(""), "attachment");
    }

    #[test]
    fn importance_and_themes() {
        assert_eq!(priority("high"), Priority::High);
        assert_eq!(priority("normal"), Priority::None);
        assert_eq!(priority("low"), Priority::Low);
        assert_eq!(theme_color("dark_blue").as_deref(), Some("#2564cf"));
        assert_eq!(theme_color("brand_new").as_deref(), Some("brand_new"));
        assert_eq!(theme_color(""), None);
    }

    #[test]
    fn recurrence_that_maps() {
        let due = Some(d(2026, 3, 2));
        let weekly = json!({
            "pattern": {"type": "weekly", "interval": 2, "daysOfWeek": ["monday", "friday"],
                        "firstDayOfWeek": "sunday", "dayOfMonth": 0, "month": 0, "index": "first"},
            "range": {"type": "endDate", "startDate": "2026-03-02", "endDate": "2026-12-31"}
        });
        let r = recurrence(&weekly, due).unwrap();
        assert_eq!(
            r.to_rrule(),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR;UNTIL=20261231"
        );
        let daily =
            json!({"pattern": {"type": "daily", "interval": 3}, "range": {"type": "noEnd"}});
        assert_eq!(
            recurrence(&daily, due).unwrap().rule,
            Rule::Daily { interval: 3 }
        );
        let monthly =
            json!({"pattern": {"type": "absoluteMonthly", "interval": 1, "dayOfMonth": 15}});
        assert_eq!(
            recurrence(&monthly, due).unwrap().rule,
            Rule::Monthly {
                interval: 1,
                day: 15
            }
        );
        let yearly = json!({"pattern": {"type": "absoluteYearly", "interval": 1, "month": 3, "dayOfMonth": 2}});
        assert_eq!(
            recurrence(&yearly, due).unwrap().rule,
            Rule::Yearly { interval: 1 }
        );
    }

    #[test]
    fn recurrence_that_does_not_map_is_an_error() {
        let due = Some(d(2026, 3, 2));
        let relative = json!({"pattern": {"type": "relativeMonthly", "interval": 1,
                                         "daysOfWeek": ["monday"], "index": "first"}});
        assert!(
            recurrence(&relative, due)
                .unwrap_err()
                .0
                .contains("relativeMonthly")
        );
        let numbered = json!({"pattern": {"type": "daily", "interval": 1},
                              "range": {"type": "numbered", "numberOfOccurrences": 5}});
        assert!(
            recurrence(&numbered, due)
                .unwrap_err()
                .0
                .contains("numbered")
        );
        let other_day = json!({"pattern": {"type": "absoluteYearly", "interval": 1, "month": 7, "dayOfMonth": 4}});
        assert!(recurrence(&other_day, due).is_err());
        let daily = json!({"pattern": {"type": "daily", "interval": 1}});
        assert!(recurrence(&daily, None).unwrap_err().0.contains("due date"));
        let zero = json!({"pattern": {"type": "daily", "interval": 0}});
        assert!(recurrence(&zero, due).is_err());
    }
}
