//! Quick-add text: a title, then optional trailing words for the due date and the priority.
//!
//! ```text
//! Draft the changelog tomorrow        → due tomorrow
//! Pay the invoice by fri !!           → due on the coming Friday, priority normal
//! Koupit mléko zítra                  → due tomorrow
//! Book flights 2026-12-01             → due 1 Dec 2026
//! ```
//!
//! After at least one title word, read at most one trailing date and one priority, in either order.
//! Dates are `today` / `dnes`, `tomorrow` / `zítra`, `+Nd`, `YYYY-MM-DD` (today or later), and English
//! or Czech weekday names (today or the next occurrence). `on` or `by` is optional before a full
//! weekday name and required before an abbreviation. Whole phrases also work:
//! - `in 2 days`, `in 3 weeks` / `za 2 dny`, `za 3 týdny`;
//! - `next week` / `příští týden` (Monday), or a full weekday in the next Monday–Sunday week;
//! - `this weekend` / `o víkendu` (Saturday, or today during the weekend);
//! - `end of month` / `konec měsíce` (the last day of this month);
//! - `1 Dec`, `1. 12.`, `1.12.` or `1. prosince` (next occurrence, including today).
//!
//! Day-month and month-end phrases accept `on`, `by` or `do` (e.g. `do konce měsíce`). A compact
//! `D.M.` stays literal after `v`, `ve`, `at`, `version`, `verze`, `section`, `sekce`, `chapter`,
//! `kapitola`, `release`, `part` or `část`. Elsewhere use `\1.12.` to keep it literal. A trailing
//! `o víkendu` can also mean “about the weekend”; use `o \víkendu` when it belongs in the title.
//! Case is ignored; Czech words also work without accents. Incomplete phrases stay literal.
//! Only standalone `!!` (normal) and `!!!` (high) are priority markers; `!` and `Wow!` stay literal.
//! A leading backslash on the final title token stops parsing there and is removed: `Plan \today !!`
//! keeps `Plan today` as the title but still sets priority.
//! Workflow lists also accept `status:wip` / `stav:rozpracováno`; timeline lists accept
//! `today..+4d` / `24 Nov..28 Nov`. These suffixes are opt-in and can also be escaped.

use std::ops::Range;

use todo_core::model::{ListFeatures, Priority, WorkflowStatus};
use todo_core::query::{EvalContext, fold, parse_date};
use todo_core::time::{Date, Weekday, add_days};

/// What the quick-add text says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub due: Option<Date>,
    pub start_date: Option<Date>,
    pub priority: Option<Priority>,
    pub status: Option<WorkflowStatus>,
}

/// The date chip shared by the clients. A start-only draft keeps its field name.
pub fn date_preview(
    start: Option<Date>,
    due: Option<Date>,
    dates: crate::date_format::DateLabels,
) -> Option<String> {
    match (start, due) {
        (Some(start), Some(due)) => Some(dates.range(start, due)),
        (Some(start), None) => Some(format!("Starts {}", dates.weekday(start))),
        (None, due) => due.map(|d| dates.weekday(d)),
    }
}

fn weekday(word: &str, abbreviated: bool) -> Option<Weekday> {
    let day = match word {
        "monday" | "pondeli" => Weekday::Mon,
        "tuesday" | "utery" => Weekday::Tue,
        "wednesday" | "streda" => Weekday::Wed,
        "thursday" | "ctvrtek" => Weekday::Thu,
        "friday" | "patek" => Weekday::Fri,
        "saturday" | "sobota" => Weekday::Sat,
        "sunday" | "nedele" => Weekday::Sun,
        "mon" | "po" if abbreviated => Weekday::Mon,
        "tue" | "tues" | "ut" if abbreviated => Weekday::Tue,
        "wed" | "st" if abbreviated => Weekday::Wed,
        "thu" | "thur" | "thurs" | "ct" if abbreviated => Weekday::Thu,
        "fri" | "pa" if abbreviated => Weekday::Fri,
        "sat" | "so" if abbreviated => Weekday::Sat,
        "sun" | "ne" if abbreviated => Weekday::Sun,
        _ => return None,
    };
    Some(day)
}

/// A date other than a weekday, resolved against the local date in `ctx`.
fn calendar_date(word: &str, ctx: &EvalContext) -> Option<Date> {
    match fold(word).as_str() {
        "dnes" => Some(ctx.today),
        "zitra" => add_days(ctx.today, 1),
        _ => ctx.resolve(parse_date(word)?),
    }
}

fn weekday_from(target: Weekday, date: Date) -> Option<Date> {
    let from = Weekday::of(date).offset() as i64;
    let ahead = (target.offset() as i64 - from).rem_euclid(7);
    add_days(date, ahead)
}

fn next_weekday(target: Weekday, ctx: &EvalContext) -> Option<Date> {
    weekday_from(target, ctx.today)
}

fn priority_word(word: &str) -> Option<Priority> {
    match word {
        "!!" => Some(Priority::Normal),
        "!!!" => Some(Priority::High),
        _ => None,
    }
}

