//! Save a corpus as JSON, one file per function or schema, and read inputs
//! back from JSON.
//!
//! Each input is written as nested JSON in serde's default layout, so a test
//! can deserialize it straight into the real argument types:
//!
//! ```text
//! struct / tuple struct   {"name": "abc", "retries": 3}   /   [1, 2]
//! newtype struct          the inner value
//! enum variant            "Off"   {"Fast": 7}   {"Custom": {"level": -1, "label": ""}}
//! Option                  null   or the inner value
//! optional property       left out   or the inner value
//! union / literal         the chosen member's value   /   the literal
//! Vec / array / tuple     [ ... ]
//! map                     {"key": value, ...}   (keys written as strings)
//! ```
//!
//! JSON has no NaN or infinity, so those floats are written as the strings
//! `"NaN"`, `"inf"` and `"-inf"`.
//!
//! [`json_to_input`] goes the other way, checking each value against the
//! shape — that's how existing inputs are read for [`crate::analysis`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Number, Value, json};

use crate::corpus::{Corpus, CorpusConfig};
use crate::schema::schema_of;
use crate::shape::{Shape, Simple, literal_label};
use crate::stats::Stats;
use crate::value::{Input, InputLeaf, Kind, LeafValue};

/// Writes `<dir>/<name>.json` and returns its path. `name` is a function
/// name (`Wrapper<T>::set` becomes `Wrapper_T_set.json`) or a schema title.
pub fn save_corpus(
    dir: &Path,
    name: &str,
    shape: &Shape,
    corpus: &Corpus,
    config: &CorpusConfig,
    stats: &Stats,
) -> io::Result<PathBuf> {
    let json = corpus_to_json(name, shape, corpus, config, stats);
    write_json(dir, &format!("{}.json", file_name(name)), &json)
}

/// The whole JSON document for one corpus: `name`, `schema` (the inputs'
/// JSON Schema, see [`crate::schema`]), `config`, `stats` and `inputs`.
pub fn corpus_to_json(
    name: &str,
    shape: &Shape,
    corpus: &Corpus,
    config: &CorpusConfig,
    stats: &Stats,
) -> Value {
    let mut inputs = Vec::new();
    for input in &corpus.inputs {
        inputs.push(input_to_json(shape, input));
    }

    json!({
        "name": name,
        "schema": schema_of(shape),
        "config": { "theta": config.theta, "seed": config.seed },
        "stats": stats,
        "inputs": inputs,
    })
}

/// Writes `value` as pretty JSON to `<dir>/<file>`, creating `dir`, and
/// returns the path.
pub fn write_json(dir: &Path, file: &str, value: &Value) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(file);
    let text = serde_json::to_string_pretty(value).expect("JSON values always serialize");
    fs::write(&path, text)?;
    Ok(path)
}

/// Rebuilds the nested value of one input from its flat leaf map, following
/// the same paths the fuzzer used to generate it.
pub fn input_to_json(shape: &Shape, input: &Input) -> Value {
    value_at(shape, "", input)
}

fn value_at(shape: &Shape, path: &str, input: &Input) -> Value {
    match shape {
        Shape::Simple(_) | Shape::Custom(_) => match input.get(path) {
            Some(leaf) => leaf_to_json(&leaf.value),
            None => Value::Null,
        },
        Shape::Fields(fields) => fields_to_json(fields, path, input),
        Shape::Enum(variants) => {
            let Some(variant) = chosen(path, input) else {
                return Value::Null;
            };
            let (_, fields) = variants
                .iter()
                .find(|(name, _)| name == variant)
                .expect("generated variants come from the shape");
            if fields.is_empty() {
                // Unit variant: just its name.
                return Value::String(variant.to_string());
            }
            let mut tagged = Map::new();
            let variant_path = child(path, variant);
            tagged.insert(
                variant.to_string(),
                fields_to_json(fields, &variant_path, input),
            );
            Value::Object(tagged)
        }
        Shape::Union(members) => {
            let Some(label) = chosen(path, input) else {
                return Value::Null;
            };
            let (_, member) = members
                .iter()
                .find(|(name, _)| name == label)
                .expect("generated members come from the shape");
            value_at(member, &child(path, label), input)
        }
        Shape::Literal(values) => {
            if let [constant] = values.as_slice() {
                return constant.clone();
            }
            let Some(label) = chosen(path, input) else {
                return Value::Null;
            };
            values
                .iter()
                .find(|value| literal_label(value) == label)
                .cloned()
                .unwrap_or(Value::Null)
        }
        Shape::Option(inner) | Shape::Optional(inner) => {
            if chosen(path, input) == Some("Some") {
                value_at(inner, &child(path, "Some"), input)
            } else {
                Value::Null
            }
        }
        Shape::List(element, _) => {
            let mut items = Vec::new();
            for i in 0..list_length(path, input) {
                items.push(value_at(element, &format!("{path}[{i}]"), input));
            }
            Value::Array(items)
        }
        Shape::Array(element, len) => {
            let mut items = Vec::new();
            for i in 0..*len {
                items.push(value_at(element, &format!("{path}[{i}]"), input));
            }
            Value::Array(items)
        }
        Shape::Map(key, value, _) => {
            let mut map = Map::new();
            for i in 0..list_length(path, input) {
                let key = value_at(key, &format!("{path}[{i}].key"), input);
                let value = value_at(value, &format!("{path}[{i}].value"), input);
                // JSON object keys must be strings.
                let key = match key {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                map.insert(key, value);
            }
            Value::Object(map)
        }
        Shape::Ignored | Shape::Unsupported(_) => Value::Null,
    }
}

/// The choice recorded at `path`, if any.
fn chosen<'a>(path: &str, input: &'a Input) -> Option<&'a str> {
    match input.get(path).map(|leaf| &leaf.value) {
        Some(LeafValue::Choice(choice)) => Some(choice),
        _ => None,
    }
}

