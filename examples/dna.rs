//! Two corpora for the same function: one with arbitrary strings, one with
//! valid DNA sequences only.
//!
//! Run with `cargo run --example dna`.

use sengi::corpus::{CorpusConfig, build_corpus};
use sengi::overrides::{Override, Overrides};
use sengi::registry::Registry;
use sengi::rng::Rng;
use sengi::save::input_to_json;
use sengi::shape::{function_shape, type_paths};
use serde_json::Value;

const SOURCE: &str = include_str!("../tests/fixtures/dna.rs");

/// A random sequence of 1 to 64 nucleotides.
fn random_dna(rng: &mut Rng) -> Value {
    let len = rng.range(1, 64) as usize;
    let mut sequence = String::new();
    for _ in 0..len {
        sequence.push(*rng.pick(&['A', 'C', 'G', 'T']));
    }
    Value::String(sequence)
}

/// Fraction of the sequence that is G or C.
fn gc_fraction(sequence: &str) -> f64 {
    let gc = sequence.chars().filter(|c| *c == 'G' || *c == 'C').count();
    gc as f64 / sequence.len().max(1) as f64
}

fn dna_override() -> Override {
    Override::new("dna", random_dna)
        .distance("length", |a, b| {
            let (a, b) = (a.as_str().unwrap().len(), b.as_str().unwrap().len());
            a.abs_diff(b) as f64 / a.max(b).max(1) as f64
        })
        .distance("gc_content", |a, b| {
            (gc_fraction(a.as_str().unwrap()) - gc_fraction(b.as_str().unwrap())).abs()
        })
        .category("starts_with_atg", |v| {
            v.as_str().unwrap().starts_with("ATG").to_string()
        })
        // length and GC content equally important, start codon twice as much.
        .comparisons(vec![
            vec![1.0, 1.0, 0.5],
            vec![1.0, 1.0, 0.5],
            vec![2.0, 2.0, 1.0],
        ])
}

fn main() {
    println!("Types in the source:");
    for parsed in sengi::parsed_types(SOURCE).unwrap() {
        println!("  {:?} {}", parsed.kind, parsed.name);
    }

    let functions = sengi::function_inputs(SOURCE).unwrap();
    let gc_content = functions.iter().find(|f| f.name == "gc_content").unwrap();

    println!("\nPaths in gc_content's inputs:");
    for (path, ty) in type_paths(gc_content) {
        println!("  {path}: {ty}");
    }

    let config = CorpusConfig::default();

    // 1. No overrides: DnaSequence is broken down to a plain String.
    let none = Overrides::new();
    let registry = Registry::with_overrides(&none).unwrap();
    let shape = function_shape(gc_content, &none);
    let any = build_corpus(&shape, &registry, &none, &config);
    println!("\nArbitrary strings: {} inputs", any.inputs.len());
    for input in any.inputs.iter().take(3) {
        println!("  {}", input_to_json(&shape, input));
    }

    // 2. DnaSequence overridden: valid sequences, DNA-specific parameters.
    let mut dna = Overrides::new();
    dna.add(dna_override());
    dna.use_for_type("DnaSequence", "dna");
    let registry = Registry::with_overrides(&dna).unwrap();
    let shape = function_shape(gc_content, &dna);
    let valid = build_corpus(&shape, &registry, &dna, &config);
    println!("\nValid DNA: {} inputs", valid.inputs.len());
    for input in valid.inputs.iter().take(3) {
        println!("  {}", input_to_json(&shape, input));
    }
}
