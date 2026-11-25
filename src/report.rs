//! An HTML report showing how a set of inputs differ from one another —
//! a generated corpus, or inputs from anywhere ([`crate::analysis`]).
//!
//! The report has these views:
//!
//! - **Tested model**: the inputs' JSON Schema, and every leaf with the
//!   parameters derived from it (distance or category, AHP weight,
//!   threshold at θ).
//!
//! - **Distance heatmap**: every pair of inputs, coloured by their weighted
//!   mean distance. Light cells off the diagonal are near-duplicates.
//! - **Corpus map**: the inputs laid out in 2-D so that the distance between
//!   dots approximates the distance between inputs (classical
//!   multidimensional scaling). An even cloud means a well-spread corpus.
//! - **What makes inputs different**: an icicle chart of input → leaf →
//!   parameter, each block as wide as its share of all differences between
//!   all pairs (AHP weight × distance, categories counting 1 when they
//!   differ).
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
//! distance matrix, coordinates, contributions, acceptance attempts) gets the
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
use crate::shape::{Shape, Simple};
use crate::stats::Stats;
use crate::value::{Input, Kind};

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
        "contributions": contributions(inputs, registry),
        "near_duplicates": near_duplicates_json(&duplicates),
        "model": model(shape, registry, theta),
    })
}

/// What is being tested: the inputs' JSON Schema, and every leaf of the
/// shape with the parameters that measure it — distance or category, AHP
/// weight, and threshold at θ (distances only).
pub fn model(shape: &Shape, registry: &Registry, theta: f64) -> Value {
    let mut leaves = Vec::new();
    collect_leaves(shape, "", registry, theta, &mut leaves);
    json!({ "schema": schema_of(shape), "leaves": leaves })
}

