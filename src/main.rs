//! The `sengi` command line tool.
//!
//! ```text
//! sengi generate <file.rs | schema.json> [--theta <theta>] [--out <dir>]
//! sengi analyze <inputs.json> [--schema <schema.json> | --rust <file.rs> --function <name>] [--theta <theta>] [--out <dir>]
//! sengi schema <file.rs> [--out <dir>]
//! sengi inspect <file.rs>
//! ```

use std::path::{Path, PathBuf};
use std::{fs, process};

use clap::{Parser, Subcommand};
use sengi::Function;
use sengi::analysis::{analysis_to_json, analyze, read_inputs};
use sengi::corpus::{CorpusConfig, build_corpus};
use sengi::overrides::Overrides;
use sengi::registry::Registry;
use sengi::report::{Generation, report_data, save_report};
use sengi::save::{file_name, save_corpus, write_json};
use sengi::schema::{document, shape_from_schema};
use sengi::shape::{Shape, function_shape};
use sengi::stats::stats;
use sengi::value::format_input;
use serde_json::Value;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build and save a corpus for every function in a Rust file, or for the
    /// inputs a JSON Schema describes.
    ///
    /// A `.json` file is read as a JSON Schema; anything else as Rust source.
    /// Each corpus is saved to `<out>/<name>.json`, with an HTML report next
    /// to it at `<out>/<name>.html`. For each it prints how many inputs were
    /// kept, why they were kept (new categories or distance), the
    /// pair-distance statistics, anything that couldn't be generated, and the
    /// first few inputs.
    Generate {
        /// The Rust source file or JSON Schema to read.
        file: PathBuf,
        /// Minimum distance for keeping an input (see `CorpusConfig::theta`).
        #[arg(long, default_value_t = CorpusConfig::default().theta)]
        theta: f64,
        /// Directory to save corpora and reports to.
        #[arg(long, default_value = "corpus")]
        out: PathBuf,
    },
    /// Measure a set of existing inputs: their categories, how they differ,
    /// and which ones are near-duplicates.
    ///
    /// The inputs file is a JSON array of inputs, or a corpus sengi saved
    /// (which carries its own schema). The inputs are checked against a JSON
    /// Schema or a Rust function's parameter types. Writes
    /// `<out>/<name>.analysis.json` and an HTML report `<out>/<name>.html`.
    Analyze {
        /// The JSON file with the inputs.
        inputs: PathBuf,
        /// The JSON Schema the inputs follow.
        #[arg(long, conflicts_with = "rust")]
        schema: Option<PathBuf>,
        /// A Rust source file whose function's parameters the inputs are for.
        #[arg(long, requires = "function")]
        rust: Option<PathBuf>,
        /// The function in `--rust`, e.g. `configure` or `Config::new`.
        #[arg(long, requires = "rust")]
        function: Option<String>,
        /// How different two inputs must be to not be near-duplicates.
        #[arg(long, default_value_t = CorpusConfig::default().theta)]
        theta: f64,
        /// Directory to save the analysis and report to.
        #[arg(long, default_value = "analysis")]
        out: PathBuf,
    },
    /// Write a JSON Schema of each function's inputs in a Rust file.
    ///
    /// Saved to `<out>/<function>.schema.json`. The schema describes the
    /// layout `generate` writes inputs in, and `generate` and `analyze` can
    /// read it back.
    Schema {
        /// The Rust source file to read.
        file: PathBuf,
        /// Directory to save the schemas to.
        #[arg(long, default_value = "schema")]
        out: PathBuf,
    },
    /// Print every function in a Rust file with its input types.
    ///
    /// For each input it prints the resolved type tree and the simple-type
    /// leaves it breaks down into, e.g. `cfg*.mode::Fast.0 -> U8`.
    Inspect {
        /// The Rust source file to read.
        file: PathBuf,
    },
}

/// Something to generate inputs for: a function, or a schema.
struct Subject {
    /// Used for file names, e.g. `configure`.
    name: String,
    /// Shown as a heading, e.g. `configure(&Config, bool)`.
    label: String,
    shape: Shape,
}

fn main() {
    match Cli::parse().command {
        Command::Generate { file, theta, out } => generate(&file, theta, &out),
        Command::Analyze {
            inputs,
            schema,
            rust,
            function,
            theta,
            out,
        } => analyze_inputs(
            &inputs,
            schema.as_deref(),
            rust.as_deref(),
            function,
            theta,
            &out,
        ),
        Command::Schema { file, out } => export_schemas(&file, &out),
        Command::Inspect { file } => inspect(&file),
    }
}

