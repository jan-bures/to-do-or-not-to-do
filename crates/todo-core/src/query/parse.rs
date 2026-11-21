//! Query lexer and parser.
//!
//! Grammar (keywords are case-insensitive; field names are lowercase):
//!
//! ```text
//! query   := sort* [ or sort* ]
//! or      := and ( "or" and )*
//! and     := unary ( [ "and" ] unary | sort )*      -- juxtaposition means "and"
//! unary   := ( "not" | "!" ) unary | "(" or ")" | atom
//! atom    := "done" | "my_day" | field op value | word | quoted
//! field   := list | collection | tag | due | created | updated | completed
//!          | priority | created_by | updated_by
//! op      := ":" | "=" | "!=" | "<" | "<=" | ">" | ">="
//! sort    := "sort:" [ "-" ] ( due | priority | created | updated | completed | title | manual )
//! ```
//!
//! A value runs to the next whitespace or parenthesis and may contain `"quoted parts"` with `\"` and `\\`
//! escapes. `sort:` clauses are only allowed at the top level, not inside parentheses.

use super::ast::{
    ActorSpec, CmpOp, DateExpr, DateField, DueState, Expr, Predicate, Query, Sort, SortKey,
};
use crate::actor::AgentId;
use crate::model::Priority;
use crate::time::Date;

const MAX_DEPTH: usize = 64;

/// What went wrong while parsing a query.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseErrorKind {
    /// The query ended where more input was expected.
    #[error("unexpected end of query")]
    UnexpectedEnd,
    /// A token appeared where it is not allowed.
    #[error("unexpected `{0}`")]
    UnexpectedToken(String),
    /// A `)` without a matching `(`.
    #[error("unmatched `)`")]
    UnmatchedParen,
    /// A `(` that is never closed.
    #[error("missing `)` for this `(`")]
    UnclosedParen,
    /// A `"` that is never closed.
    #[error("unterminated quote")]
    UnterminatedQuote,
    /// The field name is not known.
    #[error("unknown field `{0}`")]
    UnknownField(String),
    /// The field has no value.
    #[error("field `{0}` needs a value")]
    MissingValue(String),
    /// The value is not valid for the field.
    #[error("invalid value `{got}` for `{field}`: expected {expected}")]
    InvalidValue {
        /// The field.
        field: String,
        /// What is accepted.
        expected: &'static str,
        /// What was found.
        got: String,
    },
    /// The operator is not valid for the field.
    #[error("operator `{op}` cannot be used with `{field}`")]
    InvalidOperator {
        /// The field.
        field: String,
        /// The operator.
        op: String,
    },
    /// `sort:` appeared inside parentheses or after `not`/`or`/`and`.
    #[error("`sort:` is only allowed at the top level")]
    SortNotAllowed,
    /// Parentheses or `not` nested too deeply.
    #[error("query is nested too deeply")]
    TooDeep,
}

/// A parse error with the byte position where it was detected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind} (at byte {pos})")]
pub struct ParseError {
    /// Byte offset into the query text.
    pub pos: usize,
    /// Length in bytes of the offending text (0 at the end of input).
    pub len: usize,
    /// What went wrong.
    pub kind: ParseErrorKind,
}

fn err<T>(pos: usize, len: usize, kind: ParseErrorKind) -> Result<T, ParseError> {
    Err(ParseError { pos, len, kind })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    Bang,
    And,
    Or,
    Not,
    Word {
        text: String,
        quoted: bool,
    },
    Field {
        name: String,
        op: CmpOp,
        op_text: String,
        value: String,
        value_pos: usize,
    },
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    pos: usize,
    end: usize,
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    i: usize,
}