fn collect_leaves(
    shape: &Shape,
    path: &str,
    registry: &Registry,
    theta: f64,
    out: &mut Vec<Value>,
) {
    let shown = if path.is_empty() { "(input)" } else { path };
    let mut leaf = |at: &str, ty: String, kind: Option<Kind>, note: Option<String>| {
        let mut params = Vec::new();
        if let Some(kind) = &kind {
            for p in registry.distances(kind) {
                params.push(json!({
                    "name": p.name,
                    "measure": "distance",
                    "weight": p.weight,
                    "threshold": theta * p.threshold_scale,
                }));
            }
            for p in registry.categories(kind) {
                params.push(json!({ "name": p.name, "measure": "category", "weight": p.weight }));
            }
        }
        out.push(json!({ "path": at, "type": ty, "params": params, "note": note }));
    };
    match shape {
        Shape::Simple(simple) => leaf(
            shown,
            describe_simple(simple),
            Some(Kind::Simple(*simple)),
            None,
        ),
        Shape::Fields(fields) => {
            for (name, field) in fields {
                collect_leaves(field, &child(path, name), registry, theta, out);
            }
        }
        Shape::Enum(variants) => {
            let names: Vec<&str> = variants.iter().map(|(name, _)| name.as_str()).collect();
            leaf(
                shown,
                format!("variant: {}", names.join(" | ")),
                Some(Kind::Choice),
                None,
            );
            for (variant, fields) in variants {
                for (name, field) in fields {
                    let at = child(&child(path, variant), name);
                    collect_leaves(field, &at, registry, theta, out);
                }
            }
        }
        Shape::Union(members) => {
            let labels: Vec<&str> = members.iter().map(|(label, _)| label.as_str()).collect();
            leaf(
                shown,
                format!("one of: {}", labels.join(" | ")),
                Some(Kind::Choice),
                None,
            );
            for (label, member) in members {
                collect_leaves(member, &child(path, label), registry, theta, out);
            }
        }
        Shape::Literal(values) => {
            let shown_values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            if values.len() > 1 {
                leaf(
                    shown,
                    format!("one of: {}", shown_values.join(" | ")),
                    Some(Kind::Choice),
                    None,
                );
            } else {
                let note = Some("a constant: nothing to vary".to_string());
                leaf(
                    shown,
                    format!("constant {}", shown_values.join("")),
                    None,
                    note,
                );
            }
        }
        Shape::Option(inner) | Shape::Optional(inner) => {
            let ty = if matches!(shape, Shape::Option(_)) {
                "null or a value"
            } else {
                "present or left out"
            };
            leaf(shown, ty.to_string(), Some(Kind::Choice), None);
            collect_leaves(inner, &child(path, "Some"), registry, theta, out);
        }
        Shape::List(element, len) => {
            let range = match len.max {
                Some(max) => format!("{}..={max} items", len.min),
                None => format!("{} or more items", len.min),
            };
            leaf(
                &child(path, "len"),
                format!("list length, {range}"),
                Some(Kind::Length),
                None,
            );
            collect_leaves(element, &format!("{path}[]"), registry, theta, out);
        }
        Shape::Array(element, _) => {
            collect_leaves(element, &format!("{path}[]"), registry, theta, out)
        }
        Shape::Map(key, value, len) => {
            let range = match len.max {
                Some(max) => format!("{}..={max} entries", len.min),
                None => format!("{} or more entries", len.min),
            };
            leaf(
                &child(path, "len"),
                format!("map size, {range}"),
                Some(Kind::Length),
                None,
            );
            collect_leaves(key, &format!("{path}[].key"), registry, theta, out);
            collect_leaves(value, &format!("{path}[].value"), registry, theta, out);
        }
        Shape::Custom(name) => leaf(
            shown,
            format!("override `{name}`"),
            Some(Kind::Custom(name.clone())),
            None,
        ),
        Shape::Ignored => leaf(
            shown,
            "generic".to_string(),
            None,
            Some("not generated".to_string()),
        ),
        Shape::Unsupported(why) => leaf(
            shown,
            why.clone(),
            None,
            Some("not generated: needs an override".to_string()),
        ),
    }
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

/// How much each (input, leaf, parameter) contributes to the differences
/// between all pairs of inputs, as shares that sum to 1.
///
/// A distance parameter contributes `weight × distance`; a category
/// parameter contributes `weight` when the two categories differ. List
/// elements are merged (`xs[3]` counts as `xs[]`).
fn contributions(inputs: &[Input], registry: &Registry) -> Vec<Value> {
    // (input, leaf, parameter, "distance" | "category") -> total
    let mut totals: BTreeMap<(String, String, String, &str), f64> = BTreeMap::new();

    for i in 0..inputs.len() {
        for j in (i + 1)..inputs.len() {
            for (path, leaf_a) in &inputs[i] {
                let Some(leaf_b) = inputs[j].get(path) else {
                    continue;
                };
                let leaf = without_indices(path);
                let input = top_level_name(&leaf);

                for param in registry.distances(&leaf_a.kind) {
                    let distance = (param.distance)(&leaf_a.kind, &leaf_a.value, &leaf_b.value);
                    let key = (
                        input.clone(),
                        leaf.clone(),
                        param.name.to_string(),
                        "distance",
                    );
                    *totals.entry(key).or_insert(0.0) += param.weight * distance;
                }
                for param in registry.categories(&leaf_a.kind) {
                    let a = (param.category)(&leaf_a.kind, &leaf_a.value);
                    let b = (param.category)(&leaf_b.kind, &leaf_b.value);
                    if a != b {
                        let key = (
                            input.clone(),
                            leaf.clone(),
                            param.name.to_string(),
                            "category",
                        );
                        *totals.entry(key).or_insert(0.0) += param.weight;
                    }
                }
            }
        }
    }

    let grand_total: f64 = totals.values().sum();
    let mut result = Vec::new();
    for ((input, leaf, parameter, kind), total) in totals {
        if total <= 0.0 {
            continue;
        }
        result.push(json!({
            "input": input,
            "leaf": leaf,
            "parameter": parameter,
            "kind": kind,
            "share": total / grand_total,
        }));
    }
    result
}

/// `cfg.mode.Fast.0` -> `cfg`, `xs[].key` -> `xs`.
fn top_level_name(path: &str) -> String {
    let end = path.find(['.', '[']).unwrap_or(path.len());
    path[..end].to_string()
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
    fn top_level_names() {
        assert_eq!(top_level_name("cfg.mode.Fast.0"), "cfg");
        assert_eq!(top_level_name("xs[].key"), "xs");
        assert_eq!(top_level_name("left"), "left");
    }

    #[test]
    fn the_model_lists_every_leaf_with_its_parameters() {
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
        let model = model(&shape, &registry, 0.5);

        let rows: Vec<(String, String)> = model["leaves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|leaf| {
                (
                    leaf["path"].as_str().unwrap().into(),
                    leaf["type"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("age".into(), "integer 0..=150".into()),
                ("email".into(), "a string with format `email`".into()),
                ("tags.len".into(), "list length, 0 or more items".into()),
                ("tags[]".into(), "string".into()),
            ]
        );
        let age: Vec<&str> = model["leaves"][0]["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(age, ["magnitude", "bits", "boundary"]);
        assert_eq!(model["leaves"][0]["params"][0]["threshold"], 0.5);
        assert!(model["leaves"][1]["params"].as_array().unwrap().is_empty());
        assert_eq!(model["schema"]["properties"]["age"]["maximum"], 150);
    }

    #[test]
    fn data_is_embedded_safely() {
        let html = render_html(&json!({ "text": "</script><b>" }));
        assert!(!html.contains("\"</script><b>\""));
        assert!(html.contains("<\\/script>"));
    }
}
