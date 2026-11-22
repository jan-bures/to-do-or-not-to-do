//! The reminder editor's time choices. The browser supplies its time-zone conversion in live mode;
//! fixtures use a fixed offset.

use crate::model::Now;
use jiff::{SignedDuration, Span, Timestamp, civil::DateTime, tz::Offset};
use todo_client::intent::reminder::REMINDER_PAST;

pub(crate) fn fixed_instant(dt: DateTime, now: Now) -> Option<Timestamp> {
    Offset::from_seconds(now.offset_s)
        .ok()?
        .to_timestamp(dt)
        .ok()
}

#[derive(Clone, Copy)]
pub(crate) enum Preset {
    InAnHour,
    AtNineInDays(i32),
}

impl Preset {
    pub(crate) fn at(
        self,
        now: Now,
        resolve: impl FnOnce(DateTime) -> Option<Timestamp>,
    ) -> Option<Timestamp> {
        match self {
            Self::InAnHour => now.at.checked_add(SignedDuration::from_hours(1)).ok(),
            Self::AtNineInDays(days) => {
                let day = now.today().checked_add(Span::new().days(days)).ok()?;
                resolve(day.at(9, 0, 0, 0))
            }
        }
    }
}

#[derive(Default, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) date: String,
    pub(crate) time: String,
}

impl Draft {
    pub(crate) fn from_civil(dt: DateTime) -> Self {
        Self {
            date: dt.date().to_string(),
            time: format!("{:02}:{:02}", dt.hour(), dt.minute()),
        }
    }

    pub(crate) fn at(
        &self,
        original: Option<(Timestamp, DateTime)>,
        now: Timestamp,
        resolve: impl FnOnce(DateTime) -> Option<Timestamp>,
    ) -> Result<Timestamp, &'static str> {
        let dt = format!("{}T{}", self.date, self.time)
            .parse()
            .map_err(|_| "Choose a date and time.")?;
        // Preserve seconds and the exact instant in a repeated hour if nothing was changed.
        if let Some((at, civil)) = original
            && *self == Self::from_civil(civil)
        {
            return Ok(at);
        }
        let at =
            resolve(dt).ok_or("That time doesn’t exist in this time zone. Choose another time.")?;
        if at <= now {
            return Err(REMINDER_PAST);
        }
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, model};

    fn draft(date: &str, time: &str) -> Draft {
        Draft {
            date: date.into(),
            time: time.into(),
        }
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("test timestamp")
    }

    #[test]
    fn presets_use_elapsed_time_or_the_local_calendar() {
        let now = fixtures::now(); // 10:35 at UTC+01:00.
        let resolve = |dt| fixed_instant(dt, now);
        assert_eq!(
            Preset::InAnHour.at(now, resolve),
            Some(ts("2026-11-21T10:35:00Z"))
        );
        assert_eq!(
            Preset::AtNineInDays(1).at(now, resolve),
            Some(ts("2026-11-22T08:00:00Z"))
        );
        assert_eq!(
            Preset::AtNineInDays(7).at(now, resolve),
            Some(ts("2026-11-28T08:00:00Z"))
        );
        assert_eq!(Preset::AtNineInDays(1).at(now, |_| None), None);
    }

    #[test]
    fn draft_checks_the_date_time_and_future_in_a_fixed_offset() {
        let now = fixtures::now();
        let at = |date, time| draft(date, time).at(None, now.at, |dt| fixed_instant(dt, now));
        assert_eq!(at("2026-11-30", ""), Err("Choose a date and time."));
        assert_eq!(at("2026-11-31", "10:35"), Err("Choose a date and time."));
        assert_eq!(at("2026-11-30", "25:00"), Err("Choose a date and time."));
        assert_eq!(at("2026-11-21", "10:34"), Err(REMINDER_PAST));
        assert_eq!(at("2026-11-21", "10:35"), Err(REMINDER_PAST));
        assert_eq!(at("2026-11-21", "10:36"), Ok(ts("2026-11-21T09:36:00Z")));
    }

    #[test]
    fn an_unchanged_draft_preserves_the_timestamp_even_if_past() {
        let now = fixtures::now();
        let at = ts("2026-11-21T09:34:23Z");
        let civil = model::local(at, now.offset_s);
        assert_eq!(
            draft("2026-11-21", "10:34").at(Some((at, civil)), now.at, |_| {
                panic!("an unchanged draft must not resolve another instant")
            }),
            Ok(at)
        );
    }

    #[test]
    fn a_skipped_time_is_refused() {
        let now = fixtures::now();
        // Model a spring gap around 02:30 in an otherwise fixed zone. The live browser test
        // exercises the actual transition; a fixed UTC offset alone never skips a wall time.
        let skipped: DateTime = "2027-03-28T02:30".parse().expect("test wall time");
        let resolve = |dt| (dt != skipped).then(|| fixed_instant(dt, now)).flatten();
        assert_eq!(
            draft("2027-03-28", "02:30").at(None, now.at, resolve),
            Err("That time doesn’t exist in this time zone. Choose another time.")
        );
        assert_eq!(
            draft("2027-03-28", "03:30").at(None, now.at, resolve),
            Ok(ts("2027-03-28T02:30:00Z"))
        );
    }
}
