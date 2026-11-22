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
//! weekday name and required before an abbreviation. Case is ignored; Czech words also work without accents.
//! Only standalone `!!` (normal) and `!!!` (high) are priority markers; `!` and `Wow!` stay literal.
//! A leading backslash on the final title token stops parsing there and is removed: `Plan \today !!`
//! keeps `Plan today` as the title but still sets priority.

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

/// The date chip shared by the clients: `Sun 22 Nov`, adding the year only outside this year.
#[must_use]
pub fn short_date(date: Date, today: Date) -> String {
    let pattern = if date.year() == today.year() {
        "%a %-d %b"
    } else {
        "%a %-d %b %Y"
    };
    date.strftime(pattern).to_string()
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

fn next_weekday(target: Weekday, ctx: &EvalContext) -> Option<Date> {
    let from = Weekday::of(ctx.today).offset() as i64;
    let ahead = (target.offset() as i64 - from).rem_euclid(7);
    add_days(ctx.today, ahead)
}

fn priority_word(word: &str) -> Option<Priority> {
    match word {
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
    calendar_date(word, ctx).or_else(|| next_weekday(weekday(&fold(word), true)?, ctx))
}

fn date_suffix(words: &[&str], ctx: &EvalContext) -> Option<(Date, usize)> {
    let (last, before) = words.split_last()?;
    let folded = fold(last);
    if let Some(prefix) = before.last()
        && (prefix.eq_ignore_ascii_case("on") || prefix.eq_ignore_ascii_case("by"))
        && let Some(day) = weekday(&folded, true)
    {
        // A prefix and weekday are one suffix; neither is stripped from a title like "On Sunday".
        return if words.len() > 2 {
            next_weekday(day, ctx).map(|date| (date, 2))
        } else {
            None
        };
    }
    let date = if let Some(day) = weekday(&folded, false) {
        next_weekday(day, ctx)?
    } else {
        // Even `-0d` isn't quick-add syntax. The dedicated due-date field still accepts past dates.
        if last.starts_with('-') {
            return None;
        }
        calendar_date(last, ctx)?
    };
    (date >= ctx.today).then_some((date, 1))
}

/// Parses quick-add text. The title is trimmed; it is empty only if the text was.
pub fn parse(text: &str, ctx: &EvalContext) -> Parsed {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    let mut end = words.len();
    let mut due = None;
    let mut priority = None;
    // Up to one date and one priority, in either order, at the end.
    for _ in 0..2 {
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
            end -= 1;
        } else if due.is_none()
            && let Some((d, count)) = date_suffix(&words[..end], ctx)
        {
            due = Some(d);
            end -= count;
        } else {
            break;
        }
    }
    words.truncate(end);
    if let Some(last) = words.last_mut()
        && let Some(literal) = last.strip_prefix('\\').filter(|word| !word.is_empty())
    {
        *last = literal;
    }
    Parsed {
        title: words.join(" "),
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

    #[test]
    fn chip_labels_show_the_weekday_and_disambiguate_other_years() {
        let today = ctx().today;
        assert_eq!(
            short_date("2026-11-22".parse().expect("date"), today),
            "Sun 22 Nov"
        );
        assert_eq!(
            short_date("2027-01-04".parse().expect("date"), today),
            "Mon 4 Jan 2027"
        );
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
            prop_assert!(words.len() - title_words <= 3);
            let _ = due_text(&text, &ctx());
        }

        #[test]
        fn mixed_vocabulary_removes_only_one_suffix_of_each_kind(
            words in prop::collection::vec(prop::sample::select(vec![
                "Enjoy", "the", "Where", "dog", "sat", "sun", "wed", "mon", "fri", "ne", "pá",
                "Wow!", "!", "!!", "!!!", "!!!!", "on", "BY", "today", "tomorrow", "yesterday",
                "dnes", "zítra", "včera", "Monday", "Saturday", "sunday", "úterý", "pátek",
                "+0d", "+3d", "-1d", "-0d", "2026-11-20", "2026-11-21", "2026-12-01",
            ]), 1..16),
        ) {
            let parsed = parse(&words.join(" "), &ctx());
            let title_words = parsed.title.split_whitespace().count();
            prop_assert!(title_words > 0 && title_words <= words.len());
            prop_assert_eq!(parsed.title, words[..title_words].join(" "));
            let mut suffix = &words[title_words..];
            let mut priorities = Vec::new();
            let mut dates = Vec::new();
            while let [word, rest @ ..] = suffix {
                suffix = rest;
                match *word {
                    "!!" => priorities.push(Priority::Normal),
                    "!!!" => priorities.push(Priority::High),
                    other => {
                        let date = if other.eq_ignore_ascii_case("on") || other.eq_ignore_ascii_case("by") {
                            let Some((day, remaining)) = rest.split_first() else {
                                prop_assert!(false, "prefix removed without weekday");
                                break;
                            };
                            suffix = remaining;
                            prop_assert!(["sat", "sun", "wed", "mon", "fri", "ne", "pá", "Monday",
                                "Saturday", "sunday", "úterý", "pátek"].contains(day));
                            due_text(day, &ctx())
                        } else {
                            prop_assert!(["today", "tomorrow", "dnes", "zítra", "Monday", "Saturday",
                                "sunday", "úterý", "pátek", "+0d", "+3d", "2026-11-21", "2026-12-01"]
                                .contains(&other));
                            due_text(other, &ctx())
                        };
                        prop_assert!(date.is_some_and(|date| date >= ctx().today));
                        dates.extend(date);
                    }
                }
            }
            prop_assert_eq!(priorities.as_slice(), parsed.priority.as_slice());
            prop_assert_eq!(dates.as_slice(), parsed.due.as_slice());
        }

        #[test]
        fn arbitrary_title_accepts_trailing_date_and_priority(text in any::<String>(), reverse in any::<bool>()) {
            let title = format!("Task {text}");
            let normalized = literal_title(&title.split_whitespace().collect::<Vec<_>>());
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
        assert_eq!(due_text("-3d", &ctx()), d("2026-11-18"));
        assert_eq!(due_text("yesterday", &ctx()), d("2026-11-20"));
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
                    priority: None,
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
            (r"Wow \!!", "Wow !!", None, None),
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
                    priority
                }
            );
        }
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
