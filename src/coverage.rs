//! Step 4b: pairwise coverage of categories.
//!
//! Every category parameter of every leaf is a *dimension*, e.g.
//! `cfg.retries / boundary`, and an input has one category on it, e.g.
//! `"max"`. Coverage remembers every single category and every pair of
//! categories (from two different dimensions) that some kept input had.
//!
//! A candidate that brings a new single category or a new pair is worth
//! keeping, however close it is on the distance parameters.
//!
//! # Why categories aren't distances
//!
//! A category mismatch is all-or-nothing. Scored as a distance it would be
//! 1, which passes any θ — so θ couldn't control it, and with several
//! categorical leaves almost every candidate differed on *something*.
//! Pairwise coverage is bounded instead: two `bool` inputs need exactly the
//! 4 combinations, two `u64` inputs exactly 5 × 5 boundary pairs.
//!
//! Elements of a list share one dimension: `xs[0]` and `xs[3]` are both `xs[]`.

use std::collections::{BTreeSet, HashSet};

use crate::registry::Registry;
use crate::value::Input;

/// A dimension and the category an input has on it, e.g.
/// `("cfg.retries / boundary", "max")`.
pub type Category = (String, String);

/// One category, or two categories from different dimensions.
type Tuple = Vec<Category>;

/// Every (dimension, category) of an input.
pub fn categories(input: &Input, registry: &Registry) -> BTreeSet<Category> {
    let mut found = BTreeSet::new();
    for (path, leaf) in input {
        let dimension_path = without_indices(path);
        for param in registry.categories(&leaf.kind) {
            let dimension = format!("{dimension_path} / {}", param.name);
            let category = (param.category)(&leaf.kind, &leaf.value);
            found.insert((dimension, category));
        }
    }
    found
}

/// Every single category and pair of categories seen in the kept inputs.
#[derive(Default)]
pub struct Coverage {
    covered: HashSet<Tuple>,
}

impl Coverage {
    /// True if these categories include a single category or a pair that no
    /// kept input had.
    pub fn is_new(&self, categories: &BTreeSet<Category>) -> bool {
        for tuple in tuples(categories) {
            if !self.covered.contains(&tuple) {
                return true;
            }
        }
        false
    }

    /// Mark all of an input's categories, and their pairs, as covered.
    pub fn add(&mut self, categories: &BTreeSet<Category>) {
        for tuple in tuples(categories) {
            self.covered.insert(tuple);
        }
    }

    /// How many singles and pairs are covered.
    pub fn len(&self) -> usize {
        self.covered.len()
    }

    /// True if nothing is covered yet.
    pub fn is_empty(&self) -> bool {
        self.covered.is_empty()
    }
}

/// Every single category, plus every pair from two different dimensions.
fn tuples(categories: &BTreeSet<Category>) -> Vec<Tuple> {
    let list: Vec<&Category> = categories.iter().collect();
    let mut tuples = Vec::new();
    for (i, a) in list.iter().enumerate() {
        tuples.push(vec![(*a).clone()]);
        for b in &list[i + 1..] {
            // Two list elements on the same dimension aren't a combination.
            if a.0 != b.0 {
                tuples.push(vec![(*a).clone(), (*b).clone()]);
            }
        }
    }
    tuples
}

/// `xs[3].key` -> `xs[].key`
pub(crate) fn without_indices(path: &str) -> String {
    let mut result = String::new();
    let mut inside_brackets = false;
    for c in path.chars() {
        if c == '[' {
            inside_brackets = true;
            result.push_str("[]");
        } else if c == ']' {
            inside_brackets = false;
        } else if !inside_brackets {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;
    use crate::value::{InputLeaf, Kind, LeafValue};

    fn input(leaves: &[(&str, InputLeaf)]) -> Input {
        let mut input = Input::new();
        for (path, leaf) in leaves {
            input.insert(path.to_string(), leaf.clone());
        }
        input
    }

    fn flag(value: bool) -> InputLeaf {
        InputLeaf {
            kind: Kind::from(Primitive::Bool),
            value: LeafValue::Bool(value),
        }
    }

    fn byte(value: u128) -> InputLeaf {
        InputLeaf {
            kind: Kind::from(Primitive::U8),
            value: LeafValue::UInt(value),
        }
    }

    #[test]
    fn list_indices_share_a_dimension() {
        assert_eq!(without_indices("xs[3].key"), "xs[].key");
        assert_eq!(without_indices("cfg.name"), "cfg.name");
    }

    #[test]
    fn categories_of_an_input() {
        let registry = Registry::new().unwrap();
        let found = categories(&input(&[("a", flag(true)), ("n", byte(255))]), &registry);
        let expected: BTreeSet<Category> = [
            ("a / value".to_string(), "true".to_string()),
            ("n / boundary".to_string(), "max".to_string()),
        ]
        .into();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_new_pair_is_new_even_if_each_category_was_seen() {
        let registry = Registry::new().unwrap();
        let mut coverage = Coverage::default();
        coverage.add(&categories(
            &input(&[("a", flag(true)), ("b", flag(true))]),
            &registry,
        ));
        coverage.add(&categories(
            &input(&[("a", flag(false)), ("b", flag(false))]),
            &registry,
        ));

        // a = true and b = false were each seen, but never together.
        let mixed = categories(&input(&[("a", flag(true)), ("b", flag(false))]), &registry);
        assert!(coverage.is_new(&mixed));

        let repeat = categories(&input(&[("a", flag(true)), ("b", flag(true))]), &registry);
        assert!(!coverage.is_new(&repeat));
    }
}
