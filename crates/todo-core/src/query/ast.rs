//! Query AST and its canonical text form.

use crate::actor::AgentId;
use crate::model::Priority;
use crate::time::Date;
use std::fmt::{self, Write as _};

/// Comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    /// `=` (written `:` in canonical form)
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

impl CmpOp {
    /// Applies the operator to an ordering.
    #[must_use]
    pub fn test(self, ord: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Eq => ord == Equal,
            Self::Ne => ord != Equal,
            Self::Lt => ord == Less,
            Self::Le => ord != Greater,
            Self::Gt => ord == Greater,
            Self::Ge => ord != Less,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Eq => ":",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A date in a query, absolute or relative to today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateExpr {
    /// Today (owner's local date).
    Today,
    /// Tomorrow.
    Tomorrow,
    /// Yesterday.
    Yesterday,
    /// Today plus or minus whole days, written `+3d` / `-7d`.
    Offset(i32),
    /// A calendar date, `YYYY-MM-DD`.
    Absolute(Date),
}

impl fmt::Display for DateExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Today => f.write_str("today"),
            Self::Tomorrow => f.write_str("tomorrow"),
            Self::Yesterday => f.write_str("yesterday"),
            Self::Offset(n) => write!(f, "{n:+}d"),
            Self::Absolute(d) => write!(f, "{:04}-{:02}-{:02}", d.year(), d.month(), d.day()),
        }
    }
}

/// A date-valued task field that can be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    /// Due date.
    Due,
    /// Creation date.
    Created,
    /// Last update date.
    Updated,
    /// Completion date.
    Completed,
}

impl DateField {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Due => "due",
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Completed => "completed",
        }
    }
}

/// State of the due date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueState {
    /// Due before today and not done.
    Overdue,
    /// No due date.
    None,
    /// Has a due date.
    Any,
}

/// Who is meant by `created_by:` / `updated_by:`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActorSpec {
    /// The owner.
    Owner,
    /// Any agent (`agent`).
    AnyAgent,
    /// One specific agent. IDs equal to a reserved word cannot be addressed this way.
    Agent(AgentId),
    /// The importer.
    Importer,
    /// Email ingest (`email`).
    EmailIngest,
    /// The system.
    System,
}

/// An atomic condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Predicate {
    /// `list:name` (name or ID, diacritic-insensitive).
    List(String),
    /// `collection:name`.
    Collection(String),
    /// `due:overdue`, `due:none`, `due:any`.
    DueState(DueState),
    /// `due<=today`, `created>=-7d`, ...
    Date {
        /// The compared field.
        field: DateField,
        /// The operator.
        op: CmpOp,
        /// The date to compare with.
        value: DateExpr,
    },
    /// `my_day`: in My Day for today.
    MyDay,
    /// `priority>=high`.
    Priority {
        /// The operator.
        op: CmpOp,
        /// The level to compare with.
        level: Priority,
    },
    /// `tag:name`.
    Tag(String),
    /// `done`: completed.
    Done,
    /// `created_by:agent`.
    CreatedBy(ActorSpec),
    /// `updated_by:agent`.
    UpdatedBy(ActorSpec),
    /// Free text, matched against title, notes and step titles.
    Text(String),
}

/// A boolean expression over predicates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// Matches everything (an empty filter).
    True,
    /// All must match (at least two operands when produced by the parser).
    And(Vec<Expr>),
    /// Any must match (at least two operands when produced by the parser).
    Or(Vec<Expr>),
    /// Negation.
    Not(Box<Expr>),
    /// A condition.
    Pred(Predicate),
}

/// Sortable task attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Due date (tasks without one last).
    Due,
    /// Priority.
    Priority,
    /// Creation time.
    Created,
    /// Last update time.
    Updated,
    /// Completion time.
    Completed,
    /// Title, diacritic-insensitive.
    Title,
    /// Manual order: list order, then position in list.
    Manual,
}

