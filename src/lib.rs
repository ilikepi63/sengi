//! Generate maximally different test inputs — for Rust functions or
//! anything described by a JSON Schema — and analyse sets of inputs you
//! already have.
//!
//! sengi works out what an input is made of, fuzzes inputs, and keeps only
//! the ones that are meaningfully different from everything kept so far. The
//! kept inputs are the *corpus*. What a test does with them (assertions,
//! snapshots, ...) is left entirely to the user: this crate only produces and
//! measures inputs.
//!
//! # Pipeline
//!
//! One module per step:
//!
//! 0. **Front ends** turn an input definition into a [`shape::Shape`]:
//!    - Rust source ([`function_inputs`], `types`, `resolve`, then
//!      [`shape::function_shape`]): find every function and build the full
//!      [`TypeNode`] tree of each input — structs and enums defined in the
//!      file are expanded, generics are substituted;
//!    - JSON Schema ([`schema`]), which needs no source code at all.
//! 1. **Shape** ([`shape`]): what the fuzzer must know — simple values with
//!    their ranges, fields, choices (enum variants, unions, literals,
//!    `Option`) and collections. Everything below only sees shapes.
//! 2. **Parameters** ([`params`]): every simple type has a fixed set of
//!    parameters (a string has a length, its bytes, its character classes;
//!    an integer has a magnitude, its bits, its boundary class, ...). Each
//!    parameter is either a normalised *distance* or a *category*.
//! 3. **Weights** ([`ahp`], [`registry`]): the parameters of each type are
//!    weighted with the Analytic Hierarchy Process from a pairwise comparison
//!    matrix, and inconsistent matrices are rejected.
//! 4. **Comparing inputs** ([`distance`], [`coverage`]): two inputs are
//!    compared leaf by leaf. Distance parameters are checked against a
//!    threshold θ; category parameters are tracked as pairwise coverage.
//! 5. **Building the corpus** ([`fuzz`], [`corpus`]): leaf values are
//!    generated directly from the shape, and a candidate is kept if it covers
//!    a new category (or pair of categories) or is clearly different from
//!    every input already kept.
//!
//! [`overrides`] replace generation and parameters for specific types or
//! paths (e.g. a DNA sequence that must only contain `ACGT`); the type is then
//! a single leaf and isn't broken down further.
//!
//! [`value`] describes generated inputs, [`stats`] summarises a corpus,
//! [`save`] writes it to JSON (and reads inputs back), [`analysis`] measures
//! any set of inputs and [`report`] draws either as an HTML page.
//!
//! # Key design decisions
//!
//! - **Inputs are generated leaf by leaf.** A generated input is a flat map
//!   from leaf path (`cfg.mode.Fast.0`) to value, so every simple value is
//!   directly available to the distance calculations.
//! - **Distances are never averaged before deciding.** Averaging over many
//!   leaves hides one large difference (one very different string among 20
//!   equal numbers averages out to almost nothing). Instead a candidate is
//!   kept if *some* parameter of *some* leaf differs by at least that
//!   parameter's threshold — see [`distance`].
//! - **Yes/no differences are coverage, not distance.** A category mismatch
//!   would score 1 and pass any θ, letting the corpus explode with category
//!   combinations. Categories are instead covered pairwise, which is bounded
//!   — see [`coverage`].
//! - **AHP weights become thresholds.** The most important distance parameter
//!   of a type must differ by θ; less important ones need proportionally
//!   more — see [`registry`].
//!
//! # Example
//!
//! ```no_run
//! use sengi::corpus::{CorpusConfig, build_corpus};
//! use sengi::overrides::Overrides;
//! use sengi::registry::Registry;
//! use sengi::schema::shape_from_schema;
//! use sengi::shape::function_shape;
//!
//! let overrides = Overrides::new();
//! let registry = Registry::with_overrides(&overrides).unwrap();
//! let config = CorpusConfig::default();
//!
//! // From Rust source...
//! let src = std::fs::read_to_string("src/lib.rs").unwrap();
//! for function in &sengi::function_inputs(&src).unwrap() {
//!     let shape = function_shape(function, &overrides);
//!     let corpus = build_corpus(&shape, &registry, &overrides, &config);
//!     println!("{}: {} inputs", function.name, corpus.inputs.len());
//! }
//!
//! // ...or from a JSON Schema.
//! let schema = serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 10 });
//! let shape = shape_from_schema(&schema, &overrides);
//! let corpus = build_corpus(&shape, &registry, &overrides, &config);
//! ```

