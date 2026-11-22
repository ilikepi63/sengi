//! A structured, recursive view of a Rust type.
//!
//! [`TypeNode`] is the parser's output for every function input. It keeps the
//! full structure of a type — generic arguments, references, tuples, and the
//! fields of structs and enums defined in the parsed source — so the rest of
//! the crate can break an input down into the simple types it's made of.
//!
//! [`TypeNode::leaves`] flattens a tree into those simple types with paths
//! (`cfg*.mode::Fast.0 -> U8`); [`Primitive`] names the simple types.

use std::fmt;

/// One node in a type tree. Generic arguments, referenced types and the fields
/// of locally defined structs/enums are all children, so the whole thing can be
/// walked with [`TypeNode::walk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeNode {
    /// A named type: `u64`, `String`, `Vec<T>`, `my_mod::Foo<u8>`.
    Path {
        /// The path as written, without generic arguments: `std::vec::Vec`.
        path: String,
        /// The last path segment: `Vec`.
        name: String,
        /// Generic type arguments, in order (`Vec<u8>` -> `[u8]`).
        generics: Vec<TypeNode>,
        /// Associated type bindings, e.g. `Item = u8` in `Iterator<Item = u8>`.
        assoc: Vec<(String, TypeNode)>,
        /// The struct/enum/alias body when it is defined in the parsed source.
        /// `None` for external types such as `String` or `Vec`.
        definition: Option<TypeDef>,
    },
    /// A generic parameter in scope, such as `T` in `fn f<T: Clone>(x: T)`.
    Generic {
        /// The parameter's name: `T`.
        name: String,
        /// Its bounds from `<T: A>` and `where T: B`, e.g. `Clone`, `Into<u64>`.
        bounds: Vec<Bound>,
    },
    /// `&T`, `&'a mut T`.
    Reference {
        /// `'a`, if written.
        lifetime: Option<String>,
        /// `&mut` rather than `&`.
        mutable: bool,
        /// The referenced type.
        inner: Box<TypeNode>,
    },
    /// `*const T` or `*mut T`.
    Pointer {
        /// `*mut` rather than `*const`.
        mutable: bool,
        /// The pointed-to type.
        inner: Box<TypeNode>,
    },
    /// `[T]`.
    Slice(Box<TypeNode>),
    /// `[T; N]`.
    Array {
        /// The element type `T`.
        elem: Box<TypeNode>,
        /// The length expression `N` as written (usually a number).
        len: String,
    },
    /// `(A, B, ...)`; `()` is an empty tuple.
    Tuple(Vec<TypeNode>),
    /// `fn(A, B) -> C`.
    FnPointer {
        /// The argument types.
        inputs: Vec<TypeNode>,
        /// The return type, if any.
        output: Option<Box<TypeNode>>,
    },
    /// `impl A + B` in argument position.
    ImplTrait(Vec<Bound>),
    /// `dyn A + B`.
    TraitObject(Vec<Bound>),
    /// `!`.
    Never,
    /// Anything we don't model in detail (macros, `_`, qualified paths like
    /// `<T as Trait>::Assoc`), kept as source text.
    Other(String),
}

/// The body of a type defined in the parsed source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDef {
    /// A struct's fields (empty for a unit struct).
    Struct(Vec<Field>),
    /// An enum's variants.
    Enum(Vec<Variant>),
    /// `type Alias<T> = Target<T>;` — the target with generics substituted.
    Alias(Box<TypeNode>),
    /// This type is already being expanded further up the tree (e.g. the
    /// `Node` in `struct Node { next: Option<Box<Node>> }`), so it is not
    /// expanded again.
    Recursive,
}

/// One field of a struct or enum variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Field name, or its index (`"0"`, `"1"`) for tuple structs/variants.
    pub name: String,
    /// The field's type, with the containing type's generics substituted.
    pub ty: TypeNode,
}

/// One variant of an enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The variant's name: `Fast`.
    pub name: String,
    /// Its fields; empty for a unit variant like `Off`.
    pub fields: Vec<Field>,
}