/// True if every field name is an index (`0`, `1`, ...): a tuple.
fn is_tuple(fields: &[(String, Shape)]) -> bool {
    fields
        .iter()
        .all(|(name, _)| name.chars().all(|c| c.is_ascii_digit()))
}

/// Named fields become an object, tuple fields an array, and a single tuple
/// field (a newtype like `struct Dna(String)`) just its inner value. An
/// optional property that wasn't chosen is left out.
fn fields_to_json(fields: &[(String, Shape)], path: &str, input: &Input) -> Value {
    if is_tuple(fields) && fields.len() == 1 {
        let (name, field) = &fields[0];
        return value_at(field, &child(path, name), input);
    }

    if is_tuple(fields) {
        let mut items = Vec::new();
        for (name, field) in fields {
            items.push(value_at(field, &child(path, name), input));
        }
        return Value::Array(items);
    }

    let mut object = Map::new();
    for (name, field) in fields {
        let field_path = child(path, name);
        if matches!(field, Shape::Optional(_)) && chosen(&field_path, input) != Some("Some") {
            continue;
        }
        object.insert(name.clone(), value_at(field, &field_path, input));
    }
    Value::Object(object)
}

fn leaf_to_json(value: &LeafValue) -> Value {
    match value {
        LeafValue::Bool(v) => Value::Bool(*v),
        LeafValue::Int(v) => serde_json::to_value(v).expect("i128 fits with arbitrary_precision"),
        LeafValue::UInt(v) => serde_json::to_value(v).expect("u128 fits with arbitrary_precision"),
        LeafValue::Float(v) => match Number::from_f64(*v) {
            Some(number) => Value::Number(number),
            None if v.is_nan() => Value::String("NaN".to_string()),
            None if *v > 0.0 => Value::String("inf".to_string()),
            None => Value::String("-inf".to_string()),
        },
        LeafValue::Char(v) => Value::String(v.to_string()),
        LeafValue::Text(v) => Value::String(v.clone()),
        LeafValue::Choice(v) => Value::String(v.clone()),
        LeafValue::Length(v) => Value::from(*v),
        // Overrides already produce JSON in the saved layout.
        LeafValue::Custom(v) => v.clone(),
    }
}

fn list_length(path: &str, input: &Input) -> usize {
    match input.get(&child(path, "len")).map(|leaf| &leaf.value) {
        Some(LeafValue::Length(len)) => *len,
        _ => 0,
    }
}

// ---------------------------------------------------------------- reading inputs back

/// Reads one input from JSON in the layout above, checking it against the
/// shape: every simple value must have the right type and lie in its range,
/// every required field must be there. The error names the path where the
/// value doesn't fit, e.g. `cfg.retries: expected an integer in 0..=255, got -1`.
///
/// Fields the shape doesn't know are ignored; ignored and unsupported parts
/// of the shape accept anything.
pub fn json_to_input(shape: &Shape, value: &Value) -> Result<Input, String> {
    let mut input = Input::new();
    read_into(shape, "", value, &mut input)?;
    Ok(input)
}

