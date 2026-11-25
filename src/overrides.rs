//! Overrides: custom generation and custom parameters for specific types or
//! paths, e.g. a `DnaSequence(String)` that must only contain `A`, `C`, `G`
//! and `T`.
//!
//! Without an override, every type is broken down into simple types and
//! generated with the built-in generators, which know nothing about the
//! domain: a `DnaSequence` would be filled with random strings.
//!
//! An [`Override`] replaces that for one type:
//!
//! - **generate**: produces a whole value of the type, as JSON (the same
//!   layout the corpus is saved in, so `"ACGT"` for a newtype around a
//!   `String`).
//! - **parameters**: the distances and categories used to compare two
//!   values of the type (e.g. GC content), plus an optional AHP comparison
//!   matrix. They replace the built-in parameters entirely.
//!
//! Once an override matches, the type is **not broken down further**: it
//! becomes a single leaf in the shape (see [`crate::shape::Shape::Custom`]).
//!
//! Overrides are attached to a **type name** (applies wherever the type
//! appears) and/or a **path** (applies to one place in one function's
//! inputs). A path override wins over a type override. List elements use `[]`
//! in paths: `reads[]` is every element of `reads`, `scores[].key` every map
//! key. Use [`crate::shape::type_paths`] to list the paths of a function.
//!
//! ```
//! use sengi::overrides::{Override, Overrides};
//! use sengi::rng::Rng;
//! use serde_json::Value;
//!
//! fn dna(rng: &mut Rng) -> Value {
//!     let len = rng.range(1, 30) as usize;
//!     let mut sequence = String::new();
//!     for _ in 0..len {
//!         sequence.push(*rng.pick(&['A', 'C', 'G', 'T']));
//!     }
//!     Value::String(sequence)
//! }
//!
//! let mut overrides = Overrides::new();
//! overrides.add(
//!     Override::new("dna", dna)
//!         .distance("length", |a, b| {
//!             let (a, b) = (a.as_str().unwrap().len(), b.as_str().unwrap().len());
//!             a.abs_diff(b) as f64 / a.max(b).max(1) as f64
//!         })
//!         .category("starts_with_atg", |v| v.as_str().unwrap().starts_with("ATG").to_string()),
//! );
//! overrides.use_for_type("DnaSequence", "dna");
//! ```

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use crate::params::{Measure, Param};
use crate::rng::Rng;
use crate::value::LeafValue;

/// Produces one value of an overridden type, as JSON.
pub type GenerateFn = Box<dyn Fn(&mut Rng) -> Value>;

/// Custom generation and parameters for one type.
pub struct Override {
    /// Unique name; also names this override's parameter set in the registry.
    pub name: String,
    /// Produces one value.
    pub generate: GenerateFn,
    /// The distances and categories used to compare two values.
    pub params: Vec<Param>,
    /// AHP comparison matrix for `params` (see [`crate::ahp`]). If left
    /// empty, every parameter is treated as equally important.
    pub comparisons: Vec<Vec<f64>>,
}

impl Override {
    /// An override with a generator and no parameters yet.
    pub fn new(name: &str, generate: impl Fn(&mut Rng) -> Value + 'static) -> Override {
        Override {
            name: name.to_string(),
            generate: Box::new(generate),
            params: Vec::new(),
            comparisons: Vec::new(),
        }
    }

    /// Adds a distance parameter: how far apart two values are, from 0 to 1.
    pub fn distance(
        mut self,
        name: &'static str,
        distance: impl Fn(&Value, &Value) -> f64 + 'static,
    ) -> Override {
        self.params.push(Param {
            name,
            measure: Measure::Distance(Rc::new(move |_, a, b| distance(json_of(a), json_of(b)))),
        });
        self
    }

    /// Adds a category parameter: which category a value falls into. Used for
    /// pairwise coverage.
    pub fn category(
        mut self,
        name: &'static str,
        category: impl Fn(&Value) -> String + 'static,
    ) -> Override {
        self.params.push(Param {
            name,
            measure: Measure::Category(Rc::new(move |_, v| category(json_of(v)))),
        });
        self
    }

