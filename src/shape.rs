//! Step 1: turn an input definition into a [`Shape`] — what the fuzzer needs
//! to know to generate values for it.
//!
//! A shape is sengi's internal description of an input, cut down to what
//! matters for building a value. Every front end produces one: the Rust
//! parser (this module, from a [`TypeNode`]) and JSON Schema
//! ([`crate::schema`]). Everything after this step — generation, parameters,
//! distances, saving, analysis — only sees shapes.
//!
//! | Rust type | JSON Schema | Shape |
//! |---|---|---|
//! | `u64`, `bool`, `char`, `&str`, `String`, ... | `integer`, `number`, `boolean`, `string` | [`Shape::Simple`] |
//! | struct, tuple, a function's parameter list | `object` with `properties`, `prefixItems` | [`Shape::Fields`] |
//! | enum | – | [`Shape::Enum`] |
//! | – | `oneOf` / `anyOf` | [`Shape::Union`] |
//! | – | `enum` / `const` | [`Shape::Literal`] |
//! | `Option<T>` | `anyOf: [T, null]`, `type: [T, "null"]` | [`Shape::Option`] |
//! | – | a property that isn't `required` | [`Shape::Optional`] |
//! | `Vec<T>`, `[T]`, `VecDeque`, `HashSet`, ... | `array` with `items` | [`Shape::List`] |
//! | `[T; N]` | `array` with `minItems` = `maxItems` | [`Shape::Array`] |
//! | `HashMap<K, V>`, `BTreeMap<K, V>` | `object` with `additionalProperties` | [`Shape::Map`] |
//! | `&T`, `Box<T>`, `Rc<T>`, `Arc<T>`, type aliases | `$ref` | the shape of `T` |
//! | a type or path with an override | `x-sengi-override` | [`Shape::Custom`] (not broken down) |
//! | generic `T`, `impl Trait` | – | [`Shape::Ignored`] |
//! | external structs, recursive types, `dyn Trait`, ... | recursive `$ref`, `pattern`, `format`, ... | [`Shape::Unsupported`] |
//!
//! Generics are ignored on purpose: there's no concrete type to generate.
//! Unsupported parts are reported by [`Shape::unsupported`] and generate
//! nothing, so the rest of the input is still fuzzed.

use serde_json::Value;

use crate::overrides::Overrides;
use crate::{Function, Primitive, TypeDef, TypeNode};

/// A simple value the fuzzer generates directly, with the range it must stay
/// in. Rust's simple types are fixed ranges (`u8` is `UInt { min: 0, max:
/// 255 }`); a JSON Schema can narrow them (`"minimum": 1, "maximum": 10`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Simple {
    /// `true` or `false`.
    Bool,
    /// An unsigned integer.
    UInt {
        /// Smallest allowed value.
        min: u128,
        /// Largest allowed value.
        max: u128,
    },
    /// A signed integer.
    Int {
        /// Smallest allowed value.
        min: i128,
        /// Largest allowed value.
        max: i128,
    },
    /// A floating point number.
    Float {
        /// Smallest allowed value.
        min: f64,
        /// Largest allowed value.
        max: f64,
        /// `f32` rather than `f64`.
        single: bool,
        /// Whether NaN and ±∞ may be generated. Rust floats allow them;
        /// JSON numbers can't hold them.
        specials: bool,
    },
    /// A single Unicode character.
    Char,
    /// A string.
    Text {
        /// Fewest characters.
        min_len: usize,
        /// Most characters, if limited.
        max_len: Option<usize>,
    },
}

impl From<Primitive> for Simple {
    fn from(primitive: Primitive) -> Simple {
        fn uint(max: u128) -> Simple {
            Simple::UInt { min: 0, max }
        }
        fn int(min: i128, max: i128) -> Simple {
            Simple::Int { min, max }
        }
        match primitive {
            Primitive::Bool => Simple::Bool,
            Primitive::Char => Simple::Char,
            Primitive::U8 => uint(u8::MAX as u128),
            Primitive::U16 => uint(u16::MAX as u128),
            Primitive::U32 => uint(u32::MAX as u128),
            // `usize`/`isize` are treated as 64-bit.
            Primitive::U64 | Primitive::Usize => uint(u64::MAX as u128),
            Primitive::U128 => uint(u128::MAX),
            Primitive::I8 => int(i8::MIN as i128, i8::MAX as i128),
            Primitive::I16 => int(i16::MIN as i128, i16::MAX as i128),
            Primitive::I32 => int(i32::MIN as i128, i32::MAX as i128),
            Primitive::I64 | Primitive::Isize => int(i64::MIN as i128, i64::MAX as i128),
            Primitive::I128 => int(i128::MIN, i128::MAX),
            Primitive::F32 => Simple::Float {
                min: -f32::MAX as f64,
                max: f32::MAX as f64,
                single: true,
                specials: true,
            },
            Primitive::F64 => Simple::Float {
                min: -f64::MAX,
                max: f64::MAX,
                single: false,
                specials: true,
            },
            Primitive::Str | Primitive::String => Simple::Text {
                min_len: 0,
                max_len: None,
            },
        }
    }
}

