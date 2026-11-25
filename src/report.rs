//! An HTML report showing how a set of inputs differ from one another —
//! a generated corpus, or inputs from anywhere ([`crate::analysis`]).
//!
//! The report has these views:
//!
//! - **Tested model**: the inputs' JSON Schema, and the parts of the input
//!   as a foldable tree ([`model`]): what each part is, the parameters that
//!   measure it (distance or category, AHP weight, threshold at θ) and its
//!   share of all differences, summed up the tree.
//! - **Distance heatmap**: every pair of inputs, coloured by their weighted
//!   mean distance. Light cells off the diagonal are near-duplicates.
//! - **Corpus map**: the inputs laid out in 2-D so that the distance between
//!   dots approximates the distance between inputs (classical
//!   multidimensional scaling). An even cloud means a well-spread corpus.
//! - **What makes inputs different**: the same tree as an icicle chart, from
//!   the whole input down to parameters, each block as wide as its share of
//!   all differences between all pairs (AHP weight × distance; a category
//!   counts its weight when two inputs differ on it).
//! - **Growth** (generated corpora only): corpus size against attempts; a
//!   flat tail means saturation.
//! - **Near-duplicates**: pairs that exercise the same thing (see
//!   [`crate::analysis::near_duplicates`]), with the parameters that came
//!   closest to telling them apart.
//!
//! # Language independence
//!
//! The page is a fixed template (`report.html`) plus one JSON data block made
//! by [`report_data`]. It knows nothing about Rust: a future Python or
//! JavaScript front end that produces the same data (inputs as JSON, a
//! distance matrix, coordinates, the model tree, acceptance attempts) gets the
//! same report.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Primitive;
use crate::analysis::{near_duplicates, near_duplicates_json};
use crate::corpus::{Corpus, CorpusConfig, KeptFor};
use crate::coverage::{Category, Coverage, categories, without_indices};
use crate::distance::weighted_mean;
use crate::registry::Registry;
use crate::save::input_to_json;
use crate::schema::schema_of;
use crate::shape::{Len, Shape, Simple};
use crate::stats::Stats;
use crate::value::{Family, Input, Kind};

const TEMPLATE: &str = include_str!("report.html");

/// How sengi generated the inputs, for a report on a generated corpus.
pub struct Generation<'a> {
    /// The corpus the inputs come from.
    pub corpus: &'a Corpus,
    /// The settings it was built with.
    pub config: &'a CorpusConfig,
    /// Its statistics.
    pub stats: &'a Stats,
}

/// Writes the page for `data` (see [`report_data`]) to `<dir>/<name>.html`
/// and returns its path.
pub fn save_report(dir: &Path, name: &str, data: &Value) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.html", crate::save::file_name(name)));
    fs::write(&path, render_html(data))?;
    Ok(path)
}

/// The page: the template with the data block filled in.
pub fn render_html(data: &Value) -> String {
    // `</` inside the JSON would end the <script> element early.
    let json = data.to_string().replace("</", "<\\/");
    let label = data["label"].as_str().unwrap_or("inputs");
    let kind = if data["generated"] == Value::Bool(true) {
        "corpus"
    } else {
        "analysis"
    };
    let title = format!("{} {kind}", escape_html(label));
    TEMPLATE
        .replace("__TITLE__", &title)
        .replace("\"__REPORT_DATA__\"", &json)
}

