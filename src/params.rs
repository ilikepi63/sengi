//! Step 2: the parameters of every simple type, and how to measure them.
//!
//! A parameter is one of two things:
//!
//! - a **distance**: a continuous measure normalised to `0.0..=1.0` (0 = the
//!   same, 1 = as different as it gets), e.g. string length. Distances decide
//!   whether an input is far enough from the corpus (θ).
//! - a **category**: a label, e.g. "zero" / "max" / "other". Categories feed
//!   the pairwise coverage check instead of the distance check, because a
//!   0-or-1 difference would pass any θ.
//!
//! Normalising every distance to 0–1 is what makes parameters comparable: a
//! length difference of 1000 and a bit difference of 3 mean nothing next to
//! each other until both are scaled.
//!
//! | Kind | Distances | Categories |
//! |---|---|---|
//! | `bool` | – | value |
//! | unsigned integers | magnitude (log scale), bits (Hamming) | boundary: 0, 1, MAX−1, MAX, other |
//! | signed integers | magnitude (log scale), bits (Hamming) | sign, boundary: 0, 1, −1, MIN, MAX, other |
//! | floats | magnitude (exponent) | class: NaN, infinite, zero, subnormal, normal; sign |
//! | `char` | codepoint | class (see [`crate::char_class`]) |
//! | `str` / `String` | length, bytes (edit distance), classes (Jaccard) | – |
//! | enum variant / `Option` | – | variant |
//! | collection length | length | – |
//!
//! Each set also has an AHP comparison matrix: `comparisons[i][j]` says how
//! much more important parameter `i` is than parameter `j`, on Saaty's 1–9
//! scale (1 = equal, 3 = moderately, 5 = strongly more important). The
//! matrix must be reciprocal: `comparisons[j][i] == 1 / comparisons[i][j]`.
//! The weights it produces are used by [`crate::registry`].

use std::collections::BTreeSet;
use std::rc::Rc;

use crate::Primitive;
use crate::char_class::{CharClass, classify};
use crate::value::{Kind, LeafValue};

/// Distance between two values on one parameter, normalised to `0.0..=1.0`.
pub type DistanceFn = Rc<dyn Fn(&LeafValue, &LeafValue) -> f64>;

/// The category a value falls into on one parameter, e.g. `"max"`.
pub type CategoryFn = Rc<dyn Fn(&LeafValue) -> String>;

/// How a parameter compares two values.
#[derive(Clone)]
pub enum Measure {
    /// A continuous, normalised distance. Used for the θ check.
    Distance(DistanceFn),
    /// A label for each value. Used for pairwise coverage.
    Category(CategoryFn),
}

/// One parameter of a simple type, e.g. a string's `length`.
#[derive(Clone)]
pub struct Param {
    /// Short name used in reports and coverage dimensions.
    pub name: &'static str,
    /// How it compares values.
    pub measure: Measure,
}

/// All parameters of one kind of leaf, plus how important they are
/// relative to each other.
pub struct ParamSet {
    /// The parameters, in the same order as the matrix rows.
    pub params: Vec<Param>,
    /// The AHP pairwise comparison matrix (see [`crate::ahp`]).
    pub comparisons: Vec<Vec<f64>>,
}

/// The built-in parameters for a kind of leaf.
///
/// All widths of integer share one definition, scaled by their bit count.
/// `usize`/`isize` are treated as 64-bit.
pub fn param_set(kind: Kind) -> ParamSet {
    match kind {
        Kind::Choice => choice_params(),
        Kind::Length => length_params(),
        Kind::Custom(name) => {
            panic!("`{name}` is an override; its parameters come from the override")
        }
        Kind::Primitive(primitive) => match primitive {
            Primitive::Bool => bool_params(),
            Primitive::U8 => unsigned_params(8),
            Primitive::U16 => unsigned_params(16),
            Primitive::U32 => unsigned_params(32),
            Primitive::U64 => unsigned_params(64),
            Primitive::U128 => unsigned_params(128),
            Primitive::Usize => unsigned_params(64),
            Primitive::I8 => signed_params(8),
            Primitive::I16 => signed_params(16),
            Primitive::I32 => signed_params(32),
            Primitive::I64 => signed_params(64),
            Primitive::I128 => signed_params(128),
            Primitive::Isize => signed_params(64),
            Primitive::F32 => float_params(127),
            Primitive::F64 => float_params(1023),
            Primitive::Char => char_params(),
            Primitive::Str | Primitive::String => text_params(),
        },
    }
}

fn distance(
    name: &'static str,
    distance: impl Fn(&LeafValue, &LeafValue) -> f64 + 'static,
) -> Param {
    Param {
        name,
        measure: Measure::Distance(Rc::new(distance)),
    }
}

