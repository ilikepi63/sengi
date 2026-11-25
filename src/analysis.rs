//! Analyse a set of inputs that already exists — a hand-written test suite,
//! fixtures, production samples, or a corpus sengi generated earlier.
//!
//! The inputs are read through a shape (from a JSON Schema or Rust types, see
//! [`crate::save::json_to_input`]), then measured with the same parameters
//! that decide what goes into a generated corpus:
//!
//! - each input's **categories** (e.g. `cfg.retries / boundary = max`);
//! - **pairwise distances**, as a matrix of weighted means for the overview
//!   and per parameter for explanations;
//! - **coverage**: how many single categories and pairs the set covers;
//! - **near-duplicates**: pairs with the same categories that aren't clearly
//!   different on any distance at θ — the pairs sengi would never keep both
//!   of, because they exercise the same thing.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::coverage::{Category, Coverage, categories};
use crate::distance::{ParamDistance, clearly_different, compare, weighted_mean};
use crate::registry::Registry;
use crate::save::{input_to_json, json_to_input};
use crate::shape::Shape;
use crate::value::Input;

/// Two inputs that exercise the same thing.
#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicate {
    /// Index of the first input.
    pub a: usize,
    /// Index of the second input (always greater than `a`).
    pub b: usize,
    /// Their weighted mean distance (0 = identical).
    pub distance: f64,
    /// Their biggest per-parameter differences, largest share of the
    /// threshold first; all of them fall short of their threshold.
    pub closest: Vec<ParamDistance>,
}

/// Everything measured about a set of inputs.
pub struct Analysis {
    /// The inputs, as read.
    pub inputs: Vec<Input>,
    /// Each input's categories, in the same order.
    pub categories: Vec<BTreeSet<Category>>,
    /// How many single categories and pairs the inputs cover together.
    pub covered: usize,
    /// Pairs that exercise the same thing.
    pub near_duplicates: Vec<NearDuplicate>,
    /// Weighted mean distance of the closest pair (0 with fewer than two
    /// inputs).
    pub min_pair_distance: f64,
    /// Average weighted mean distance over all pairs.
    pub mean_pair_distance: f64,
}

/// Reads every value through the shape. The error says which input (counted
/// from 1) and where it doesn't fit, e.g.
/// `input 3: cfg.retries: expected an integer in 0..=255, got -1`.
pub fn read_inputs(shape: &Shape, values: &[Value]) -> Result<Vec<Input>, String> {
    let mut inputs = Vec::new();
    for (i, value) in values.iter().enumerate() {
        let input = json_to_input(shape, value).map_err(|e| format!("input {}: {e}", i + 1))?;
        inputs.push(input);
    }
    Ok(inputs)
}

/// Measures a set of inputs. Comparing every pair is quadratic in the number
/// of inputs.
pub fn analyze(inputs: Vec<Input>, registry: &Registry, theta: f64) -> Analysis {
    let categories: Vec<BTreeSet<Category>> = inputs
        .iter()
        .map(|input| categories(input, registry))
        .collect();
    let mut coverage = Coverage::default();
    for input_categories in &categories {
        coverage.add(input_categories);
    }

    let mut min = f64::INFINITY;
    let mut total = 0.0;
    let mut pairs = 0;
    for (i, a) in inputs.iter().enumerate() {
        for b in &inputs[i + 1..] {
            let distance = weighted_mean(a, b, registry);
            min = min.min(distance);
            total += distance;
            pairs += 1;
        }
    }

    let near_duplicates = near_duplicates(&inputs, &categories, registry, theta);
    Analysis {
        inputs,
        categories,
        covered: coverage.len(),
        near_duplicates,
        min_pair_distance: if pairs == 0 { 0.0 } else { min },
        mean_pair_distance: if pairs == 0 {
            0.0
        } else {
            total / pairs as f64
        },
    }
}

