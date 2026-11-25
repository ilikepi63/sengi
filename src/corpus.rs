//! Step 5b: build a corpus — fuzz candidates and keep only the ones that
//! add something new.
//!
//! ```text
//! keep candidate  ⇔  it covers a category or pair of categories not seen yet
//!                 or  for every kept input t: clearly_different(candidate, t)
//! ```
//!
//! θ controls the distance half: a lower θ keeps inputs that are closer
//! together, so the corpus is bigger and denser. It doesn't affect coverage —
//! a new category or pair is always kept.
//!
//! The corpus depends on the order candidates arrive in: early inputs get in
//! easily and later ones must be far from all of them. The result is
//! well spread, not optimally spread. A fixed seed makes it reproducible.

use crate::coverage::{Coverage, categories};
use crate::distance::clearly_different;
use crate::fuzz::generate;
use crate::overrides::Overrides;
use crate::registry::Registry;
use crate::rng::Rng;
use crate::shape::Shape;
use crate::value::Input;

/// Settings for [`build_corpus`]. Use `CorpusConfig::default()` and override
/// what you need.
pub struct CorpusConfig {
    /// θ: how different (0..=1) a new input must be on its most important
    /// distance parameter. Bigger θ means a smaller, more spread-out corpus.
    pub theta: f64,
    /// Seed for the random generator: the same seed (and config) always
    /// produces the same corpus.
    pub seed: u64,
    /// Stop once the corpus has this many inputs.
    pub max_size: usize,
    /// Stop after this many candidates in total.
    pub max_attempts: usize,
    /// Stop after this many rejections in a row: the corpus is saturated.
    pub max_rejections_in_a_row: usize,
}

impl Default for CorpusConfig {
    fn default() -> Self {
        CorpusConfig {
            theta: 0.5,
            seed: 0,
            max_size: 200,
            max_attempts: 100_000,
            max_rejections_in_a_row: 2_000,
        }
    }
}

/// Why [`build_corpus`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The corpus reached `max_size`.
    Full,
    /// `max_rejections_in_a_row` candidates in a row brought nothing new:
    /// the space is covered as well as θ allows.
    Saturated,
    /// `max_attempts` candidates were generated in total.
    OutOfAttempts,
}

/// Why an input was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptFor {
    /// It covered a category, or pair of categories, no earlier input had.
    NewCategories,
    /// It was clearly different from every earlier input on some distance.
    Distance,
}

/// The result of [`build_corpus`].
pub struct Corpus {
    /// The kept inputs, in the order they were accepted.
    pub inputs: Vec<Input>,
    /// `kept_for[i]` says why `inputs[i]` was kept.
    pub kept_for: Vec<KeptFor>,
    /// `accepted_at[i]` is the attempt number (1-based) at which `inputs[i]`
    /// was kept. Plotting it against `i` shows how the corpus grew.
    pub accepted_at: Vec<usize>,
    /// How many candidates were generated, kept or not.
    pub attempts: usize,
    /// Why generation stopped.
    pub stop_reason: StopReason,
    /// How many single categories and pairs the corpus covers.
    pub covered: usize,
}