/// A bound in `T: Bound`, `impl Bound` or `dyn Bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    /// A trait bound such as `Clone`, `Into<u64>` or `Fn(u8) -> bool`.
    Trait {
        /// The trait's path as written: `std::hash::Hash`.
        path: String,
        /// The last path segment: `Hash`.
        name: String,
        /// `?Sized`
        maybe: bool,
        /// Generic arguments; for `Fn(A, B) -> C` these are the inputs `A, B`.
        generics: Vec<TypeNode>,
        /// Associated type bindings: `Item = u8`.
        assoc: Vec<(String, TypeNode)>,
        /// The `-> C` of `Fn(A, B) -> C`.
        output: Option<Box<TypeNode>>,
    },
    /// A lifetime bound: `'a`.
    Lifetime(String),
    /// Anything else (e.g. `use<..>` precise captures), as source text.
    Other(String),
}

impl TypeNode {
    /// The type's own name where it has one (`Vec`, `T`), otherwise `None`.
    pub fn name(&self) -> Option<&str> {
        match self {
            TypeNode::Path { name, .. } | TypeNode::Generic { name, .. } => Some(name),
            _ => None,
        }
    }

    /// Direct children: generic arguments, pointees, tuple elements, bound
    /// arguments and the field types of a local definition.
    pub fn children(&self) -> Vec<&TypeNode> {
        let mut out = Vec::new();
        match self {
            TypeNode::Path {
                generics,
                assoc,
                definition,
                ..
            } => {
                out.extend(generics);
                out.extend(assoc.iter().map(|(_, t)| t));
                match definition {
                    Some(TypeDef::Struct(fields)) => out.extend(fields.iter().map(|f| &f.ty)),
                    Some(TypeDef::Enum(variants)) => {
                        out.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)))
                    }
                    Some(TypeDef::Alias(target)) => out.push(target),
                    Some(TypeDef::Recursive) | None => {}
                }
            }
            TypeNode::Generic { bounds, .. }
            | TypeNode::ImplTrait(bounds)
            | TypeNode::TraitObject(bounds) => {
                for b in bounds {
                    out.extend(b.children());
                }
            }
            TypeNode::Reference { inner, .. } | TypeNode::Pointer { inner, .. } => out.push(inner),
            TypeNode::Slice(elem) | TypeNode::Array { elem, .. } => out.push(elem),
            TypeNode::Tuple(elems) => out.extend(elems),
            TypeNode::FnPointer { inputs, output } => {
                out.extend(inputs);
                out.extend(output.as_deref());
            }
            TypeNode::Never | TypeNode::Other(_) => {}
        }
        out
    }

    /// Depth-first, pre-order iterator over this node and every node beneath it.
    pub fn walk(&self) -> impl Iterator<Item = &TypeNode> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            // Push in reverse so children come out in source order.
            stack.extend(node.children().into_iter().rev());
            Some(node)
        })
    }

    /// Pretty-print the tree, one node per line, with fields and bounds labelled.
    pub fn tree(&self) -> String {
        let mut out = String::new();
        self.write_tree(&mut out, 0);
        out
    }

    fn write_tree(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        match self {
            TypeNode::Path {
                generics,
                assoc,
                definition,
                ..
            } => {
                let kind = match definition {
                    Some(TypeDef::Struct(_)) => " (struct)",
                    Some(TypeDef::Enum(_)) => " (enum)",
                    Some(TypeDef::Alias(_)) => " (alias)",
                    Some(TypeDef::Recursive) => " (recursive, see above)",
                    None => "",
                };
                out.push_str(&format!("{pad}{self}{kind}\n"));
                if !generics.is_empty() || !assoc.is_empty() {
                    out.push_str(&format!("{pad}  generics:\n"));
                    for g in generics {
                        g.write_tree(out, depth + 2);
                    }
                    for (name, t) in assoc {
                        out.push_str(&format!("{pad}    {name} =\n"));
                        t.write_tree(out, depth + 3);
                    }
                }
                match definition {
                    Some(TypeDef::Struct(fields)) => write_fields(out, fields, depth + 1),
                    Some(TypeDef::Enum(variants)) => {
                        for v in variants {
                            out.push_str(&format!("{pad}  variant {}\n", v.name));
                            write_fields(out, &v.fields, depth + 2);
                        }
                    }
                    Some(TypeDef::Alias(target)) => {
                        out.push_str(&format!("{pad}  =\n"));
                        target.write_tree(out, depth + 2);
                    }
                    Some(TypeDef::Recursive) | None => {}
                }
            }
            TypeNode::Generic { bounds, .. }
            | TypeNode::ImplTrait(bounds)
            | TypeNode::TraitObject(bounds) => {
                let kind = if matches!(self, TypeNode::Generic { .. }) {
                    " (generic)"
                } else {
                    ""
                };
                out.push_str(&format!("{pad}{self}{kind}\n"));
                for b in bounds {
                    let children = b.children();
                    if children.is_empty() {
                        out.push_str(&format!("{pad}  bound {b}\n"));
                    } else {
                        out.push_str(&format!("{pad}  bound {b}:\n"));
                        for c in children {
                            c.write_tree(out, depth + 2);
                        }
                    }
                }
            }
            _ => {
                out.push_str(&format!("{pad}{self}\n"));
                for c in self.children() {
                    c.write_tree(out, depth + 1);
                }
            }
        }
    }
}