fn read_into(shape: &Shape, path: &str, value: &Value, input: &mut Input) -> Result<(), String> {
    let fail = |expected: String| Err(format!("{}: expected {expected}, got {value}", shown(path)));
    match shape {
        Shape::Simple(simple) => {
            let leaf = read_simple(simple, value).ok_or_else(|| {
                format!(
                    "{}: expected {}, got {value}",
                    shown(path),
                    describe(simple)
                )
            })?;
            input.insert(
                path.to_string(),
                InputLeaf {
                    kind: Kind::Simple(*simple),
                    value: leaf,
                },
            );
        }
        Shape::Fields(fields) => read_fields(fields, path, value, input)?,
        Shape::Enum(variants) => {
            let (variant, payload) = match value {
                Value::String(name) => (name.as_str(), None),
                Value::Object(object) if object.len() == 1 => {
                    let (name, payload) = object.iter().next().expect("one entry");
                    (name.as_str(), Some(payload))
                }
                _ => return fail("an enum variant".to_string()),
            };
            let Some((_, fields)) = variants.iter().find(|(name, _)| name == variant) else {
                let names: Vec<&str> = variants.iter().map(|(name, _)| name.as_str()).collect();
                return fail(format!("one of the variants {}", names.join(", ")));
            };
            input.insert(path.to_string(), choice(variant));
            match payload {
                None if fields.is_empty() => {}
                None => return fail(format!("`{variant}` with its fields")),
                Some(payload) => read_fields(fields, &child(path, variant), payload, input)?,
            }
        }
        Shape::Union(members) => {
            // The first member the value fits.
            let mut reasons = Vec::new();
            for (label, member) in members {
                let mut attempt = Input::new();
                match read_into(member, &child(path, label), value, &mut attempt) {
                    Ok(()) => {
                        input.insert(path.to_string(), choice(label));
                        input.extend(attempt);
                        return Ok(());
                    }
                    Err(reason) => reasons.push(reason),
                }
            }
            return Err(format!(
                "{}: matches none of the alternatives ({})",
                shown(path),
                reasons.join("; ")
            ));
        }
        Shape::Literal(values) => {
            if !values.contains(value) {
                let shown: Vec<String> = values.iter().map(literal_label).collect();
                return fail(format!("one of {}", shown.join(", ")));
            }
            if values.len() > 1 {
                input.insert(path.to_string(), choice(&literal_label(value)));
            }
        }
        Shape::Option(inner) | Shape::Optional(inner) => {
            if value.is_null() {
                input.insert(path.to_string(), choice("None"));
            } else {
                input.insert(path.to_string(), choice("Some"));
                read_into(inner, &child(path, "Some"), value, input)?;
            }
        }
        Shape::List(element, len) => {
            let Some(items) = value.as_array() else {
                return fail("an array".to_string());
            };
            if items.len() < len.min || len.max.is_some_and(|max| items.len() > max) {
                return fail(format!("{} items", describe_len(len)));
            }
            input.insert(child(path, "len"), length(items.len()));
            for (i, item) in items.iter().enumerate() {
                read_into(element, &format!("{path}[{i}]"), item, input)?;
            }
        }
        Shape::Array(element, len) => {
            let Some(items) = value.as_array().filter(|items| items.len() == *len) else {
                return fail(format!("an array of {len} items"));
            };
            for (i, item) in items.iter().enumerate() {
                read_into(element, &format!("{path}[{i}]"), item, input)?;
            }
        }
        Shape::Map(key, item, len) => {
            let Some(object) = value.as_object() else {
                return fail("an object".to_string());
            };
            if object.len() < len.min || len.max.is_some_and(|max| object.len() > max) {
                return fail(format!("{} entries", describe_len(len)));
            }
            input.insert(child(path, "len"), length(object.len()));
            for (i, (name, item_value)) in object.iter().enumerate() {
                // Keys are strings in JSON; a number key is written as text.
                let key_path = format!("{path}[{i}].key");
                let as_text = Value::String(name.clone());
                if read_into(key, &key_path, &as_text, input).is_err() {
                    let parsed: Value = serde_json::from_str(name).map_err(|_| {
                        format!("{}: key `{name}` doesn't fit the key type", shown(path))
                    })?;
                    read_into(key, &key_path, &parsed, input)?;
                }
                read_into(item, &format!("{path}[{i}].value"), item_value, input)?;
            }
        }
        Shape::Custom(name) => {
            input.insert(
                path.to_string(),
                InputLeaf {
                    kind: Kind::Custom(name.clone()),
                    value: LeafValue::Custom(value.clone()),
                },
            );
        }
        Shape::Ignored | Shape::Unsupported(_) => {}
    }
    Ok(())
}

