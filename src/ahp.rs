//! Step 3a: the Analytic Hierarchy Process — turn a pairwise comparison
//! matrix into weights, and check the comparisons are consistent.
//!
//! Choosing weights directly ("length 0.4, bytes 0.4, classes 0.2") is hard
//! to get right. AHP asks only for pairwise judgements ("length is twice as
//! important as classes"), derives the weights from them, and reports how
//! consistent the judgements were.

/// The weights derived from one comparison matrix, and how consistent it was.
pub struct AhpResult {
    /// One weight per parameter, summing to 1.
    pub weights: Vec<f64>,
    /// Above 0.1 means the comparisons contradict each other too much.
    pub consistency_ratio: f64,
}

/// Saaty's random index: the average consistency index of random matrices,
/// by matrix size (index 0 is unused).
const RANDOM_INDEX: [f64; 11] = [
    0.0, 0.0, 0.0, 0.58, 0.90, 1.12, 1.24, 1.32, 1.41, 1.45, 1.49,
];

/// Derive weights from a pairwise comparison matrix.
///
/// `matrix[i][j]` says how much more important item `i` is than item `j`
/// (Saaty's 1–9 scale), and the matrix must be reciprocal
/// (`matrix[j][i] == 1 / matrix[i][j]`). The weights are the matrix's
/// principal eigenvector, normalised to sum to 1.
///
/// The consistency ratio measures how much the comparisons contradict each
/// other: if A is 2× B and B is 2× C, then A should be 4× C. Saaty's rule of
/// thumb is that a ratio above 0.1 means the matrix should be revised.
///
/// ```
/// use sengi::ahp::ahp;
///
/// // A is twice as important as B.
/// let result = ahp(&[vec![1.0, 2.0], vec![0.5, 1.0]]);
/// assert!((result.weights[0] - 2.0 / 3.0).abs() < 1e-9);
/// ```
pub fn ahp(matrix: &[Vec<f64>]) -> AhpResult {
    let n = matrix.len();

    // The weights are the principal eigenvector; power iteration finds it.
    let mut weights = vec![1.0 / n as f64; n];
    for _ in 0..100 {
        let next = multiply(matrix, &weights);
        let total: f64 = next.iter().sum();
        weights = next.iter().map(|w| w / total).collect();
    }

    // λ_max: how much the matrix stretches the eigenvector. For perfectly
    // consistent comparisons it equals n.
    let stretched = multiply(matrix, &weights);
    let mut lambda_max = 0.0;
    for i in 0..n {
        lambda_max += stretched[i] / weights[i];
    }
    lambda_max /= n as f64;

    let consistency_ratio = if n <= 2 {
        0.0 // One or two parameters can't be inconsistent.
    } else {
        let consistency_index = (lambda_max - n as f64) / (n as f64 - 1.0);
        consistency_index / RANDOM_INDEX[n.min(10)]
    };

    AhpResult {
        weights,
        consistency_ratio,
    }
}

fn multiply(matrix: &[Vec<f64>], vector: &[f64]) -> Vec<f64> {
    let mut result = Vec::new();
    for row in matrix {
        let mut sum = 0.0;
        for (a, b) in row.iter().zip(vector) {
            sum += a * b;
        }
        result.push(sum);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn consistent_matrix_gives_exact_weights() {
        // A is 2x as important as B, B is 2x as important as C.
        let matrix = vec![
            vec![1.0, 2.0, 4.0],
            vec![0.5, 1.0, 2.0],
            vec![0.25, 0.5, 1.0],
        ];
        let result = ahp(&matrix);
        assert!(close(result.weights[0], 4.0 / 7.0));
        assert!(close(result.weights[1], 2.0 / 7.0));
        assert!(close(result.weights[2], 1.0 / 7.0));
        assert!(result.consistency_ratio.abs() < 1e-9);
    }

    #[test]
    fn contradictory_matrix_is_inconsistent() {
        // A > B > C > A: a cycle.
        let matrix = vec![
            vec![1.0, 9.0, 1.0 / 9.0],
            vec![1.0 / 9.0, 1.0, 9.0],
            vec![9.0, 1.0 / 9.0, 1.0],
        ];
        assert!(ahp(&matrix).consistency_ratio > 0.1);
    }
}