/// Escapes text for use inside HTML, e.g. `Wrapper<T>::set` in the title.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Everything the page draws, as JSON.
///
/// `label` heads the page, e.g. `configure(&Config, bool)`. With a
/// [`Generation`], the page also explains why each input was kept and how
/// the corpus grew; without one it reports on inputs from anywhere.
pub fn report_data(
    label: &str,
    shape: &Shape,
    inputs: &[Input],
    registry: &Registry,
    theta: f64,
    generation: Option<Generation>,
) -> Value {
    let distances = distance_matrix(inputs, registry);
    let coordinates = classical_mds(&distances);
    let input_categories: Vec<BTreeSet<Category>> = inputs
        .iter()
        .map(|input| categories(input, registry))
        .collect();
    let duplicates = near_duplicates(inputs, &input_categories, registry, theta);

    let mut entries = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let mut entry = json!({
            "value": input_to_json(shape, input),
            "x": coordinates[i].0,
            "y": coordinates[i].1,
        });
        if let Some(nearest) = nearest(&distances, i) {
            entry["nearest"] = json!({ "index": nearest + 1, "distance": distances[i][nearest] });
        }
        if let Some(generation) = &generation {
            entry["kept_for"] = json!(match generation.corpus.kept_for[i] {
                KeptFor::NewCategories => "new categories",
                KeptFor::Distance => "distance",
            });
            entry["accepted_at"] = json!(generation.corpus.accepted_at[i]);
        }
        entries.push(entry);
    }

    let facts = match &generation {
        Some(Generation {
            corpus,
            config,
            stats,
        }) => json!([
            ["inputs", stats.size],
            ["attempts", stats.attempts],
            ["accepted", format!("{:.1}%", stats.acceptance_rate * 100.0)],
            ["for categories", stats.kept_for_categories],
            ["for distance", stats.kept_for_distance],
            ["near-duplicates", duplicates.len()],
            ["θ", config.theta],
            ["seed", config.seed],
            ["stopped", format!("{:?}", corpus.stop_reason)],
        ]),
        None => {
            let mut coverage = Coverage::default();
            for categories in &input_categories {
                coverage.add(categories);
            }
            let (min, mean) = pair_distances(&distances);
            json!([
                ["inputs", inputs.len()],
                ["categories/pairs covered", coverage.len()],
                ["near-duplicates", duplicates.len()],
                ["min distance", format!("{min:.3}")],
                ["mean distance", format!("{mean:.3}")],
                ["θ", theta],
            ])
        }
    };

    json!({
        "label": label,
        "generated": generation.is_some(),
        "attempts": generation.as_ref().map(|g| g.corpus.attempts),
        "facts": facts,
        "inputs": entries,
        "distances": distances,
        "near_duplicates": near_duplicates_json(&duplicates),
        "model": model(shape, inputs, registry, theta),
    })
}

/// What is being tested: the inputs' JSON Schema, and the shape as a tree.
///
/// Every node of the tree is a part of the input — the whole input, a field,
/// an enum, a list, a list's items, a simple value — with a description of
/// what it is. A node that varies on its own (a simple value, which variant
/// or option is chosen, how many items a list has) lists the parameters that
/// measure it: distance or category, AHP weight, threshold at θ (distances
/// only) and what it captures.
///
/// `share` is the node's part of all the differences between all pairs of
/// `inputs`, summed over everything below it: the root's share is 1 (or 0
/// with fewer than two inputs), and a node's share is its own parameters'
/// shares plus its children's. A distance parameter contributes
/// `weight × distance` for each pair; a category contributes `weight` when
/// the two categories differ. List items are merged (`xs[3]` counts as
/// `xs[]`).
pub fn model(shape: &Shape, inputs: &[Input], registry: &Registry, theta: f64) -> Value {
    let builder = ModelBuilder {
        registry,
        theta,
        shares: leaf_shares(inputs, registry),
    };
    json!({
        "schema": schema_of(shape),
        "tree": builder.node("input", "", shape, "the whole input"),
    })
}

struct ModelBuilder<'a> {
    registry: &'a Registry,
    theta: f64,
    /// (leaf path without indices, parameter) -> share of all differences.
    shares: BTreeMap<(String, String), f64>,
}