/// The inverse of [`fields_to_json`]: a newtype is its inner value, a tuple
/// an array, anything else an object.
fn read_fields(
    fields: &[(String, Shape)],
    path: &str,
    value: &Value,
    input: &mut Input,
) -> Result<(), String> {
    if is_tuple(fields) && fields.len() == 1 {
        let (name, field) = &fields[0];
        return read_into(field, &child(path, name), value, input);
    }

    if is_tuple(fields) {
        let Some(items) = value.as_array().filter(|items| items.len() == fields.len()) else {
            return Err(format!(
                "{}: expected an array of {} items, got {value}",
                shown(path),
                fields.len()
            ));
        };
        for ((name, field), item) in fields.iter().zip(items) {
            read_into(field, &child(path, name), item, input)?;
        }
        return Ok(());
    }

    let Some(object) = value.as_object() else {
        return Err(format!("{}: expected an object, got {value}", shown(path)));
    };
    for (name, field) in fields {
        let field_path = child(path, name);
        match (object.get(name), field) {
            (Some(field_value), _) => read_into(field, &field_path, field_value, input)?,
            (None, Shape::Optional(_)) => {
                input.insert(field_path, choice("None"));
            }
            (None, Shape::Ignored | Shape::Unsupported(_)) => {}
            (None, _) => return Err(format!("{field_path}: missing")),
        }
    }
    Ok(())
}

fn read_simple(simple: &Simple, value: &Value) -> Option<LeafValue> {
    match *simple {
        Simple::Bool => value.as_bool().map(LeafValue::Bool),
        Simple::UInt { min, max } => {
            let number: u128 = value.as_number()?.to_string().parse().ok()?;
            (min..=max)
                .contains(&number)
                .then_some(LeafValue::UInt(number))
        }
        Simple::Int { min, max } => {
            let number: i128 = value.as_number()?.to_string().parse().ok()?;
            (min..=max)
                .contains(&number)
                .then_some(LeafValue::Int(number))
        }
        Simple::Float {
            min, max, specials, ..
        } => {
            let number = match value {
                Value::Number(number) => number.as_f64()?,
                Value::String(text) if specials => match text.as_str() {
                    "NaN" => f64::NAN,
                    "inf" => f64::INFINITY,
                    "-inf" => f64::NEG_INFINITY,
                    _ => return None,
                },
                _ => return None,
            };
            let fits = !number.is_finite() || (min..=max).contains(&number);
            fits.then_some(LeafValue::Float(number))
        }
        Simple::Char => {
            let mut chars = value.as_str()?.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(LeafValue::Char(c)),
                _ => None,
            }
        }
        Simple::Text { min_len, max_len } => {
            let text = value.as_str()?;
            let len = text.chars().count();
            let fits = len >= min_len && max_len.is_none_or(|max| len <= max);
            fits.then(|| LeafValue::Text(text.to_string()))
        }
    }
}

/// `an integer in 0..=255`, `a string of 1..=8 characters`, ...
fn describe(simple: &Simple) -> String {
    match *simple {
        Simple::Bool => "a boolean".to_string(),
        Simple::UInt { min, max } => format!("an integer in {min}..={max}"),
        Simple::Int { min, max } => format!("an integer in {min}..={max}"),
        Simple::Float { min, max, .. } => format!("a number in {min}..={max}"),
        Simple::Char => "a single character".to_string(),
        Simple::Text { min_len, max_len } => format!(
            "a string of {} characters",
            describe_len(&crate::shape::Len {
                min: min_len,
                max: max_len
            })
        ),
    }
}

fn describe_len(len: &crate::shape::Len) -> String {
    match len.max {
        Some(max) => format!("{}..={max}", len.min),
        None => format!("at least {}", len.min),
    }
}

/// The path for messages: the empty root path reads as `input`.
fn shown(path: &str) -> &str {
    if path.is_empty() { "input" } else { path }
}

fn choice(name: &str) -> InputLeaf {
    InputLeaf {
        kind: Kind::Choice,
        value: LeafValue::Choice(name.to_string()),
    }
}