#![warn(missing_docs)]

pub mod ahp;
pub mod analysis;
pub mod char_class;
pub mod corpus;
pub mod coverage;
pub mod distance;
pub mod fuzz;
pub mod overrides;
pub mod params;
pub mod registry;
pub mod report;
mod resolve;
pub mod rng;
pub mod save;
pub mod schema;
pub mod shape;
pub mod stats;
mod types;
pub mod value;

use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::{
    FnArg, ImplItemFn, ItemFn, ItemImpl, ItemTrait, Receiver, ReceiverKind, Signature, TraitItemFn,
    Type,
};

use resolve::{Definitions, Resolver, Scope};
pub use types::{Bound, Field, Leaf, PathStep, Primitive, TypeDef, TypeNode, Variant};

/// One function parameter: its pattern (e.g. `left`), its type as written
/// (e.g. `Vec<u8>`) and the fully resolved type tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// The parameter's pattern as written: `left`, `self`, or a
    /// destructuring pattern such as `(a , b)`.
    pub name: String,
    /// The type as written in the source, e.g. `&str` or `Wrapper<bool>`.
    pub ty: String,
    /// The resolved type tree: local types expanded, generics substituted,
    /// `Self` replaced by the impl's type.
    pub node: TypeNode,
}

/// A function, method or trait method found in the source, with its inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// The function's name, prefixed with its `impl` type or trait for
    /// methods: `add`, `Config::new`, `Wrapper<T>::set`.
    pub name: String,
    /// The inputs in order, including `self` for methods.
    pub params: Vec<Param>,
}

/// Walks a parsed file and records every function it finds.
///
/// It keeps track of where it is — inside which `impl` or `trait`, and which
/// generic parameters are in scope — so each parameter type can be resolved
/// correctly.
struct FnCollector<'a, 'ast> {
    /// Turns `syn` types into [`TypeNode`] trees.
    resolver: Resolver<'a, 'ast>,
    /// Enclosing `impl`/`trait` names so methods get a `Type::` prefix.
    owners: Vec<String>,
    /// Generic parameters and `Self` visible at the current point.
    scopes: Vec<Scope<'ast>>,
    /// Everything recorded so far, in source order.
    functions: Vec<Function>,
}

impl<'ast> FnCollector<'_, 'ast> {
    /// The innermost scope: the generics and `Self` visible right now.
    fn scope(&self) -> &Scope<'ast> {
        self.scopes.last().expect("root scope is never popped")
    }

    /// Records one function signature, resolving each input's type in the
    /// current scope plus the function's own generic parameters.
    fn record(&mut self, sig: &'ast Signature) {
        let mut name = sig.ident.to_string();
        if let Some(owner) = self.owners.last() {
            name = format!("{owner}::{name}");
        }

        let scope = self.scope().with_generics(&sig.generics);
        let params = sig
            .inputs
            .iter()
            .map(|arg| match arg {
                FnArg::Receiver(recv) => Param {
                    name: "self".to_string(),
                    ty: receiver_type(recv),
                    node: self.receiver_node(recv, &scope),
                },
                FnArg::Typed(pat_ty) => Param {
                    name: pat_ty.pat.to_token_stream().to_string(),
                    ty: type_to_string(&pat_ty.ty),
                    node: self.resolver.convert(&pat_ty.ty, &scope),
                },
            })
            .collect();

        self.functions.push(Function { name, params });
    }

    /// The type tree of a `self` parameter, with `Self` replaced by the
    /// `impl` type (`&mut self` in `impl Wrapper<T>` becomes `&mut Wrapper<T>`).
    fn receiver_node(&mut self, recv: &'ast Receiver, scope: &Scope<'ast>) -> TypeNode {
        match &recv.kind {
            ReceiverKind::Value => scope.self_type(),
            ReceiverKind::Reference(_, lifetime, mutability) => TypeNode::Reference {
                lifetime: lifetime.as_ref().map(ToString::to_string),
                mutable: mutability.is_some(),
                inner: Box::new(scope.self_type()),
            },
            ReceiverKind::Typed(_, ty) => self.resolver.convert(ty, scope),
            _ => TypeNode::Other(recv.to_token_stream().to_string()),
        }
    }
}

