//! Offline evaluation only. This module is not compiled into clients.

use super::*;
use proptest::{
    prelude::RngExt,
    test_runner::{RngAlgorithm, TestRng},
};

const THRESHOLDS: [f64; 3] = [0.5, 0.7, 0.9];

#[derive(Default)]
struct Metrics {
    tested: usize,
    first: usize,
    top_three: usize,
    accepted: [usize; 3],
    correct: [usize; 3],
}

impl Metrics {
    fn score(&mut self, ranked: &[Suggestion], truth: ListId) {
        // Include abstentions and cold-start targets in the denominator.
        self.tested += 1;
        let hit = ranked.first().is_some_and(|s| s.list == truth);
        self.first += usize::from(hit);
        self.top_three += usize::from(ranked.iter().take(3).any(|s| s.list == truth));
        for (i, p) in THRESHOLDS.into_iter().enumerate() {
            if ranked.first().is_some_and(|s| s.confidence >= p) {
                self.accepted[i] += 1;
                self.correct[i] += usize::from(hit);
            }
        }
    }

    fn report(&self, split: &str) {
        fn rate(n: usize, d: usize) -> String {
            if d == 0 {
                "n/a".into()
            } else {
                format!("{:.1}%", 100. * n as f64 / d as f64)
            }
        }
        println!(
            "{split}: n={}, top-1={}, top-3={}",
            self.tested,
            rate(self.first, self.tested),
            rate(self.top_three, self.tested)
        );
        for (i, p) in THRESHOLDS.into_iter().enumerate() {
            println!(
                "  {p:.1}: coverage={}, precision={}",
                rate(self.accepted[i], self.tested),
                rate(self.correct[i], self.accepted[i])
            );
        }
    }
}

fn holdout(tasks: &[&Task], test: std::ops::Range<usize>, metrics: &mut Metrics) {
    let mut model = ListSuggester::default();
    for task in tasks[..test.start].iter().chain(&tasks[test.end..]) {
        model.observe(&task.title, task.list);
    }
    for task in &tasks[test] {
        metrics.score(&model.ranked(&task.title), task.list);
    }
}

#[test]
#[ignore = "set TODO_SUGGEST_SNAPSHOT to a local JSON replica snapshot"]
fn suggest_eval() -> Result<(), &'static str> {
    let path = std::env::var_os("TODO_SUGGEST_SNAPSHOT").ok_or("missing snapshot path")?;
    let bytes = std::fs::read(path).map_err(|_| "cannot read snapshot")?;
    #[derive(serde::Deserialize)]
    struct Snapshot {
        state: State,
    }
    // Do not display deserializer errors: they can contain private input.
    let state = serde_json::from_slice::<Snapshot>(&bytes)
        .map(|s| s.state)
        .or_else(|_| serde_json::from_slice::<State>(&bytes))
        .map_err(|_| "invalid replica snapshot")?;
    let mut tasks: Vec<_> = live_tasks(&state).collect();
    if tasks.len() < 5 {
        return Err("need at least five live tasks");
    }
    let n = tasks.len();
    let mut rng = TestRng::from_seed(RngAlgorithm::ChaCha, &[17; 32]);
    for i in (1..n).rev() {
        tasks.swap(i, rng.random_range(0..=i));
    }
    let mut random = Metrics::default();
    for fold in 0..5 {
        holdout(&tasks, fold * n / 5..(fold + 1) * n / 5, &mut random);
    }
    random.report("random five-fold");

    tasks.sort_unstable_by_key(|t| (t.created.at, t.id));
    let mut chronological = Metrics::default();
    holdout(&tasks, 4 * n / 5..n, &mut chronological);
    chronological.report("chronological 80/20");
    Ok(())
}

#[test]
fn metrics_include_abstentions_and_only_credit_accepted_predictions() {
    let a = ListId::from_uuid(uuid::Uuid::from_u128(1));
    let b = ListId::from_uuid(uuid::Uuid::from_u128(2));
    let ranked = [
        Suggestion {
            list: a,
            confidence: 0.75,
        },
        Suggestion {
            list: b,
            confidence: 0.25,
        },
    ];
    let mut metrics = Metrics::default();
    metrics.score(&[], a);
    metrics.score(&ranked, a);
    metrics.score(&ranked, b);
    assert_eq!(
        (metrics.tested, metrics.first, metrics.top_three),
        (3, 1, 2)
    );
    assert_eq!(metrics.accepted, [2, 2, 0]);
    assert_eq!(metrics.correct, [1, 1, 0]);
}