impl SortKey {
    pub(crate) const ALL: [(&'static str, SortKey); 7] = [
        ("due", SortKey::Due),
        ("priority", SortKey::Priority),
        ("created", SortKey::Created),
        ("updated", SortKey::Updated),
        ("completed", SortKey::Completed),
        ("title", SortKey::Title),
        ("manual", SortKey::Manual),
    ];

    fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, k)| *k == self)
            .map_or("manual", |(n, _)| n)
    }
}

/// One `sort:` clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    /// What to sort by.
    pub key: SortKey,
    /// Descending order (`sort:-due`).
    pub descending: bool,
}

/// A parsed smart-list query: a filter and a sort order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The filter.
    pub expr: Expr,
    /// Sort clauses, most significant first. Empty means manual order.
    pub sort: Vec<Sort>,
}

const RESERVED: [&str; 5] = ["and", "or", "not", "done", "my_day"];

fn push_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

fn push_value(out: &mut String, s: &str) {
    let bare = !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '"' | '\\'));
    if bare {
        out.push_str(s);
    } else {
        push_quoted(out, s);
    }
}

fn push_text(out: &mut String, s: &str) {
    let bare = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && !s.starts_with('-')
        && !RESERVED.contains(&s.to_ascii_lowercase().as_str());
    if bare {
        out.push_str(s);
    } else {
        push_quoted(out, s);
    }
}

pub(crate) fn actor_spec_name(spec: &ActorSpec) -> String {
    match spec {
        ActorSpec::Owner => "owner".into(),
        ActorSpec::AnyAgent => "agent".into(),
        ActorSpec::Agent(a) => a.as_str().to_owned(),
        ActorSpec::Importer => "importer".into(),
        ActorSpec::EmailIngest => "email".into(),
        ActorSpec::System => "system".into(),
    }
}

impl Predicate {
    fn write(&self, out: &mut String) {
        match self {
            Self::List(v) => {
                out.push_str("list:");
                push_value(out, v);
            }
            Self::Collection(v) => {
                out.push_str("collection:");
                push_value(out, v);
            }
            Self::Tag(v) => {
                out.push_str("tag:");
                push_value(out, v);
            }
            Self::DueState(s) => out.push_str(match s {
                DueState::Overdue => "due:overdue",
                DueState::None => "due:none",
                DueState::Any => "due:any",
            }),
            Self::Date { field, op, value } => {
                let _ = write!(out, "{}{}{value}", field.name(), op.symbol());
            }
            Self::MyDay => out.push_str("my_day"),
            Self::Priority { op, level } => {
                let _ = write!(out, "priority{}{}", op.symbol(), level.name());
            }
            Self::Done => out.push_str("done"),
            Self::CreatedBy(s) => {
                let _ = write!(out, "created_by:{}", actor_spec_name(s));
            }
            Self::UpdatedBy(s) => {
                let _ = write!(out, "updated_by:{}", actor_spec_name(s));
            }
            Self::Text(t) => push_text(out, t),
        }
    }
}

impl Expr {
    /// Writes `self`; `ctx` is the parent: 0 top level, 1 inside `or`, 2 inside `and`, 3 inside `not`.
    fn write(&self, out: &mut String, ctx: u8) {
        match self {
            Self::True => {}
            Self::Pred(p) => p.write(out),
            Self::Not(e) => {
                out.push_str("not ");
                e.write(out, 3);
            }
            Self::And(items) => {
                let paren = ctx >= 2;
                if paren {
                    out.push('(');
                }
                for (i, e) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(" and ");
                    }
                    e.write(out, 2);
                }
                if paren {
                    out.push(')');
                }
            }
            Self::Or(items) => {
                let paren = ctx >= 1;
                if paren {
                    out.push('(');
                }
                for (i, e) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(" or ");
                    }
                    e.write(out, 1);
                }
                if paren {
                    out.push(')');
                }
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = String::new();
        self.write(&mut s, 0);
        f.write_str(&s)
    }
}

impl fmt::Display for Query {
    /// The canonical text form; parsing it yields an equal [`Query`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = self.expr.to_string();
        for sort in &self.sort {
            if !s.is_empty() {
                s.push(' ');
            }
            let _ = write!(
                s,
                "sort:{}{}",
                if sort.descending { "-" } else { "" },
                sort.key.name()
            );
        }
        f.write_str(&s)
    }
}