/// `syn`'s visitor calls these for every item in the file, including items
/// nested inside modules and function bodies.
impl<'ast> Visit<'ast> for FnCollector<'_, 'ast> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.record(&node.sig);
        visit::visit_item_fn(self, node);
    }

    /// Inside an `impl`, methods are prefixed with the type and `Self` means
    /// that type; the impl's own generics are in scope.
    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        let scope = self.scope().with_generics(&node.generics);
        let self_node = self.resolver.convert(&node.self_ty, &scope);
        self.scopes.push(scope.with_self(Some(self_node)));
        self.owners.push(type_to_string(&node.self_ty));
        visit::visit_item_impl(self, node);
        self.owners.pop();
        self.scopes.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.record(&node.sig);
        visit::visit_impl_item_fn(self, node);
    }

    /// Inside a `trait`, methods are prefixed with the trait name and `Self`
    /// stays an unknown generic.
    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        let scope = self.scope().with_generics(&node.generics).with_self(None);
        self.scopes.push(scope);
        self.owners.push(node.ident.to_string());
        visit::visit_item_trait(self, node);
        self.owners.pop();
        self.scopes.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        self.record(&node.sig);
        visit::visit_trait_item_fn(self, node);
    }
}

impl Function {
    /// Just the parameter type names, in order.
    pub fn input_types(&self) -> Vec<&str> {
        self.params.iter().map(|p| p.ty.as_str()).collect()
    }
}

/// Parse a Rust source file and return every function with its inputs.
///
/// Free functions, `impl` methods and trait methods are all included, in
/// source order. Structs, enums and type aliases defined anywhere in the same
/// file are expanded inside the input types; types from other files or crates
/// are left as named, unexpanded paths.
///
/// ```
/// let functions = sengi::function_inputs("fn add(a: u64, b: &str) {}").unwrap();
/// assert_eq!(functions[0].name, "add");
/// assert_eq!(functions[0].input_types(), ["u64", "&str"]);
/// ```
pub fn function_inputs(src: &str) -> syn::Result<Vec<Function>> {
    let ast = syn::parse_file(src)?;
    let defs = Definitions::collect(&ast);
    let mut collector = FnCollector {
        resolver: Resolver::new(&defs),
        owners: Vec::new(),
        scopes: vec![Scope::default()],
        functions: Vec::new(),
    };
    collector.visit_file(&ast);
    Ok(collector.functions)
}

/// A struct, enum or type alias defined in the parsed source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedType {
    /// The type's name, e.g. `DnaSequence`. This is the name
    /// [`overrides::Overrides::use_for_type`] matches.
    pub name: String,
    /// Whether it is a struct, enum or alias.
    pub kind: ParsedTypeKind,
    /// The definition, formatted, e.g. `pub struct DnaSequence(String);`.
    pub source: String,
}

/// What kind of definition a [`ParsedType`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedTypeKind {
    /// `struct`
    Struct,
    /// `enum`
    Enum,
    /// `type Alias = ...;`
    Alias,
}