fn generate(path: &Path, theta: f64, out_dir: &Path) {
    // The CLI has no way to declare overrides yet; use the library for that.
    let overrides = Overrides::new();
    let subjects = if is_json(path) {
        vec![schema_subject(path, &read_json(path), &overrides)]
    } else {
        read_functions(path)
            .iter()
            .map(|function| function_subject(function, &overrides))
            .collect()
    };
    let registry = registry(&overrides);
    let config = CorpusConfig {
        theta,
        ..CorpusConfig::default()
    };

    for subject in &subjects {
        let shape = &subject.shape;
        let corpus = build_corpus(shape, &registry, &overrides, &config);
        let stats = stats(&corpus, &registry);

        println!("{}", subject.label);
        println!(
            "  {} inputs from {} attempts ({:.1}% accepted, stopped: {:?})",
            stats.size,
            stats.attempts,
            stats.acceptance_rate * 100.0,
            corpus.stop_reason
        );
        println!(
            "  kept for new categories: {}, for distance: {} ({} categories/pairs covered)",
            stats.kept_for_categories, stats.kept_for_distance, stats.covered
        );
        println!(
            "  pair distance: min {:.3}, mean {:.3}",
            stats.min_pair_distance, stats.mean_pair_distance
        );
        for unsupported in shape.unsupported() {
            println!("  skipped: {unsupported}");
        }
        for input in corpus.inputs.iter().take(3) {
            println!("    {}", format_input(input));
        }
        match save_corpus(out_dir, &subject.name, shape, &corpus, &config, &stats) {
            Ok(saved) => println!("  saved to {}", saved.display()),
            Err(e) => eprintln!("  failed to save: {e}"),
        }
        let generation = Generation {
            corpus: &corpus,
            config: &config,
            stats: &stats,
        };
        let data = report_data(
            &subject.label,
            shape,
            &corpus.inputs,
            &registry,
            theta,
            Some(generation),
        );
        match save_report(out_dir, &subject.name, &data) {
            Ok(saved) => println!("  report:  {}", saved.display()),
            Err(e) => eprintln!("  failed to save report: {e}"),
        }
        println!();
    }
}

fn analyze_inputs(
    path: &Path,
    schema: Option<&Path>,
    rust: Option<&Path>,
    function: Option<String>,
    theta: f64,
    out_dir: &Path,
) {
    let overrides = Overrides::new();
    let document = read_json(path);
    // A plain array of inputs, or a corpus sengi saved.
    let (values, embedded) = match &document {
        Value::Array(values) => (values.clone(), None),
        Value::Object(object) => match object.get("inputs").and_then(Value::as_array) {
            Some(values) => (values.clone(), object.get("schema")),
            None => fail(&format!(
                "{}: expected an array of inputs, or an object with `inputs`",
                path.display()
            )),
        },
        _ => fail(&format!("{}: expected an array of inputs", path.display())),
    };

    let mut subject = match (schema, rust, embedded) {
        (Some(schema), _, _) => schema_subject(schema, &read_json(schema), &overrides),
        (None, Some(rust), _) => {
            let name = function.expect("clap requires --function with --rust");
            let functions = read_functions(rust);
            let Some(found) = functions.iter().find(|f| f.name == name) else {
                let names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
                fail(&format!(
                    "no function `{name}` in {}; found: {}",
                    rust.display(),
                    names.join(", ")
                ));
            };
            function_subject(found, &overrides)
        }
        (None, None, Some(embedded)) => {
            let name = document["name"].as_str().map(str::to_string);
            let name = name.unwrap_or_else(|| file_stem(path));
            Subject {
                label: name.clone(),
                name,
                shape: shape_from_schema(embedded, &overrides),
            }
        }
        (None, None, None) => fail(
            "the inputs need a schema: pass --schema <schema.json> or --rust <file.rs> --function <name>",
        ),
    };
    if schema.is_some() && rust.is_none() && embedded.is_none() {
        // Name the output after the inputs, not the schema, when they differ.
        subject.name = file_stem(path);
    }

    let registry = registry(&overrides);
    let inputs = read_inputs(&subject.shape, &values)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let analysis = analyze(inputs, &registry, theta);

    println!("{}", subject.label);
    println!(
        "  {} inputs, {} categories/pairs covered",
        analysis.inputs.len(),
        analysis.covered
    );
    println!(
        "  pair distance: min {:.3}, mean {:.3}",
        analysis.min_pair_distance, analysis.mean_pair_distance
    );
    for unsupported in subject.shape.unsupported() {
        println!("  not measured: {unsupported}");
    }
    let duplicates = &analysis.near_duplicates;
    println!("  near-duplicates: {} pairs", duplicates.len());
    for pair in duplicates.iter().take(10) {
        let closest: Vec<String> = pair
            .closest
            .iter()
            .map(|d| {
                format!(
                    "{} / {} {:.2} < {:.2}",
                    d.path, d.param, d.distance, d.threshold
                )
            })
            .collect();
        let closest = if closest.is_empty() {
            "identical".to_string()
        } else {
            closest.join(", ")
        };
        println!(
            "    #{} ~ #{} (distance {:.3}): {closest}",
            pair.a + 1,
            pair.b + 1,
            pair.distance
        );
    }
    if duplicates.len() > 10 {
        println!("    ... and {} more", duplicates.len() - 10);
    }

    let json = analysis_to_json(&subject.name, &subject.shape, &analysis, theta);
    let file = format!("{}.analysis.json", file_name(&subject.name));
    match write_json(out_dir, &file, &json) {
        Ok(saved) => println!("  analysis: {}", saved.display()),
        Err(e) => eprintln!("  failed to save analysis: {e}"),
    }
    let data = report_data(
        &subject.label,
        &subject.shape,
        &analysis.inputs,
        &registry,
        theta,
        None,
    );
    match save_report(out_dir, &subject.name, &data) {
        Ok(saved) => println!("  report:   {}", saved.display()),
        Err(e) => eprintln!("  failed to save report: {e}"),
    }
}

