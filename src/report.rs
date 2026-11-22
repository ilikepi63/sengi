//! An HTML report showing how a corpus's inputs differ from one another.
//!
//! The report has four views:
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
//! - **Growth**: corpus size against attempts; a flat tail means saturation.
//!
//! # Language independence
//!
//! The page is a fixed template (`report.html`) plus one JSON data block made
//! by [`report_data`]. It knows nothing about Rust: a future Python or
//! JavaScript front end that produces the same data (inputs as JSON, a
//! distance matrix, coordinates, contributions, acceptance attempts) gets the
//! same report.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Function;
use crate::corpus::{Corpus, CorpusConfig, KeptFor};
use crate::coverage::without_indices;
use crate::distance::weighted_mean;
use crate::registry::Registry;
use crate::save::input_to_json;
use crate::shape::Shape;
use crate::stats::Stats;
use crate::value::Input;

const TEMPLATE: &str = include_str!("report.html");

/// Writes `<dir>/<function name>.html` and returns its path.
pub fn save_report(
    dir: &Path,
    function: &Function,
    shape: &Shape,
    corpus: &Corpus,
    registry: &Registry,
    config: &CorpusConfig,
    stats: &Stats,
) -> io::Result<PathBuf> {
    let data = report_data(function, shape, corpus, registry, config, stats);
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.html", crate::save::file_name(&function.name)));
    fs::write(&path, render_html(&data))?;
    Ok(path)
}

/// The page: the template with the data block filled in.
pub fn render_html(data: &Value) -> String {
    // `</` inside the JSON would end the <script> element early.
    let json = data.to_string().replace("</", "<\\/");
    let function = data["function"].as_str().unwrap_or("corpus");
    let title = format!("{} corpus", escape_html(function));
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
pub fn report_data(
    function: &Function,
    shape: &Shape,
    corpus: &Corpus,
    registry: &Registry,
    config: &CorpusConfig,
    stats: &Stats,
) -> Value {
    let distances = distance_matrix(&corpus.inputs, registry);
    let coordinates = classical_mds(&distances);

    let mut inputs = Vec::new();
    for (i, input) in corpus.inputs.iter().enumerate() {
        let kept_for = match corpus.kept_for[i] {
            KeptFor::NewCategories => "new categories",
            KeptFor::Distance => "distance",
        };
        inputs.push(json!({
            "value": input_to_json(shape, input),
            "kept_for": kept_for,
            "accepted_at": corpus.accepted_at[i],
            "x": coordinates[i].0,
            "y": coordinates[i].1,
        }));
    }

    let mut params = Vec::new();
    for param in &function.params {
        params.push(json!({ "name": param.name, "type": param.ty }));
    }

    json!({
        "function": function.name,
        "params": params,
        "theta": config.theta,
        "seed": config.seed,
        "stats": stats,
        "stop_reason": format!("{:?}", corpus.stop_reason),
        "inputs": inputs,
        "distances": distances,
        "contributions": contributions(&corpus.inputs, registry),
    })
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
                    let distance = (param.distance)(&leaf_a.value, &leaf_b.value);
                    let key = (
                        input.clone(),
                        leaf.clone(),
                        param.name.to_string(),
                        "distance",
                    );
                    *totals.entry(key).or_insert(0.0) += param.weight * distance;
                }
                for param in registry.categories(&leaf_a.kind) {
                    let a = (param.category)(&leaf_a.value);
                    let b = (param.category)(&leaf_b.value);
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
    fn data_is_embedded_safely() {
        let html = render_html(&json!({ "text": "</script><b>" }));
        assert!(!html.contains("\"</script><b>\""));
        assert!(html.contains("<\\/script>"));
    }
}
