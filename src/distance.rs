//! Step 4: compare two inputs leaf by leaf, parameter by parameter.
//!
//! Only distance parameters are used here; category parameters are handled
//! by `coverage`. Distances are kept per (leaf, parameter) and never averaged
//! before the acceptance decision, so one large difference can't be hidden by many
//! small ones:
//!
//! ```text
//! a and b are clearly different  ⇔  some (leaf, parameter) p has d_p ≥ θ_p
//! ```
//!
//! Leaves that exist in only one of the inputs (e.g. the fields of an enum
//! variant the other input didn't choose) are skipped: the difference is
//! already measured on the choice leaf above them.

use crate::registry::Registry;
use crate::value::Input;

/// The distance between two inputs on one parameter of one leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDistance {
    /// The leaf, e.g. `cfg.name`.
    pub path: String,
    /// The parameter, e.g. `length`.
    pub param: &'static str,
    /// How far apart the two inputs are on it, from 0 to 1.
    pub distance: f64,
    /// `θ_p`: the distance needed on this parameter to count as different.
    pub threshold: f64,
}

/// Every per-parameter distance between `a` and `b`. Useful for reporting
/// *why* two inputs are (or aren't) different.
pub fn compare(a: &Input, b: &Input, registry: &Registry, theta: f64) -> Vec<ParamDistance> {
    let mut distances = Vec::new();
    for (path, leaf_a) in a {
        let Some(leaf_b) = b.get(path) else {
            continue;
        };
        for param in registry.distances(&leaf_a.kind) {
            distances.push(ParamDistance {
                path: path.clone(),
                param: param.name,
                distance: (param.distance)(&leaf_a.kind, &leaf_a.value, &leaf_b.value),
                threshold: theta * param.threshold_scale,
            });
        }
    }
    distances
}

/// True if `a` and `b` differ by at least `θ_p` on some parameter of some leaf.
///
/// Same rule as checking `compare(..)` for any distance over its threshold,
/// but stops at the first one found.
pub fn clearly_different(a: &Input, b: &Input, registry: &Registry, theta: f64) -> bool {
    for (path, leaf_a) in a {
        let Some(leaf_b) = b.get(path) else {
            continue;
        };
        for param in registry.distances(&leaf_a.kind) {
            let threshold = theta * param.threshold_scale;
            if threshold > 1.0 {
                continue; // Distances never exceed 1, so this can't trigger.
            }
            if (param.distance)(&leaf_a.kind, &leaf_a.value, &leaf_b.value) >= threshold {
                return true;
            }
        }
    }
    false
}

/// A single summary number: the AHP-weighted distance of each shared leaf,
/// averaged over the leaves. Only for statistics and ranking — never for the
/// accept/discard decision, because averaging hides large differences.
///
/// Leaves with only category parameters (bool, enum choices) are skipped.
pub fn weighted_mean(a: &Input, b: &Input, registry: &Registry) -> f64 {
    let mut total = 0.0;
    let mut shared_leaves = 0;
    for (path, leaf_a) in a {
        let Some(leaf_b) = b.get(path) else {
            continue;
        };
        let params = registry.distances(&leaf_a.kind);
        if params.is_empty() {
            continue;
        }
        let mut weighted_sum = 0.0;
        let mut weight_sum = 0.0;
        for param in params {
            weighted_sum +=
                param.weight * (param.distance)(&leaf_a.kind, &leaf_a.value, &leaf_b.value);
            weight_sum += param.weight;
        }
        // Rescale so a leaf that differs maximally on every distance scores 1.
        total += weighted_sum / weight_sum;
        shared_leaves += 1;
    }
    if shared_leaves == 0 {
        return 0.0;
    }
    total / shared_leaves as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;
    use crate::value::{InputLeaf, Kind, LeafValue};

    fn number(value: u128) -> InputLeaf {
        InputLeaf {
            kind: Kind::from(Primitive::U8),
            value: LeafValue::UInt(value),
        }
    }

    fn text(value: &str) -> InputLeaf {
        InputLeaf {
            kind: Kind::from(Primitive::String),
            value: LeafValue::Text(value.to_string()),
        }
    }

    /// 20 identical number leaves plus one string leaf.
    fn input_with_name(name: &str) -> Input {
        let mut input = Input::new();
        for i in 0..20 {
            input.insert(format!("n{i}"), number(42));
        }
        input.insert("name".to_string(), text(name));
        input
    }

    #[test]
    fn one_large_difference_is_not_hidden_by_many_equal_leaves() {
        let registry = Registry::new().unwrap();
        let a = input_with_name("");
        let b = input_with_name("a completely different string!");

        // Averaged, the difference almost disappears...
        assert!(weighted_mean(&a, &b, &registry) < 0.1);
        // ...but per parameter it is clearly there.
        assert!(clearly_different(&a, &b, &registry, 0.3));
    }

    #[test]
    fn identical_inputs_are_not_different() {
        let registry = Registry::new().unwrap();
        let a = input_with_name("same");
        assert!(!clearly_different(&a, &a, &registry, 0.3));
        assert_eq!(weighted_mean(&a, &a, &registry), 0.0);
    }

    #[test]
    fn compare_explains_the_difference() {
        let registry = Registry::new().unwrap();
        let a = input_with_name("");
        let b = input_with_name("abc");
        let over_threshold: Vec<_> = compare(&a, &b, &registry, 0.3)
            .into_iter()
            .filter(|d| d.distance >= d.threshold)
            .map(|d| (d.path, d.param))
            .collect();
        assert_eq!(
            over_threshold,
            vec![
                ("name".to_string(), "length"),
                ("name".to_string(), "bytes"),
                ("name".to_string(), "classes"),
            ]
        );
    }
}
