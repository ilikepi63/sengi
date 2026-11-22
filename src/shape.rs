//! Step 1: turn a parsed type into a [`Shape`] — what the fuzzer needs to
//! know to generate leaf values for it.
//!
//! A [`TypeNode`] describes a type as written; a shape describes how to build
//! a value of it:
//!
//! | Type | Shape |
//! |---|---|
//! | `u64`, `bool`, `char`, `&str`, `String`, ... | [`Shape::Simple`] |
//! | struct, tuple, a function's parameter list | [`Shape::Fields`] |
//! | enum | [`Shape::Enum`] |
//! | `Option<T>` | [`Shape::Option`] |
//! | `Vec<T>`, `[T]`, `VecDeque`, `HashSet`, ... | [`Shape::List`] |
//! | `[T; N]` | [`Shape::Array`] |
//! | `HashMap<K, V>`, `BTreeMap<K, V>` | [`Shape::Map`] |
//! | `&T`, `Box<T>`, `Rc<T>`, `Arc<T>`, type aliases | the shape of `T` |
//! | a type or path with an override | [`Shape::Custom`] (not broken down) |
//! | generic `T`, `impl Trait` | [`Shape::Ignored`] |
//! | external structs, recursive types, `dyn Trait`, ... | [`Shape::Unsupported`] |
//!
//! Generics are ignored on purpose: there's no concrete type to generate.
//! Unsupported parts are reported by [`Shape::unsupported`] and generate
//! nothing, so the rest of the input is still fuzzed.

use crate::overrides::Overrides;
use crate::{Function, Primitive, TypeDef, TypeNode};

/// What the fuzzer generates for one type.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A simple type: a leaf the fuzzer generates directly.
    Simple(Primitive),
    /// Structs, tuples and a function's parameter list: every field is present.
    Fields(Vec<(String, Shape)>),
    /// One variant is chosen, then that variant's fields are generated.
    Enum(Vec<(String, Vec<(String, Shape)>)>),
    /// Either `None`, or `Some` with an inner value.
    Option(Box<Shape>),
    /// A collection with a random length: `Vec`, slices, sets.
    List(Box<Shape>),
    /// A fixed-length array: `[T; N]`.
    Array(Box<Shape>, usize),
    /// A map with a random number of key/value pairs.
    Map(Box<Shape>, Box<Shape>),
    /// A type handled by an override (the string is the override's name).
    /// The fuzzer asks the override for a whole value; the type is not
    /// broken down any further.
    Custom(String),
    /// Generic parameters and `impl Trait`: deliberately not generated.
    Ignored,
    /// A type we can't generate yet, with a short description of why.
    Unsupported(String),
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
            Shape::List(Box::new(element))
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
        return Shape::Simple(primitive);
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
                Shape::List(Box::new(element))
            }
            ("HashMap" | "BTreeMap", [key, value]) => {
                let key = shape_of(key, &format!("{path}[].key"), overrides);
                let value = shape_of(value, &format!("{path}[].value"), overrides);
                Shape::Map(Box::new(key), Box::new(value))
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
        Shape::Fields(fields) => {
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
        Shape::Option(inner) | Shape::List(inner) | Shape::Array(inner, _) => {
            collect_unsupported(inner, found)
        }
        Shape::Map(key, value) => {
            collect_unsupported(key, found);
            collect_unsupported(value, found);
        }
        Shape::Simple(_) | Shape::Custom(_) | Shape::Ignored => {}
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
                vec![("0".into(), Shape::Simple(Primitive::U8))],
            ),
            (
                "Custom".into(),
                vec![
                    ("level".into(), Shape::Simple(Primitive::I64)),
                    ("label".into(), Shape::Simple(Primitive::Str)),
                ],
            ),
        ]);
        let limits = Shape::Option(Box::new(Shape::Fields(vec![
            ("0".into(), Shape::Simple(Primitive::U8)),
            ("1".into(), Shape::Simple(Primitive::U16)),
        ])));
        assert_eq!(
            *cfg,
            Shape::Fields(vec![
                ("name".into(), Shape::Simple(Primitive::String)),
                ("retries".into(), Shape::Simple(Primitive::U32)),
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
                Box::new(Shape::Simple(Primitive::String)),
                Box::new(Shape::List(Box::new(Shape::Simple(Primitive::F64)))),
            )
        );
        assert_eq!(params[1].1, Shape::Ignored); // impl Fn(u8) -> bool
        assert_eq!(
            params[2].1,
            Shape::Array(Box::new(Shape::Simple(Primitive::Char)), 4)
        );
    }

    #[test]
    fn recursive_types_are_reported_as_unsupported() {
        let shape = shape_of_function("generic");
        assert_eq!(shape.unsupported(), vec!["recursive type `Node`"]);
    }
}