fn write_fields(out: &mut String, fields: &[Field], depth: usize) {
    let pad = "  ".repeat(depth);
    for f in fields {
        out.push_str(&format!("{pad}{}:\n", f.name));
        f.ty.write_tree(out, depth + 1);
    }
}

impl Bound {
    /// The types mentioned in this bound: generic arguments, associated type
    /// bindings and `Fn` outputs.
    pub fn children(&self) -> Vec<&TypeNode> {
        match self {
            Bound::Trait {
                generics,
                assoc,
                output,
                ..
            } => generics
                .iter()
                .chain(assoc.iter().map(|(_, t)| t))
                .chain(output.as_deref())
                .collect(),
            Bound::Lifetime(_) | Bound::Other(_) => Vec::new(),
        }
    }
}

fn join<T: fmt::Display>(items: &[T], sep: &str) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(sep)
}

fn angle_args(generics: &[TypeNode], assoc: &[(String, TypeNode)]) -> String {
    let mut parts: Vec<String> = generics.iter().map(ToString::to_string).collect();
    parts.extend(assoc.iter().map(|(n, t)| format!("{n} = {t}")));
    if parts.is_empty() {
        String::new()
    } else {
        format!("<{}>", parts.join(", "))
    }
}

impl fmt::Display for TypeNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeNode::Path {
                path,
                generics,
                assoc,
                ..
            } => write!(f, "{path}{}", angle_args(generics, assoc)),
            TypeNode::Generic { name, .. } => write!(f, "{name}"),
            TypeNode::Reference {
                lifetime,
                mutable,
                inner,
            } => {
                write!(f, "&")?;
                if let Some(lt) = lifetime {
                    write!(f, "{lt} ")?;
                }
                if *mutable {
                    write!(f, "mut ")?;
                }
                write!(f, "{inner}")
            }
            TypeNode::Pointer { mutable, inner } => {
                write!(f, "*{} {inner}", if *mutable { "mut" } else { "const" })
            }
            TypeNode::Slice(elem) => write!(f, "[{elem}]"),
            TypeNode::Array { elem, len } => write!(f, "[{elem}; {len}]"),
            TypeNode::Tuple(elems) if elems.len() == 1 => write!(f, "({},)", elems[0]),
            TypeNode::Tuple(elems) => write!(f, "({})", join(elems, ", ")),
            TypeNode::FnPointer { inputs, output } => {
                write!(f, "fn({})", join(inputs, ", "))?;
                if let Some(out) = output {
                    write!(f, " -> {out}")?;
                }
                Ok(())
            }
            TypeNode::ImplTrait(bounds) => write!(f, "impl {}", join(bounds, " + ")),
            TypeNode::TraitObject(bounds) => write!(f, "dyn {}", join(bounds, " + ")),
            TypeNode::Never => write!(f, "!"),
            TypeNode::Other(s) => write!(f, "{s}"),
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bound::Trait {
                path,
                maybe,
                generics,
                assoc,
                output,
                ..
            } => {
                if *maybe {
                    write!(f, "?")?;
                }
                match output {
                    // Parenthesized `Fn(A) -> B` form.
                    Some(out) => write!(f, "{path}({}) -> {out}", join(generics, ", ")),
                    None => write!(f, "{path}{}", angle_args(generics, assoc)),
                }
            }
            Bound::Lifetime(lt) => write!(f, "{lt}"),
            Bound::Other(s) => write!(f, "{s}"),
        }
    }
}

