use crate::read::{MAX_QUERY_BYTES, MAX_QUERY_TERMS, QueryError};
use todo_core::query::fold;

pub(super) struct TitleSearch(Vec<Vec<char>>);

impl TitleSearch {
    pub(super) fn new(text: &str) -> Result<Self, QueryError> {
        if text.len() > MAX_QUERY_BYTES {
            return Err(QueryError::TooComplex {
                limit: "bytes",
                value: text.len(),
                max: MAX_QUERY_BYTES,
            });
        }
        let words = words(text);
        if words.len() > MAX_QUERY_TERMS {
            return Err(QueryError::TooComplex {
                limit: "terms",
                value: words.len(),
                max: MAX_QUERY_TERMS,
            });
        }
        Ok(Self(words))
    }

    /// Lower scores are better. Each search word must match somewhere in the title.
    pub(super) fn score(&self, title: &str) -> Option<i64> {
        if self.0.is_empty() {
            return Some(0);
        }
        let title = words(title);
        if title == self.0 {
            return Some(-1);
        }
        self.0
            .iter()
            .map(|term| {
                if title.contains(term) {
                    Some(0)
                } else if term.len() >= 3 && title.iter().any(|word| word.starts_with(term)) {
                    Some(1)
                } else if term.len() >= 4 && title.iter().any(|word| one_edit_apart(word, term)) {
                    Some(2)
                } else {
                    None
                }
            })
            .sum()
    }
}

fn words(text: &str) -> Vec<Vec<char>> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.chars().collect())
        .collect()
}

fn one_edit_apart(a: &[char], b: &[char]) -> bool {
    let common = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let (a, b) = (&a[common..], &b[common..]);
    match (a, b) {
        ([_, a_tail @ ..], b) if a_tail.len() == b.len() => a_tail == b,
        (a, [_, b_tail @ ..]) if a.len() == b_tail.len() => a == b_tail,
        ([_, a_tail @ ..], [_, b_tail @ ..]) if a_tail == b_tail => true,
        ([first, second, a_tail @ ..], [other_first, other_second, b_tail @ ..]) => {
            first == other_second && second == other_first && a_tail == b_tail
        }
        _ => false,
    }
}
