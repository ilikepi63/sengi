//! JSON Schema: describe inputs without any source code, and export the
//! inputs of Rust functions as schemas.
//!
//! [`shape_from_schema`] turns a schema (draft 2020-12, as `schemars`
//! produces for serde types) into a [`Shape`]; [`schema_of`] goes the other
//! way. Generated inputs are written in the same layout the schema describes,
//! so a schema derived from your Rust types with `schemars` matches what
//! sengi produces for them.
//!
//! | JSON Schema | Shape |
//! |---|---|
//! | `"type": "boolean"` | [`Simple::Bool`] |
//! | `"type": "integer"` + `minimum`/`maximum`/`exclusive*`, `format: "uint8"`, ... | [`Simple::UInt`] if it can't be negative, else [`Simple::Int`] (default: the `i64` range) |
//! | `"type": "number"` + `minimum`/`maximum`/`exclusive*`, `format: "float"` | [`Simple::Float`] (never NaN or ±∞) |
//! | `"type": "string"` + `minLength`/`maxLength` | [`Simple::Text`] |
//! | `"type": "object"` + `properties` (+ `required`) | [`Shape::Fields`]; properties not in `required` are [`Shape::Optional`] |
//! | `"type": "object"` + `additionalProperties: {...}` only | [`Shape::Map`] with string keys, `minProperties`/`maxProperties` |
//! | `"type": "array"` + `items` (+ `minItems`/`maxItems`) | [`Shape::List`], or [`Shape::Array`] when the length is fixed |
//! | `"type": "array"` + `prefixItems` | a tuple: [`Shape::Fields`] named `0`, `1`, ... |
//! | `enum`, `const`, `"type": "null"` | [`Shape::Literal`] |
//! | `oneOf`, `anyOf` | [`Shape::Union`]; with exactly one `null` alternative, [`Shape::Option`] |
//! | `"type": ["string", "null"]` | [`Shape::Option`] |
//! | `$ref` to `#/$defs/...` or `#/definitions/...` | the referenced schema |
//! | `allOf` with one schema | that schema |
//! | `"x-sengi-override": "dna"` | [`Shape::Custom`] |
//!
//! Anything sengi can't generate valid values for becomes
//! [`Shape::Unsupported`] with the reason, so the rest of the input is still
//! generated: `pattern`, string `format`s, `multipleOf`, `allOf` with several
//! schemas, recursive `$ref`s, external `$ref`s and the `true` schema. Give
//! those parts an override.
//!
//! # Overrides
//!
//! Three ways, in order of precedence:
//!
//! 1. a path override ([`Overrides::use_for_path`]), with the same path
//!    syntax as for Rust: `reads[].sequence`; a union member is transparent
//!    and an optional property's value is at the property's own path;
//! 2. `"x-sengi-override": "<name>"` inside the schema;
//! 3. a type override ([`Overrides::use_for_type`]) on the name of a
//!    `$ref`ed definition, e.g. `DnaSequence` for `#/$defs/DnaSequence`.
//!
//! # Choice labels
//!
//! A union member's label (used in paths, coverage and reports) is its
//! `title`; otherwise its `const`, its single required property (serde's
//! enum layout, `{"Fast": ...}`), its `$ref` name, its `type`, or its index.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::overrides::Overrides;
use crate::shape::{Len, Shape, Simple};

/// The `$schema` written by [`schema_of`].
pub const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The shape of the inputs a JSON Schema describes.
///
/// Never fails: parts that can't be generated become
/// [`Shape::Unsupported`]; see [`Shape::unsupported`].
///
/// ```
/// use sengi::overrides::Overrides;
/// use sengi::schema::shape_from_schema;
/// use sengi::shape::{Shape, Simple};
///
/// let schema = serde_json::json!({
///     "type": "object",
///     "properties": { "ok": { "type": "boolean" } },
///     "required": ["ok"]
/// });
/// let shape = shape_from_schema(&schema, &Overrides::new());
/// assert_eq!(shape, Shape::Fields(vec![("ok".into(), Shape::Simple(Simple::Bool))]));
/// ```
pub fn shape_from_schema(schema: &Value, overrides: &Overrides) -> Shape {
    let mut compiler = Compiler {
        root: schema,
        overrides,
        expanding: Vec::new(),
    };
    compiler.compile(schema, "")
}