fn export_schemas(path: &Path, out_dir: &Path) {
    let overrides = Overrides::new();
    for function in &read_functions(path) {
        let shape = function_shape(function, &overrides);
        let schema = document(&function.name, &shape);
        let file = format!("{}.schema.json", file_name(&function.name));
        match write_json(out_dir, &file, &schema) {
            Ok(saved) => println!("{}: {}", function.name, saved.display()),
            Err(e) => eprintln!("{}: failed to save: {e}", function.name),
        }
        for unsupported in shape.unsupported() {
            println!("  unsupported: {unsupported}");
        }
    }
}

fn inspect(path: &Path) {
    for f in &read_functions(path) {
        println!("{}({})", f.name, f.input_types().join(", "));
        for p in &f.params {
            println!("  {}: {}", p.name, p.ty);
            for line in p.node.tree().lines() {
                println!("    {line}");
            }
            println!("    leaves:");
            for leaf in p.node.leaves() {
                let kind = match leaf.primitive() {
                    Some(prim) => format!("{prim:?}"),
                    None => format!("non-primitive `{}`", leaf.node),
                };
                println!("      {} -> {kind}", leaf.path_string(&p.name));
            }
        }
        println!();
    }
}

fn function_subject(function: &Function, overrides: &Overrides) -> Subject {
    Subject {
        name: function.name.clone(),
        label: format!("{}({})", function.name, function.input_types().join(", ")),
        shape: function_shape(function, overrides),
    }
}

/// Named after the schema's `title`, or else the file.
fn schema_subject(path: &Path, schema: &Value, overrides: &Overrides) -> Subject {
    let name = match schema.get("title").and_then(Value::as_str) {
        Some(title) => title.to_string(),
        None => file_stem(path),
    };
    Subject {
        label: name.clone(),
        name,
        shape: shape_from_schema(schema, overrides),
    }
}

fn registry(overrides: &Overrides) -> Registry {
    Registry::with_overrides(overrides)
        .unwrap_or_else(|e| fail(&format!("invalid parameter registry: {e}")))
}

/// Read and parse a Rust file, exiting with a message on failure.
fn read_functions(path: &Path) -> Vec<Function> {
    let display = path.display();
    let src = fs::read_to_string(path)
        .unwrap_or_else(|e| fail(&format!("failed to read {display}: {e}")));
    sengi::function_inputs(&src)
        .unwrap_or_else(|e| fail(&format!("failed to parse {display}: {e}")))
}

/// Read and parse a JSON file, exiting with a message on failure.
fn read_json(path: &Path) -> Value {
    let display = path.display();
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| fail(&format!("failed to read {display}: {e}")));
    serde_json::from_str(&text).unwrap_or_else(|e| fail(&format!("failed to parse {display}: {e}")))
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "json")
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "inputs".to_string())
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    process::exit(1);
}