/// A due date typed on its own. Unlike quick-add, the explicit field also accepts past dates and
/// unprefixed weekday abbreviations.
pub fn due_text(text: &str, ctx: &EvalContext) -> Option<Date> {
    let words: Vec<_> = text.split_whitespace().collect();
    if let [word] = words.as_slice()
        && let Some(date) =
            calendar_date(word, ctx).or_else(|| next_weekday(weekday(&fold(word), true)?, ctx))
    {
        return Some(date);
    }
    let (date, count) = date_suffix(&words, ctx)?;
    (count == words.len()).then_some(date)
}

fn date_suffix(words: &[&str], ctx: &EvalContext) -> Option<(Date, usize)> {
    let (last, before) = words.split_last()?;
    let last = fold(last);
    let (prefix, before_prefix) = before.split_last().unwrap_or((&"", &[]));
    let prefix = fold(prefix);
    let first = before_prefix.last().map_or(String::new(), |s| fold(s));
    let today = ctx.today;
    let offset = Weekday::of(today).offset() as i64;
    let scale = match (first.as_str(), last.as_str()) {
        ("in", "day" | "days") | ("za", "den" | "dny" | "dni" | "dnu") => Some(1),
        ("in", "week" | "weeks") | ("za", "tyden" | "tydny" | "tydnu") => Some(7),
        _ => None,
    };
    if let Some(scale) = scale
        && prefix.bytes().all(|b| b.is_ascii_digit())
    {
        let days = prefix.parse::<i64>().ok()?.checked_mul(scale)?;
        return Some((add_days(today, days)?, 3));
    }
    if matches!(prefix.as_str(), "next" | "pristi") {
        let day = match last.as_str() {
            "stredu" => Some(Weekday::Wed),
            "sobotu" => Some(Weekday::Sat),
            "nedeli" => Some(Weekday::Sun),
            day => weekday(day, false),
        };
        if let Some(day) = day {
            return Some((add_days(today, 7 - offset + day.offset() as i64)?, 2));
        }
    }
    if matches!(prefix.as_str(), "on" | "by")
        && let Some(day) = weekday(&last, true)
    {
        return Some((next_weekday(day, ctx)?, 2));
    }
    // A date introducer is part of the suffix (and therefore cannot serve as the only title word).
    let with_intro = |date, count| {
        let has_intro = words[..words.len() - count]
            .last()
            .is_some_and(|w| matches!(fold(w).as_str(), "on" | "by" | "do"));
        Some((date, count + usize::from(has_intro)))
    };
    match (prefix.as_str(), last.as_str()) {
        ("of", "month") if first == "end" => with_intro(today.last_of_month(), 3),
        ("konec", "mesice") => with_intro(today.last_of_month(), 2),
        ("konce", "mesice") if first == "do" => Some((today.last_of_month(), 3)),
        ("this", "weekend") | ("o", "vikendu") => Some((add_days(today, (5 - offset).max(0))?, 2)),
        ("next", "week") | ("pristi", "tyden") => Some((super::calendar::next_monday(today)?, 2)),
        _ => {
            if let Some(date) = day_month(&prefix, &last, today) {
                return with_intro(date, 2);
            }
            if let Some((day, month)) = last.split_once('.') {
                if matches!(
                    prefix.as_str(),
                    "v" | "ve"
                        | "at"
                        | "version"
                        | "verze"
                        | "section"
                        | "sekce"
                        | "chapter"
                        | "kapitola"
                        | "release"
                        | "part"
                        | "cast"
                ) {
                    return None;
                }
                return with_intro(numeric_day_month(day, month, today)?, 1);
            }
            let date = if let Some(day) = weekday(&last, false) {
                next_weekday(day, ctx)?
            } else {
                // Even `-0d` isn't quick-add syntax.
                if last.starts_with('-') {
                    return None;
                }
                calendar_date(&last, ctx)?
            };
            (date >= today).then_some((date, 1))
        }
    }
}

