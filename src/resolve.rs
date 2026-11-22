//! Turn `syn::Type`s into [`TypeNode`] trees, expanding types defined in the
//! same source and substituting generic parameters as we go.
//!
//! - **Definitions** are looked up by name across the whole file, so
//!   `Config` in a parameter type expands to its fields. Two types with the
//!   same name in different modules aren't told apart yet.
//! - **Scopes** track which generic parameters are visible and what `Self`
//!   means. Using `Wrapper<bool>` binds the struct's `T` to `bool` while its
//!   fields are expanded.
//! - **Recursion** (`struct Node { next: Option<Box<Node>> }`) stops at the
//!   second occurrence, which becomes [`TypeDef::Recursive`].

use std::collections::HashMap;

use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::{
    Fields, GenericArgument, GenericParam, Generics, ItemEnum, ItemStruct, ItemType, Path,
    PathArguments, PointerMutability, ReturnType, Type, TypeParamBound, WherePredicate,
};

use crate::types::{Bound, Field, TypeDef, TypeNode, Variant};

/// Structs, enums and type aliases declared anywhere in the parsed file, by name.
#[derive(Default)]
pub(crate) struct Definitions<'ast> {
    items: HashMap<String, Def<'ast>>,
}

#[derive(Clone, Copy)]
enum Def<'ast> {
    Struct(&'ast ItemStruct),
    Enum(&'ast ItemEnum),
    Alias(&'ast ItemType),
}

impl<'ast> Definitions<'ast> {
    pub(crate) fn collect(file: &'ast syn::File) -> Self {
        let mut defs = Definitions::default();
        defs.visit_file(file);
        defs
    }
}

impl<'ast> Visit<'ast> for Definitions<'ast> {
    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        self.items.insert(node.ident.to_string(), Def::Struct(node));
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        self.items.insert(node.ident.to_string(), Def::Enum(node));
        visit::visit_item_enum(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast ItemType) {
        self.items.insert(node.ident.to_string(), Def::Alias(node));
        visit::visit_item_type(self, node);
    }
}

/// What a generic parameter name means in the current scope.
#[derive(Clone)]
enum Binding<'ast> {
    /// Substituted with a concrete argument, e.g. `T = u8` when expanding `Wrapper<u8>`.
    Concrete(TypeNode),
    /// Still a free parameter, with its bounds from `<T: A>` and `where T: B`.
    Param(Vec<&'ast TypeParamBound>),
}

/// The generic parameters and `Self` type visible at some point in the source.
#[derive(Clone, Default)]
pub(crate) struct Scope<'ast> {
    params: HashMap<String, Binding<'ast>>,
    /// What `Self` means: the impl's type, or `None` inside a trait.
    self_ty: Option<TypeNode>,
}

impl<'ast> Scope<'ast> {
    /// This scope plus the free parameters declared by `generics`.
    pub(crate) fn with_generics(&self, generics: &'ast Generics) -> Self {
        let mut scope = self.clone();
        for param in &generics.params {
            if let GenericParam::Type(tp) = param {
                let bounds = tp.bounds.iter().collect();
                scope
                    .params
                    .insert(tp.ident.to_string(), Binding::Param(bounds));
            }
        }
        add_where_bounds(&mut scope, generics);
        scope
    }

    /// What `Self` resolves to: the impl type, or a bare generic inside a trait.
    pub(crate) fn self_type(&self) -> TypeNode {
        self.self_ty.clone().unwrap_or(TypeNode::Generic {
            name: "Self".to_string(),
            bounds: Vec::new(),
        })
    }

    pub(crate) fn with_self(mut self, self_ty: Option<TypeNode>) -> Self {
        self.self_ty = self_ty;
        self
    }
}

fn add_where_bounds<'ast>(scope: &mut Scope<'ast>, generics: &'ast Generics) {
    let Some(where_clause) = &generics.where_clause else {
        return;
    };
    for pred in &where_clause.predicates {
        let WherePredicate::Type(pred) = pred else {
            continue;
        };
        let Some(ident) = single_ident(&pred.bounded_ty) else {
            continue;
        };
        if let Some(Binding::Param(bounds)) = scope.params.get_mut(&ident) {
            bounds.extend(pred.bounds.iter());
        }
    }
}

/// `T` -> `Some("T")`; anything longer or with arguments -> `None`.
fn single_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(tp) if tp.qself.is_none() => tp.path.get_ident().map(ToString::to_string),
        _ => None,
    }
}