/// How many elements a list or map may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Len {
    /// Fewest elements.
    pub min: usize,
    /// Most elements, if limited.
    pub max: Option<usize>,
}

/// What the fuzzer generates for one type.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A simple value: a leaf the fuzzer generates directly.
    Simple(Simple),
    /// Structs, tuples, objects and a function's parameter list: every field
    /// is present (unless it is [`Shape::Optional`]).
    Fields(Vec<(String, Shape)>),
    /// A Rust enum: one variant is chosen, then that variant's fields are
    /// generated. Written in serde's externally tagged layout.
    Enum(Vec<(String, Vec<(String, Shape)>)>),
    /// One of several shapes, each with a label (JSON Schema `oneOf` /
    /// `anyOf`). The value is just the chosen shape's value, untagged.
    Union(Vec<(String, Shape)>),
    /// One of a fixed set of JSON values (JSON Schema `enum` / `const`).
    Literal(Vec<Value>),
    /// Either `None` (written as `null`), or `Some` with an inner value.
    Option(Box<Shape>),
    /// An object property that may be left out entirely.
    Optional(Box<Shape>),
    /// A collection with a random length: `Vec`, slices, sets, arrays.
    List(Box<Shape>, Len),
    /// A fixed-length array: `[T; N]`.
    Array(Box<Shape>, usize),
    /// A map with a random number of key/value pairs.
    Map(Box<Shape>, Box<Shape>, Len),
    /// A type handled by an override (the string is the override's name).
    /// The fuzzer asks the override for a whole value; the type is not
    /// broken down any further.
    Custom(String),
    /// Generic parameters and `impl Trait`: deliberately not generated.
    Ignored,
    /// A type we can't generate yet, with a short description of why.
    Unsupported(String),
}

/// The choice label of a literal value: its JSON text, e.g. `"Off"` or `3`.
pub fn literal_label(value: &Value) -> String {
    value.to_string()
}

/// The shape of a whole function call: one field per parameter.
///
/// Overrides are applied on the way down: when a path or type name has an
/// override, that part becomes [`Shape::Custom`] and isn't broken down
/// further.
pub fn function_shape(function: &Function, overrides: &Overrides) -> Shape {
    let mut fields = Vec::new();
    for param in &function.params {
        fields.push((
            param.name.clone(),
            shape_of(&param.node, &param.name, overrides),
        ));
    }
    Shape::Fields(fields)
}

/// The shape of one type, found at `path` in a function's inputs.
///
/// A path override wins over a type override. Without overrides: references
/// and `Box`/`Rc`/`Arc` are see-through; `Option`, the standard collections
/// and maps get their own variants; structs, enums and aliases defined in the
/// parsed source are expanded. Anything else from outside the source becomes
/// [`Shape::Unsupported`].
pub fn shape_of(node: &TypeNode, path: &str, overrides: &Overrides) -> Shape {
    if let Some(name) = overrides.for_path(path) {
        return Shape::Custom(name.to_string());
    }

    match node {
        TypeNode::Path { name, .. } => match overrides.for_type(name) {
            Some(override_name) => Shape::Custom(override_name.to_string()),
            None => shape_of_path(node, path, overrides),
        },
        // References don't change what values look like.
        TypeNode::Reference { inner, .. } => shape_of(inner, path, overrides),
        TypeNode::Slice(element) => {
            let element = shape_of(element, &format!("{path}[]"), overrides);
            Shape::List(Box::new(element), Len::default())
        }
        TypeNode::Array { elem, len } => match len.parse::<usize>() {
            Ok(len) => {
                let element = shape_of(elem, &format!("{path}[]"), overrides);
                Shape::Array(Box::new(element), len)
            }
            Err(_) => Shape::Unsupported(format!("array with non-literal length `{len}`")),
        },
        TypeNode::Tuple(elements) => {
            let mut fields = Vec::new();
            for (index, element) in elements.iter().enumerate() {
                let name = index.to_string();
                let field = shape_of(element, &child(path, &name), overrides);
                fields.push((name, field));
            }
            Shape::Fields(fields)
        }
        TypeNode::Generic { .. } | TypeNode::ImplTrait(_) => Shape::Ignored,
        other => Shape::Unsupported(other.to_string()),
    }
}