/// Every pair with the same categories that isn't clearly different at θ.
///
/// `categories[i]` must be the categories of `inputs[i]`.
pub fn near_duplicates(
    inputs: &[Input],
    categories: &[BTreeSet<Category>],
    registry: &Registry,
    theta: f64,
) -> Vec<NearDuplicate> {
    let mut found = Vec::new();
    for a in 0..inputs.len() {
        for b in (a + 1)..inputs.len() {
            if categories[a] != categories[b]
                || clearly_different(&inputs[a], &inputs[b], registry, theta)
            {
                continue;
            }
            let mut closest: Vec<ParamDistance> = compare(&inputs[a], &inputs[b], registry, theta)
                .into_iter()
                .filter(|d| d.distance > 0.0)
                .collect();
            closest.sort_by(|x, y| {
                let x = x.distance / x.threshold;
                let y = y.distance / y.threshold;
                y.total_cmp(&x)
            });
            closest.truncate(3);
            found.push(NearDuplicate {
                a,
                b,
                distance: weighted_mean(&inputs[a], &inputs[b], registry),
                closest,
            });
        }
    }
    found
}

/// The analysis as JSON: `name`, `theta`, `stats`, `inputs` (each with its
/// value and categories) and `near_duplicates`.
pub fn analysis_to_json(name: &str, shape: &Shape, analysis: &Analysis, theta: f64) -> Value {
    let mut inputs = Vec::new();
    for (input, input_categories) in analysis.inputs.iter().zip(&analysis.categories) {
        let mut by_dimension = Map::new();
        for (dimension, category) in input_categories {
            by_dimension.insert(dimension.clone(), Value::String(category.clone()));
        }
        inputs.push(json!({
            "value": input_to_json(shape, input),
            "categories": by_dimension,
        }));
    }

    json!({
        "name": name,
        "theta": theta,
        "stats": {
            "size": analysis.inputs.len(),
            "covered": analysis.covered,
            "near_duplicates": analysis.near_duplicates.len(),
            "min_pair_distance": analysis.min_pair_distance,
            "mean_pair_distance": analysis.mean_pair_distance,
        },
        "inputs": inputs,
        "near_duplicates": near_duplicates_json(&analysis.near_duplicates),
    })
}

/// Near-duplicates as JSON, inputs counted from 1 as in the CLI and report.
pub fn near_duplicates_json(pairs: &[NearDuplicate]) -> Vec<Value> {
    let mut result = Vec::new();
    for pair in pairs {
        let closest: Vec<Value> = pair
            .closest
            .iter()
            .map(|d| {
                json!({
                    "leaf": d.path,
                    "parameter": d.param,
                    "distance": d.distance,
                    "threshold": d.threshold,
                })
            })
            .collect();
        result.push(json!({
            "a": pair.a + 1,
            "b": pair.b + 1,
            "distance": pair.distance,
            "closest": closest,
        }));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overrides::Overrides;
    use crate::schema::shape_from_schema;

    fn shape() -> Shape {
        shape_from_schema(
            &json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "count": { "type": "integer", "minimum": 0, "maximum": 1000 }
                },
                "required": ["name", "count"]
            }),
            &Overrides::new(),
        )
    }

    #[test]
    fn finds_inputs_that_exercise_the_same_thing() {
        let values = vec![
            json!({ "name": "alice", "count": 500 }),
            json!({ "name": "alicf", "count": 501 }), // barely differs from #1
            json!({ "name": "", "count": 0 }),
            json!({ "name": "a much longer name with spaces", "count": 1000 }),
        ];
        let registry = Registry::new().unwrap();
        let inputs = read_inputs(&shape(), &values).unwrap();
        let analysis = analyze(inputs, &registry, 0.5);

        let pairs: Vec<(usize, usize)> = analysis
            .near_duplicates
            .iter()
            .map(|d| (d.a, d.b))
            .collect();
        assert_eq!(pairs, [(0, 1)]);
        assert!(analysis.near_duplicates[0].distance < 0.1);
        assert!(analysis.covered > 0);
        assert!(analysis.mean_pair_distance > analysis.min_pair_distance);
    }

    #[test]
    fn inputs_that_do_not_fit_are_refused() {
        let values = vec![
            json!({ "name": "ok", "count": 1 }),
            json!({ "name": "x", "count": -1 }),
        ];
        let error = read_inputs(&shape(), &values).err().unwrap();
        assert_eq!(
            error,
            "input 2: count: expected an integer in 0..=1000, got -1"
        );
    }
}