impl ModelBuilder<'_> {
    /// One node: `name` as shown, `path` as in the inputs (without list
    /// indices), `what` says what this part is to its parent.
    fn node(&self, name: &str, path: &str, shape: &Shape, what: &str) -> Value {
        // The leaf this node varies on by itself, if any: (what varies, kind, leaf path).
        let own: Option<(&str, Kind, String)> = match shape {
            Shape::Simple(simple) => Some(("the value", Kind::Simple(*simple), path.to_string())),
            Shape::Enum(_) => Some(("which variant is chosen", Kind::Choice, path.to_string())),
            Shape::Union(_) => Some((
                "which alternative is chosen",
                Kind::Choice,
                path.to_string(),
            )),
            Shape::Literal(values) if values.len() > 1 => {
                Some(("which value is chosen", Kind::Choice, path.to_string()))
            }
            Shape::Option(_) => Some(("whether it is null", Kind::Choice, path.to_string())),
            Shape::Optional(_) => Some(("whether it is there", Kind::Choice, path.to_string())),
            Shape::List(..) => Some(("how many items", Kind::Length, child(path, "len"))),
            Shape::Map(..) => Some(("how many entries", Kind::Length, child(path, "len"))),
            Shape::Custom(name) => {
                Some(("the value", Kind::Custom(name.clone()), path.to_string()))
            }
            _ => None,
        };

        let mut params = Vec::new();
        let mut share = 0.0;
        if let Some((_, kind, leaf)) = &own {
            for p in self.registry.distances(kind) {
                let s = self.share(leaf, p.name);
                share += s;
                params.push(json!({
                    "name": p.name,
                    "measure": "distance",
                    "about": about(kind, p.name),
                    "weight": p.weight,
                    "threshold": self.theta * p.threshold_scale,
                    "share": s,
                }));
            }
            for p in self.registry.categories(kind) {
                let s = self.share(leaf, p.name);
                share += s;
                params.push(json!({
                    "name": p.name,
                    "measure": "category",
                    "about": about(kind, p.name),
                    "weight": p.weight,
                    "share": s,
                }));
            }
        }

        let mut children = Vec::new();
        match shape {
            Shape::Fields(fields) => {
                for (field, field_shape) in fields {
                    let what = if path.is_empty() && !is_tuple(fields) {
                        "a parameter or property"
                    } else {
                        "a field"
                    };
                    children.push(self.node(field, &child(path, field), field_shape, what));
                }
            }
            Shape::Enum(variants) => {
                for (variant, fields) in variants.iter().filter(|(_, fields)| !fields.is_empty()) {
                    let variant_path = child(path, variant);
                    let inner = Shape::Fields(fields.clone());
                    let what = format!("the fields of `{variant}`, when chosen");
                    children.push(self.node(variant, &variant_path, &inner, &what));
                }
            }
            Shape::Union(members) => {
                for (label, member) in members {
                    if matches!(member, Shape::Literal(values) if values.len() < 2) {
                        continue; // A constant alternative has nothing below it.
                    }
                    let what = format!("alternative `{label}`, when chosen");
                    children.push(self.node(label, &child(path, label), member, &what));
                }
            }
            Shape::Option(inner) | Shape::Optional(inner) => {
                let inner_path = child(path, "Some");
                children.push(self.node("Some", &inner_path, inner, "the value, when there"));
            }
            Shape::List(element, _) | Shape::Array(element, _) => {
                children.push(self.node("[]", &format!("{path}[]"), element, "each item"));
            }
            Shape::Map(key, value, _) => {
                children.push(self.node("key", &format!("{path}[].key"), key, "each key"));
                children.push(self.node("value", &format!("{path}[].value"), value, "each value"));
            }
            _ => {}
        }
        for node in &children {
            share += node["share"].as_f64().unwrap_or(0.0);
        }

        let note = match shape {
            Shape::Ignored => Some("not generated: a generic type"),
            Shape::Unsupported(_) => Some("not generated or measured: needs an override"),
            Shape::Literal(values) if values.len() < 2 => Some("a constant: never varies"),
            _ => None,
        };
        json!({
            "name": name,
            "path": path,
            "type": describe(shape),
            "what": what,
            "varies": own.as_ref().map(|(varies, _, _)| *varies),
            "params": params,
            "children": children,
            "share": share,
            "note": note,
        })
    }

    fn share(&self, leaf: &str, param: &str) -> f64 {
        let key = (leaf.to_string(), param.to_string());
        self.shares.get(&key).copied().unwrap_or(0.0)
    }
}

/// How much each (leaf, parameter) contributes to the differences between
/// all pairs of inputs, as shares that sum to 1 (empty with no differences).
fn leaf_shares(inputs: &[Input], registry: &Registry) -> BTreeMap<(String, String), f64> {
    let mut totals: BTreeMap<(String, String), f64> = BTreeMap::new();
    for i in 0..inputs.len() {
        for j in (i + 1)..inputs.len() {
            for (path, leaf_a) in &inputs[i] {
                let Some(leaf_b) = inputs[j].get(path) else {
                    continue;
                };
                let leaf = without_indices(path);
                for param in registry.distances(&leaf_a.kind) {
                    let distance = (param.distance)(&leaf_a.kind, &leaf_a.value, &leaf_b.value);
                    *totals
                        .entry((leaf.clone(), param.name.to_string()))
                        .or_insert(0.0) += param.weight * distance;
                }
                for param in registry.categories(&leaf_a.kind) {
                    let a = (param.category)(&leaf_a.kind, &leaf_a.value);
                    let b = (param.category)(&leaf_b.kind, &leaf_b.value);
                    if a != b {
                        *totals
                            .entry((leaf.clone(), param.name.to_string()))
                            .or_insert(0.0) += param.weight;
                    }
                }
            }
        }
    }
    let grand_total: f64 = totals.values().sum();
    if grand_total > 0.0 {
        for total in totals.values_mut() {
            *total /= grand_total;
        }
    }
    totals
}