    /// Sets the AHP comparison matrix, one row and column per parameter in
    /// the order they were added.
    pub fn comparisons(mut self, comparisons: Vec<Vec<f64>>) -> Override {
        self.comparisons = comparisons;
        self
    }
}

/// The JSON value inside a custom leaf.
fn json_of(value: &LeafValue) -> &Value {
    match value {
        LeafValue::Custom(json) => json,
        other => panic!("expected a custom value, got {other:?}"),
    }
}

/// All overrides, and which types and paths use them.
#[derive(Default)]
pub struct Overrides {
    by_name: HashMap<String, Override>,
    /// Type name (e.g. `DnaSequence`) -> override name.
    by_type: HashMap<String, String>,
    /// Path (e.g. `seq` or `reads[]`) -> override name.
    by_path: HashMap<String, String>,
}

impl Overrides {
    /// No overrides: everything is generated with the built-in generators.
    pub fn new() -> Overrides {
        Overrides::default()
    }

    /// Registers an override under its name. It does nothing until it's
    /// attached with [`Overrides::use_for_type`] or [`Overrides::use_for_path`].
    pub fn add(&mut self, the_override: Override) {
        self.by_name.insert(the_override.name.clone(), the_override);
    }

    /// Use override `name` wherever a type called `type_name` appears.
    pub fn use_for_type(&mut self, type_name: &str, name: &str) {
        self.by_type.insert(type_name.to_string(), name.to_string());
    }

    /// Use override `name` at one input path, e.g. `seq` or `cfg.name`.
    pub fn use_for_path(&mut self, path: &str, name: &str) {
        self.by_path.insert(path.to_string(), name.to_string());
    }

    /// The override registered under `name`.
    pub fn get(&self, name: &str) -> Option<&Override> {
        self.by_name.get(name)
    }

    /// The override name for a type, if any.
    pub fn for_type(&self, type_name: &str) -> Option<&str> {
        self.by_type.get(type_name).map(String::as_str)
    }

    /// The override name for a path, if any.
    pub fn for_path(&self, path: &str) -> Option<&str> {
        self.by_path.get(path).map(String::as_str)
    }