struct Compiler<'a> {
    /// The whole document, for resolving `$ref`s.
    root: &'a Value,
    overrides: &'a Overrides,
    /// `$ref`s being expanded right now, to stop on recursion.
    expanding: Vec<String>,
}

impl Compiler<'_> {
    fn compile(&mut self, schema: &Value, path: &str) -> Shape {
        if let Some(name) = self.overrides.for_path(path) {
            return Shape::Custom(name.to_string());
        }
        let object = match schema {
            Value::Object(object) => object,
            Value::Bool(true) => return unsupported("any value (the `true` schema)"),
            Value::Bool(false) => return unsupported("no value (the `false` schema)"),
            other => return unsupported(&format!("`{other}` is not a schema")),
        };

        if let Some(name) = object.get("x-sengi-override").and_then(Value::as_str) {
            return Shape::Custom(name.to_string());
        }
        if object.get("x-sengi-ignored") == Some(&Value::Bool(true)) {
            return Shape::Ignored;
        }
        if let Some(why) = object.get("x-sengi-unsupported").and_then(Value::as_str) {
            return Shape::Unsupported(why.to_string());
        }
        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            return self.compile_ref(reference, path);
        }
        if let Some(value) = object.get("const") {
            return Shape::Literal(vec![value.clone()]);
        }
        if let Some(values) = object.get("enum").and_then(Value::as_array) {
            return Shape::Literal(values.clone());
        }
        for keyword in ["oneOf", "anyOf"] {
            if let Some(members) = object.get(keyword).and_then(Value::as_array) {
                return self.compile_union(members, path);
            }
        }
        if let Some(all) = object.get("allOf").and_then(Value::as_array) {
            return match all.as_slice() {
                [single] => self.compile(single, path),
                _ => unsupported("`allOf` with several schemas"),
            };
        }

        match object.get("type") {
            Some(Value::String(ty)) => self.compile_type(ty, object, path),
            Some(Value::Array(types)) => {
                let types: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
                let nullable = types.contains(&"null");
                let others: Vec<&str> = types.into_iter().filter(|ty| *ty != "null").collect();
                // Like `Option`, the value of a nullable lives under `Some`.
                let inner_path = if nullable && !others.is_empty() {
                    child(path, "Some")
                } else {
                    path.to_string()
                };
                let inner = match others.as_slice() {
                    [] => Shape::Literal(vec![Value::Null]),
                    [single] => match self.overrides.for_path(&inner_path) {
                        Some(name) => Shape::Custom(name.to_string()),
                        None => self.compile_type(single, object, &inner_path),
                    },
                    several => {
                        let mut members = Vec::new();
                        for ty in several {
                            members
                                .push((ty.to_string(), self.compile_type(ty, object, &inner_path)));
                        }
                        Shape::Union(members)
                    }
                };
                if nullable && !others.is_empty() {
                    Shape::Option(Box::new(inner))
                } else {
                    inner
                }
            }
            Some(other) => unsupported(&format!("`type` {other}")),
            None => match implied_type(object) {
                Some(ty) => self.compile_type(ty, object, path),
                None => unsupported("a schema without a `type`"),
            },
        }
    }

    fn compile_ref(&mut self, reference: &str, path: &str) -> Shape {
        let Some(pointer) = reference.strip_prefix('#') else {
            return unsupported(&format!("external `$ref` `{reference}`"));
        };
        let name = pointer
            .rsplit('/')
            .next()
            .unwrap_or(pointer)
            .replace("~1", "/")
            .replace("~0", "~");
        if let Some(override_name) = self.overrides.for_type(&name) {
            return Shape::Custom(override_name.to_string());
        }
        if self.expanding.contains(&name) {
            return unsupported(&format!("recursive type `{name}`"));
        }
        let Some(target) = self.root.pointer(pointer) else {
            return unsupported(&format!("`$ref` `{reference}` points nowhere"));
        };
        self.expanding.push(name);
        let shape = self.compile(target, path);
        self.expanding.pop();
        shape
    }

    fn compile_union(&mut self, members: &[Value], path: &str) -> Shape {
        let (nulls, others): (Vec<&Value>, Vec<&Value>) = members.iter().partition(|m| is_null(m));
        if nulls.len() == 1 && others.len() == 1 {
            let inner = self.compile(others[0], &child(path, "Some"));
            return Shape::Option(Box::new(inner));
        }

        let mut shapes = Vec::new();
        for member in members {
            // Members are transparent for path overrides.
            shapes.push(self.compile(member, path));
        }
        // A union of literals (serde's unit-only enums) is one literal.
        if shapes
            .iter()
            .all(|shape| matches!(shape, Shape::Literal(_)))
        {
            let mut values = Vec::new();
            for shape in shapes {
                if let Shape::Literal(literal) = shape {
                    values.extend(literal);
                }
            }
            return Shape::Literal(values);
        }

        let mut used = BTreeSet::new();
        let mut labelled = Vec::new();
        for (index, (member, shape)) in members.iter().zip(shapes).enumerate() {
            let mut label = member_label(member, index);
            if !used.insert(label.clone()) {
                label = format!("{label}#{index}");
                used.insert(label.clone());
            }
            labelled.push((label, shape));
        }
        Shape::Union(labelled)
    }

    fn compile_type(&mut self, ty: &str, object: &Map<String, Value>, path: &str) -> Shape {
        match ty {
            "null" => Shape::Literal(vec![Value::Null]),
            "boolean" => Shape::Simple(Simple::Bool),
            "integer" => integer(object),
            "number" => number(object),
            "string" => string(object),
            "array" => self.array(object, path),
            "object" => self.object(object, path),
            other => unsupported(&format!("type `{other}`")),
        }
    }

    fn array(&mut self, object: &Map<String, Value>, path: &str) -> Shape {
        let len = Len {
            min: usize_of(object.get("minItems")).unwrap_or(0),
            max: usize_of(object.get("maxItems")),
        };

        // Tuples: `prefixItems` (2020-12) or an array of `items` (draft 7).
        let prefix = match (object.get("prefixItems"), object.get("items")) {
            (Some(Value::Array(prefix)), _) | (None, Some(Value::Array(prefix))) => Some(prefix),
            _ => None,
        };
        if let Some(prefix) = prefix {
            if prefix.len() == 1 {
                // A one-element tuple is written `[x]`, not as a newtype.
                let element = self.compile(&prefix[0], &child(path, "0"));
                return Shape::Array(Box::new(element), 1);
            }
            let mut fields = Vec::new();
            for (index, item) in prefix.iter().enumerate() {
                let name = index.to_string();
                fields.push((name.clone(), self.compile(item, &child(path, &name))));
            }
            return Shape::Fields(fields);
        }

        let element = match object.get("items") {
            Some(items) => self.compile(items, &format!("{path}[]")),
            None => unsupported("an array without `items`"),
        };
        match len.max {
            Some(max) if max == len.min => Shape::Array(Box::new(element), max),
            _ => Shape::List(Box::new(element), len),
        }
    }

    fn object(&mut self, object: &Map<String, Value>, path: &str) -> Shape {
        let properties = object.get("properties").and_then(Value::as_object);
        let additional = object.get("additionalProperties").filter(|v| v.is_object());

        if let (None, Some(additional)) = (properties, additional) {
            let len = Len {
                min: usize_of(object.get("minProperties")).unwrap_or(0),
                max: usize_of(object.get("maxProperties")),
            };
            let key = Shape::Simple(Simple::Text {
                min_len: 0,
                max_len: None,
            });
            let value = self.compile(additional, &format!("{path}[].value"));
            return Shape::Map(Box::new(key), Box::new(value), len);
        }

        let required: BTreeSet<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields = Vec::new();
        for (name, property) in properties.into_iter().flatten() {
            let shape = self.compile(property, &child(path, name));
            let shape = if required.contains(name.as_str()) {
                shape
            } else {
                Shape::Optional(Box::new(shape))
            };
            fields.push((name.clone(), shape));
        }
        Shape::Fields(fields)
    }
}