/// What a built-in parameter captures, in a few words.
fn about(kind: &Kind, param: &str) -> &'static str {
    match (kind.family(), param) {
        (Family::UInt | Family::Int, "magnitude") => {
            "how far apart, on a log scale: 1 vs 2 counts as much as 1000 vs 2000"
        }
        (Family::UInt | Family::Int, "bits") => "how many bits differ",
        (Family::UInt | Family::Int, "boundary") => {
            "which edge case: zero, one, min, max, one step in from either end, or other"
        }
        (Family::Int, "sign") => "negative, zero or positive",
        (Family::Float, "magnitude") => {
            "how far apart: by exponent, or relative to the range when it is limited"
        }
        (Family::Float, "class") => "NaN, infinite, zero, subnormal or normal",
        (Family::Float, "sign") => "negative or positive",
        (Family::Char, "class") => {
            "lowercase, uppercase, digit, punctuation, whitespace, control or non-ASCII"
        }
        (Family::Char, "codepoint") => "how far apart the code points are",
        (Family::Text, "length") => "difference in length, relative to the longer string",
        (Family::Text, "bytes") => "edit distance between the bytes, relative to the longer string",
        (Family::Text, "classes") => {
            "which kinds of characters appear: letters, digits, punctuation, ..."
        }
        (Family::Bool, "value") => "true or false",
        (Family::Choice, "variant") => "which one was chosen",
        (Family::Length, "length") => "difference in count, relative to the larger",
        (Family::Custom(_), _) => "defined by the override",
        _ => "",
    }
}

/// What a part of the input is, e.g. `integer 0..=150` or `enum: Off | Fast`.
fn describe(shape: &Shape) -> String {
    match shape {
        Shape::Simple(simple) => describe_simple(simple),
        Shape::Fields(fields) if fields.is_empty() => "nothing (no fields)".to_string(),
        Shape::Fields(fields) if is_tuple(fields) && fields.len() == 1 => {
            format!("wrapper around {}", describe(&fields[0].1))
        }
        Shape::Fields(fields) if is_tuple(fields) => format!("tuple of {}", fields.len()),
        Shape::Fields(fields) => {
            let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
            format!("{{ {} }}", names.join(", "))
        }
        Shape::Enum(variants) => {
            let names: Vec<&str> = variants.iter().map(|(name, _)| name.as_str()).collect();
            format!("enum: {}", names.join(" | "))
        }
        Shape::Union(members) => {
            let labels: Vec<&str> = members.iter().map(|(label, _)| label.as_str()).collect();
            format!("one of: {}", labels.join(" | "))
        }
        Shape::Literal(values) => {
            let shown: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            if values.len() == 1 {
                format!("constant {}", shown[0])
            } else {
                format!("one of: {}", shown.join(" | "))
            }
        }
        Shape::Option(inner) => format!("{} or null", describe(inner)),
        Shape::Optional(inner) => format!("{}, may be left out", describe(inner)),
        Shape::List(element, len) => format!(
            "list of {}, {}",
            describe(element),
            describe_len(len, "items")
        ),
        Shape::Array(element, len) => format!("array of {len} × {}", describe(element)),
        Shape::Map(_, value, len) => format!(
            "map to {}, {}",
            describe(value),
            describe_len(len, "entries")
        ),
        Shape::Custom(name) => format!("override `{name}`"),
        Shape::Ignored => "generic".to_string(),
        Shape::Unsupported(why) => why.clone(),
    }
}

fn describe_len(len: &Len, unit: &str) -> String {
    match len.max {
        Some(max) => format!("{}..={max} {unit}", len.min),
        None if len.min == 0 => format!("any number of {unit}"),
        None => format!("{} or more {unit}", len.min),
    }
}

fn is_tuple(fields: &[(String, Shape)]) -> bool {
    fields
        .iter()
        .all(|(name, _)| name.chars().all(|c| c.is_ascii_digit()))
}

