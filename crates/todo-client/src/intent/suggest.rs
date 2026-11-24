//! A per-replica list suggestion for quick-add outside list rooms. No titles, I/O or platform state
//! are retained. Train with [`ListSuggester::from_state`]; use `observe` for new tasks, and retrain
//! after moves, edits or deletions. Pass the **current** replica to `suggest` so even a stale model
//! cannot propose a list that has disappeared. Pass the parsed title from [`super::quick_add::parse`],
//! not raw composer text: date and priority suffixes such as `tomorrow !!` are not title features.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Weak};
use todo_core::{
    State,
    ids::ListId,
    model::Task,
    query::{EvalContext, fold},
};

use super::{TaskRoom, default_list, draft_features, quick_add::Draft};

/// Train once per replica view, including optimistic state changes.
#[derive(Debug, Default)]
pub struct ReplicaSuggester {
    state: Weak<State>,
    model: ListSuggester,
}

impl ReplicaSuggester {
    pub fn for_state(&mut self, state: &Arc<State>) -> &ListSuggester {
        if !std::ptr::eq(self.state.as_ptr(), Arc::as_ptr(state)) {
            self.model = ListSuggester::from_state(state);
            self.state = Arc::downgrade(state);
        }
        &self.model
    }

    pub fn model(&self) -> &ListSuggester {
        &self.model
    }
}

pub fn suggest_for_draft(
    suggester: &ListSuggester,
    state: &State,
    room: TaskRoom,
    draft: &Draft,
    ctx: &EvalContext,
) -> Option<ListId> {
    if !room.allows_list_choice() || draft.list.is_some() {
        return None;
    }
    let parsed = draft.parsed_with_features(ctx, draft_features(state, room, draft));
    let list = suggester.suggest(&parsed.title, state)?.list;
    (Some(list) != default_list(state)).then_some(list)
}

const MIN_TASKS: u64 = 3;

// Type tags distinguish feature families. Keep the full hash, without retaining title text.
fn feature_id(tag: u8, first: &str, second: &str) -> u64 {
    let mut hash = DefaultHasher::new();
    (tag, first, second).hash(&mut hash);
    hash.finish()
}

/// Emit repeated features. Return whether the folded title has a word of at least three characters.
fn features(title: &str, mut emit: impl FnMut(u64)) -> bool {
    let text = fold(title);
    let mut previous = None;
    let mut boundaries = Vec::new();
    let mut has_long_word = false;
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        emit(feature_id(1, word, ""));
        if let Some(prev) = previous {
            emit(feature_id(2, prev, word));
        }
        previous = Some(word);
        boundaries.clear();
        boundaries.extend(word.char_indices().map(|(i, _)| i));
        has_long_word |= boundaries.len() >= 3;
        boundaries.push(word.len());
        for n in [3, 4] {
            for span in boundaries.windows(n + 1) {
                emit(feature_id(n as u8, &word[span[0]..span[n]], ""));
            }
        }
    }
    has_long_word
}

#[derive(Debug, Default)]
struct Class {
    tasks: u64,
    total: u64,
    counts: BTreeMap<u64, u32>,
}

/// A destination and its probability among lists with at least three training tasks.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Suggestion {
    pub list: ListId,
    pub confidence: f64,
}

/// Multinomial naive Bayes over folded word unigrams, word bigrams and character 3-/4-grams.
#[derive(Debug)]
pub struct ListSuggester {
    classes: BTreeMap<ListId, Class>,
    vocabulary: BTreeSet<u64>,
    threshold: f64,
}

impl Default for ListSuggester {
    fn default() -> Self {
        Self {
            classes: BTreeMap::new(),
            vocabulary: BTreeSet::new(),
            threshold: 0.9,
        }
    }
}

fn live_tasks(state: &State) -> impl Iterator<Item = &Task> {
    state
        .tasks
        .values()
        .filter(|t| t.deleted.is_none() && state.is_live(t.list.into()))
}

impl ListSuggester {
    /// Train on undeleted tasks in live lists, irrespective of completion.
    pub fn from_state(state: &State) -> Self {
        let mut model = Self::default();
        for task in live_tasks(state) {
            model.observe(&task.title, task.list);
        }
        model
    }