fn category(name: &'static str, category: impl Fn(&LeafValue) -> String + 'static) -> Param {
    Param {
        name,
        measure: Measure::Category(Rc::new(category)),
    }
}

// ---------------------------------------------------------------- bool

fn bool_params() -> ParamSet {
    ParamSet {
        params: vec![category("value", |v| bool_of(v).to_string())],
        comparisons: vec![vec![1.0]],
    }
}

// ---------------------------------------------------------------- unsigned integers

fn unsigned_params(bits: u32) -> ParamSet {
    let max = unsigned_max(bits);
    ParamSet {
        params: vec![
            // How far apart the values are on a log scale, so 1 vs 2 counts
            // as much as 1000 vs 2000.
            distance("magnitude", move |a, b| {
                let a = log2_1p(uint_of(a) as f64);
                let b = log2_1p(uint_of(b) as f64);
                (a - b).abs() / bits as f64
            }),
            // How many bits differ.
            distance("bits", move |a, b| {
                let differing = uint_of(a) ^ uint_of(b);
                differing.count_ones() as f64 / bits as f64
            }),
            // Which edge case it is: 0, 1, MAX - 1, MAX or other.
            category("boundary", move |v| {
                unsigned_boundary(uint_of(v), max).to_string()
            }),
        ],
        #[rustfmt::skip]
        comparisons: vec![
            //                   magnitude  bits  boundary
            /* magnitude */ vec![1.0,       3.0,  1.0 / 2.0],
            /* bits      */ vec![1.0 / 3.0, 1.0,  1.0 / 5.0],
            /* boundary  */ vec![2.0,       5.0,  1.0],
        ],
    }
}

fn unsigned_boundary(value: u128, max: u128) -> &'static str {
    if value == 0 {
        "zero"
    } else if value == 1 {
        "one"
    } else if value == max {
        "max"
    } else if value == max - 1 {
        "max - 1"
    } else {
        "other"
    }
}

// ---------------------------------------------------------------- signed integers

fn signed_params(bits: u32) -> ParamSet {
    let (min, max) = signed_range(bits);
    ParamSet {
        params: vec![
            category("sign", |v| match int_of(v).signum() {
                -1 => "negative".to_string(),
                0 => "zero".to_string(),
                _ => "positive".to_string(),
            }),
            // Distance between the absolute values on a log scale.
            distance("magnitude", move |a, b| {
                let a = log2_1p(int_of(a).unsigned_abs() as f64);
                let b = log2_1p(int_of(b).unsigned_abs() as f64);
                (a - b).abs() / bits as f64
            }),
            // How many bits of the two's complement representation differ.
            distance("bits", move |a, b| {
                let differing = (int_of(a) ^ int_of(b)) as u128 & unsigned_max(bits);
                differing.count_ones() as f64 / bits as f64
            }),
            // Which edge case it is: 0, 1, -1, MIN, MAX or other.
            category("boundary", move |v| {
                signed_boundary(int_of(v), min, max).to_string()
            }),
        ],
        #[rustfmt::skip]
        comparisons: vec![
            //                   sign       magnitude  bits  boundary
            /* sign      */ vec![1.0,       1.0,       3.0,  1.0 / 2.0],
            /* magnitude */ vec![1.0,       1.0,       3.0,  1.0 / 2.0],
            /* bits      */ vec![1.0 / 3.0, 1.0 / 3.0, 1.0,  1.0 / 5.0],
            /* boundary  */ vec![2.0,       2.0,       5.0,  1.0],
        ],
    }
}

fn signed_boundary(value: i128, min: i128, max: i128) -> &'static str {
    if value == 0 {
        "zero"
    } else if value == 1 {
        "one"
    } else if value == -1 {
        "minus one"
    } else if value == min {
        "min"
    } else if value == max {
        "max"
    } else {
        "other"
    }
}

// ---------------------------------------------------------------- floats

/// `max_exponent` is 127 for `f32` and 1023 for `f64`.
fn float_params(max_exponent: i32) -> ParamSet {
    ParamSet {
        params: vec![
            // NaN, infinite, zero, subnormal or normal.
            category("class", |v| float_class(float_of(v)).to_string()),
            category("sign", |v| {
                if float_of(v).is_sign_negative() {
                    "negative".to_string()
                } else {
                    "positive".to_string()
                }
            }),
            // Distance between the exponents. Only meaningful for two normal
            // numbers; any other difference is already covered by `class`.
            distance("magnitude", move |a, b| {
                let (a, b) = (float_of(a), float_of(b));
                if !(a.is_normal() && b.is_normal()) {
                    return 0.0;
                }
                let difference = (a.abs().log2() - b.abs().log2()).abs();
                (difference / (2.0 * max_exponent as f64)).min(1.0)
            }),
        ],
        #[rustfmt::skip]
        comparisons: vec![
            //                   class      sign  magnitude
            /* class     */ vec![1.0,       3.0,  2.0],
            /* sign      */ vec![1.0 / 3.0, 1.0,  1.0 / 2.0],
            /* magnitude */ vec![1.0 / 2.0, 2.0,  1.0],
        ],
    }
}