/// `u8`, `integer 1..=10`, `string of 1..=40 characters`, ...
fn describe_simple(simple: &Simple) -> String {
    const NAMED: [(Primitive, &str); 15] = [
        (Primitive::Bool, "bool"),
        (Primitive::Char, "char"),
        (Primitive::U8, "u8"),
        (Primitive::U16, "u16"),
        (Primitive::U32, "u32"),
        (Primitive::U64, "u64"),
        (Primitive::U128, "u128"),
        (Primitive::I8, "i8"),
        (Primitive::I16, "i16"),
        (Primitive::I32, "i32"),
        (Primitive::I64, "i64"),
        (Primitive::I128, "i128"),
        (Primitive::F32, "f32"),
        (Primitive::F64, "f64"),
        (Primitive::String, "string"),
    ];
    for (primitive, name) in NAMED {
        if Simple::from(primitive) == *simple {
            return name.to_string();
        }
    }
    match *simple {
        Simple::UInt { min, max } => format!("integer {min}..={max}"),
        Simple::Int { min, max } => format!("integer {min}..={max}"),
        Simple::Float {
            min, max, single, ..
        } => {
            let limit = if single { f32::MAX as f64 } else { f64::MAX };
            match (min > -limit, max < limit) {
                (false, false) => "number".to_string(),
                _ => format!("number {min}..={max}"),
            }
        }
        Simple::Text { min_len, max_len } => match max_len {
            Some(max) => format!("string of {min_len}..={max} characters"),
            None => format!("string of {min_len} or more characters"),
        },
        Simple::Bool => "bool".to_string(),
        Simple::Char => "char".to_string(),
    }
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// The input closest to input `i`, if there is another one.
fn nearest(distances: &[Vec<f64>], i: usize) -> Option<usize> {
    (0..distances.len())
        .filter(|&j| j != i)
        .min_by(|&a, &b| distances[i][a].total_cmp(&distances[i][b]))
}

/// The smallest and mean distance over all pairs.
fn pair_distances(distances: &[Vec<f64>]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut total = 0.0;
    let mut pairs = 0;
    for (i, row) in distances.iter().enumerate() {
        for &distance in &row[i + 1..] {
            min = min.min(distance);
            total += distance;
            pairs += 1;
        }
    }
    if pairs == 0 {
        (0.0, 0.0)
    } else {
        (min, total / pairs as f64)
    }
}

/// Weighted mean distance between every pair of inputs (symmetric, 0 on the
/// diagonal).
fn distance_matrix(inputs: &[Input], registry: &Registry) -> Vec<Vec<f64>> {
    let n = inputs.len();
    let mut matrix = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let distance = weighted_mean(&inputs[i], &inputs[j], registry);
            matrix[i][j] = distance;
            matrix[j][i] = distance;
        }
    }
    matrix
}

/// Classical multidimensional scaling: 2-D coordinates whose distances
/// approximate `distances`.
///
/// Square the distances, double-centre them into a matrix B, and use B's two
/// largest eigenvectors (found by power iteration) scaled by the square root
/// of their eigenvalues as the x and y coordinates.
fn classical_mds(distances: &[Vec<f64>]) -> Vec<(f64, f64)> {
    let n = distances.len();
    if n < 2 {
        return vec![(0.0, 0.0); n];
    }

    // B = -1/2 · J · D² · J, where J subtracts row and column means.
    let mut squared = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            squared[i][j] = distances[i][j] * distances[i][j];
        }
    }
    let mut row_means = vec![0.0; n];
    let mut total_mean = 0.0;
    for i in 0..n {
        row_means[i] = squared[i].iter().sum::<f64>() / n as f64;
        total_mean += row_means[i];
    }
    total_mean /= n as f64;
    let mut b = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            b[i][j] = -0.5 * (squared[i][j] - row_means[i] - row_means[j] + total_mean);
        }
    }

    let (value_1, vector_1) = largest_eigenvector(&b);
    // Remove the first component so the second-largest one comes out next.
    for i in 0..n {
        for j in 0..n {
            b[i][j] -= value_1 * vector_1[i] * vector_1[j];
        }
    }
    let (value_2, vector_2) = largest_eigenvector(&b);

    let scale_1 = value_1.max(0.0).sqrt();
    let scale_2 = value_2.max(0.0).sqrt();
    let mut coordinates = Vec::new();
    for i in 0..n {
        coordinates.push((vector_1[i] * scale_1, vector_2[i] * scale_2));
    }
    coordinates
}