/// The simple types every input is broken down into. Each one has its own
/// parameter set in the registry (see `params`).
///
/// `std::string::String` and the like are recognised too, not just the bare
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    /// `bool`
    Bool,
    /// `char`
    Char,
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `i128`
    I128,
    /// `isize`
    Isize,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `u128`
    U128,
    /// `usize`
    Usize,
    /// `f32`
    F32,
    /// `f64`
    F64,
    /// `str` (usually reached through `&str`).
    Str,
    /// `String`.
    String,
}

impl Primitive {
    /// Recognises a primitive from its path, with or without a `std::`,
    /// `core::` or `alloc::` prefix.
    fn from_path(path: &str) -> Option<Self> {
        let name = path
            .trim_start_matches("::")
            .trim_start_matches("std::")
            .trim_start_matches("core::")
            .trim_start_matches("alloc::");
        Some(match name {
            "bool" => Primitive::Bool,
            "char" => Primitive::Char,
            "i8" => Primitive::I8,
            "i16" => Primitive::I16,
            "i32" => Primitive::I32,
            "i64" => Primitive::I64,
            "i128" => Primitive::I128,
            "isize" => Primitive::Isize,
            "u8" => Primitive::U8,
            "u16" => Primitive::U16,
            "u32" => Primitive::U32,
            "u64" => Primitive::U64,
            "u128" => Primitive::U128,
            "usize" => Primitive::Usize,
            "f32" => Primitive::F32,
            "f64" => Primitive::F64,
            "str" | "primitive::str" => Primitive::Str,
            "String" | "string::String" => Primitive::String,
            _ => return None,
        })
    }
}

/// One step on the way from a parameter down to a leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStep {
    /// Struct field or tuple-struct index.
    Field(String),
    /// Enum variant (a choice point for combinatorial coverage).
    Variant(String),
    /// Nth generic argument, e.g. `<0>` is the `u8` in `Vec<u8>`.
    GenericArg(usize),
    /// Associated type binding, e.g. `Item` in `Iterator<Item = u8>`.
    Assoc(String),
    /// Tuple element.
    TupleIndex(usize),
    /// Through a reference or pointer.
    Deref,
    /// Element of a slice or array.
    Element,
    /// Through a type alias to its target.
    Alias,
}

impl fmt::Display for PathStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathStep::Field(name) => write!(f, ".{name}"),
            PathStep::Variant(name) => write!(f, "::{name}"),
            PathStep::GenericArg(i) => write!(f, "<{i}>"),
            PathStep::Assoc(name) => write!(f, "<{name}>"),
            PathStep::TupleIndex(i) => write!(f, ".{i}"),
            PathStep::Deref => write!(f, "*"),
            PathStep::Element => write!(f, "[]"),
            PathStep::Alias => Ok(()),
        }
    }
}

/// A node with nothing beneath it, and how to get there from the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf<'a> {
    /// The steps from the input down to this leaf.
    pub path: Vec<PathStep>,
    /// The leaf itself.
    pub node: &'a TypeNode,
}