fn float_class(value: f64) -> &'static str {
    if value.is_nan() {
        "nan"
    } else if value.is_infinite() {
        "infinite"
    } else if value == 0.0 {
        "zero"
    } else if value.is_subnormal() {
        "subnormal"
    } else {
        "normal"
    }
}

// ---------------------------------------------------------------- char

fn char_params() -> ParamSet {
    ParamSet {
        params: vec![
            category("class", |v| format!("{:?}", classify(char_of(v)))),
            distance("codepoint", |a, b| {
                let difference = (char_of(a) as u32).abs_diff(char_of(b) as u32);
                difference as f64 / char::MAX as u32 as f64
            }),
        ],
        #[rustfmt::skip]
        comparisons: vec![
            //                   class      codepoint
            /* class     */ vec![1.0,       3.0],
            /* codepoint */ vec![1.0 / 3.0, 1.0],
        ],
    }
}

// ---------------------------------------------------------------- strings

fn text_params() -> ParamSet {
    ParamSet {
        params: vec![
            // Difference in length, relative to the longer string.
            distance("length", |a, b| {
                relative_difference(text_of(a).chars().count(), text_of(b).chars().count())
            }),
            // Edit distance between the bytes, relative to the longer string.
            distance("bytes", |a, b| {
                let (a, b) = (text_of(a).as_bytes(), text_of(b).as_bytes());
                let longest = a.len().max(b.len());
                if longest == 0 {
                    return 0.0;
                }
                levenshtein(a, b) as f64 / longest as f64
            }),
            // Which kinds of characters appear (Jaccard distance).
            distance("classes", |a, b| {
                let a: BTreeSet<CharClass> = text_of(a).chars().map(classify).collect();
                let b: BTreeSet<CharClass> = text_of(b).chars().map(classify).collect();
                let union = a.union(&b).count();
                if union == 0 {
                    return 0.0;
                }
                let intersection = a.intersection(&b).count();
                1.0 - intersection as f64 / union as f64
            }),
        ],
        #[rustfmt::skip]
        comparisons: vec![
            //                 length     bytes      classes
            /* length  */ vec![1.0,       1.0,       2.0],
            /* bytes   */ vec![1.0,       1.0,       2.0],
            /* classes */ vec![1.0 / 2.0, 1.0 / 2.0, 1.0],
        ],
    }
}

fn levenshtein(a: &[u8], b: &[u8]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, byte_a) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, byte_b) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(byte_a != byte_b);
            let delete = previous[j + 1] + 1;
            let insert = current[j] + 1;
            current.push(substitute.min(delete).min(insert));
        }
        previous = current;
    }
    previous[b.len()]
}

// ---------------------------------------------------------------- choices and lengths

fn choice_params() -> ParamSet {
    ParamSet {
        params: vec![category("variant", |v| choice_of(v).to_string())],
        comparisons: vec![vec![1.0]],
    }
}

fn length_params() -> ParamSet {
    ParamSet {
        params: vec![distance("length", |a, b| {
            relative_difference(length_of(a), length_of(b))
        })],
        comparisons: vec![vec![1.0]],
    }
}

// ---------------------------------------------------------------- small helpers

fn log2_1p(x: f64) -> f64 {
    (1.0 + x).log2()
}

/// `|a - b| / max(a, b)`, or 0 when both are 0.
fn relative_difference(a: usize, b: usize) -> f64 {
    let largest = a.max(b);
    if largest == 0 {
        return 0.0;
    }
    a.abs_diff(b) as f64 / largest as f64
}

fn unsigned_max(bits: u32) -> u128 {
    if bits == 128 {
        u128::MAX
    } else {
        (1 << bits) - 1
    }
}