/// The largest eigenvalue of a symmetric matrix and its unit eigenvector.
fn largest_eigenvector(matrix: &[Vec<f64>]) -> (f64, Vec<f64>) {
    let n = matrix.len();
    // A fixed, uneven start vector so the result is deterministic.
    let mut vector: Vec<f64> = (0..n).map(|i| 1.0 + (i % 7) as f64 / 10.0).collect();
    let mut value = 0.0;
    for _ in 0..300 {
        let mut next = vec![0.0; n];
        for i in 0..n {
            for j in 0..n {
                next[i] += matrix[i][j] * vector[j];
            }
        }
        let length = next.iter().map(|x| x * x).sum::<f64>().sqrt();
        if length < 1e-12 {
            return (0.0, vec![0.0; n]);
        }
        // Rayleigh quotient: how much the matrix stretches the vector.
        value = 0.0;
        for i in 0..n {
            value += vector[i] * next[i];
        }
        let current_length_squared: f64 = vector.iter().map(|x| x * x).sum();
        value /= current_length_squared;
        vector = next.iter().map(|x| x / length).collect();
    }
    (value, vector)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn mds_recovers_points_on_a_line() {
        // Three points at 0, 1 and 3 on a line.
        let positions = [0.0, 1.0, 3.0];
        let mut distances = vec![vec![0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                distances[i][j] = f64::abs(positions[i] - positions[j]);
            }
        }
        let coordinates = classical_mds(&distances);
        for i in 0..3 {
            for j in 0..3 {
                let (xi, yi) = coordinates[i];
                let (xj, yj) = coordinates[j];
                let recovered = ((xi - xj).powi(2) + (yi - yj).powi(2)).sqrt();
                assert!(close(recovered, distances[i][j]), "{i} {j}: {recovered}");
            }
        }
    }

    #[test]
    fn the_model_is_a_tree_whose_shares_add_up() {
        use crate::overrides::Overrides;
        let schema = json!({
            "type": "object",
            "properties": {
                "age": { "type": "integer", "minimum": 0, "maximum": 150 },
                "tags": { "type": "array", "items": { "type": "string" } },
                "email": { "type": "string", "format": "email" }
            },
            "required": ["age", "tags", "email"]
        });
        let shape = crate::schema::shape_from_schema(&schema, &Overrides::new());
        let registry = Registry::new().unwrap();
        let values = [
            json!({ "age": 0, "tags": [], "email": "a@b.c" }),
            json!({ "age": 150, "tags": ["x", "a long tag"], "email": "a@b.c" }),
            json!({ "age": 40, "tags": ["y"], "email": "a@b.c" }),
        ];
        let inputs = crate::analysis::read_inputs(&shape, &values).unwrap();
        let model = model(&shape, &inputs, &registry, 0.5);
        let tree = &model["tree"];

        let names: Vec<&str> = tree["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["age", "email", "tags"]);

        // `tags` varies by its length, and its items sit below it.
        let tags = &tree["children"][2];
        assert_eq!(tags["varies"], "how many items");
        assert_eq!(tags["params"][0]["name"], "length");
        assert_eq!(tags["children"][0]["path"], "tags[]");
        assert_eq!(tags["children"][0]["type"], "string");

        // Every node's share is its own parameters' plus its children's.
        fn check(node: &Value) -> f64 {
            let own: f64 = node["params"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["share"].as_f64().unwrap())
                .sum();
            let below: f64 = node["children"].as_array().unwrap().iter().map(check).sum();
            assert!((node["share"].as_f64().unwrap() - (own + below)).abs() < 1e-9);
            own + below
        }
        assert!((check(tree) - 1.0).abs() < 1e-9);

        // What can't be measured is said so, and contributes nothing.
        let email = &tree["children"][1];
        assert_eq!(
            email["note"],
            "not generated or measured: needs an override"
        );
        assert_eq!(email["share"], 0.0);
        assert_eq!(model["schema"]["properties"]["age"]["maximum"], 150);
    }

    #[test]
    fn data_is_embedded_safely() {
        let html = render_html(&json!({ "text": "</script><b>" }));
        assert!(!html.contains("\"</script><b>\""));
        assert!(html.contains("<\\/script>"));
    }
}
