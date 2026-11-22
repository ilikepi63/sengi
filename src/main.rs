//! The `sengi` command line tool.
//!
//! ```text
//! sengi generate <file.rs> [--theta <theta>] [--out <dir>]
//! sengi inspect <file.rs>
//! ```
//!
//! `generate` builds and saves a corpus for every function in the file;
//! `inspect` prints how each function's input types break down.

use std::path::{Path, PathBuf};
use std::{fs, process};

use clap::{Parser, Subcommand};
use sengi::Function;
use sengi::corpus::{CorpusConfig, build_corpus};
use sengi::overrides::Overrides;
use sengi::registry::Registry;
use sengi::report::save_report;
use sengi::save::save_corpus;
use sengi::shape::function_shape;
use sengi::stats::stats;
use sengi::value::format_input;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build and save a corpus for every function in a Rust file.
    ///
    /// Each corpus is saved to `<out>/<function>.json`, with an HTML report
    /// next to it at `<out>/<function>.html`. For each function it prints how
    /// many inputs were kept, why they were kept (new categories or
    /// distance), the pair-distance statistics, anything that couldn't be
    /// generated, and the first few inputs.
    Generate {
        /// The Rust source file to read.
        file: PathBuf,
        /// Minimum distance for keeping an input (see `CorpusConfig::theta`).
        #[arg(long, default_value_t = CorpusConfig::default().theta)]
        theta: f64,
        /// Directory to save corpora and reports to.
        #[arg(long, default_value = "corpus")]
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

fn main() {
    match Cli::parse().command {
        Command::Generate { file, theta, out } => generate(&file, theta, &out),
        Command::Inspect { file } => inspect(&file),
    }
}

fn generate(path: &Path, theta: f64, out_dir: &Path) {
    let functions = read_functions(path);
    // The CLI has no way to declare overrides yet; use the library for that.
    let overrides = Overrides::new();
    let registry = Registry::new().unwrap_or_else(|e| {
        eprintln!("invalid parameter registry: {e}");
        process::exit(1);
    });
    let config = CorpusConfig {
        theta,
        ..CorpusConfig::default()
    };

    for function in &functions {
        let shape = function_shape(function, &overrides);
        let corpus = build_corpus(&shape, &registry, &overrides, &config);
        let stats = stats(&corpus, &registry);

        println!("{}({})", function.name, function.input_types().join(", "));
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
        match save_corpus(out_dir, function, &shape, &corpus, &config, &stats) {
            Ok(saved) => println!("  saved to {}", saved.display()),
            Err(e) => eprintln!("  failed to save: {e}"),
        }
        match save_report(
            out_dir, function, &shape, &corpus, &registry, &config, &stats,
        ) {
            Ok(saved) => println!("  report:  {}", saved.display()),
            Err(e) => eprintln!("  failed to save report: {e}"),
        }
        println!();
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

/// Read and parse a Rust file, exiting with a message on failure.
fn read_functions(path: &Path) -> Vec<Function> {
    let display = path.display();
    let src = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {display}: {e}");
        process::exit(1);
    });
    sengi::function_inputs(&src).unwrap_or_else(|e| {
        eprintln!("failed to parse {display}: {e}");
        process::exit(1);
    })
}