fn integer(object: &Map<String, Value>) -> Shape {
    if object.contains_key("multipleOf") {
        return unsupported("an integer with `multipleOf`");
    }
    let (default_min, default_max): (i128, u128) =
        match object.get("format").and_then(Value::as_str) {
            Some("int8") => (i8::MIN as i128, i8::MAX as u128),
            Some("int16") => (i16::MIN as i128, i16::MAX as u128),
            Some("int32") => (i32::MIN as i128, i32::MAX as u128),
            Some("uint8") => (0, u8::MAX as u128),
            Some("uint16") => (0, u16::MAX as u128),
            Some("uint32") => (0, u32::MAX as u128),
            Some("uint64" | "uint") => (0, u64::MAX as u128),
            Some("uint128") => (0, u128::MAX),
            Some("int128") => (i128::MIN, i128::MAX as u128),
            _ => (i64::MIN as i128, i64::MAX as u128),
        };

    // Bounds may be written as decimals (`"minimum": 0.5` means 1 or more).
    let lower = |key: &str, exclusive: bool| {
        let number = object.get(key)?.as_number()?.to_string();
        match number.parse::<i128>() {
            Ok(n) => Some(if exclusive { n.saturating_add(1) } else { n }),
            Err(_) => {
                let n: f64 = number.parse().ok()?;
                Some(if exclusive {
                    n.floor() as i128 + 1
                } else {
                    n.ceil() as i128
                })
            }
        }
    };
    let upper = |key: &str, exclusive: bool| {
        let number = object.get(key)?.as_number()?.to_string();
        if let Ok(n) = number.parse::<u128>() {
            return Some(match (exclusive, n) {
                (true, 0) => Bound::Signed(-1),
                (true, n) => Bound::Unsigned(n - 1),
                (false, n) => Bound::Unsigned(n),
            });
        }
        let n = match number.parse::<i128>() {
            Ok(n) => {
                if exclusive {
                    n - 1
                } else {
                    n
                }
            }
            Err(_) => {
                let n: f64 = number.parse().ok()?;
                if exclusive {
                    n.ceil() as i128 - 1
                } else {
                    n.floor() as i128
                }
            }
        };
        Some(if n >= 0 {
            Bound::Unsigned(n as u128)
        } else {
            Bound::Signed(n)
        })
    };

    let min = lower("exclusiveMinimum", true)
        .into_iter()
        .chain(lower("minimum", false))
        .max()
        .unwrap_or(default_min);
    let max = [upper("exclusiveMaximum", true), upper("maximum", false)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(Bound::Unsigned(default_max));

    match (min >= 0, max) {
        (true, Bound::Unsigned(max)) if max >= min as u128 => Shape::Simple(Simple::UInt {
            min: min as u128,
            max,
        }),
        (_, Bound::Signed(max)) if max >= min => Shape::Simple(Simple::Int { min, max }),
        (false, Bound::Unsigned(max)) => Shape::Simple(Simple::Int {
            min,
            max: max.min(i128::MAX as u128) as i128,
        }),
        _ => unsupported("an integer with an empty range"),
    }
}

/// An upper integer bound. Negative bounds are always `Signed` and others
/// `Unsigned`, so the derived order is the numeric order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Bound {
    Signed(i128),
    Unsigned(u128),
}

