//! Quick-add text: a title, then optional trailing words for the due date and the priority.
//!
//! ```text
//! Draft the changelog tomorrow        → due tomorrow
//! Pay the invoice fri !!              → due on the coming Friday, priority normal
//! Koupit mléko zítra                  → due tomorrow
//! Book flights 2026-12-01             → due 1 Dec 2026
//! ```
//!
//! Dates are what the query language reads (`today`, `tomorrow`, `+3d`, `YYYY-MM-DD`, via
//! `todo_core::query::parse_date`), plus weekday names in English and Czech and `dnes` / `zítra`. Only
//! trailing words count, so a title can still say "today" in the middle. A title made only of date words
//! keeps them as its title.

use todo_core::model::Priority;
use todo_core::query::{EvalContext, fold, parse_date};
use todo_core::time::{Date, Weekday, add_days};

/// What the quick-add text says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub title: String,
    pub due: Option<Date>,
    pub priority: Option<Priority>,
}

fn weekday(word: &str) -> Option<Weekday> {
    const NAMES: [(&[&str], Weekday); 7] = [
        (&["mon", "monday", "pondeli"], Weekday::Mon),
        (&["tue", "tues", "tuesday", "utery"], Weekday::Tue),
        (&["wed", "wednesday", "streda"], Weekday::Wed),
        (
            &["thu", "thur", "thurs", "thursday", "ctvrtek"],
            Weekday::Thu,
        ),
        (&["fri", "friday", "patek"], Weekday::Fri),
        (&["sat", "saturday", "sobota"], Weekday::Sat),
        (&["sun", "sunday", "nedele"], Weekday::Sun),
    ];
    let w = fold(word);
    NAMES
        .iter()
        .find(|(names, _)| names.contains(&w.as_str()))
        .map(|(_, d)| *d)
}

/// A date word, resolved against `ctx` (a weekday is the next one, today included).
fn date_word(word: &str, ctx: &EvalContext) -> Option<Date> {
    match fold(word).as_str() {
        "dnes" => return Some(ctx.today),
        "zitra" => return add_days(ctx.today, 1),
        _ => {}
    }
    if let Some(d) = parse_date(word) {
        return ctx.resolve(d);
    }
    let target = weekday(word)?;
    let from = Weekday::of(ctx.today).offset() as i64;
    let ahead = (target.offset() as i64 - from).rem_euclid(7);
    add_days(ctx.today, ahead)
}

fn priority_word(word: &str) -> Option<Priority> {
    match word {
        "!" => Some(Priority::Low),
        "!!" => Some(Priority::Normal),
        "!!!" => Some(Priority::High),
        _ => None,
    }
}

/// A due date typed on its own ("tomorrow", "fri", "+3d", "2026-12-01", "zítra"). `None` when it isn't
/// one.
pub fn due_text(text: &str, ctx: &EvalContext) -> Option<Date> {
    let word = text.trim();
    if word.is_empty() || word.contains(char::is_whitespace) {
        return None;
    }
    date_word(&word.to_lowercase(), ctx)
}

/// Parses quick-add text. The title is trimmed; it is empty only if the text was.
pub fn parse(text: &str, ctx: &EvalContext) -> Parsed {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut end = words.len();
    let mut due = None;
    let mut priority = None;
    // Up to one date and one priority, in either order, at the end.
    for _ in 0..2 {
        let Some(last) = end.checked_sub(1).map(|i| words[i]) else {
            break;
        };
        if end <= 1 {
            break; // Keep at least one word of title.
        }
        if priority.is_none()
            && let Some(p) = priority_word(last)
        {
            priority = Some(p);
            end -= 1;
        } else if due.is_none()
            && let Some(d) = date_word(last, ctx)
        {
            due = Some(d);
            end -= 1;
        } else {
            break;
        }
    }
    Parsed {
        title: words[..end].join(" "),
        due,
        priority,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ctx() -> EvalContext {
        // Saturday 21 Nov 2026.
        EvalContext::utc("2026-11-21T10:35:00Z".parse().expect("timestamp"))
    }

    proptest! {
        #[test]
        fn arbitrary_text_keeps_a_normalized_title(text in any::<String>()) {
            let parsed = parse(&text, &ctx());
            let words = text.split_whitespace().collect::<Vec<_>>();
            let title_words = parsed.title.split_whitespace().count();
            let suffixes = usize::from(parsed.due.is_some()) + usize::from(parsed.priority.is_some());
            prop_assert_eq!(parsed.title.is_empty(), words.is_empty());
            prop_assert_eq!(title_words + suffixes, words.len());
            prop_assert_eq!(parsed.title, words[..title_words].join(" "));
            let _ = due_text(&text, &ctx());
        }

        #[test]
        fn arbitrary_title_accepts_trailing_date_and_priority(text in any::<String>(), reverse in any::<bool>()) {
            let title = format!("Task {text}");
            let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
            let suffix = if reverse { "!! zítra" } else { "zítra !!" };
            prop_assert_eq!(parse(&format!("{title} {suffix}"), &ctx()), Parsed {
                title: normalized,
                due: d("2026-11-22"),
                priority: Some(Priority::Normal),
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
        assert_eq!(due_text("2026-12-01", &ctx()), d("2026-12-01"));
        assert_eq!(due_text("zítra", &ctx()), d("2026-11-22"));
        assert_eq!(due_text("", &ctx()), None);
        assert_eq!(due_text("next week", &ctx()), None);
        assert_eq!(due_text("soonish", &ctx()), None);
    }

    #[test]
    fn trailing_dates_and_priorities_are_read() {
        let p = parse("Draft the changelog for 0.4 tomorrow", &ctx());
        assert_eq!(p.title, "Draft the changelog for 0.4");
        assert_eq!(p.due, d("2026-11-22"));
        assert_eq!(p.priority, None);

        let p = parse("Pay the invoice fri !!", &ctx());
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
