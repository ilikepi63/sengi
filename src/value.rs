//! What a generated input looks like: a flat map from leaf path to leaf value.
//!
//! ```text
//! cfg.name          -> Text("abc")
//! cfg.mode          -> Choice("Fast")
//! cfg.mode.Fast.0   -> UInt(7)
//! cfg.limits        -> Choice("None")
//! xs.len            -> Length(3)
//! xs[0]             -> Char('a')
//! ```
//!
//! Inputs are flat because every comparison happens leaf by leaf: two inputs
//! are compared on the leaves they share (same path).
//!
//! Besides simple values there are two structural kinds of leaf, so that
//! structural differences are measured too:
//!
//! - **choice** leaves record which enum variant, union member or literal,
//!   or `Some`/`None`, was picked. The chosen variant's fields live below it
//!   (`cfg.mode.Fast.0`).
//! - **length** leaves record how many elements a list or map has
//!   (`xs.len`); the elements are `xs[0]`, `xs[1]`, ...
//!
//! A third kind, **custom** leaves, holds a whole value produced by an
//! override (see [`crate::overrides`]); the tree stops there.
//!
//! Without them, two inputs that differ only in variant or length would
//! share no differing leaf and look identical.

use std::collections::BTreeMap;
use std::fmt;

use crate::Primitive;
use crate::shape::Simple;

/// Which kind of leaf this is. It decides which parameters measure it.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A simple value — number, bool, char or string — with its range.
    Simple(Simple),
    /// Which enum variant, union member or literal was chosen, or
    /// `Some`/`None` for an `Option`.
    Choice,
    /// How many elements a collection has.
    Length,
    /// A type handled by an override; the string is the override's name.
    Custom(String),
}

impl From<Primitive> for Kind {
    fn from(primitive: Primitive) -> Kind {
        Kind::Simple(primitive.into())
    }
}

/// A kind without its range: every member of a family shares one parameter
/// set. `u8` and `u64` are both [`Family::UInt`], measured relative to their
/// own ranges.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Family {
    /// `bool`
    Bool,
    /// Unsigned integers.
    UInt,
    /// Signed integers.
    Int,
    /// Floats.
    Float,
    /// `char`
    Char,
    /// Strings.
    Text,
    /// [`Kind::Choice`]
    Choice,
    /// [`Kind::Length`]
    Length,
    /// An override, by name.
    Custom(String),
}

impl Kind {
    /// The family whose parameters measure this kind.
    pub fn family(&self) -> Family {
        match self {
            Kind::Simple(simple) => match simple {
                Simple::Bool => Family::Bool,
                Simple::UInt { .. } => Family::UInt,
                Simple::Int { .. } => Family::Int,
                Simple::Float { .. } => Family::Float,
                Simple::Char => Family::Char,
                Simple::Text { .. } => Family::Text,
            },
            Kind::Choice => Family::Choice,
            Kind::Length => Family::Length,
            Kind::Custom(name) => Family::Custom(name.clone()),
        }
    }
}

/// A generated value at one leaf.
#[derive(Debug, Clone, PartialEq)]
pub enum LeafValue {
    /// A `bool`.
    Bool(bool),
    /// Every signed integer type.
    Int(i128),
    /// Every unsigned integer type.
    UInt(u128),
    /// `f32` and `f64` (an `f32` converts to `f64` exactly).
    Float(f64),
    /// A `char`.
    Char(char),
    /// `str` and `String`.
    Text(String),
    /// The chosen enum variant, or `"Some"` / `"None"`.
    Choice(String),
    /// How many elements a list or map has.
    Length(usize),
    /// A whole value produced by an override, as JSON.
    Custom(serde_json::Value),
}

/// One leaf of a generated input.
#[derive(Debug, Clone, PartialEq)]
pub struct InputLeaf {
    /// Which parameters measure it.
    pub kind: Kind,
    /// The generated value.
    pub value: LeafValue,
}

/// One generated input for a function: every leaf path mapped to its value.
pub type Input = BTreeMap<String, InputLeaf>;

impl fmt::Display for LeafValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeafValue::Bool(value) => write!(f, "{value}"),
            LeafValue::Int(value) => write!(f, "{value}"),
            LeafValue::UInt(value) => write!(f, "{value}"),
            LeafValue::Float(value) => write!(f, "{value:?}"),
            LeafValue::Char(value) => write!(f, "{value:?}"),
            LeafValue::Text(value) => write!(f, "{value:?}"),
            LeafValue::Choice(value) => write!(f, "{value}"),
            LeafValue::Length(value) => write!(f, "{value}"),
            LeafValue::Custom(value) => write!(f, "{value}"),
        }
    }
}

/// `cfg.name = "abc", cfg.retries = 3, ...`
pub fn format_input(input: &Input) -> String {
    let mut parts = Vec::new();
    for (path, leaf) in input {
        parts.push(format!("{path} = {}", leaf.value));
    }
    parts.join(", ")
}
