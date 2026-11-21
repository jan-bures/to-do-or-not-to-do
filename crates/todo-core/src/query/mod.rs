//! The smart-list query language: a small text DSL, its AST, parser, printer and evaluator.
//!
//! ```text
//! list:shopping and not done sort:due
//! (due<=today or priority>=high) tag:work milk
//! created_by:agent updated>=-7d
//! ```
//!
//! Text outside of `field:value` form is a free-text word, matched case- and diacritic-insensitively
//! against title, notes and step titles (`mleko` matches `Mléko`). See [`parse`] for the grammar.
//! Parsing a [`Query`] printed with `to_string()` yields an equal query.

mod ast;
mod eval;
mod fold;
pub mod parse;

pub use ast::{
    ActorSpec, CmpOp, DateExpr, DateField, DueState, Expr, Predicate, Query, Sort, SortKey,
};
pub use eval::{EvalContext, KeyPart, KeyValue, Matcher, OrderKey, Stop};
pub use fold::fold;
pub use parse::{ParseError, ParseErrorKind, parse_date};

use std::str::FromStr;

impl FromStr for Query {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse::parse(s)
    }
}

impl Query {
    /// Parses query text. Equivalent to `s.parse()`.
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        parse::parse(s)
    }
}

/// A built-in smart list, defined as a saved query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinList {
    /// Stable identifier.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// The query text.
    pub query: &'static str,
}

/// The built-in smart lists.
pub const BUILTIN_LISTS: [BuiltinList; 6] = [
    BuiltinList {
        id: "my_day",
        name: "My Day",
        query: "my_day and not done",
    },
    BuiltinList {
        id: "important",
        name: "Important",
        query: "priority>=high and not done sort:due",
    },
    BuiltinList {
        id: "planned",
        name: "Planned",
        query: "due:any and not done sort:due",
    },
    BuiltinList {
        id: "all",
        name: "All",
        query: "not done",
    },
    BuiltinList {
        id: "completed",
        name: "Completed",
        query: "done sort:-completed",
    },
    BuiltinList {
        id: "agent_activity",
        name: "Agent activity (7 days)",
        query: "updated_by:agent and updated>=-7d sort:-updated",
    },
];

impl BuiltinList {
    /// Parses the built-in query.
    pub fn parse(&self) -> Result<Query, ParseError> {
        Query::parse(self.query)
    }
}

#[cfg(test)]
mod date_tests {
    use super::{DateExpr, EvalContext, parse_date};
    use crate::time::{Date, Timestamp};

    #[test]
    fn typed_dates_parse_and_resolve_like_query_dates() {
        let today: Date = "2026-11-21".parse().unwrap();
        let now: Timestamp = "2026-11-21T09:35:00Z".parse().unwrap();
        let ctx = EvalContext::new(now, today, 3600);
        let resolve = |s: &str| parse_date(s).and_then(|d| ctx.resolve(d));
        assert_eq!(parse_date("Tomorrow"), Some(DateExpr::Tomorrow));
        assert_eq!(resolve("tomorrow"), Some("2026-11-22".parse().unwrap()));
        assert_eq!(resolve("+10d"), Some("2026-12-01".parse().unwrap()));
        assert_eq!(resolve("-1d"), Some("2026-11-20".parse().unwrap()));
        assert_eq!(resolve("2027-01-05"), Some("2027-01-05".parse().unwrap()));
        assert_eq!(parse_date("someday"), None);
        assert_eq!(parse_date("+d"), None);
    }
}