/// Every struct, enum and type alias defined in a Rust source file, in source
/// order, including those inside modules.
///
/// Use it to see which types can be given an override. To see where a type
/// appears in a function's inputs, use [`shape::type_paths`].
///
/// ```
/// let types = sengi::parsed_types("pub struct DnaSequence(String);").unwrap();
/// assert_eq!(types[0].name, "DnaSequence");
/// assert_eq!(types[0].kind, sengi::ParsedTypeKind::Struct);
/// ```
pub fn parsed_types(src: &str) -> syn::Result<Vec<ParsedType>> {
    let ast = syn::parse_file(src)?;
    let mut collector = TypeCollector { types: Vec::new() };
    collector.visit_file(&ast);
    Ok(collector.types)
}

/// Walks a parsed file and records every type definition.
struct TypeCollector {
    types: Vec<ParsedType>,
}

impl TypeCollector {
    fn record(&mut self, name: &syn::Ident, kind: ParsedTypeKind, item: syn::Item) {
        let file: syn::File = syn::parse_quote!(#item);
        self.types.push(ParsedType {
            name: name.to_string(),
            kind,
            source: prettyplease::unparse(&file).trim().to_string(),
        });
    }
}

impl<'ast> Visit<'ast> for TypeCollector {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let item = syn::Item::Struct(node.clone());
        self.record(&node.ident, ParsedTypeKind::Struct, item);
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        let item = syn::Item::Enum(node.clone());
        self.record(&node.ident, ParsedTypeKind::Enum, item);
        visit::visit_item_enum(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        let item = syn::Item::Type(node.clone());
        self.record(&node.ident, ParsedTypeKind::Alias, item);
        visit::visit_item_type(self, node);
    }
}

/// The type `self` has: `Self`, `&Self`, `&'a mut Self`, or an explicit `Box<Self>` etc.
fn receiver_type(recv: &Receiver) -> String {
    match &recv.kind {
        ReceiverKind::Value => "Self".to_string(),
        ReceiverKind::Reference(_, lifetime, mutability) => {
            let lifetime = lifetime
                .as_ref()
                .map(|l| format!("{l} "))
                .unwrap_or_default();
            let mutability = if mutability.is_some() { "mut " } else { "" };
            format!("&{lifetime}{mutability}Self")
        }
        ReceiverKind::Typed(_, ty) => type_to_string(ty),
        // `ReceiverKind` is non-exhaustive; fall back to the source tokens.
        _ => recv.to_token_stream().to_string(),
    }
}

/// Render a type the way a human would write it (`&str`, not `& str`).
///
/// `prettyplease` only formats whole items, so the type is wrapped in
/// `type _T = <type>;`, formatted, and unwrapped again.
pub(crate) fn type_to_string(ty: &Type) -> String {
    let file: syn::File = syn::parse_quote!(type _T = #ty;);
    let pretty = prettyplease::unparse(&file);
    pretty
        .trim()
        .trim_start_matches("type _T = ")
        .trim_end_matches(';')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_free_fns_methods_and_traits() {
        let src = r#"
            pub fn add(left: u64, right: u64) -> u64 { left + right }
            pub fn add_string(left: &str, right: &str) -> String { todo!() }
            struct Foo;
            impl Foo {
                fn bar(&self, xs: Vec<Option<i32>>, (a, b): (u8, u8)) {}
            }
            trait T { fn baz(&mut self, f: impl Fn(u8) -> u8); }
        "#;
        let fns = function_inputs(src).unwrap();
        let got: Vec<(String, Vec<&str>)> = fns
            .iter()
            .map(|f| (f.name.clone(), f.input_types()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("add".into(), vec!["u64", "u64"]),
                ("add_string".into(), vec!["&str", "&str"]),
                (
                    "Foo::bar".into(),
                    vec!["&Self", "Vec<Option<i32>>", "(u8, u8)"]
                ),
                ("T::baz".into(), vec!["&mut Self", "impl Fn(u8) -> u8"]),
            ]
        );
        assert_eq!(fns[2].params[2].name, "(a , b)");
    }

    fn leaves_of(src: &str, func: &str, param: &str) -> Vec<(String, Option<Primitive>)> {
        let fns = function_inputs(src).unwrap();
        let f = fns.iter().find(|f| f.name == func).unwrap();
        let p = f.params.iter().find(|p| p.name == param).unwrap();
        p.node
            .leaves()
            .iter()
            .map(|l| (l.path_string(param), l.primitive()))
            .collect()
    }

    const FIXTURE: &str = include_str!("../tests/fixtures/complex.rs");

    #[test]
    fn expands_local_structs_and_enums() {
        use Primitive::*;
        assert_eq!(
            leaves_of(FIXTURE, "configure", "cfg"),
            vec![
                ("cfg*.name".into(), Some(String)),
                ("cfg*.retries".into(), Some(U32)),
                ("cfg*.mode::Off".into(), None),
                ("cfg*.mode::Fast.0".into(), Some(U8)),
                ("cfg*.mode::Custom.level".into(), Some(I64)),
                ("cfg*.mode::Custom.label*".into(), Some(Str)),
                ("cfg*.limits<0>.0".into(), Some(U8)),
                ("cfg*.limits<0>.1".into(), Some(U16)),
            ]
        );
    }

    #[test]
    fn substitutes_generic_struct_arguments() {
        assert_eq!(
            leaves_of(FIXTURE, "configure", "overrides"),
            vec![
                ("overrides.inner".into(), Some(Primitive::Bool)),
                ("overrides.count".into(), Some(Primitive::Usize)),
            ]
        );
    }

    #[test]
    fn generic_params_carry_inline_and_where_bounds() {
        let fns = function_inputs(FIXTURE).unwrap();
        let f = fns.iter().find(|f| f.name == "generic").unwrap();
        let TypeNode::Generic { name, bounds } = &f.params[1].node else {
            panic!("expected generic, got {:?}", f.params[1].node);
        };
        assert_eq!(name, "K");
        let bounds: Vec<std::string::String> = bounds.iter().map(ToString::to_string).collect();
        assert_eq!(bounds, ["std::hash::Hash", "Eq"]);

        // `T: Into<u64>` exposes `u64` as a child of the bound.
        let names: Vec<_> = f.params[0].node.walk().filter_map(TypeNode::name).collect();
        assert_eq!(names, ["Vec", "T", "u64"]);
    }

    #[test]
    fn generics_and_impl_trait_are_ignored_in_leaves() {
        assert_eq!(leaves_of(FIXTURE, "generic", "items"), vec![]);
        assert_eq!(leaves_of(FIXTURE, "generic", "key"), vec![]);
        assert_eq!(leaves_of(FIXTURE, "scores", "f"), vec![]);
        // Concrete fields next to a generic one are still reported.
        assert_eq!(
            leaves_of(FIXTURE, "Wrapper<T>::set", "self"),
            vec![("self*.count".into(), Some(Primitive::Usize))]
        );
    }

    #[test]
    fn recursive_types_terminate() {
        let fns = function_inputs(FIXTURE).unwrap();
        let f = fns.iter().find(|f| f.name == "generic").unwrap();
        let recursive = f.params[2].node.walk().any(|n| {
            matches!(
                n,
                TypeNode::Path {
                    definition: Some(TypeDef::Recursive),
                    ..
                }
            )
        });
        assert!(recursive);
    }

    #[test]
    fn aliases_resolve_to_their_target() {
        use Primitive::*;
        assert_eq!(
            leaves_of(FIXTURE, "scores", "s"),
            vec![("s<0>".into(), Some(String)), ("s<1><0>".into(), Some(F64))]
        );
    }

    #[test]
    fn self_resolves_to_impl_type() {
        let fns = function_inputs(FIXTURE).unwrap();
        let f = fns.iter().find(|f| f.name == "Wrapper<T>::set").unwrap();
        assert_eq!(f.params[0].ty, "&mut Self");
        assert_eq!(f.params[0].node.to_string(), "&mut Wrapper<T>");
    }
}
