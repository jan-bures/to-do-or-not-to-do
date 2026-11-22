//! A per-replica list suggestion for quick-add outside list rooms. No titles, I/O or platform state
//! are retained. Train with [`ListSuggester::from_state`]; use `observe` for new tasks, and retrain
//! after moves, edits or deletions. Pass the **current** replica to `suggest` so even a stale model
//! cannot propose a list that has disappeared.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use todo_core::{State, ids::ListId, model::Task, query::fold};

// Feature hashing fixes the size of each class at 8 KiB regardless of vocabulary size. Type tags
// distinguish feature families. Collisions are allowed and counted as one vocabulary entry.
const BINS: usize = 2048;
const MIN_TASKS: u64 = 3;

fn bin(tag: u8, first: &str, second: &str) -> usize {
    let mut hash = DefaultHasher::new();
    (tag, first, second).hash(&mut hash);
    hash.finish() as usize % BINS
}

/// Emit repeated features (multinomial counts). Keep character grams within word boundaries.
fn features(title: &str, mut emit: impl FnMut(usize)) {
    let text = fold(title);
    let mut previous = None;
    let mut boundaries = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        emit(bin(1, word, ""));
        if let Some(prev) = previous {
            emit(bin(2, prev, word));
        }
        previous = Some(word);
        boundaries.clear();
        boundaries.extend(word.char_indices().map(|(i, _)| i));
        boundaries.push(word.len());
        for n in [3, 4] {
            for span in boundaries.windows(n + 1) {
                emit(bin(n as u8, &word[span[0]..span[n]], ""));
            }
        }
    }
}

#[derive(Debug)]
struct Class {
    tasks: u64,
    total: u64,
    counts: Box<[u32; BINS]>,
}

impl Default for Class {
    fn default() -> Self {
        Self {
            tasks: 0,
            total: 0,
            counts: Box::new([0; BINS]),
        }
    }
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
    threshold: f64,
}

impl Default for ListSuggester {
    fn default() -> Self {
        Self {
            classes: BTreeMap::new(),
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

    /// Learn one new task assigned to `list`.
    pub fn observe(&mut self, title: &str, list: ListId) {
        let class = self.classes.entry(list).or_default();
        class.tasks += 1;
        features(title, |i| {
            if class.counts[i] < u32::MAX {
                class.counts[i] += 1;
                class.total += 1;
            }
        });
    }

    fn ranked(&self, title: &str) -> Vec<Suggestion> {
        if title
            .chars()
            .filter(|c| c.is_alphanumeric())
            .nth(1)
            .is_none()
        {
            return Vec::new();
        }
        let mut bins = Vec::new();
        features(title, |i| bins.push(i));
        let temperature = (bins.len() as f64).sqrt();
        let mut ranked = Vec::new();
        for (&list, class) in self.classes.iter().filter(|(_, c)| c.tasks >= MIN_TASKS) {
            // Add-one smoothing. log(class.tasks) is the log prior up to a shared constant.
            let denom = (class.total as f64 + BINS as f64).ln();
            let likelihood = bins
                .iter()
                .map(|&i| f64::from(class.counts[i]).ln_1p() - denom)
                .sum::<f64>();
            let score = (class.tasks as f64).ln() + likelihood / temperature;
            ranked.push(Suggestion {
                list,
                confidence: score,
            });
        }
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

    /// Abstain on short input, low confidence or a stale winning list. Do not substitute a runner-up.
    pub fn suggest(&self, title: &str, state: &State) -> Option<Suggestion> {
        let best = self.ranked(title).first().copied()?;
        (best.confidence >= self.threshold && state.is_live(best.list.into())).then_some(best)
    }
}

#[cfg(test)]
mod eval;

#[test]
fn folded_feature_stream_counts_words_pairs_and_overlapping_grams() {
    let mut actual = Vec::new();
    features("ČTYŘI, čtyři!", |i| actual.push(i));
    let word = bin(1, "ctyri", "");
    let grams = [
        bin(3, "cty", ""),
        bin(3, "tyr", ""),
        bin(3, "yri", ""),
        bin(4, "ctyr", ""),
        bin(4, "tyri", ""),
    ];
    let expected = [
        vec![word],
        grams.to_vec(),
        vec![word, bin(2, "ctyri", "ctyri")],
        grams.to_vec(),
    ]
    .concat();
    assert_eq!(actual, expected);
}