pub(crate) struct Resolver<'a, 'ast> {
    defs: &'a Definitions<'ast>,
    /// Names currently being expanded, to stop on recursive types and
    /// self-referential bounds like `T: PartialEq<T>`.
    expanding: Vec<String>,
}

impl<'a, 'ast> Resolver<'a, 'ast> {
    pub(crate) fn new(defs: &'a Definitions<'ast>) -> Self {
        Resolver {
            defs,
            expanding: Vec::new(),
        }
    }

    pub(crate) fn convert(&mut self, ty: &'ast Type, scope: &Scope<'ast>) -> TypeNode {
        match ty {
            Type::Path(tp) if tp.qself.is_none() => self.convert_path(&tp.path, scope),
            Type::Reference(r) => TypeNode::Reference {
                lifetime: r.lifetime.as_ref().map(ToString::to_string),
                mutable: r.mutability.is_some(),
                inner: Box::new(self.convert(&r.elem, scope)),
            },
            Type::Ptr(p) => TypeNode::Pointer {
                mutable: matches!(p.mutability, PointerMutability::Mut(_)),
                inner: Box::new(self.convert(&p.elem, scope)),
            },
            Type::Slice(s) => TypeNode::Slice(Box::new(self.convert(&s.elem, scope))),
            Type::Array(a) => TypeNode::Array {
                elem: Box::new(self.convert(&a.elem, scope)),
                len: a.len.to_token_stream().to_string(),
            },
            Type::Tuple(t) => {
                TypeNode::Tuple(t.elems.iter().map(|e| self.convert(e, scope)).collect())
            }
            Type::FnPtr(f) => TypeNode::FnPointer {
                inputs: f
                    .inputs
                    .iter()
                    .map(|a| self.convert(&a.ty, scope))
                    .collect(),
                output: self.convert_return(&f.output, scope),
            },
            Type::ImplTrait(i) => TypeNode::ImplTrait(self.convert_bounds(&i.bounds, scope)),
            Type::TraitObject(t) => TypeNode::TraitObject(self.convert_bounds(&t.bounds, scope)),
            Type::Paren(p) => self.convert(&p.elem, scope),
            Type::Group(g) => self.convert(&g.elem, scope),
            Type::Never(_) => TypeNode::Never,
            other => TypeNode::Other(crate::type_to_string(other)),
        }
    }

    fn convert_return(
        &mut self,
        ret: &'ast ReturnType,
        scope: &Scope<'ast>,
    ) -> Option<Box<TypeNode>> {
        match ret {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(Box::new(self.convert(ty, scope))),
        }
    }

    fn convert_path(&mut self, path: &'ast Path, scope: &Scope<'ast>) -> TypeNode {
        // A lone identifier may be a generic parameter or `Self`.
        if let Some(ident) = path.get_ident() {
            let name = ident.to_string();
            if let Some(binding) = scope.params.get(&name) {
                return self.convert_binding(&name, binding.clone(), scope);
            }
            if name == "Self" {
                return scope.self_type();
            }
        }

        let path_str = path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let path_str = if path.leading_colon.is_some() {
            format!("::{path_str}")
        } else {
            path_str
        };
        let last = path
            .segments
            .last()
            .expect("paths have at least one segment");
        let name = last.ident.to_string();
        let (generics, assoc, _output) = self.convert_args(&last.arguments, scope);
        let definition = self.expand_definition(&name, &generics);

        TypeNode::Path {
            path: path_str,
            name,
            generics,
            assoc,
            definition,
        }
    }

    fn convert_binding(
        &mut self,
        name: &str,
        binding: Binding<'ast>,
        scope: &Scope<'ast>,
    ) -> TypeNode {
        match binding {
            Binding::Concrete(node) => node,
            Binding::Param(bounds) => {
                let key = format!("generic {name}");
                // `T: PartialEq<T>` — don't expand `T`'s bounds inside its own bounds.
                let bounds = if self.expanding.contains(&key) {
                    Vec::new()
                } else {
                    self.expanding.push(key);
                    let bounds = bounds
                        .iter()
                        .map(|b| self.convert_bound(b, scope))
                        .collect();
                    self.expanding.pop();
                    bounds
                };
                TypeNode::Generic {
                    name: name.to_string(),
                    bounds,
                }
            }
        }
    }