fn number(object: &Map<String, Value>) -> Shape {
    if object.contains_key("multipleOf") {
        return unsupported("a number with `multipleOf`");
    }
    let single = object.get("format").and_then(Value::as_str) == Some("float");
    let limit = if single { f32::MAX as f64 } else { f64::MAX };
    let get = |key: &str| object.get(key).and_then(Value::as_f64);

    let mut min = -limit;
    let mut max = limit;
    if let Some(n) = get("minimum") {
        min = min.max(n);
    }
    if let Some(n) = get("exclusiveMinimum") {
        min = min.max(n.next_up());
    }
    if let Some(n) = get("maximum") {
        max = max.min(n);
    }
    if let Some(n) = get("exclusiveMaximum") {
        max = max.min(n.next_down());
    }
    if min > max {
        return unsupported("a number with an empty range");
    }
    Shape::Simple(Simple::Float {
        min,
        max,
        single,
        specials: false,
    })
}

fn string(object: &Map<String, Value>) -> Shape {
    if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
        return unsupported(&format!("a string matching `{pattern}`"));
    }
    if let Some(format) = object.get("format").and_then(Value::as_str) {
        return unsupported(&format!("a string with format `{format}`"));
    }
    let min_len = usize_of(object.get("minLength")).unwrap_or(0);
    let max_len = usize_of(object.get("maxLength"));
    if max_len.is_some_and(|max| max < min_len) {
        return unsupported("a string with an empty length range");
    }
    Shape::Simple(Simple::Text { min_len, max_len })
}