fn signed_range(bits: u32) -> (i128, i128) {
    if bits == 128 {
        (i128::MIN, i128::MAX)
    } else {
        (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
    }
}

fn bool_of(value: &LeafValue) -> bool {
    match value {
        LeafValue::Bool(v) => *v,
        other => panic!("expected a bool, got {other:?}"),
    }
}

fn int_of(value: &LeafValue) -> i128 {
    match value {
        LeafValue::Int(v) => *v,
        other => panic!("expected a signed integer, got {other:?}"),
    }
}

fn uint_of(value: &LeafValue) -> u128 {
    match value {
        LeafValue::UInt(v) => *v,
        other => panic!("expected an unsigned integer, got {other:?}"),
    }
}

fn float_of(value: &LeafValue) -> f64 {
    match value {
        LeafValue::Float(v) => *v,
        other => panic!("expected a float, got {other:?}"),
    }
}

fn char_of(value: &LeafValue) -> char {
    match value {
        LeafValue::Char(v) => *v,
        other => panic!("expected a char, got {other:?}"),
    }
}

fn text_of(value: &LeafValue) -> &str {
    match value {
        LeafValue::Text(v) => v,
        other => panic!("expected text, got {other:?}"),
    }
}

fn choice_of(value: &LeafValue) -> &str {
    match value {
        LeafValue::Choice(v) => v,
        other => panic!("expected a choice, got {other:?}"),
    }
}

fn length_of(value: &LeafValue) -> usize {
    match value {
        LeafValue::Length(v) => *v,
        other => panic!("expected a length, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance on one named distance parameter of a kind.
    fn distance_on(kind: Kind, name: &str, a: LeafValue, b: LeafValue) -> f64 {
        for param in param_set(kind.clone()).params {
            if let (true, Measure::Distance(distance)) = (param.name == name, &param.measure) {
                return distance(&a, &b);
            }
        }
        panic!("{kind:?} has no distance parameter {name}");
    }

    /// Category of a value on one named category parameter of a kind.
    fn category_of(kind: Kind, name: &str, value: LeafValue) -> String {
        for param in param_set(kind.clone()).params {
            if let (true, Measure::Category(category)) = (param.name == name, &param.measure) {
                return category(&value);
            }
        }
        panic!("{kind:?} has no category parameter {name}");
    }

    fn text(s: &str) -> LeafValue {
        LeafValue::Text(s.to_string())
    }

    const STRING: Kind = Kind::Primitive(Primitive::String);
    const U8: Kind = Kind::Primitive(Primitive::U8);
    const I32: Kind = Kind::Primitive(Primitive::I32);

    #[test]
    fn text_distances() {
        assert_eq!(distance_on(STRING, "length", text("ab"), text("abcd")), 0.5);
        assert_eq!(distance_on(STRING, "length", text(""), text("")), 0.0);
        assert_eq!(
            distance_on(STRING, "bytes", text("kitten"), text("sitting")),
            3.0 / 7.0
        );
        // {Lowercase} vs {Lowercase, Digit}: 1 shared out of 2.
        assert_eq!(distance_on(STRING, "classes", text("abc"), text("a1")), 0.5);
    }

    #[test]
    fn unsigned_parameters() {
        use LeafValue::UInt;
        assert_eq!(distance_on(U8, "bits", UInt(0), UInt(255)), 1.0);
        assert_eq!(distance_on(U8, "magnitude", UInt(0), UInt(255)), 1.0);
        assert_eq!(category_of(U8, "boundary", UInt(5)), "other");
        assert_eq!(category_of(U8, "boundary", UInt(254)), "max - 1");
        assert_eq!(category_of(U8, "boundary", UInt(255)), "max");
    }

    #[test]
    fn signed_parameters() {
        use LeafValue::Int;
        assert_eq!(category_of(I32, "sign", Int(-4)), "negative");
        assert_eq!(category_of(I32, "sign", Int(0)), "zero");
        assert_eq!(category_of(I32, "boundary", Int(i32::MIN as i128)), "min");
        assert_eq!(distance_on(I32, "bits", Int(0), Int(-1)), 1.0);
    }

    #[test]
    fn every_distance_stays_between_zero_and_one() {
        use LeafValue::*;
        let extremes = [
            (Kind::Primitive(Primitive::U128), UInt(0), UInt(u128::MAX)),
            (
                Kind::Primitive(Primitive::I128),
                Int(i128::MIN),
                Int(i128::MAX),
            ),
            (
                Kind::Primitive(Primitive::F64),
                Float(f64::MIN_POSITIVE),
                Float(f64::MAX),
            ),
            (
                Kind::Primitive(Primitive::F64),
                Float(f64::NAN),
                Float(-0.0),
            ),
            (
                Kind::Primitive(Primitive::Char),
                Char('\0'),
                Char(char::MAX),
            ),
            (STRING, text(""), text("a long string")),
            (Kind::Length, Length(0), Length(1000)),
        ];
        for (kind, a, b) in extremes {
            for param in param_set(kind.clone()).params {
                if let Measure::Distance(distance) = &param.measure {
                    let d = distance(&a, &b);
                    assert!((0.0..=1.0).contains(&d), "{kind:?} {}: {d}", param.name);
                }
            }
        }
    }
}
