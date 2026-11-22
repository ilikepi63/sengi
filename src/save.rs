//! Save a corpus as JSON, one file per function.
//!
//! Each input is written as nested JSON in serde's default layout, so a test
//! can deserialize it straight into the real argument types:
//!
//! ```text
//! struct / tuple struct   {"name": "abc", "retries": 3}   /   [1, 2]
//! newtype struct          the inner value
//! enum variant            "Off"   {"Fast": 7}   {"Custom": {"level": -1, "label": ""}}
//! Option                  null   or the inner value
//! Vec / array / tuple     [ ... ]
//! map                     {"key": value, ...}   (keys written as strings)
//! ```
//!
//! JSON has no NaN or infinity, so those floats are written as the strings
//! `"NaN"`, `"inf"` and `"-inf"`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Number, Value, json};

use crate::Function;
use crate::corpus::{Corpus, CorpusConfig};
use crate::shape::Shape;
use crate::stats::Stats;
use crate::value::{Input, LeafValue};

/// Writes `<dir>/<function name>.json` and returns its path.
pub fn save_corpus(
    dir: &Path,
    function: &Function,
    shape: &Shape,
    corpus: &Corpus,
    config: &CorpusConfig,
    stats: &Stats,
) -> io::Result<PathBuf> {
    let json = corpus_to_json(function, shape, corpus, config, stats);
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", file_name(&function.name)));
    let text = serde_json::to_string_pretty(&json).expect("JSON values always serialize");
    fs::write(&path, text)?;
    Ok(path)
}

/// The whole JSON document for one function's corpus:
/// `function`, `params`, `config`, `stats` and `inputs`.
pub fn corpus_to_json(
    function: &Function,
    shape: &Shape,
    corpus: &Corpus,
    config: &CorpusConfig,
    stats: &Stats,
) -> Value {
    let mut params = Vec::new();
    for param in &function.params {
        params.push(json!({ "name": param.name, "type": param.ty }));
    }

    let mut inputs = Vec::new();
    for input in &corpus.inputs {
        inputs.push(input_to_json(shape, input));
    }

    json!({
        "function": function.name,
        "params": params,
        "config": { "theta": config.theta, "seed": config.seed },
        "stats": stats,
        "inputs": inputs,
    })
}

/// Rebuilds the nested value of one input from its flat leaf map, following
/// the same paths the fuzzer used to generate it.
pub fn input_to_json(shape: &Shape, input: &Input) -> Value {
    value_at(shape, "", input)
}

fn value_at(shape: &Shape, path: &str, input: &Input) -> Value {
    match shape {
        Shape::Simple(_) => match input.get(path) {
            Some(leaf) => leaf_to_json(&leaf.value),
            None => Value::Null,
        },
        Shape::Fields(fields) => fields_to_json(fields, path, input),
        Shape::Enum(_) => {
            let Some(LeafValue::Choice(variant)) = input.get(path).map(|leaf| &leaf.value) else {
                return Value::Null;
            };
            let Shape::Enum(variants) = shape else {
                unreachable!()
            };
            let (_, fields) = variants
                .iter()
                .find(|(name, _)| name == variant)
                .expect("generated variants come from the shape");
            if fields.is_empty() {
                // Unit variant: just its name.
                return Value::String(variant.clone());
            }
            let mut tagged = Map::new();
            let variant_path = child(path, variant);
            tagged.insert(
                variant.clone(),
                fields_to_json(fields, &variant_path, input),
            );
            Value::Object(tagged)
        }
        Shape::Option(inner) => {
            let Some(LeafValue::Choice(choice)) = input.get(path).map(|leaf| &leaf.value) else {
                return Value::Null;
            };
            if choice == "Some" {
                value_at(inner, &child(path, "Some"), input)
            } else {
                Value::Null
            }
        }
        Shape::List(element) => {
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
        Shape::Map(key, value) => {
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
        Shape::Custom(_) => match input.get(path) {
            Some(leaf) => leaf_to_json(&leaf.value),
            None => Value::Null,
        },
        Shape::Ignored | Shape::Unsupported(_) => Value::Null,
    }
}

/// Named fields become an object, tuple fields an array, and a single tuple
/// field (a newtype like `struct Dna(String)`) just its inner value.
fn fields_to_json(fields: &[(String, Shape)], path: &str, input: &Input) -> Value {
    let is_tuple = fields
        .iter()
        .all(|(name, _)| name.chars().all(|c| c.is_ascii_digit()));

    if is_tuple && fields.len() == 1 {
        let (name, field) = &fields[0];
        return value_at(field, &child(path, name), input);
    }

    if is_tuple {
        let mut items = Vec::new();
        for (name, field) in fields {
            items.push(value_at(field, &child(path, name), input));
        }
        return Value::Array(items);
    }

    let mut object = Map::new();
    for (name, field) in fields {
        object.insert(name.clone(), value_at(field, &child(path, name), input));
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

fn child(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// `Wrapper<T>::set` -> `Wrapper_T_set`
pub(crate) fn file_name(function_name: &str) -> String {
    let mut name = String::new();
    for c in function_name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
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
    use crate::value::{InputLeaf, Kind};

    fn leaf(kind: Kind, value: LeafValue) -> InputLeaf {
        InputLeaf { kind, value }
    }

    fn config_shape() -> Shape {
        Shape::Fields(vec![
            ("name".into(), Shape::Simple(Primitive::String)),
            (
                "mode".into(),
                Shape::Enum(vec![
                    ("Off".into(), vec![]),
                    (
                        "Fast".into(),
                        vec![("0".into(), Shape::Simple(Primitive::U8))],
                    ),
                ]),
            ),
            (
                "limit".into(),
                Shape::Option(Box::new(Shape::Simple(Primitive::U128))),
            ),
            (
                "scores".into(),
                Shape::List(Box::new(Shape::Simple(Primitive::F64))),
            ),
        ])
    }

    fn config_input() -> Input {
        let mut input = Input::new();
        let string = Kind::Primitive(Primitive::String);
        input.insert("name".into(), leaf(string, LeafValue::Text("abc".into())));
        input.insert(
            "mode".into(),
            leaf(Kind::Choice, LeafValue::Choice("Fast".into())),
        );
        let u8_kind = Kind::Primitive(Primitive::U8);
        input.insert("mode.Fast.0".into(), leaf(u8_kind, LeafValue::UInt(7)));
        input.insert(
            "limit".into(),
            leaf(Kind::Choice, LeafValue::Choice("Some".into())),
        );
        let u128_kind = Kind::Primitive(Primitive::U128);
        input.insert(
            "limit.Some".into(),
            leaf(u128_kind, LeafValue::UInt(u128::MAX)),
        );
        input.insert(
            "scores.len".into(),
            leaf(Kind::Length, LeafValue::Length(2)),
        );
        let f64_kind = Kind::Primitive(Primitive::F64);
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
    fn file_names_are_safe() {
        assert_eq!(file_name("Wrapper<T>::set"), "Wrapper_T_set");
        assert_eq!(file_name("add"), "add");
    }
}