fn part(s: &str) -> Option<i8> {
    (s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn occurrence(day: i8, month: i8, today: Date) -> Option<Date> {
    // Reject impossible dates immediately; a leap day can be eight years away across a century.
    Date::new(2000, month, day).ok()?;
    (today.year()..=today.year() + 8)
        .find_map(|year| Date::new(year, month, day).ok().filter(|d| *d >= today))
}

fn numeric_day_month(day: &str, month: &str, today: Date) -> Option<Date> {
    occurrence(part(day)?, part(month.strip_suffix('.')?)?, today)
}

fn day_month(day: &str, month: &str, today: Date) -> Option<Date> {
    if let Some(day) = day.strip_suffix('.')
        && month.ends_with('.')
    {
        return numeric_day_month(day, month, today);
    }
    let month = match month {
        "jan" | "january" | "ledna" => 1,
        "feb" | "february" | "unora" => 2,
        "mar" | "march" | "brezna" => 3,
        "apr" | "april" | "dubna" => 4,
        "may" | "kvetna" => 5,
        "jun" | "june" | "cervna" => 6,
        "jul" | "july" | "cervence" => 7,
        "aug" | "august" | "srpna" => 8,
        "sep" | "september" | "zari" => 9,
        "oct" | "october" | "rijna" => 10,
        "nov" | "november" | "listopadu" => 11,
        "dec" | "december" | "prosince" => 12,
        _ => return None,
    };
    occurrence(part(day.strip_suffix('.').unwrap_or(day))?, month, today)
}

/// Parses quick-add text. The title is trimmed; it is empty only if the text was.
pub fn parse(text: &str, ctx: &EvalContext) -> Parsed {
    parse_with_features(text, ctx, ListFeatures::default())
}

/// Parses a composer whose destination list supplies the opt-in capabilities.
pub fn parse_with_features(text: &str, ctx: &EvalContext, features: ListFeatures) -> Parsed {
    parse_parts(text, ctx, features).0
}

fn status_word(word: &str) -> Option<WorkflowStatus> {
    let word = fold(word);
    let (key, value) = word.split_once(':')?;
    if !matches!(key, "status" | "stav") {
        return None;
    }
    Some(match value {
        "backlog" => WorkflowStatus::Backlog,
        "todo" => WorkflowStatus::Todo,
        "in_progress" | "wip" | "rozpracovano" => WorkflowStatus::InProgress,
        _ => return None,
    })
}

// An ordinary date phrase takes at most four words with an introducer. Try the entire suffix,
// including spaces inside either endpoint, without scanning the whole title for separators.
fn range_suffix(words: &[&str], ctx: &EvalContext) -> Option<(Date, Date, usize)> {
    for count in (1..=words.len().saturating_sub(1).min(8)).rev() {
        let suffix = &words[words.len() - count..];
        if !suffix.iter().any(|w| w.contains("..")) || suffix.iter().any(|w| w.starts_with('\\')) {
            continue;
        }
        let text = suffix.join(" ");
        // The separator can follow a Czech date's trailing dot (`1.12...4.12.`).
        let Some(sep) = text.find("..") else { continue };
        let after_dot = text[sep..].starts_with("...").then_some(sep + 1);
        for i in [Some(sep), after_dot].into_iter().flatten() {
            let Some(start) = due_text(&text[..i], ctx).filter(|d| *d >= ctx.today) else {
                continue;
            };
            let end = &text[i + 2..];
            // Plain weekday endpoints belong to the week of the start (or the next one),
            // while explicit dates and relative offsets keep their usual meaning.
            let end = match weekday(&fold(end.trim()), true) {
                Some(day) => weekday_from(day, start),
                None => due_text(end, ctx),
            };
            if let Some(end) = end.filter(|d| *d >= start) {
                return Some((start, end, count));
            }
        }
    }
    None
}

#[derive(Default)]
struct Suffixes {
    title_end: usize,
    due: Option<Range<usize>>,
    priority: Option<Range<usize>>,
    status: Option<Range<usize>>,
}

fn parse_parts(text: &str, ctx: &EvalContext, features: ListFeatures) -> (Parsed, Suffixes) {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let mut suffixes = Suffixes::default();
    let span = |start: usize, end: usize| {
        let offset = |word: &str| word.as_ptr().addr() - text.as_ptr().addr();
        offset(words[start])..offset(words[end - 1]) + words[end - 1].len()
    };
    let mut end = words.len();
    let mut due = None;
    let mut start_date = None;
    let mut priority = None;
    let mut status = None;
    // At most one of each field, in any order, at the end.
    for _ in 0..3 {
        let Some(last) = end.checked_sub(1).map(|i| words[i]) else {
            break;
        };
        if end <= 1 || last.starts_with('\\') {
            break; // Keep at least one word of title. An escape also ends suffix parsing.
        }
        if priority.is_none()
            && let Some(p) = priority_word(last)
        {
            priority = Some(p);
            suffixes.priority = Some(span(end - 1, end));
            end -= 1;
        } else if features.workflow
            && status.is_none()
            && let Some(stage) = status_word(last)
        {
            status = Some(stage);
            suffixes.status = Some(span(end - 1, end));
            end -= 1;
        } else if due.is_none()
            && let Some((start, d, count)) = range_suffix(&words[..end], ctx)
        {
            if !features.timeline {
                break;
            }
            start_date = Some(start);
            due = Some(d);
            suffixes.due = Some(span(end - count, end));
            end -= count;
        } else if due.is_none()
            && let Some((d, count)) = date_suffix(&words[..end], ctx)
        {
            // Never take only part of a date phrase or the trailing half of a range.
            if count == end
                || (features.timeline
                    && words[end.saturating_sub(count + 1)..end]
                        .iter()
                        .any(|w| w.contains("..") && !w.contains("...")))
            {
                break;
            }
            due = Some(d);
            suffixes.due = Some(span(end - count, end));
            end -= count;
        } else {
            break;
        }
    }
    suffixes.title_end = if end == 0 { 0 } else { span(0, end).end };
    words.truncate(end);
    if let Some(last) = words.last_mut()
        && let Some(literal) = last.strip_prefix('\\').filter(|word| !word.is_empty())
    {
        *last = literal;
    }
    (
        Parsed {
            title: words.join(" "),
            due,
            start_date,
            priority,
            status,
        },
        suffixes,
    )
}

/// A date choice can explicitly clear the room default, or leave it alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DueChoice {
    #[default]
    Default,
    Clear,
    Date(Date),
}

/// One composer's unsent input. Typed and picked values share the same preview and submit path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub due: DueChoice,
    /// Picked start date; hidden when Timeline is disabled.
    pub start_date: Option<Date>,
    /// `None` keeps the room default; `Some(Priority::None)` explicitly clears it.
    pub priority: Option<Priority>,
    /// Picked open stage; hidden when Workflow is disabled.
    pub status: Option<WorkflowStatus>,
}