impl Lexer<'_> {
    fn peek_at(&self, i: usize) -> Option<char> {
        self.chars.get(i).map(|c| c.1)
    }

    fn pos_at(&self, i: usize) -> usize {
        self.chars.get(i).map_or(self.src.len(), |c| c.0)
    }

    /// Reads a value: text up to whitespace or a parenthesis, with quoted parts. Returns (text, quoted).
    fn run(&mut self) -> Result<(String, bool), ParseError> {
        let mut s = String::new();
        let mut quoted = false;
        while let Some(c) = self.peek_at(self.i) {
            if c == '"' {
                quoted = true;
                let qpos = self.pos_at(self.i);
                self.i += 1;
                loop {
                    match self.peek_at(self.i) {
                        None => return err(qpos, 1, ParseErrorKind::UnterminatedQuote),
                        Some('\\') => {
                            self.i += 1;
                            match self.peek_at(self.i) {
                                Some(n) => {
                                    s.push(n);
                                    self.i += 1;
                                }
                                None => return err(qpos, 1, ParseErrorKind::UnterminatedQuote),
                            }
                        }
                        Some('"') => {
                            self.i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(ch);
                            self.i += 1;
                        }
                    }
                }
            } else if c.is_whitespace() || c == '(' || c == ')' {
                break;
            } else {
                s.push(c);
                self.i += 1;
            }
        }
        Ok((s, quoted))
    }

    /// Recognises `ident` followed by an operator; returns (name, op, op text, index after operator).
    fn field_prefix(&self) -> Option<(String, CmpOp, String, usize)> {
        let start = self.i;
        let first = self.peek_at(start)?;
        if !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
        let mut j = start;
        while self
            .peek_at(j)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            j += 1;
        }
        let name: String = self.chars[start..j].iter().map(|c| c.1).collect();
        let (op, text, len) = match (self.peek_at(j)?, self.peek_at(j + 1)) {
            (':', _) => (CmpOp::Eq, ":", 1),
            ('=', _) => (CmpOp::Eq, "=", 1),
            ('!', Some('=')) => (CmpOp::Ne, "!=", 2),
            ('<', Some('=')) => (CmpOp::Le, "<=", 2),
            ('<', _) => (CmpOp::Lt, "<", 1),
            ('>', Some('=')) => (CmpOp::Ge, ">=", 2),
            ('>', _) => (CmpOp::Gt, ">", 1),
            _ => return None,
        };
        Some((name.to_ascii_lowercase(), op, text.to_owned(), j + len))
    }

    fn lex(mut self) -> Result<Vec<Token>, ParseError> {
        let mut out = Vec::new();
        while let Some(c) = self.peek_at(self.i) {
            let pos = self.pos_at(self.i);
            if c.is_whitespace() {
                self.i += 1;
                continue;
            }
            let tok = match c {
                '(' => {
                    self.i += 1;
                    Tok::LParen
                }
                ')' => {
                    self.i += 1;
                    Tok::RParen
                }
                '!' if self.peek_at(self.i + 1) != Some('=') => {
                    self.i += 1;
                    Tok::Bang
                }
                _ => {
                    if let Some((name, op, op_text, after)) = self.field_prefix() {
                        self.i = after;
                        let value_pos = self.pos_at(self.i);
                        let (value, quoted) = self.run()?;
                        if value.is_empty() && !quoted {
                            return err(
                                pos,
                                self.pos_at(after) - pos,
                                ParseErrorKind::MissingValue(name),
                            );
                        }
                        Tok::Field {
                            name,
                            op,
                            op_text,
                            value,
                            value_pos,
                        }
                    } else {
                        let (text, quoted) = self.run()?;
                        let lower = text.to_ascii_lowercase();
                        match (quoted, lower.as_str()) {
                            (false, "and") => Tok::And,
                            (false, "or") => Tok::Or,
                            (false, "not") => Tok::Not,
                            _ => Tok::Word { text, quoted },
                        }
                    }
                }
            };
            out.push(Token {
                tok,
                pos,
                end: self.pos_at(self.i),
            });
        }
        Ok(out)
    }
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    src_len: usize,
    sorts: Vec<Sort>,
    paren_depth: usize,
    depth: usize,
}

fn invalid<T>(
    pos: usize,
    len: usize,
    field: &str,
    expected: &'static str,
    got: &str,
) -> Result<T, ParseError> {
    err(
        pos,
        len,
        ParseErrorKind::InvalidValue {
            field: field.to_owned(),
            expected,
            got: got.to_owned(),
        },
    )
}

/// Parses a date as the query language writes it: `today`, `tomorrow`, `yesterday`, `+Nd`, `-Nd` or
/// `YYYY-MM-DD` (case-insensitive). Clients use it for typed dates too, so both read dates alike.
#[must_use]
pub fn parse_date(value: &str) -> Option<DateExpr> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "today" => return Some(DateExpr::Today),
        "tomorrow" => return Some(DateExpr::Tomorrow),
        "yesterday" => return Some(DateExpr::Yesterday),
        _ => {}
    }
    if (lower.starts_with('+') || lower.starts_with('-')) && lower.ends_with('d') && lower.len() > 2
    {
        let digits = &lower[1..lower.len() - 1];
        if digits.bytes().all(|b| b.is_ascii_digit()) {
            let n: i32 = digits.parse().ok()?;
            return Some(DateExpr::Offset(if lower.starts_with('-') {
                -n
            } else {
                n
            }));
        }
        return None;
    }
    if value.len() == 10 {
        return value.parse::<Date>().ok().map(DateExpr::Absolute);
    }
    None
}