fn length(len: usize) -> InputLeaf {
    InputLeaf {
        kind: Kind::Length,
        value: LeafValue::Length(len),
    }
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// A name made safe for a file name: `Wrapper<T>::set` -> `Wrapper_T_set`.
pub fn file_name(function_name: &str) -> String {
    let mut name = String::new();
    for c in function_name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            name.push(c);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    name.trim_end_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Primitive;
    use crate::shape::Len;
    use crate::value::{InputLeaf, Kind};

    fn leaf(kind: Kind, value: LeafValue) -> InputLeaf {
        InputLeaf { kind, value }
    }

    fn config_shape() -> Shape {
        Shape::Fields(vec![
            ("name".into(), Shape::Simple(Primitive::String.into())),
            (
                "mode".into(),
                Shape::Enum(vec![
                    ("Off".into(), vec![]),
                    (
                        "Fast".into(),
                        vec![("0".into(), Shape::Simple(Primitive::U8.into()))],
                    ),
                ]),
            ),
            (
                "limit".into(),
                Shape::Option(Box::new(Shape::Simple(Primitive::U128.into()))),
            ),
            (
                "scores".into(),
                Shape::List(
                    Box::new(Shape::Simple(Primitive::F64.into())),
                    Len::default(),
                ),
            ),
        ])
    }

    fn config_input() -> Input {
        let mut input = Input::new();
        let string = Kind::from(Primitive::String);
        input.insert("name".into(), leaf(string, LeafValue::Text("abc".into())));
        input.insert(
            "mode".into(),
            leaf(Kind::Choice, LeafValue::Choice("Fast".into())),
        );
        let u8_kind = Kind::from(Primitive::U8);
        input.insert("mode.Fast.0".into(), leaf(u8_kind, LeafValue::UInt(7)));
        input.insert(
            "limit".into(),
            leaf(Kind::Choice, LeafValue::Choice("Some".into())),
        );
        let u128_kind = Kind::from(Primitive::U128);
        input.insert(
            "limit.Some".into(),
            leaf(u128_kind, LeafValue::UInt(u128::MAX)),
        );
        input.insert(
            "scores.len".into(),
            leaf(Kind::Length, LeafValue::Length(2)),
        );
        let f64_kind = Kind::from(Primitive::F64);
        input.insert(
            "scores[0]".into(),
            leaf(f64_kind.clone(), LeafValue::Float(1.5)),
        );
        input.insert(
            "scores[1]".into(),
            leaf(f64_kind, LeafValue::Float(f64::NAN)),
        );
        input
    }

    #[test]
    fn nests_the_flat_leaves() {
        let json = input_to_json(&config_shape(), &config_input());
        let expected: Value = serde_json::from_str(
            r#"{
                "name": "abc",
                "mode": {"Fast": 7},
                "limit": 340282366920938463463374607431768211455,
                "scores": [1.5, "NaN"]
            }"#,
        )
        .unwrap();
        assert_eq!(json, expected);
    }

    #[test]
    fn deserializes_into_the_real_types() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        enum Mode {
            Off,
            Fast(u8),
        }
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Config {
            name: String,
            mode: Mode,
            limit: Option<u128>,
        }

        let mut input = config_input();
        input.insert(
            "limit".into(),
            leaf(Kind::Choice, LeafValue::Choice("None".into())),
        );
        let json = input_to_json(&config_shape(), &input);
        let config: Config = serde_json::from_value(json).unwrap();
        assert_eq!(
            config,
            Config {
                name: "abc".into(),
                mode: Mode::Fast(7),
                limit: None
            }
        );
    }

    #[test]
    fn reading_back_gives_the_same_input() {
        let shape = config_shape();
        let input = config_input();
        let json = input_to_json(&shape, &input);
        let read = json_to_input(&shape, &json).unwrap();
        // NaN != NaN, so compare the JSON again rather than the leaves.
        assert_eq!(input_to_json(&shape, &read), json);
        assert_eq!(read.len(), input.len());
    }

    #[test]
    fn reading_reports_where_a_value_does_not_fit() {
        let shape = config_shape();
        let bad: Value = serde_json::from_str(
            r#"{"name": "abc", "mode": {"Fast": 300}, "limit": null, "scores": []}"#,
        )
        .unwrap();
        let error = json_to_input(&shape, &bad).unwrap_err();
        assert_eq!(
            error,
            "mode.Fast.0: expected an integer in 0..=255, got 300"
        );

        let missing: Value = serde_json::from_str(r#"{"name": "abc"}"#).unwrap();
        assert_eq!(
            json_to_input(&shape, &missing).unwrap_err(),
            "mode: missing"
        );
    }

    #[test]
    fn optional_properties_are_left_out() {
        let shape = Shape::Fields(vec![
            (
                "a".into(),
                Shape::Optional(Box::new(Shape::Simple(Simple::Bool))),
            ),
            ("b".into(), Shape::Simple(Simple::Bool)),
        ]);
        let json: Value = serde_json::from_str(r#"{"b": true}"#).unwrap();
        let input = json_to_input(&shape, &json).unwrap();
        assert_eq!(input_to_json(&shape, &input), json);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Wrapper<T>::set"), "Wrapper_T_set");
        assert_eq!(file_name("add"), "add");
    }
}