/// A picker change, reconciled with the typed suffixes for the same fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Due(Option<Date>),
    Start(Option<Date>),
    Priority(Priority),
    Status(Option<WorkflowStatus>),
}

impl Draft {
    /// Edit the legacy two-field composer.
    pub fn edit(&mut self, text: String, ctx: &EvalContext) {
        self.edit_with_features(text, ctx, ListFeatures::default());
    }

    /// Newly typed suffixes supersede picked values of the same fields.
    pub fn edit_with_features(&mut self, text: String, ctx: &EvalContext, features: ListFeatures) {
        let typed = parse_with_features(&text, ctx, features);
        if typed.due.is_some() {
            self.due = DueChoice::Default;
        }
        if typed.start_date.is_some() {
            self.start_date = None;
        }
        if typed.priority.is_some() {
            self.priority = None;
        }
        if typed.status.is_some() {
            self.status = None;
        }
        self.text = text;
    }

    pub fn pick_due(&mut self, date: Option<Date>, ctx: &EvalContext) {
        self.pick(Pick::Due(date), ctx, ListFeatures::default());
    }

    pub fn pick_priority(&mut self, level: Priority, ctx: &EvalContext) {
        self.pick(Pick::Priority(level), ctx, ListFeatures::default());
    }

    /// Remove just the relevant suffix, preserving the other date if the suffix was a range.
    pub fn pick(&mut self, pick: Pick, ctx: &EvalContext, features: ListFeatures) {
        let (mut expected, mut parts) = parse_parts(&self.text, ctx, features);
        let removed = match pick {
            Pick::Due(date) => {
                self.start_date = expected.start_date.or(self.start_date);
                self.due = date.map_or(DueChoice::Clear, DueChoice::Date);
                expected.start_date = None;
                expected.due = None;
                parts.due.take()
            }
            Pick::Start(start) if features.timeline => {
                // A single typed due date can remain in the text.
                self.start_date = start;
                if expected.start_date.take().is_some() {
                    if let Some(due) = expected.due.take() {
                        self.due = DueChoice::Date(due);
                    }
                    parts.due.take()
                } else {
                    None
                }
            }
            Pick::Priority(level) => {
                self.priority = Some(level);
                expected.priority = None;
                parts.priority.take()
            }
            Pick::Status(status) if features.workflow => {
                self.status = status;
                expected.status = None;
                parts.status.take()
            }
            Pick::Start(_) | Pick::Status(_) => return,
        };
        let Some(removed) = removed else { return };
        let mut rewritten = self.text.clone();
        let start = self.text[..removed.start].trim_end().len();
        rewritten.replace_range(start..removed.end, "");
        if parse_with_features(&rewritten, ctx, features) != expected {
            // Avoid exposing a second suffix in the title. Keep retained suffixes in input order.
            let title = &self.text[..parts.title_end];
            rewritten = title.to_owned();
            if let Some(last) = title.split_whitespace().last()
                && !last.starts_with('\\')
            {
                rewritten.insert(title.len() - last.len(), '\\');
            }
            let mut kept: Vec<_> = [parts.due, parts.priority, parts.status]
                .into_iter()
                .flatten()
                .collect();
            kept.sort_by_key(|range| range.start);
            for range in kept {
                rewritten.push(' ');
                rewritten.push_str(&self.text[range]);
            }
        }
        self.text = rewritten;
    }

    /// Preview exactly the fields the legacy draft supplies, before room defaults.
    pub fn parsed(&self, ctx: &EvalContext) -> Parsed {
        self.parsed_with_features(ctx, ListFeatures::default())
    }

    /// Preview the same fields that an enabled composer will submit.
    pub fn parsed_with_features(&self, ctx: &EvalContext, features: ListFeatures) -> Parsed {
        let mut parsed = parse_with_features(&self.text, ctx, features);
        if parsed.due.is_none()
            && let DueChoice::Date(date) = self.due
        {
            parsed.due = Some(date);
        }
        if features.timeline {
            parsed.start_date = parsed.start_date.or(self.start_date);
            if let (Some(start), Some(due)) = (parsed.start_date, parsed.due) {
                parsed.due = Some(due.max(start));
            }
        }
        if features.workflow {
            parsed.status = parsed.status.or(self.status);
        }
        parsed.priority = parsed.priority.or(self.priority);
        parsed
    }
}

/// One option in the priority menu, shared between detail and quick-add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriorityOption {
    pub level: Priority,
    pub label: &'static str,
    pub mark: &'static str,
}

pub const PRIORITIES: [PriorityOption; 4] = [
    PriorityOption {
        level: Priority::High,
        label: "High",
        mark: "!!!",
    },
    PriorityOption {
        level: Priority::Normal,
        label: "Normal",
        mark: "!!",
    },
    PriorityOption {
        level: Priority::Low,
        label: "Low",
        mark: "!",
    },
    PriorityOption {
        level: Priority::None,
        label: "None",
        mark: "",
    },
];