    /// Generic args of one path segment: (type args, assoc bindings, `-> Output`).
    #[allow(clippy::type_complexity)]
    fn convert_args(
        &mut self,
        args: &'ast PathArguments,
        scope: &Scope<'ast>,
    ) -> (
        Vec<TypeNode>,
        Vec<(String, TypeNode)>,
        Option<Box<TypeNode>>,
    ) {
        let mut generics = Vec::new();
        let mut assoc = Vec::new();
        let mut output = None;
        match args {
            PathArguments::None => {}
            PathArguments::AngleBracketed(ab) => {
                for arg in &ab.args {
                    match arg {
                        GenericArgument::Type(ty) => generics.push(self.convert(ty, scope)),
                        GenericArgument::AssocType(a) => {
                            assoc.push((a.ident.to_string(), self.convert(&a.ty, scope)))
                        }
                        GenericArgument::Const(c) => {
                            generics.push(TypeNode::Other(c.to_token_stream().to_string()))
                        }
                        // Lifetimes don't affect what values can be generated.
                        _ => {}
                    }
                }
            }
            PathArguments::Parenthesized(p) => {
                generics.extend(p.inputs.iter().map(|a| self.convert(&a.ty, scope)));
                output = self.convert_return(&p.output, scope);
            }
        }
        (generics, assoc, output)
    }

    fn convert_bounds(
        &mut self,
        bounds: impl IntoIterator<Item = &'ast TypeParamBound>,
        scope: &Scope<'ast>,
    ) -> Vec<Bound> {
        bounds
            .into_iter()
            .map(|b| self.convert_bound(b, scope))
            .collect()
    }

    fn convert_bound(&mut self, bound: &'ast TypeParamBound, scope: &Scope<'ast>) -> Bound {
        match bound {
            TypeParamBound::Trait(tb) => {
                let path = tb
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                let last = tb.path.segments.last().expect("non-empty path");
                let (generics, assoc, output) = self.convert_args(&last.arguments, scope);
                Bound::Trait {
                    path,
                    name: last.ident.to_string(),
                    maybe: tb.maybe.is_some(),
                    generics,
                    assoc,
                    output,
                }
            }
            TypeParamBound::Lifetime(lt) => Bound::Lifetime(lt.to_string()),
            other => Bound::Other(other.to_token_stream().to_string()),
        }
    }

    /// Expand a struct/enum/alias defined in the source, substituting `args`
    /// for its type parameters.
    fn expand_definition(&mut self, name: &str, args: &[TypeNode]) -> Option<TypeDef> {
        let def = *self.defs.items.get(name)?;
        if self.expanding.iter().any(|n| n == name) {
            return Some(TypeDef::Recursive);
        }

        let generics = match def {
            Def::Struct(s) => &s.generics,
            Def::Enum(e) => &e.generics,
            Def::Alias(a) => &a.generics,
        };
        // A definition only sees its own generics, not the caller's.
        let mut scope = Scope::default().with_generics(generics);
        let mut args = args.iter();
        for param in &generics.params {
            let GenericParam::Type(tp) = param else {
                continue;
            };
            let concrete = match (args.next(), &tp.default) {
                (Some(arg), _) => Some(arg.clone()),
                // `struct Foo<T = u8>` used as plain `Foo`.
                (None, Some((_, default))) => Some(self.convert(default, &scope)),
                (None, None) => None,
            };
            if let Some(node) = concrete {
                scope
                    .params
                    .insert(tp.ident.to_string(), Binding::Concrete(node));
            }
        }
        // `Self` inside the definition refers back to the type itself.
        scope.self_ty = Some(TypeNode::Path {
            path: name.to_string(),
            name: name.to_string(),
            generics: Vec::new(),
            assoc: Vec::new(),
            definition: Some(TypeDef::Recursive),
        });

        self.expanding.push(name.to_string());
        let body = match def {
            Def::Struct(s) => TypeDef::Struct(self.convert_fields(&s.fields, &scope)),
            Def::Enum(e) => TypeDef::Enum(
                e.variants
                    .iter()
                    .map(|v| Variant {
                        name: v.ident.to_string(),
                        fields: self.convert_fields(&v.fields, &scope),
                    })
                    .collect(),
            ),
            Def::Alias(a) => TypeDef::Alias(Box::new(self.convert(&a.ty, &scope))),
        };
        self.expanding.pop();
        Some(body)
    }

    fn convert_fields(&mut self, fields: &'ast Fields, scope: &Scope<'ast>) -> Vec<Field> {
        fields
            .iter()
            .enumerate()
            .map(|(i, f)| Field {
                name: f
                    .ident
                    .as_ref()
                    .map_or_else(|| i.to_string(), ToString::to_string),
                ty: self.convert(&f.ty, scope),
            })
            .collect()
    }
}
