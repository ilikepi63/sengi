//! Step 3b: the registry — every kind's parameters together with their AHP
//! weights, split into distance parameters (with thresholds) and category
//! parameters (for coverage). Parameters are shared by a [`Family`] of
//! kinds (every unsigned integer range is [`Family::UInt`]). Overrides add one
//! family each ([`Family::Custom`]) with their own parameters.
//!
//! # Thresholds
//!
//! The weights decide how big a difference each distance parameter needs:
//!
//! ```text
//! θ_p = θ · threshold_scale,   threshold_scale = (largest distance weight) / w_p
//! ```
//!
//! The most important distance parameter of a kind needs a difference of θ;
//! a parameter half as important needs 2θ. For a `u64` at θ = 0.5, that
//! makes `magnitude` (weight 0.31) need 0.5 and `bits` (weight 0.11) need
//! 1.4 — more than the maximum of 1, so `bits` alone never makes an input
//! different. That's intended: an unimportant parameter shouldn't decide.
//!
//! The scale is relative to the largest *distance* weight, not the largest
//! weight overall, so a heavily weighted category (like `boundary`) can't
//! push every distance threshold above 1.

use std::collections::HashMap;

use crate::ahp::ahp;
use crate::overrides::Overrides;
use crate::params::{CategoryFn, DistanceFn, Measure, Param, param_set};
use crate::value::{Family, Kind};

/// A distance parameter with its AHP weight and threshold scale.
pub struct DistanceParam {
    /// Short name, e.g. `length`.
    pub name: &'static str,
    /// Distance between two values, from 0 to 1.
    pub distance: DistanceFn,
    /// AHP weight within this kind's parameters (all weights sum to 1).
    pub weight: f64,
    /// This parameter's threshold is `θ · threshold_scale`, where
    /// `threshold_scale = max distance weight / weight`. The most important
    /// distance parameter has scale 1; less important ones need a bigger
    /// difference to count.
    pub threshold_scale: f64,
}

/// A category parameter with its AHP weight.
pub struct CategoryParam {
    /// Short name, e.g. `boundary`.
    pub name: &'static str,
    /// The category a value falls into, e.g. `"max"`.
    pub category: CategoryFn,
    /// AHP weight within this kind's parameters (all weights sum to 1).
    pub weight: f64,
}

/// Every kind of leaf's parameters, split into distances and categories,
/// with their AHP weights.
///
/// Built once with [`Registry::new`], which checks every comparison matrix
/// for consistency.
pub struct Registry {
    distances: HashMap<Family, Vec<DistanceParam>>,
    categories: HashMap<Family, Vec<CategoryParam>>,
}

/// Every built-in family of leaf.
const ALL_FAMILIES: [Family; 8] = [
    Family::Bool,
    Family::UInt,
    Family::Int,
    Family::Float,
    Family::Char,
    Family::Text,
    Family::Choice,
    Family::Length,
];

impl Registry {
    /// The built-in parameters only. Same as `Registry::with_overrides(&Overrides::new())`.
    pub fn new() -> Result<Registry, String> {
        Registry::with_overrides(&Overrides::new())
    }

    /// The built-in parameters plus every override's parameters.
    ///
    /// Refuses any parameter set whose AHP comparisons are inconsistent
    /// (consistency ratio above 0.1).
    pub fn with_overrides(overrides: &Overrides) -> Result<Registry, String> {
        let mut registry = Registry {
            distances: HashMap::new(),
            categories: HashMap::new(),
        };
        for family in ALL_FAMILIES {
            let set = param_set(&family);
            registry.add_family(family, set.params, set.comparisons)?;
        }
        for the_override in overrides.all() {
            let family = Family::Custom(the_override.name.clone());
            let params = the_override.params.clone();
            let comparisons = if the_override.comparisons.is_empty() {
                // No matrix given: every parameter is equally important.
                vec![vec![1.0; params.len()]; params.len()]
            } else {
                the_override.comparisons.clone()
            };
            registry.add_family(family, params, comparisons)?;
        }
        Ok(registry)
    }

    /// Weights one family's parameters with AHP and stores them.
    fn add_family(
        &mut self,
        family: Family,
        params: Vec<Param>,
        comparisons: Vec<Vec<f64>>,
    ) -> Result<(), String> {
        if comparisons.len() != params.len() {
            return Err(format!(
                "{family:?}: {} parameters but a {}x{} comparison matrix",
                params.len(),
                comparisons.len(),
                comparisons.len()
            ));
        }

        let weights = if params.is_empty() {
            Vec::new()
        } else {
            let result = ahp(&comparisons);
            if result.consistency_ratio > 0.1 {
                return Err(format!(
                    "{family:?}: inconsistent comparisons (consistency ratio {:.3})",
                    result.consistency_ratio
                ));
            }
            result.weights
        };

        // Thresholds are relative to the most important *distance*
        // parameter; categories don't take part in the θ check.
        let mut max_distance_weight: f64 = 0.0;
        for (param, weight) in params.iter().zip(&weights) {
            if let Measure::Distance(_) = param.measure {
                max_distance_weight = max_distance_weight.max(*weight);
            }
        }

        let mut kind_distances = Vec::new();
        let mut kind_categories = Vec::new();
        for (param, weight) in params.into_iter().zip(weights) {
            match param.measure {
                Measure::Distance(distance) => kind_distances.push(DistanceParam {
                    name: param.name,
                    distance,
                    weight,
                    threshold_scale: max_distance_weight / weight,
                }),
                Measure::Category(category) => kind_categories.push(CategoryParam {
                    name: param.name,
                    category,
                    weight,
                }),
            }
        }
        self.distances.insert(family.clone(), kind_distances);
        self.categories.insert(family, kind_categories);
        Ok(())
    }

    /// The distance parameters of a kind (may be empty, e.g. for `bool`).
    pub fn distances(&self, kind: &Kind) -> &[DistanceParam] {
        &self.distances[&kind.family()]
    }

    /// The category parameters of a kind (may be empty, e.g. for strings).
    pub fn categories(&self, kind: &Kind) -> &[CategoryParam] {
        &self.categories[&kind.family()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;

    #[test]
    fn built_in_comparisons_are_consistent() {
        let registry = Registry::new().unwrap();
        for family in ALL_FAMILIES {
            let distance_weights: f64 = registry.distances[&family].iter().map(|p| p.weight).sum();
            let category_weights: f64 = registry.categories[&family].iter().map(|p| p.weight).sum();
            let total = distance_weights + category_weights;
            assert!(
                (total - 1.0).abs() < 1e-9,
                "{family:?} weights sum to {total}"
            );
        }
    }

    #[test]
    fn most_important_distance_has_the_lowest_threshold() {
        let registry = Registry::new().unwrap();
        let params = registry.distances(&Kind::from(Primitive::U32));
        let magnitude = params.iter().find(|p| p.name == "magnitude").unwrap();
        let bits = params.iter().find(|p| p.name == "bits").unwrap();
        assert_eq!(magnitude.threshold_scale, 1.0);
        assert!(bits.threshold_scale > 1.0);
    }

    #[test]
    fn categories_are_kept_apart_from_distances() {
        let registry = Registry::new().unwrap();
        let u32_categories: Vec<_> = registry
            .categories(&Kind::from(Primitive::U32))
            .iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(u32_categories, ["boundary"]);
        assert!(registry.distances(&Kind::Choice).is_empty());
    }
}