/// The `type` a schema without one most likely means, from its keywords.
fn implied_type(object: &Map<String, Value>) -> Option<&'static str> {
    let has = |keys: &[&str]| keys.iter().any(|key| object.contains_key(*key));
    if has(&["properties", "additionalProperties", "required"]) {
        Some("object")
    } else if has(&["items", "prefixItems", "minItems", "maxItems"]) {
        Some("array")
    } else if has(&["minLength", "maxLength", "pattern"]) {
        Some("string")
    } else if has(&["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"]) {
        Some("number")
    } else {
        None
    }
}

fn is_null(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
        || schema.get("const") == Some(&Value::Null)
}

/// See "Choice labels" in the module docs.
fn member_label(member: &Value, index: usize) -> String {
    if let Some(title) = member.get("title").and_then(Value::as_str) {
        return title.to_string();
    }
    if let Some(value) = member.get("const") {
        return match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
    }
    let required = member.get("required").and_then(Value::as_array);
    if let Some([Value::String(name)]) = required.map(Vec::as_slice) {
        return name.clone();
    }
    if let Some(reference) = member.get("$ref").and_then(Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string();
    }
    if let Some(ty) = member.get("type").and_then(Value::as_str) {
        return ty.to_string();
    }
    index.to_string()
}

fn usize_of(value: Option<&Value>) -> Option<usize> {
    value?.as_u64().map(|n| n as usize)
}