pub fn priority_option(level: Priority) -> PriorityOption {
    PRIORITIES[3 - level as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ctx() -> EvalContext {
        // Saturday 21 Nov 2026.
        EvalContext::utc("2026-11-21T10:35:00Z".parse().expect("timestamp"))
    }

    #[test]
    fn chip_labels_show_the_weekday_and_disambiguate_other_years() {
        let dates = crate::date_format::DateFormat::default().on(ctx().today);
        let label = |iso: &str| date_preview(None, Some(iso.parse().expect("date")), dates);
        assert_eq!(label("2026-11-22").as_deref(), Some("Sun 22.11."));
        assert_eq!(label("2027-01-04").as_deref(), Some("Mon 04.01.2027"));
    }

    #[test]
    fn choosing_consumes_only_the_matching_suffix_in_either_order() {
        let ctx = ctx();
        for (text, after_date, after_priority) in [
            (
                "Kup  mléko by 1 Dec !!",
                "Kup  mléko !!",
                "Kup  mléko by 1 Dec",
            ),
            (
                "Kup  mléko !! příští týden",
                "Kup  mléko !!",
                "Kup  mléko příští týden",
            ),
        ] {
            let draft = Draft {
                text: text.into(),
                ..Draft::default()
            };
            let mut due = draft.clone();
            due.pick_due(Some(ctx.today), &ctx);
            assert_eq!(due.text, after_date);
            assert_eq!(due.parsed(&ctx).due, Some(ctx.today));
            assert_eq!(due.parsed(&ctx).priority, Some(Priority::Normal));
            let mut priority = draft;
            priority.pick_priority(Priority::Low, &ctx);
            assert_eq!(priority.text, after_priority);
            assert_eq!(priority.parsed(&ctx).priority, Some(Priority::Low));
            assert_eq!(priority.parsed(&ctx).due, parse(text, &ctx).due);
        }
    }

    #[test]
    fn removing_a_chip_keeps_escapes_and_does_not_expose_an_old_suffix() {
        let ctx = ctx();
        for (text, title) in [
            ("Plan today tomorrow !!", "Plan today"),
            (r"Plan \today tomorrow !!", "Plan today"),
        ] {
            let mut draft = Draft {
                text: text.into(),
                ..Draft::default()
            };
            draft.pick_due(None, &ctx);
            assert_eq!(draft.text, r"Plan \today !!");
            assert_eq!(
                draft.parsed(&ctx),
                Parsed {
                    title: title.into(),
                    due: None,
                    priority: Some(Priority::Normal),
                    ..Parsed::default()
                }
            );
        }
        let mut draft = Draft {
            text: "Plan by !! 1 Dec".into(),
            ..Draft::default()
        };
        draft.pick_priority(Priority::None, &ctx);
        assert_eq!(draft.parsed(&ctx).title, "Plan by");
        assert_eq!(draft.parsed(&ctx).due, Some(Date::constant(2026, 12, 1)));
    }

    #[test]
    fn typing_over_a_pick_does_not_resurrect_it_after_deleting_the_token() {
        let ctx = ctx();
        let mut draft = Draft {
            text: "Plans".into(),
            ..Draft::default()
        };
        draft.pick_priority(Priority::Low, &ctx);
        draft.pick_due(Some(ctx.today), &ctx);
        draft.edit("Plans tomorrow".into(), &ctx);
        assert_eq!(draft.parsed(&ctx).due, add_days(ctx.today, 1));
        draft.edit("Plans".into(), &ctx);
        assert_eq!(draft.parsed(&ctx).due, None);
        assert_eq!(draft.parsed(&ctx).priority, Some(Priority::Low));
    }

    fn literal_title(words: &[&str]) -> String {
        let mut words = words.to_vec();
        if let Some(last) = words.last_mut()
            && let Some(literal) = last.strip_prefix('\\').filter(|word| !word.is_empty())
        {
            *last = literal;
        }
        words.join(" ")
    }

    proptest! {
        #[test]
        fn arbitrary_text_keeps_a_normalized_title(text in any::<String>()) {
            let parsed = parse(&text, &ctx());
            let words = text.split_whitespace().collect::<Vec<_>>();
            let title_words = parsed.title.split_whitespace().count();
            prop_assert_eq!(parsed.title.is_empty(), words.is_empty());
            prop_assert!(title_words <= words.len());
            prop_assert_eq!(parsed.title, literal_title(&words[..title_words]));
            prop_assert!(words.len() - title_words <= 5);
            let _ = due_text(&text, &ctx());
        }

        #[test]
        fn mixed_vocabulary_removes_only_one_suffix_of_each_kind(
            words in prop::collection::vec(prop::sample::select(vec![
                "Enjoy", "the", "Where", "dog", "sat", "sun", "wed", "mon", "fri", "ne", "pá",
                "Wow!", "!", "!!", "!!!", "!!!!", "on", "BY", "today", "tomorrow", "yesterday",
                "dnes", "zítra", "včera", "Monday", "Saturday", "sunday", "úterý", "pátek",
                "+0d", "+3d", "-1d", "-0d", "2026-11-20", "2026-11-21", "2026-12-01",
                "next", "week", "příští týden", "in", "2", "days", "za 3 týdny", "end of month",
                "this weekend", "o víkendu", "konec měsíce", "1. prosince", "1.12.", r"\week", "by end of month", "do", "konce měsíce", "v", "Release",
            ]), 1..16),
        ) {
            let input = words.join("\t");
            let words: Vec<_> = input.split_whitespace().collect();
            let parsed = parse(&input, &ctx());
            let title_words = parsed.title.split_whitespace().count();
            prop_assert!(title_words > 0 && title_words <= words.len());
            prop_assert_eq!(parsed.title, literal_title(&words[..title_words]));
            let mut suffix = &words[title_words..];
            let priority = if let [p, rest @ ..] = suffix && priority_word(p).is_some() {
                suffix = rest;
                priority_word(p)
            } else if let [rest @ .., p] = suffix && priority_word(p).is_some() {
                suffix = rest;
                priority_word(p)
            } else { None };
            prop_assert_eq!(priority, parsed.priority);
            prop_assert_eq!(parsed.due.is_some(), !suffix.is_empty());
            if !suffix.is_empty() {
                prop_assert!(suffix.len() <= 4);
                prop_assert!(!matches!(suffix, [w] if weekday(&fold(w), true).is_some() && weekday(&fold(w), false).is_none()));
                prop_assert!(!suffix.iter().any(|w| w.starts_with('-')));
                prop_assert_eq!(parsed.due, due_text(&suffix.join(" "), &ctx()));
                prop_assert!(parsed.due.is_some_and(|d| d >= ctx().today));
            }
        }

        #[test]
        fn arbitrary_title_accepts_trailing_date_and_priority(text in any::<String>(), reverse in any::<bool>()) {
            let title = format!("Task {text}");
            let normalized = literal_title(&title.split_whitespace().collect::<Vec<_>>());
            let suffix = if reverse { "!! za 1 den" } else { "in 1 day !!" };
            prop_assert_eq!(parse(&format!("{title} {suffix}"), &ctx()), Parsed {
                title: normalized,
                due: d("2026-11-22"),
                priority: Some(Priority::Normal),
                ..Parsed::default()
            });
        }
    }

    fn d(s: &str) -> Option<Date> {
        Some(s.parse().expect("date"))
    }

    #[test]
    fn due_dates_typed_on_their_own() {
        assert_eq!(due_text(" Tomorrow ", &ctx()), d("2026-11-22"));
        assert_eq!(due_text("fri", &ctx()), d("2026-11-27"));
        assert_eq!(due_text("+3d", &ctx()), d("2026-11-24"));
        assert_eq!(due_text("-3d", &ctx()), d("2026-11-18"));
        assert_eq!(due_text("yesterday", &ctx()), d("2026-11-20"));
        assert_eq!(due_text("2026-12-01", &ctx()), d("2026-12-01"));
        assert_eq!(due_text("zítra", &ctx()), d("2026-11-22"));
        assert_eq!(due_text("", &ctx()), None);
        assert_eq!(due_text("next week", &ctx()), d("2026-11-23"));
        assert_eq!(due_text("soonish", &ctx()), None);
    }

    #[test]
    fn trailing_dates_and_priorities_are_read() {
        let p = parse("Draft the changelog for 0.4 tomorrow", &ctx());
        assert_eq!(p.title, "Draft the changelog for 0.4");
        assert_eq!(p.due, d("2026-11-22"));
        assert_eq!(p.priority, None);

        let p = parse("Pay the invoice by fri !!", &ctx());
        assert_eq!(
            (p.title.as_str(), p.due, p.priority),
            ("Pay the invoice", d("2026-11-27"), Some(Priority::Normal))
        );
        let p = parse("Pay the invoice !!! +3d", &ctx());
        assert_eq!((p.due, p.priority), (d("2026-11-24"), Some(Priority::High)));
        assert_eq!(parse("Koupit mléko zítra", &ctx()).due, d("2026-11-22"));
        assert_eq!(parse("Zavolat do banky pátek", &ctx()).due, d("2026-11-27"));
        assert_eq!(
            parse("Book flights 2026-12-01", &ctx()).due,
            d("2026-12-01")
        );
        // Today's own weekday means today.
        assert_eq!(parse("Laundry saturday", &ctx()).due, d("2026-11-21"));
    }

    #[test]
    fn ordinary_prose_is_not_silently_rewritten() {
        for text in [
            "Enjoy the sun",
            "Where the dog sat",
            "People who wed",
            "Talk with Mon",
            "Notes from the call yesterday",
            "Poznámky ze včera",
            "Wow !",
            "Wow!",
            "Wow!!",
            "Report -3d",
            "Report -0d",
            "Report 2026-11-20",
            "Meeting in 2",
            "Read 1984",
            "Version 1.12",
            "Issue 1 12",
            "Next Sunday",
            "Plan next fri",
            "Plan in -2 days",
            "Plan in 1.5 weeks",
            "Plan za dva dny",
            "Plan in 9999999999999999999 weeks",
            "Plan in 2 days time",
            "Plan end month",
            "Plan weekend",
            "Plan 31 Apr",
            "Plan 1.13.",
            "Plan 1. 12",
            "Plan 001 Dec",
            "On Sunday",
            "by fri",
            "!!!",
        ] {
            assert_eq!(
                parse(text, &ctx()),
                Parsed {
                    title: text.into(),
                    ..Parsed::default()
                }
            );
        }
    }

    #[test]
    fn phrases_are_whole_suffixes_in_either_priority_order() {
        for (phrase, date) in [
            ("in 2 days", "2026-11-23"),
            ("ZA 2 DNY", "2026-11-23"),
            ("za 5 dní", "2026-11-26"),
            ("in 3 weeks", "2026-12-12"),
            ("za 3 tydny", "2026-12-12"),
            ("za 5 týdnů", "2026-12-26"),
            ("NEXT WEEK", "2026-11-23"),
            ("pristi tyden", "2026-11-23"),
            ("next Sunday", "2026-11-29"),
            ("příští neděli", "2026-11-29"),
            ("příští středu", "2026-11-25"),
            ("this weekend", "2026-11-21"),
            ("o VÍKENDU", "2026-11-21"),
            ("end of month", "2026-11-30"),
            ("konec mesice", "2026-11-30"),
            ("1 Dec", "2026-12-01"),
            ("BY 1 Dec", "2026-12-01"),
            ("on 1.12.", "2026-12-01"),
            ("by 1. 12.", "2026-12-01"),
            ("do 1. prosince", "2026-12-01"),
            ("by end of month", "2026-11-30"),
            ("on end of month", "2026-11-30"),
            ("do konce měsíce", "2026-11-30"),
            ("1. 12.", "2026-12-01"),
            ("1.12.", "2026-12-01"),
            ("1. prosince", "2026-12-01"),
            ("21 November", "2026-11-21"),
            ("1 Jan", "2027-01-01"),
            ("29. února", "2028-02-29"),
        ] {
            for suffix in [format!("{phrase} !!"), format!("!! {phrase}")] {
                assert_eq!(
                    parse(&format!("Task {suffix}"), &ctx()),
                    Parsed {
                        title: "Task".into(),
                        due: d(date),
                        priority: Some(Priority::Normal),
                        ..Parsed::default()
                    },
                    "{suffix}"
                );
            }
            assert_eq!(parse(phrase, &ctx()).title, phrase);
            assert_eq!(parse(phrase, &ctx()).due, None);
            assert_eq!(due_text(phrase, &ctx()), d(date));
        }
    }

    #[test]
    fn compact_dates_stay_literal_after_number_introducers() {
        for intro in [
            "v", "ve", "at", "VERSION", "verze", "section", "sekce", "chapter", "kapitola",
            "Release", "part", "ČÁST", "cast",
        ] {
            let title = format!("Inspect {intro} 10.10.");
            let p = parse(&format!("{title} !!"), &ctx());
            assert_eq!(
                (p.title, p.due, p.priority),
                (title, None, Some(Priority::Normal))
            );
        }
        assert_eq!(parse("Plan konce měsíce", &ctx()).due, None);
        assert_eq!(parse("Release 1. 12.", &ctx()).due, d("2026-12-01"));
    }

    #[test]
    fn weekends_and_next_week_are_anchored_to_the_local_week() {
        for (today, weekend, next_week) in [
            ("2026-11-22", "2026-11-22", "2026-11-23"),
            ("2026-11-23", "2026-11-28", "2026-11-30"),
        ] {
            let mut ctx = ctx();
            ctx.today = today.parse().unwrap();
            assert_eq!(due_text("this weekend", &ctx), d(weekend));
            assert_eq!(due_text("next Monday", &ctx), d(next_week));
        }
    }

    #[test]
    fn weekday_prefixes_are_part_of_the_suffix() {
        for (text, date) in [
            ("Meeting on sun", "2026-11-22"),
            ("Meeting by sat", "2026-11-21"),
            ("Meeting BY wed", "2026-11-25"),
            ("Meeting ON mon", "2026-11-23"),
            ("Meeting on Sunday", "2026-11-22"),
            ("Meeting by pá", "2026-11-27"),
            ("Meeting neděle", "2026-11-22"),
        ] {
            assert_eq!(
                parse(text, &ctx()),
                Parsed {
                    title: "Meeting".into(),
                    due: d(date),
                    ..Parsed::default()
                }
            );
        }
    }

    #[test]
    fn an_escape_keeps_the_trailing_title_token_literal() {
        for (text, title, due, priority) in [
            (r"Enjoy the \sun", "Enjoy the sun", None, None),
            (r"See you \tomorrow", "See you tomorrow", None, None),
            (r"Meet on \fri", "Meet on fri", None, None),
            (r"Úvaha o \víkendu", "Úvaha o víkendu", None, None),
            (r"Build \1.12.", "Build 1.12.", None, None),
            (r"Wow \!!", "Wow !!", None, None),
            (
                r"Plan next \Monday !!",
                "Plan next Monday",
                None,
                Some(Priority::Normal),
            ),
            (
                r"Review \pátek !!",
                "Review pátek",
                None,
                Some(Priority::Normal),
            ),
            (r"Review \!! +3d", "Review !!", d("2026-11-24"), None),
            (r"Review today \!!", "Review today !!", None, None),
            (r"\today", "today", None, None),
            (r"Leave \this alone", r"Leave \this alone", None, None),
        ] {
            assert_eq!(
                parse(text, &ctx()),
                Parsed {
                    title: title.into(),
                    due,
                    priority,
                    ..Parsed::default()
                }
            );
        }
    }

    const BOTH: ListFeatures = ListFeatures {
        workflow: true,
        timeline: true,
        default_view: todo_core::model::ListView::List,
    };

    #[test]
    fn workflow_suffixes_are_opt_in_escaped_and_work_in_either_language() {
        for (text, first, last) in [
            ("Plan mon..fri", "2026-11-23", "2026-11-27"),
            ("Plán po..pá", "2026-11-23", "2026-11-27"),
            ("Plan 24 Nov..28 Nov", "2026-11-24", "2026-11-28"),
            ("Plan 2026-11-24..2026-11-28", "2026-11-24", "2026-11-28"),
            ("Plán dnes..+4d", "2026-11-21", "2026-11-25"),
            ("Plan fri..mon", "2026-11-27", "2026-11-30"),
            ("Plán 1.12...4.12.", "2026-12-01", "2026-12-04"),
        ] {
            assert_eq!(parse(text, &ctx()).title, text);
            let p = parse_with_features(&format!("{text} stav:rozpracováno !!!"), &ctx(), BOTH);
            assert_eq!(p.status, Some(WorkflowStatus::InProgress));
            assert_eq!(p.start_date, d(first));
            assert_eq!(p.due, d(last));
            assert_eq!(p.priority, Some(Priority::High));
        }
        assert_eq!(parse("Plan status:wip", &ctx()).title, "Plan status:wip");
        for text in [
            "Read status:done",
            "Read status:is",
            "Read status:unknown",
            "Read 24 Nov..23 Nov",
            "Read +3d..today",
            "Read v1-2",
            "status:todo",
        ] {
            let p = parse_with_features(text, &ctx(), BOTH);
            assert_eq!(
                p,
                Parsed {
                    title: text.into(),
                    ..Parsed::default()
                }
            );
        }
        for (text, title) in [
            (r"Plan \status:wip", "Plan status:wip"),
            (r"Plan \today..+4d", "Plan today..+4d"),
        ] {
            assert_eq!(
                parse_with_features(text, &ctx(), BOTH),
                Parsed {
                    title: title.into(),
                    ..Parsed::default()
                }
            );
        }
        let workflow_only = ListFeatures {
            timeline: false,
            ..BOTH
        };
        let p = parse_with_features("Plan 24 Nov..28 Nov status:backlog", &ctx(), workflow_only);
        assert_eq!(p.title, "Plan 24 Nov..28 Nov");
        assert_eq!(p.status, Some(WorkflowStatus::Backlog));
        assert!(p.due.is_none() && p.start_date.is_none());
    }

    #[test]
    fn workflow_draft_removes_only_the_chosen_suffix_and_preserves_the_other_date() {
        let mut draft = Draft {
            text: "Plan status:todo status:wip 24 Nov..28 Nov !!".into(),
            ..Draft::default()
        };
        let ctx = ctx();
        draft.pick(Pick::Status(Some(WorkflowStatus::Backlog)), &ctx, BOTH);
        assert_eq!(draft.text, r"Plan \status:todo 24 Nov..28 Nov !!");
        draft.pick(Pick::Due(None), &ctx, BOTH);
        let parsed = draft.parsed_with_features(&ctx, BOTH);
        assert_eq!(parsed.title, "Plan status:todo");
        assert_eq!(parsed.start_date, d("2026-11-24"));
        assert_eq!(parsed.status, Some(WorkflowStatus::Backlog));
        assert_eq!(parsed.priority, Some(Priority::Normal));
        assert_eq!(parsed.due, None);
        draft.edit_with_features("Plan +5d..+6d status:todo".into(), &ctx, BOTH);
        assert_eq!(draft.status, None);
        assert_eq!(draft.start_date, None);
        draft.pick(Pick::Start(d("2026-12-01")), &ctx, BOTH);
        let p = draft.parsed_with_features(&ctx, BOTH);
        assert_eq!(p.title, "Plan");
        assert_eq!(p.start_date, d("2026-12-01"));
        assert_eq!(p.due, p.start_date);
        assert_eq!(p.status, Some(WorkflowStatus::Todo));
    }

    #[test]
    fn titles_keep_words_that_are_not_trailing_dates() {
        let p = parse("  Plan today's agenda   with Alex ", &ctx());
        assert_eq!(p.title, "Plan today's agenda with Alex");
        assert_eq!(p.due, None);
        // A lone date word is the title.
        assert_eq!(parse("tomorrow", &ctx()).title, "tomorrow");
        assert_eq!(parse("tomorrow", &ctx()).due, None);
        assert_eq!(parse("", &ctx()).title, "");
        // Only one date: the earlier word stays in the title.
        let p = parse("Move from friday to monday", &ctx());
        assert_eq!(p.title, "Move from friday to");
        assert_eq!(p.due, d("2026-11-23"));
    }
}