    /// Select the minimum accepted probability (inclusive).
    #[must_use]
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold.clamp(0., 1.);
        self
    }

    /// Learn one new task assigned to `list`, using its stored or parsed title.
    pub fn observe(&mut self, title: &str, list: ListId) {
        let class = self.classes.entry(list).or_default();
        class.tasks += 1;
        features(title, |id| {
            self.vocabulary.insert(id);
            let count = class.counts.entry(id).or_default();
            if *count < u32::MAX {
                *count += 1;
                class.total += 1;
            }
        });
    }

    fn ranked(&self, title: &str) -> Vec<Suggestion> {
        let mut known = Vec::new();
        let has_long_word = features(title, |id| {
            if self.vocabulary.contains(&id) {
                known.push(id);
            }
        });
        // No lexical evidence: neither class priors nor the smoothing denominator are a match.
        if !has_long_word || known.is_empty() {
            return Vec::new();
        }
        let temperature = (known.len() as f64).sqrt();
        let mut ranked = Vec::new();
        for (&list, class) in self.classes.iter().filter(|(_, c)| c.tasks >= MIN_TASKS) {
            // Add-one smoothing. log(class.tasks) is the log prior up to a shared constant.
            let denom = (class.total as f64 + self.vocabulary.len() as f64).ln();
            let likelihood = known
                .iter()
                .map(|id| f64::from(class.counts.get(id).copied().unwrap_or(0)).ln_1p() - denom)
                .sum::<f64>();
            let score = (class.tasks as f64).ln() + likelihood / temperature;
            ranked.push(Suggestion {
                list,
                confidence: score,
            });
        }
        if ranked.len() < 2 {
            return Vec::new();
        } // A single class would always receive 1.0.
        // Stable ties retain ListId order. Subtract the maximum before softmax.
        ranked.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        if let Some(&best) = ranked.first() {
            let mut sum = 0.;
            for s in &mut ranked {
                s.confidence = (s.confidence - best.confidence).exp();
                sum += s.confidence;
            }
            for s in &mut ranked {
                s.confidence /= sum;
            }
        }
        ranked
    }

    /// Suggest among at least two qualifying live lists. Use [`super::quick_add::Parsed::title`],
    /// not the raw quick-add text. Do not substitute a runner-up for a stale winner.
    pub fn suggest(&self, title: &str, state: &State) -> Option<Suggestion> {
        let ranked = self.ranked(title);
        let (best, others) = ranked.split_first()?;
        (best.confidence >= self.threshold
            && state.is_live(best.list.into())
            && others.iter().any(|s| state.is_live(s.list.into())))
        .then_some(*best)
    }
}

#[cfg(test)]
mod eval;

#[test]
fn ranking_empty_folded_text_never_produces_nan() {
    let mut model = ListSuggester::default();
    for i in 1..=2 {
        for _ in 0..3 {
            model.observe("sample", ListId::from_uuid(uuid::Uuid::from_u128(i)));
        }
    }
    assert!(model.ranked("\u{345}\u{345}").is_empty());
}

#[test]
fn distinct_word_features_are_not_compressed_into_a_small_table() {
    let hashes: std::collections::BTreeSet<_> = (0..4096)
        .map(|i| feature_id(1, &format!("sample{i}"), ""))
        .collect();
    assert_eq!(hashes.len(), 4096);
}

#[test]
fn folded_feature_stream_counts_words_pairs_and_overlapping_grams() {
    let mut actual = Vec::new();
    features("ČTYŘI, čtyři!", |i| actual.push(i));
    let word = feature_id(1, "ctyri", "");
    let grams = [
        feature_id(3, "cty", ""),
        feature_id(3, "tyr", ""),
        feature_id(3, "yri", ""),
        feature_id(4, "ctyr", ""),
        feature_id(4, "tyri", ""),
    ];
    let expected = [
        vec![word],
        grams.to_vec(),
        vec![word, feature_id(2, "ctyri", "ctyri")],
        grams.to_vec(),
    ]
    .concat();
    assert_eq!(actual, expected);
}