const DATE_EXPECTED: &str = "today, tomorrow, yesterday, +Nd, -Nd or YYYY-MM-DD";

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.i)
    }

    fn end_pos(&self) -> usize {
        self.src_len
    }

    fn skip_sorts(&mut self) -> Result<(), ParseError> {
        while self.paren_depth == 0 {
            let Some(Token {
                tok:
                    Tok::Field {
                        name,
                        op,
                        value,
                        value_pos,
                        ..
                    },
                pos,
                end,
            }) = self.peek().cloned()
            else {
                break;
            };
            if name != "sort" {
                break;
            }
            if op != CmpOp::Eq {
                return err(
                    pos,
                    end - pos,
                    ParseErrorKind::InvalidOperator {
                        field: name,
                        op: "non-equality".into(),
                    },
                );
            }
            let (desc, key) = match value.strip_prefix('-') {
                Some(k) => (true, k),
                None => (false, value.as_str()),
            };
            let Some((_, key)) = SortKey::ALL
                .iter()
                .find(|(n, _)| *n == key.to_ascii_lowercase())
            else {
                return invalid(
                    value_pos,
                    value.len(),
                    "sort",
                    "due, priority, created, updated, completed, title or manual (optionally prefixed by -)",
                    &value,
                );
            };
            // A repeated key can never break a tie the first one left, so only the first counts.
            if !self.sorts.iter().any(|s| s.key == *key) {
                self.sorts.push(Sort {
                    key: *key,
                    descending: desc,
                });
            }
            self.i += 1;
        }
        Ok(())
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let pos = self.peek().map_or(self.end_pos(), |t| t.pos);
            return err(pos, 0, ParseErrorKind::TooDeep);
        }
        let mut items = vec![self.parse_and()?];
        while matches!(self.peek(), Some(Token { tok: Tok::Or, .. })) {
            self.i += 1;
            items.push(self.parse_and()?);
        }
        self.depth -= 1;
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::Or(items)
        })
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        self.skip_sorts()?;
        let mut items = vec![self.parse_unary()?];
        loop {
            self.skip_sorts()?;
            match self.peek().map(|t| &t.tok) {
                None | Some(Tok::Or | Tok::RParen) => break,
                Some(Tok::And) => {
                    self.i += 1;
                    items.push(self.parse_unary()?);
                }
                Some(_) => items.push(self.parse_unary()?),
            }
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::And(items)
        })
    }

    fn describe(&self, t: &Token) -> String {
        match &t.tok {
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Bang => "!".into(),
            Tok::And => "and".into(),
            Tok::Or => "or".into(),
            Tok::Not => "not".into(),
            Tok::Word { text, .. } => text.clone(),
            Tok::Field { name, .. } => name.clone(),
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let Some(token) = self.peek().cloned() else {
            return err(self.end_pos(), 0, ParseErrorKind::UnexpectedEnd);
        };
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err(token.pos, token.end - token.pos, ParseErrorKind::TooDeep);
        }
        let result = match &token.tok {
            Tok::Not | Tok::Bang => {
                self.i += 1;
                Expr::Not(Box::new(self.parse_unary()?))
            }
            Tok::LParen => {
                self.i += 1;
                self.paren_depth += 1;
                let inner = self.parse_or()?;
                self.paren_depth -= 1;
                match self.peek() {
                    Some(Token {
                        tok: Tok::RParen, ..
                    }) => self.i += 1,
                    _ => return err(token.pos, 1, ParseErrorKind::UnclosedParen),
                }
                inner
            }
            Tok::RParen => return err(token.pos, 1, ParseErrorKind::UnmatchedParen),
            Tok::And | Tok::Or => {
                let d = self.describe(&token);
                return err(
                    token.pos,
                    token.end - token.pos,
                    ParseErrorKind::UnexpectedToken(d),
                );
            }
            Tok::Word { text, quoted } => {
                self.i += 1;
                if text.is_empty() {
                    return invalid(
                        token.pos,
                        token.end - token.pos,
                        "text",
                        "a non-empty word",
                        "",
                    );
                }
                match (quoted, text.to_ascii_lowercase().as_str()) {
                    (false, "done") => Expr::Pred(Predicate::Done),
                    (false, "my_day") => Expr::Pred(Predicate::MyDay),
                    _ => Expr::Pred(Predicate::Text(text.clone())),
                }
            }
            Tok::Field {
                name,
                op,
                op_text,
                value,
                value_pos,
            } => {
                self.i += 1;
                Expr::Pred(self.field(&token, name, *op, op_text, value, *value_pos)?)
            }
        };
        self.depth -= 1;
        Ok(result)
    }

    fn field(
        &self,
        token: &Token,
        name: &str,
        op: CmpOp,
        op_text: &str,
        value: &str,
        vpos: usize,
    ) -> Result<Predicate, ParseError> {
        let tlen = token.end - token.pos;
        let vlen = token.end.saturating_sub(vpos);
        let only_eq = |p: Predicate| {
            if op == CmpOp::Eq {
                Ok(p)
            } else {
                err(
                    token.pos,
                    tlen,
                    ParseErrorKind::InvalidOperator {
                        field: name.to_owned(),
                        op: op_text.to_owned(),
                    },
                )
            }
        };
        let date_field = |field: DateField| match parse_date(value) {
            Some(d) => Ok(Predicate::Date {
                field,
                op,
                value: d,
            }),
            None => invalid(vpos, vlen, name, DATE_EXPECTED, value),
        };
        match name {
            "list" => only_eq(Predicate::List(value.to_owned())),
            "collection" => only_eq(Predicate::Collection(value.to_owned())),
            "tag" => only_eq(Predicate::Tag(value.to_owned())),
            "due" => {
                let state = match value.to_ascii_lowercase().as_str() {
                    "overdue" => Some(DueState::Overdue),
                    "none" => Some(DueState::None),
                    "any" => Some(DueState::Any),
                    _ => None,
                };
                match state {
                    Some(s) => only_eq(Predicate::DueState(s)),
                    None => date_field(DateField::Due),
                }
            }
            "created" => date_field(DateField::Created),
            "updated" => date_field(DateField::Updated),
            "completed" => date_field(DateField::Completed),
            "priority" => match Priority::from_name(&value.to_ascii_lowercase()) {
                Some(level) => Ok(Predicate::Priority { op, level }),
                None => invalid(vpos, vlen, name, "none, low, normal or high", value),
            },
            "created_by" | "updated_by" => {
                let spec = match value.to_ascii_lowercase().as_str() {
                    "owner" => Some(ActorSpec::Owner),
                    "agent" => Some(ActorSpec::AnyAgent),
                    "importer" => Some(ActorSpec::Importer),
                    "email" => Some(ActorSpec::EmailIngest),
                    "system" => Some(ActorSpec::System),
                    _ => AgentId::new(value).ok().map(ActorSpec::Agent),
                };
                match spec {
                    Some(s) => only_eq(if name == "created_by" {
                        Predicate::CreatedBy(s)
                    } else {
                        Predicate::UpdatedBy(s)
                    }),
                    None => invalid(
                        vpos,
                        vlen,
                        name,
                        "owner, agent, importer, email, system or an agent id",
                        value,
                    ),
                }
            }
            "sort" => err(token.pos, tlen, ParseErrorKind::SortNotAllowed),
            other => err(
                token.pos,
                other.len(),
                ParseErrorKind::UnknownField(other.to_owned()),
            ),
        }
    }
}

/// Parses query text.
pub fn parse(src: &str) -> Result<Query, ParseError> {
    let lexer = Lexer {
        src,
        chars: src.char_indices().collect(),
        i: 0,
    };
    let toks = lexer.lex()?;
    let mut p = Parser {
        toks,
        i: 0,
        src_len: src.len(),
        sorts: Vec::new(),
        paren_depth: 0,
        depth: 0,
    };
    p.skip_sorts()?;
    let expr = if p.peek().is_none() {
        Expr::True
    } else {
        p.parse_or()?
    };
    if let Some(t) = p.peek() {
        return match t.tok {
            Tok::RParen => err(t.pos, 1, ParseErrorKind::UnmatchedParen),
            _ => {
                let d = p.describe(t);
                err(t.pos, t.end - t.pos, ParseErrorKind::UnexpectedToken(d))
            }
        };
    }
    Ok(Query {
        expr,
        sort: p.sorts,
    })
}