    /// Every registered override.
    pub fn all(&self) -> impl Iterator<Item = &Override> {
        self.by_name.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;
    use crate::corpus::{CorpusConfig, build_corpus};
    use crate::function_inputs;
    use crate::registry::Registry;
    use crate::save::input_to_json;
    use crate::shape::{Shape, function_shape, type_paths};
    use crate::value::Kind;

    const FIXTURE: &str = include_str!("../tests/fixtures/dna.rs");

    fn random_dna(rng: &mut Rng) -> Value {
        let len = rng.range(1, 40) as usize;
        let mut sequence = String::new();
        for _ in 0..len {
            sequence.push(*rng.pick(&['A', 'C', 'G', 'T']));
        }
        Value::String(sequence)
    }

    fn dna_overrides() -> Overrides {
        let dna = Override::new("dna", random_dna)
            .distance("length", |a, b| {
                let (a, b) = (a.as_str().unwrap().len(), b.as_str().unwrap().len());
                a.abs_diff(b) as f64 / a.max(b).max(1) as f64
            })
            .category("starts_with_a", |v| {
                v.as_str().unwrap().starts_with('A').to_string()
            });
        let mut overrides = Overrides::new();
        overrides.add(dna);
        overrides.use_for_type("DnaSequence", "dna");
        overrides
    }

    fn function(name: &str) -> crate::Function {
        let functions = function_inputs(FIXTURE).unwrap();
        functions.into_iter().find(|f| f.name == name).unwrap()
    }

    #[test]
    fn the_tree_stops_at_an_overridden_type() {
        let shape = function_shape(&function("gc_content"), &dna_overrides());
        assert_eq!(
            shape,
            Shape::Fields(vec![("seq".into(), Shape::Custom("dna".into()))])
        );

        // Nested inside a list of structs, too.
        let shape = function_shape(&function("align"), &dna_overrides());
        let Shape::Fields(params) = shape else {
            panic!()
        };
        let Shape::List(read, _) = &params[1].1 else {
            panic!()
        };
        let Shape::Fields(read_fields) = read.as_ref() else {
            panic!()
        };
        assert_eq!(
            read_fields[1],
            ("sequence".into(), Shape::Custom("dna".into()))
        );
        assert_eq!(
            read_fields[0],
            ("id".into(), Shape::Simple(Primitive::U32.into()))
        );
    }

    #[test]
    fn a_path_override_wins_over_a_type_override() {
        let mut overrides = dna_overrides();
        overrides.add(Override::new("fixed", |_| Value::String("ACGT".into())));
        overrides.use_for_path("reference", "fixed");

        let Shape::Fields(params) = function_shape(&function("align"), &overrides) else {
            panic!()
        };
        assert_eq!(params[0].1, Shape::Custom("fixed".into()));
    }

    #[test]
    fn only_valid_sequences_are_generated() {
        let overrides = dna_overrides();
        let registry = Registry::with_overrides(&overrides).unwrap();
        let shape = function_shape(&function("gc_content"), &overrides);
        let corpus = build_corpus(&shape, &registry, &overrides, &CorpusConfig::default());

        assert!(corpus.inputs.len() > 1);
        for input in &corpus.inputs {
            let json = input_to_json(&shape, input);
            let sequence = json["seq"].as_str().unwrap();
            assert!(sequence.chars().all(|c| "ACGT".contains(c)), "{sequence}");
        }
    }

    #[test]
    fn without_the_override_strings_are_arbitrary() {
        let none = Overrides::new();
        let registry = Registry::with_overrides(&none).unwrap();
        let shape = function_shape(&function("gc_content"), &none);
        let corpus = build_corpus(&shape, &registry, &none, &CorpusConfig::default());

        let any_invalid = corpus.inputs.iter().any(|input| {
            let json = input_to_json(&shape, input);
            !json["seq"]
                .as_str()
                .unwrap()
                .chars()
                .all(|c| "ACGT".contains(c))
        });
        assert!(any_invalid);
    }

    #[test]
    fn the_override_brings_its_own_parameters() {
        let registry = Registry::with_overrides(&dna_overrides()).unwrap();
        let kind = Kind::Custom("dna".into());
        let distances: Vec<_> = registry.distances(&kind).iter().map(|p| p.name).collect();
        let categories: Vec<_> = registry.categories(&kind).iter().map(|p| p.name).collect();
        assert_eq!(distances, ["length"]);
        assert_eq!(categories, ["starts_with_a"]);
    }

    #[test]
    fn inconsistent_override_comparisons_are_refused() {
        let mut overrides = Overrides::new();
        overrides.add(
            Override::new("bad", |_| Value::Null)
                .distance("a", |_, _| 0.0)
                .distance("b", |_, _| 0.0)
                .distance("c", |_, _| 0.0)
                .comparisons(vec![
                    vec![1.0, 9.0, 1.0 / 9.0],
                    vec![1.0 / 9.0, 1.0, 9.0],
                    vec![9.0, 1.0 / 9.0, 1.0],
                ]),
        );
        assert!(Registry::with_overrides(&overrides).is_err());
    }

    #[test]
    fn type_paths_and_parsed_types_show_what_can_be_overridden() {
        let paths = type_paths(&function("gc_content"));
        assert_eq!(
            paths,
            vec![
                ("seq".to_string(), "DnaSequence".to_string()),
                ("seq.0".to_string(), "String".to_string()),
            ]
        );

        let names: Vec<_> = crate::parsed_types(FIXTURE)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["DnaSequence", "Read"]);
    }
}
