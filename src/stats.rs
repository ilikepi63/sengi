//! Statistics about a finished corpus: how big it is, how hard it was to
//! fill, and how spread out it ended up.
//!
//! The pair distances use [`crate::distance::weighted_mean`], a single
//! averaged number per pair. That's fine for a summary, but it is never used
//! to decide what goes into the corpus.

use crate::corpus::{Corpus, KeptFor};
use crate::distance::weighted_mean;
use crate::registry::Registry;

/// A summary of one corpus.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Stats {
    /// Number of inputs kept.
    pub size: usize,
    /// Number of candidates generated.
    pub attempts: usize,
    /// Fraction of candidates that were kept.
    pub acceptance_rate: f64,
    /// Inputs kept because they covered a new category or pair.
    pub kept_for_categories: usize,
    /// Inputs kept because they were clearly different on a distance.
    pub kept_for_distance: usize,
    /// How many single categories and pairs of categories are covered.
    pub covered: usize,
    /// Over every pair of inputs, using the weighted mean distance.
    pub min_pair_distance: f64,
    /// Average weighted mean distance between pairs of inputs.
    pub mean_pair_distance: f64,
}

/// Summarise a corpus. Comparing every pair is quadratic in the corpus size,
/// which is fine for the default cap of 200 inputs.
pub fn stats(corpus: &Corpus, registry: &Registry) -> Stats {
    let mut min = f64::INFINITY;
    let mut total = 0.0;
    let mut pairs = 0;
    for (i, a) in corpus.inputs.iter().enumerate() {
        for b in &corpus.inputs[i + 1..] {
            let distance = weighted_mean(a, b, registry);
            min = min.min(distance);
            total += distance;
            pairs += 1;
        }
    }

    Stats {
        size: corpus.inputs.len(),
        attempts: corpus.attempts,
        acceptance_rate: corpus.inputs.len() as f64 / corpus.attempts.max(1) as f64,
        kept_for_categories: corpus
            .kept_for
            .iter()
            .filter(|reason| **reason == KeptFor::NewCategories)
            .count(),
        kept_for_distance: corpus
            .kept_for
            .iter()
            .filter(|reason| **reason == KeptFor::Distance)
            .count(),
        covered: corpus.covered,
        min_pair_distance: if pairs == 0 { 0.0 } else { min },
        mean_pair_distance: if pairs == 0 {
            0.0
        } else {
            total / pairs as f64
        },
    }
}