fn unsupported(why: &str) -> Shape {
    Shape::Unsupported(why.to_string())
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

// ---------------------------------------------------------------- exporting

/// The JSON Schema describing the inputs of a shape, in the layout sengi
/// writes them. Everything is inlined (no `$defs`).
///
/// [`shape_from_schema`] reads it back to the same shape, except that:
/// `char` becomes a one-character string, Rust floats lose NaN and ±∞ (JSON
/// can't hold them), and map keys become strings. Overrides, ignored and
/// unsupported parts are kept with `x-sengi-*` keywords.
pub fn schema_of(shape: &Shape) -> Value {
    match shape {
        Shape::Simple(simple) => simple_schema(simple),
        Shape::Fields(fields) => fields_schema(fields),
        Shape::Enum(variants) => {
            let mut members = Vec::new();
            for (name, fields) in variants {
                if fields.is_empty() {
                    members.push(json!({ "const": name }));
                } else {
                    members.push(json!({
                        "type": "object",
                        "properties": { name.as_str(): fields_schema(fields) },
                        "required": [name],
                        "additionalProperties": false,
                    }));
                }
            }
            json!({ "oneOf": members })
        }
        Shape::Union(members) => {
            let mut schemas = Vec::new();
            for (label, member) in members {
                let mut schema = schema_of(member);
                if let Value::Object(object) = &mut schema {
                    object.insert("title".to_string(), Value::String(label.clone()));
                }
                schemas.push(schema);
            }
            json!({ "oneOf": schemas })
        }
        Shape::Literal(values) => match values.as_slice() {
            [single] => json!({ "const": single }),
            _ => json!({ "enum": values }),
        },
        Shape::Option(inner) => json!({ "anyOf": [schema_of(inner), { "type": "null" }] }),
        // The enclosing object leaves it out of `required`.
        Shape::Optional(inner) => schema_of(inner),
        Shape::List(element, len) => {
            let mut schema = json!({ "type": "array", "items": schema_of(element) });
            add_len(&mut schema, "minItems", "maxItems", len);
            schema
        }
        Shape::Array(element, len) => json!({
            "type": "array",
            "items": schema_of(element),
            "minItems": len,
            "maxItems": len,
        }),
        Shape::Map(_, value, len) => {
            let mut schema = json!({ "type": "object", "additionalProperties": schema_of(value) });
            add_len(&mut schema, "minProperties", "maxProperties", len);
            schema
        }
        Shape::Custom(name) => json!({ "x-sengi-override": name }),
        Shape::Ignored => json!({ "x-sengi-ignored": true }),
        Shape::Unsupported(why) => json!({ "x-sengi-unsupported": why }),
    }
}

/// [`schema_of`] plus `$schema` and `title`: a standalone schema document.
pub fn document(title: &str, shape: &Shape) -> Value {
    let mut schema = schema_of(shape);
    if let Value::Object(object) = &mut schema {
        let mut document = Map::new();
        document.insert("$schema".to_string(), Value::String(DRAFT.to_string()));
        document.insert("title".to_string(), Value::String(title.to_string()));
        document.extend(std::mem::take(object));
        return Value::Object(document);
    }
    schema
}

fn simple_schema(simple: &Simple) -> Value {
    match *simple {
        Simple::Bool => json!({ "type": "boolean" }),
        Simple::UInt { min, max } => json!({ "type": "integer", "minimum": min, "maximum": max }),
        Simple::Int { min, max } => json!({ "type": "integer", "minimum": min, "maximum": max }),
        Simple::Float {
            min, max, single, ..
        } => {
            let mut schema = json!({ "type": "number" });
            let limit = if single { f32::MAX as f64 } else { f64::MAX };
            if min > -limit {
                schema["minimum"] = json!(min);
            }
            if max < limit {
                schema["maximum"] = json!(max);
            }
            if single {
                schema["format"] = json!("float");
            }
            schema
        }
        Simple::Char => json!({ "type": "string", "minLength": 1, "maxLength": 1 }),
        Simple::Text { min_len, max_len } => {
            let mut schema = json!({ "type": "string" });
            add_len(
                &mut schema,
                "minLength",
                "maxLength",
                &Len {
                    min: min_len,
                    max: max_len,
                },
            );
            schema
        }
    }
}

/// Same layout rules as saving: a newtype is its inner value, a tuple an
/// array, anything else an object.
fn fields_schema(fields: &[(String, Shape)]) -> Value {
    let is_tuple = fields
        .iter()
        .all(|(name, _)| name.chars().all(|c| c.is_ascii_digit()));
    if is_tuple && fields.len() == 1 {
        return schema_of(&fields[0].1);
    }
    if is_tuple {
        let items: Vec<Value> = fields.iter().map(|(_, shape)| schema_of(shape)).collect();
        return json!({
            "type": "array",
            "prefixItems": items,
            "items": false,
            "minItems": fields.len(),
            "maxItems": fields.len(),
        });
    }

    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, shape) in fields {
        properties.insert(name.clone(), schema_of(shape));
        if !matches!(shape, Shape::Optional(_)) {
            required.push(Value::String(name.clone()));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn add_len(schema: &mut Value, min_key: &str, max_key: &str, len: &Len) {
    if len.min > 0 {
        schema[min_key] = json!(len.min);
    }
    if let Some(max) = len.max {
        schema[max_key] = json!(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;
    use crate::corpus::{CorpusConfig, build_corpus};
    use crate::function_inputs;
    use crate::registry::Registry;
    use crate::save::{input_to_json, json_to_input};
    use crate::shape::function_shape;

    fn shape(schema: Value) -> Shape {
        shape_from_schema(&schema, &Overrides::new())
    }

    fn simple(schema: Value) -> Simple {
        match shape(schema) {
            Shape::Simple(simple) => simple,
            other => panic!("expected a simple shape, got {other:?}"),
        }
    }

    #[test]
    fn integer_ranges() {
        assert_eq!(
            simple(json!({ "type": "integer", "minimum": 1, "maximum": 10 })),
            Simple::UInt { min: 1, max: 10 }
        );
        assert_eq!(
            simple(json!({ "type": "integer", "exclusiveMinimum": -5, "exclusiveMaximum": 5 })),
            Simple::Int { min: -4, max: 4 }
        );
        assert_eq!(
            simple(json!({ "type": "integer", "format": "uint8", "minimum": 0 })),
            Simple::from(Primitive::U8)
        );
        assert_eq!(
            simple(json!({ "type": "integer" })),
            Simple::from(Primitive::I64)
        );
        assert_eq!(
            shape(json!({ "type": "integer", "minimum": 5, "maximum": 1 })),
            Shape::Unsupported("an integer with an empty range".into())
        );
    }

    #[test]
    fn objects_optional_properties_and_nullables() {
        let got = shape(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "maxLength": 8 },
                "nickname": { "type": ["string", "null"] },
                "age": { "type": "integer", "minimum": 0, "maximum": 150 }
            },
            "required": ["name", "nickname"]
        }));
        let text = |max_len| {
            Shape::Simple(Simple::Text {
                min_len: 0,
                max_len,
            })
        };
        assert_eq!(
            got,
            Shape::Fields(vec![
                (
                    "age".into(),
                    Shape::Optional(Box::new(Shape::Simple(Simple::UInt { min: 0, max: 150 })))
                ),
                ("name".into(), text(Some(8))),
                ("nickname".into(), Shape::Option(Box::new(text(None)))),
            ])
        );
    }

    #[test]
    fn serde_enums_become_unions_and_literals() {
        // What schemars writes for `enum Mode { Off, Slow, Fast(u8) }`.
        let got = shape(json!({
            "oneOf": [
                { "type": "string", "enum": ["Off", "Slow"] },
                {
                    "type": "object",
                    "properties": { "Fast": { "type": "integer", "format": "uint8", "minimum": 0 } },
                    "required": ["Fast"],
                    "additionalProperties": false
                }
            ]
        }));
        let Shape::Union(members) = got else { panic!() };
        let labels: Vec<&str> = members.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["string", "Fast"]);
        assert_eq!(
            members[0].1,
            Shape::Literal(vec![json!("Off"), json!("Slow")])
        );

        // Only unit variants: a single literal.
        let units = shape(json!({ "oneOf": [{ "const": "A" }, { "const": "B" }] }));
        assert_eq!(units, Shape::Literal(vec![json!("A"), json!("B")]));
    }

    #[test]
    fn refs_resolve_and_recursion_is_reported() {
        let schema = json!({
            "type": "object",
            "properties": {
                "seq": { "$ref": "#/$defs/Dna" },
                "tree": { "$ref": "#/$defs/Node" }
            },
            "required": ["seq", "tree"],
            "$defs": {
                "Dna": { "type": "string" },
                "Node": {
                    "type": "object",
                    "properties": { "next": { "anyOf": [{ "$ref": "#/$defs/Node" }, { "type": "null" }] } },
                    "required": ["next"]
                }
            }
        });
        let got = shape(schema.clone());
        assert_eq!(got.unsupported(), ["recursive type `Node`"]);

        // A type override on the definition's name.
        let mut overrides = Overrides::new();
        overrides.add(crate::overrides::Override::new("dna", |_| json!("ACGT")));
        overrides.use_for_type("Dna", "dna");
        let Shape::Fields(fields) = shape_from_schema(&schema, &overrides) else {
            panic!()
        };
        assert_eq!(fields[0], ("seq".into(), Shape::Custom("dna".into())));
    }

    #[test]
    fn patterns_and_formats_need_an_override() {
        let got = shape(json!({
            "type": "object",
            "properties": {
                "email": { "type": "string", "format": "email" },
                "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
                "dna": { "type": "string", "pattern": "^[ACGT]+$", "x-sengi-override": "dna" }
            },
            "required": ["email", "code", "dna"]
        }));
        assert_eq!(
            got.unsupported(),
            [
                "a string matching `^[A-Z]{3}$`",
                "a string with format `email`"
            ]
        );
    }

    #[test]
    fn generated_inputs_fit_the_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level": { "type": "integer", "minimum": 1, "maximum": 5 },
                "ratio": { "type": "number", "minimum": 0, "maximum": 1 },
                "tags": { "type": "array", "items": { "type": "string", "minLength": 2, "maxLength": 3 }, "maxItems": 4 },
                "mode": { "enum": ["a", "b", null] },
                "note": { "type": "string" }
            },
            "required": ["level", "ratio", "tags", "mode"]
        });
        let shape = shape(schema);
        let registry = Registry::new().unwrap();
        let corpus = build_corpus(
            &shape,
            &registry,
            &Overrides::new(),
            &CorpusConfig::default(),
        );
        assert!(corpus.inputs.len() > 10);
        let mut saw_missing_note = false;
        for input in &corpus.inputs {
            let json = input_to_json(&shape, input);
            let level = json["level"].as_u64().unwrap();
            assert!((1..=5).contains(&level), "{json}");
            let ratio = json["ratio"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&ratio), "{json}");
            let tags = json["tags"].as_array().unwrap();
            assert!(tags.len() <= 4);
            for tag in tags {
                assert!(
                    (2..=3).contains(&tag.as_str().unwrap().chars().count()),
                    "{json}"
                );
            }
            assert!([json!("a"), json!("b"), Value::Null].contains(&json["mode"]));
            saw_missing_note |= json.get("note").is_none();
            // Every generated input reads back.
            json_to_input(&shape, &json).unwrap();
        }
        assert!(saw_missing_note);
    }

    #[test]
    fn optional_properties_that_cannot_be_generated_are_left_out() {
        let shape = shape(json!({
            "type": "object",
            "properties": { "email": { "type": "string", "format": "email" } }
        }));
        let registry = Registry::new().unwrap();
        let corpus = build_corpus(
            &shape,
            &registry,
            &Overrides::new(),
            &CorpusConfig::default(),
        );
        for input in &corpus.inputs {
            assert_eq!(input_to_json(&shape, input), json!({}));
        }
    }

    #[test]
    fn rust_inputs_round_trip_through_a_schema() {
        let src = include_str!("../tests/fixtures/complex.rs");
        let functions = function_inputs(src).unwrap();
        let configure = functions.iter().find(|f| f.name == "configure").unwrap();
        let rust_shape = function_shape(configure, &Overrides::new());

        let exported = document("configure", &rust_shape);
        let schema_shape = shape_from_schema(&exported, &Overrides::new());

        // Inputs generated from the Rust types fit the exported schema...
        let registry = Registry::new().unwrap();
        let corpus = build_corpus(
            &rust_shape,
            &registry,
            &Overrides::new(),
            &CorpusConfig::default(),
        );
        for input in &corpus.inputs {
            let json = input_to_json(&rust_shape, input);
            let read =
                json_to_input(&schema_shape, &json).unwrap_or_else(|e| panic!("{e} in {json}"));
            // ...and come back out unchanged.
            assert_eq!(input_to_json(&schema_shape, &read), json);
        }
        // Exporting the schema's shape again changes nothing.
        assert_eq!(
            schema_of(&schema_shape),
            schema_of(&shape_from_schema(
                &schema_of(&schema_shape),
                &Overrides::new()
            ))
        );
    }
}