fn shape_of_path(node: &TypeNode, path: &str, overrides: &Overrides) -> Shape {
    let TypeNode::Path {
        name,
        generics,
        definition,
        ..
    } = node
    else {
        unreachable!("only called with paths");
    };

    if let Some(primitive) = node.primitive() {
        return Shape::Simple(primitive.into());
    }

    match definition {
        Some(TypeDef::Struct(fields)) => {
            let mut shapes = Vec::new();
            for field in fields {
                let shape = shape_of(&field.ty, &child(path, &field.name), overrides);
                shapes.push((field.name.clone(), shape));
            }
            Shape::Fields(shapes)
        }
        Some(TypeDef::Enum(variants)) => {
            let mut shapes = Vec::new();
            for variant in variants {
                let variant_path = child(path, &variant.name);
                let mut fields = Vec::new();
                for field in &variant.fields {
                    let shape = shape_of(&field.ty, &child(&variant_path, &field.name), overrides);
                    fields.push((field.name.clone(), shape));
                }
                shapes.push((variant.name.clone(), fields));
            }
            Shape::Enum(shapes)
        }
        Some(TypeDef::Alias(target)) => shape_of(target, path, overrides),
        Some(TypeDef::Recursive) => Shape::Unsupported(format!("recursive type `{name}`")),
        // Not defined in the parsed source: only well-known std types are understood.
        None => match (name.as_str(), generics.as_slice()) {
            ("Option", [inner]) => {
                let inner = shape_of(inner, &child(path, "Some"), overrides);
                Shape::Option(Box::new(inner))
            }
            (
                "Vec" | "VecDeque" | "LinkedList" | "HashSet" | "BTreeSet" | "BinaryHeap",
                [inner],
            ) => {
                let element = shape_of(inner, &format!("{path}[]"), overrides);
                Shape::List(Box::new(element), Len::default())
            }
            ("HashMap" | "BTreeMap", [key, value]) => {
                let key = shape_of(key, &format!("{path}[].key"), overrides);
                let value = shape_of(value, &format!("{path}[].value"), overrides);
                Shape::Map(Box::new(key), Box::new(value), Len::default())
            }
            ("Box" | "Rc" | "Arc", [inner]) => shape_of(inner, path, overrides),
            _ => Shape::Unsupported(node.to_string()),
        },
    }
}

/// Every path in a function's inputs with the type found there, e.g.
/// `("seq", "DnaSequence")`, `("seq.0", "String")`. These are the paths
/// [`Overrides::use_for_path`] accepts; the type names are what
/// [`Overrides::use_for_type`] accepts.
///
/// Paths follow the same rules as the shape: references are see-through,
/// list elements are `[]`, map entries `[].key` / `[].value`, enum variant
/// fields `variant.field`, and the inside of an `Option` is `Some`.
pub fn type_paths(function: &Function) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for param in &function.params {
        collect_type_paths(&param.node, &param.name, &mut found);
    }
    found
}