impl Leaf<'_> {
    /// `Some` if this leaf is a basic type with per-type generator parameters.
    pub fn primitive(&self) -> Option<Primitive> {
        self.node.primitive()
    }

    /// The path rendered relative to a root name, e.g. `cfg.mode::Fast.0`.
    pub fn path_string(&self, root: &str) -> String {
        let mut s = root.to_string();
        for step in &self.path {
            s.push_str(&step.to_string());
        }
        s
    }
}

impl TypeNode {
    /// `Some` if this node is itself a basic type (`u64`, `bool`, `str`, ...).
    pub fn primitive(&self) -> Option<Primitive> {
        match self {
            TypeNode::Path {
                path,
                definition: None,
                ..
            } => Primitive::from_path(path),
            _ => None,
        }
    }

    /// Every leaf under this node with its path. Leaves are primitives, but
    /// also unit structs/variants, recursive references and opaque external
    /// types like `std::path::PathBuf`.
    ///
    /// Generic parameters and `impl Trait` are ignored entirely: they have no
    /// concrete type to generate values for, so they yield no leaves (and
    /// `Vec<T>` yields none either).
    pub fn leaves(&self) -> Vec<Leaf<'_>> {
        let mut out = Vec::new();
        self.collect_leaves(&mut Vec::new(), &mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, path: &mut Vec<PathStep>, out: &mut Vec<Leaf<'a>>) {
        if matches!(self, TypeNode::Generic { .. } | TypeNode::ImplTrait(_)) {
            return;
        }
        // Whether this node has any structure beneath it. If it does, it is
        // not a leaf itself, even when everything beneath was ignored.
        let mut descended = false;
        let mut descend = |step: PathStep, node: &'a TypeNode, out: &mut Vec<Leaf<'a>>| {
            descended = true;
            path.push(step);
            node.collect_leaves(path, out);
            path.pop();
        };
        let before = out.len();
        match self {
            TypeNode::Path {
                generics,
                assoc,
                definition,
                ..
            } => {
                match definition {
                    // A local type's shape is its fields; its generic args are
                    // already substituted into them.
                    Some(TypeDef::Struct(fields)) => {
                        for f in fields {
                            descend(PathStep::Field(f.name.clone()), &f.ty, out);
                        }
                    }
                    Some(TypeDef::Enum(variants)) => {
                        for v in variants {
                            path.push(PathStep::Variant(v.name.clone()));
                            let before_variant = out.len();
                            for f in &v.fields {
                                path.push(PathStep::Field(f.name.clone()));
                                f.ty.collect_leaves(path, out);
                                path.pop();
                            }
                            // Unit variants are still a choice worth recording.
                            if out.len() == before_variant {
                                out.push(Leaf {
                                    path: path.clone(),
                                    node: self,
                                });
                            }
                            path.pop();
                        }
                    }
                    Some(TypeDef::Alias(target)) => descend(PathStep::Alias, target, out),
                    Some(TypeDef::Recursive) => {}
                    // External container like `Vec<u8>`: descend into its args.
                    None => {
                        for (i, g) in generics.iter().enumerate() {
                            descend(PathStep::GenericArg(i), g, out);
                        }
                        for (name, t) in assoc {
                            descend(PathStep::Assoc(name.clone()), t, out);
                        }
                    }
                }
            }
            TypeNode::Reference { inner, .. } | TypeNode::Pointer { inner, .. } => {
                descend(PathStep::Deref, inner, out)
            }
            TypeNode::Slice(elem) | TypeNode::Array { elem, .. } => {
                descend(PathStep::Element, elem, out)
            }
            TypeNode::Tuple(elems) => {
                for (i, e) in elems.iter().enumerate() {
                    descend(PathStep::TupleIndex(i), e, out);
                }
            }
            TypeNode::Generic { .. } | TypeNode::ImplTrait(_) => unreachable!("returned above"),
            TypeNode::FnPointer { .. }
            | TypeNode::TraitObject(_)
            | TypeNode::Never
            | TypeNode::Other(_) => {}
        }
        if !descended && out.len() == before && !matches!(self, TypeNode::Tuple(t) if t.is_empty())
        {
            out.push(Leaf {
                path: path.clone(),
                node: self,
            });
        }
    }
}