/// Fuzz inputs for `shape` and keep the ones that add something new.
///
/// Each candidate is kept if it covers a category or pair of categories no
/// kept input had (see [`crate::coverage`]), or if it is clearly different
/// from every kept input on some distance parameter (see
/// [`crate::distance`]). Coverage is checked first because it's cheaper.
///
/// Generation stops when the corpus is full, saturated, or out of attempts
/// (see [`StopReason`]).
pub fn build_corpus(
    shape: &Shape,
    registry: &Registry,
    overrides: &Overrides,
    config: &CorpusConfig,
) -> Corpus {
    let mut rng = Rng::new(config.seed);
    let mut inputs: Vec<Input> = Vec::new();
    let mut kept_for = Vec::new();
    let mut accepted_at = Vec::new();
    let mut coverage = Coverage::default();
    let mut attempts = 0;
    let mut rejections_in_a_row = 0;

    let stop_reason = loop {
        if inputs.len() >= config.max_size {
            break StopReason::Full;
        }
        if rejections_in_a_row >= config.max_rejections_in_a_row {
            break StopReason::Saturated;
        }
        if attempts >= config.max_attempts {
            break StopReason::OutOfAttempts;
        }

        let candidate = generate(shape, overrides, &mut rng);
        attempts += 1;

        let candidate_categories = categories(&candidate, registry);
        let reason = if coverage.is_new(&candidate_categories) {
            Some(KeptFor::NewCategories)
        } else if far_from_all(&candidate, &inputs, registry, config.theta) {
            Some(KeptFor::Distance)
        } else {
            None
        };

        match reason {
            Some(reason) => {
                coverage.add(&candidate_categories);
                inputs.push(candidate);
                kept_for.push(reason);
                accepted_at.push(attempts);
                rejections_in_a_row = 0;
            }
            None => rejections_in_a_row += 1,
        }
    };

    Corpus {
        inputs,
        kept_for,
        accepted_at,
        attempts,
        stop_reason,
        covered: coverage.len(),
    }
}

/// True if `candidate` is clearly different from every input in `inputs`.
fn far_from_all(candidate: &Input, inputs: &[Input], registry: &Registry, theta: f64) -> bool {
    for existing in inputs {
        if !clearly_different(candidate, existing, registry, theta) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;

    fn two_u64s() -> Shape {
        Shape::Fields(vec![
            ("left".into(), Shape::Simple(Primitive::U64.into())),
            ("right".into(), Shape::Simple(Primitive::U64.into())),
        ])
    }

    #[test]
    fn inputs_kept_for_distance_are_far_from_everything_before_them() {
        let registry = Registry::new().unwrap();
        // Low enough that distance adds inputs beyond the 25 boundary combinations.
        let config = CorpusConfig {
            theta: 0.2,
            ..CorpusConfig::default()
        };
        let corpus = build_corpus(&two_u64s(), &registry, &Overrides::new(), &config);

        assert!(corpus.kept_for.contains(&KeptFor::Distance));
        for (i, input) in corpus.inputs.iter().enumerate() {
            if corpus.kept_for[i] == KeptFor::Distance {
                let earlier = &corpus.inputs[..i];
                assert!(far_from_all(input, earlier, &registry, config.theta));
            }
        }
    }

    #[test]
    fn two_bools_give_exactly_the_four_combinations() {
        // Bools only have a category, so coverage alone decides: every pair
        // (a, b) is needed once, and nothing else is ever new.
        let shape = Shape::Fields(vec![
            ("a".into(), Shape::Simple(Primitive::Bool.into())),
            ("b".into(), Shape::Simple(Primitive::Bool.into())),
        ]);
        let registry = Registry::new().unwrap();
        let corpus = build_corpus(
            &shape,
            &registry,
            &Overrides::new(),
            &CorpusConfig::default(),
        );
        assert_eq!(corpus.inputs.len(), 4);
        assert_eq!(corpus.stop_reason, StopReason::Saturated);
    }

    #[test]
    fn smaller_theta_gives_a_bigger_corpus() {
        let registry = Registry::new().unwrap();
        let strict = CorpusConfig {
            theta: 0.9,
            ..CorpusConfig::default()
        };
        let loose = CorpusConfig {
            theta: 0.3,
            ..CorpusConfig::default()
        };
        let strict = build_corpus(&two_u64s(), &registry, &Overrides::new(), &strict);
        let loose = build_corpus(&two_u64s(), &registry, &Overrides::new(), &loose);
        assert!(loose.inputs.len() > strict.inputs.len());
    }

    #[test]
    fn same_seed_same_corpus() {
        let registry = Registry::new().unwrap();
        let config = CorpusConfig::default();
        let a = build_corpus(&two_u64s(), &registry, &Overrides::new(), &config);
        let b = build_corpus(&two_u64s(), &registry, &Overrides::new(), &config);
        assert_eq!(a.inputs, b.inputs);
    }
}