fn collect_type_paths(node: &TypeNode, path: &str, found: &mut Vec<(String, String)>) {
    match node {
        TypeNode::Reference { inner, .. } => collect_type_paths(inner, path, found),
        TypeNode::Slice(element) | TypeNode::Array { elem: element, .. } => {
            found.push((path.to_string(), node.to_string()));
            collect_type_paths(element, &format!("{path}[]"), found);
        }
        TypeNode::Tuple(elements) => {
            found.push((path.to_string(), node.to_string()));
            for (index, element) in elements.iter().enumerate() {
                collect_type_paths(element, &child(path, &index.to_string()), found);
            }
        }
        TypeNode::Path {
            name,
            generics,
            definition,
            ..
        } => {
            found.push((path.to_string(), node.to_string()));
            match definition {
                Some(TypeDef::Struct(fields)) => {
                    for field in fields {
                        collect_type_paths(&field.ty, &child(path, &field.name), found);
                    }
                }
                Some(TypeDef::Enum(variants)) => {
                    for variant in variants {
                        let variant_path = child(path, &variant.name);
                        for field in &variant.fields {
                            collect_type_paths(
                                &field.ty,
                                &child(&variant_path, &field.name),
                                found,
                            );
                        }
                    }
                }
                Some(TypeDef::Alias(target)) => collect_type_paths(target, path, found),
                Some(TypeDef::Recursive) => {}
                None => match (name.as_str(), generics.as_slice()) {
                    ("Option", [inner]) => collect_type_paths(inner, &child(path, "Some"), found),
                    (
                        "Vec" | "VecDeque" | "LinkedList" | "HashSet" | "BTreeSet" | "BinaryHeap",
                        [inner],
                    ) => collect_type_paths(inner, &format!("{path}[]"), found),
                    ("HashMap" | "BTreeMap", [key, value]) => {
                        collect_type_paths(key, &format!("{path}[].key"), found);
                        collect_type_paths(value, &format!("{path}[].value"), found);
                    }
                    ("Box" | "Rc" | "Arc", [inner]) => collect_type_paths(inner, path, found),
                    _ => {}
                },
            }
        }
        _ => found.push((path.to_string(), node.to_string())),
    }
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

impl Shape {
    /// Descriptions of every part of this shape that can't be generated.
    pub fn unsupported(&self) -> Vec<String> {
        let mut found = Vec::new();
        collect_unsupported(self, &mut found);
        found
    }
}

fn collect_unsupported(shape: &Shape, found: &mut Vec<String>) {
    match shape {
        Shape::Unsupported(why) => found.push(why.clone()),
        Shape::Fields(fields) | Shape::Union(fields) => {
            for (_, field) in fields {
                collect_unsupported(field, found);
            }
        }
        Shape::Enum(variants) => {
            for (_, fields) in variants {
                for (_, field) in fields {
                    collect_unsupported(field, found);
                }
            }
        }
        Shape::Option(inner)
        | Shape::Optional(inner)
        | Shape::List(inner, _)
        | Shape::Array(inner, _) => collect_unsupported(inner, found),
        Shape::Map(key, value, _) => {
            collect_unsupported(key, found);
            collect_unsupported(value, found);
        }
        Shape::Simple(_) | Shape::Literal(_) | Shape::Custom(_) | Shape::Ignored => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function_inputs;

    const FIXTURE: &str = include_str!("../tests/fixtures/complex.rs");

    fn shape_of_function(name: &str) -> Shape {
        let functions = function_inputs(FIXTURE).unwrap();
        let function = functions.iter().find(|f| f.name == name).unwrap();
        function_shape(function, &Overrides::new())
    }

    #[test]
    fn struct_enum_option_and_tuple() {
        let Shape::Fields(params) = shape_of_function("configure") else {
            panic!()
        };
        let (name, cfg) = &params[0];
        assert_eq!(name, "cfg");

        let mode = Shape::Enum(vec![
            ("Off".into(), vec![]),
            (
                "Fast".into(),
                vec![("0".into(), Shape::Simple(Primitive::U8.into()))],
            ),
            (
                "Custom".into(),
                vec![
                    ("level".into(), Shape::Simple(Primitive::I64.into())),
                    ("label".into(), Shape::Simple(Primitive::Str.into())),
                ],
            ),
        ]);
        let limits = Shape::Option(Box::new(Shape::Fields(vec![
            ("0".into(), Shape::Simple(Primitive::U8.into())),
            ("1".into(), Shape::Simple(Primitive::U16.into())),
        ])));
        assert_eq!(
            *cfg,
            Shape::Fields(vec![
                ("name".into(), Shape::Simple(Primitive::String.into())),
                ("retries".into(), Shape::Simple(Primitive::U32.into())),
                ("mode".into(), mode),
                ("limits".into(), limits),
            ])
        );
    }

    #[test]
    fn collections_arrays_and_generics() {
        let Shape::Fields(params) = shape_of_function("scores") else {
            panic!()
        };
        // `Scores` is an alias for `HashMap<String, Vec<f64>>`.
        assert_eq!(
            params[0].1,
            Shape::Map(
                Box::new(Shape::Simple(Primitive::String.into())),
                Box::new(Shape::List(
                    Box::new(Shape::Simple(Primitive::F64.into())),
                    Len::default()
                )),
                Len::default(),
            )
        );
        assert_eq!(params[1].1, Shape::Ignored); // impl Fn(u8) -> bool
        assert_eq!(
            params[2].1,
            Shape::Array(Box::new(Shape::Simple(Primitive::Char.into())), 4)
        );
    }

    #[test]
    fn recursive_types_are_reported_as_unsupported() {
        let shape = shape_of_function("generic");
        assert_eq!(shape.unsupported(), vec!["recursive type `Node`"]);
    }
}
