# sengi

Generator for testing inputs, maximizing user defined differences.

sengi works out what an input is made of — from Rust source or a JSON Schema —
and generates inputs for it, keeping only the ones that are meaningfully
different from everything kept so far. The result is a small, well-spread set
of inputs, saved as JSON your tests can deserialize straight into the real
types. It can also take a set of inputs you already have and measure how they
differ, what they cover, and which ones test the same thing.

sengi only produces and measures inputs. What your tests do with them —
assertions, snapshots, property checks, comparing two implementations — is up
to you.

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [Command line](#command-line)
- [Output](#output)
- [JSON Schema](#json-schema)
- [Analysing existing inputs](#analysing-existing-inputs)
- [How inputs are chosen](#how-inputs-are-chosen)
- [Supported Rust types](#supported-rust-types)
- [Built-in parameters](#built-in-parameters)
- [Library](#library)
- [Overrides](#overrides)
- [Weights and thresholds](#weights-and-thresholds)
- [Limitations](#limitations)
- [Examples](#examples)
- [License](#license)

## Install

```sh
cargo install sengi      # the command line tool
cargo add sengi          # the library
```

## Quick start

### From Rust source

Given `src/config.rs`:

```rust
pub enum Mode { Off, Fast(u8), Custom { level: i64 } }
pub struct Config { pub name: String, pub retries: u32, pub mode: Mode }

pub fn configure(cfg: &Config, verbose: bool) { /* ... */ }
```

```sh
sengi generate src/config.rs
```

```text
configure(&Config, bool)
  200 inputs from 218 attempts (91.7% accepted, stopped: Full)
  kept for new categories: 47, for distance: 153 (145 categories/pairs covered)
  pair distance: min 0.000, mean 0.607
    cfg.mode = Off, cfg.name = "\t\r𢈙…", cfg.retries = 0, verbose = true
    cfg.mode = Fast, cfg.mode.Fast.0 = 0, cfg.name = "  i tq", cfg.retries = 4294967295, verbose = true
    ...
  saved to corpus/configure.json
  report:  corpus/configure.html
```

### From a JSON Schema

`user.schema.json`:

```json
{
  "title": "user",
  "type": "object",
  "properties": {
    "name": { "type": "string", "minLength": 1, "maxLength": 40 },
    "age": { "type": "integer", "minimum": 0, "maximum": 150 },
    "role": { "enum": ["admin", "member", "guest"] },
    "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 5 }
  },
  "required": ["name", "age", "role"]
}
```

```sh
sengi generate user.schema.json      # → corpus/user.json, corpus/user.html
```

### Measuring inputs you already have

```sh
sengi analyze tests.json --schema user.schema.json
```

```text
user
  4 inputs, 11 categories/pairs covered
  pair distance: min 0.000, mean 0.408
  near-duplicates: 3 pairs
    #1 ~ #2 (distance 0.119): name / bytes 0.33 < 0.50, name / length 0.17 < 0.50, age / bits 0.12 < 1.41
    #1 ~ #3 (distance 0.000): identical
    ...
  analysis: analysis/tests.analysis.json
  report:   analysis/tests.html
```

## Command line

### `sengi generate`

```text
sengi generate <FILE> [--theta <THETA>] [--out <OUT>]
```

Builds and saves a corpus. A `.json` `FILE` is read as a
[JSON Schema](#json-schema) and gives one corpus, named after the schema's
`title` (or the file). Anything else is read as Rust source and gives one
corpus per function.

| Option | Default | Meaning |
|---|---|---|
| `--theta` | `0.5` | How different (0–1) a new input must be to be kept. Higher → fewer, more spread-out inputs. See [θ](#θ). |
| `--out` | `corpus` | Directory for `<name>.json` and `<name>.html`. Created if missing. |

For each corpus it prints:

- how many inputs were kept, from how many attempts, and why it stopped
  (`Full`, `Saturated` or `OutOfAttempts` — see [Stopping](#stopping));
- how many were kept for new categories vs. distance;
- the min and mean pair distance;
- any part that couldn't be generated (`skipped: ...`);
- the first three inputs.

### `sengi analyze`

```text
sengi analyze <INPUTS> [--schema <FILE> | --rust <FILE> --function <NAME>] [--theta <THETA>] [--out <OUT>]
```

Measures a set of existing inputs. See
[Analysing existing inputs](#analysing-existing-inputs).

| Option | Default | Meaning |
|---|---|---|
| `--schema` | – | The JSON Schema the inputs follow. |
| `--rust`, `--function` | – | A Rust file and the function (`configure`, `Config::new`) whose parameters the inputs are for. |
| `--theta` | `0.5` | How different two inputs must be to not count as near-duplicates. |
| `--out` | `analysis` | Directory for `<name>.analysis.json` and `<name>.html`. |

`INPUTS` is a JSON array of inputs, or a file sengi saved — which carries its
own schema, so `--schema` isn't needed. Every input is checked first; the
first one that doesn't fit stops the run with its position and path:

```text
tests.json: input 1: name: expected a string of 1..=40 characters, got ""
```

### `sengi schema`

```text
sengi schema <FILE.rs> [--out <OUT>]
```

Writes a JSON Schema of each function's inputs to
`<out>/<function>.schema.json` (default `schema/`). The schema describes
exactly the layout `generate` writes, so it can be shared with other tools or
languages, edited (narrow a range, add an override), and fed back to
`generate` or `analyze`.

### `sengi inspect`

```text
sengi inspect <FILE.rs>
```

Prints every function with its resolved type trees and the simple types
("leaves") each argument breaks down into. Nothing is generated. Use it to
check how sengi sees your types and to find paths for
[overrides](#overrides).

```text
configure(&Config, bool)
  cfg: &Config
    &Config
      Config (struct)
        name:
          String
        retries:
          u32
        mode:
          Mode (enum)
            variant Off
            variant Fast
              0:
                u8
            variant Custom
              level:
                i64
    leaves:
      cfg*.name -> String
      cfg*.retries -> U32
      cfg*.mode::Off -> non-primitive `Mode`
      cfg*.mode::Fast.0 -> U8
      cfg*.mode::Custom.level -> I64
  verbose: bool
    bool
    leaves:
      verbose -> Bool
```

`*` marks a step through a reference, `::` an enum variant.

## Output

### Corpus JSON

One file per function or schema (`Wrapper<T>::set` → `Wrapper_T_set.json`):

```json
{
  "name": "configure",
  "schema": { "type": "object", "properties": { "cfg": { "...": "..." }, "verbose": { "type": "boolean" } } },
  "config": { "theta": 0.5, "seed": 0 },
  "stats": { "size": 200, "attempts": 218, "acceptance_rate": 0.917, "...": "..." },
  "inputs": [
    { "cfg": { "name": "  i tq", "retries": 4294967295, "mode": { "Fast": 0 } }, "verbose": true },
    { "cfg": { "name": "", "retries": 27, "mode": "Off" }, "verbose": false }
  ]
}
```

`schema` is the inputs' [JSON Schema](#json-schema), so the file is
self-describing: `sengi analyze` reads it without `--schema`.

For a Rust function each input is an object keyed by parameter name; for a
schema it is a value of that schema. Values use serde's default layout, so
`#[derive(Deserialize)]` types read them directly:

| Rust / JSON Schema | JSON |
|---|---|
| struct, object | `{"name": "abc", "retries": 3}` |
| tuple struct, tuple, `prefixItems` | `[1, 2]` |
| newtype struct (`struct Dna(String)`) | the inner value: `"ACGT"` |
| unit variant | `"Off"` |
| tuple / newtype variant | `{"Fast": 7}` |
| struct variant | `{"Custom": {"level": -1}}` |
| `Option`, nullable | `null` or the inner value |
| property not in `required` | left out, or the value |
| `oneOf` / `anyOf` | the chosen alternative's value |
| `enum` / `const` | one of the listed values |
| `Vec`, slice, array, set | `[ ... ]` |
| map, `additionalProperties` | `{"key": value}` (keys written as strings) |
| `NaN`, `∞`, `-∞` (Rust floats only) | `"NaN"`, `"inf"`, `"-inf"` (JSON has no such numbers) |

References, `Box`, `Rc` and `Arc` are transparent: `&Config` is written as a
`Config`.

### Using the inputs in a test

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct Case { cfg: Config, verbose: bool }

#[derive(Deserialize)]
struct Corpus { inputs: Vec<Case> }

#[test]
fn configure_never_panics() {
    let corpus: Corpus = serde_json::from_str(include_str!("../corpus/configure.json")).unwrap();
    for case in corpus.inputs {
        configure(&case.cfg, case.verbose);
    }
}
```

Rust floats that may be `NaN` or infinite need a custom deserializer for the
string forms above. Map keys are always strings, so non-string keys need one
too.

### HTML report

A self-contained page per corpus or analysis:

- **Tested model** — what is being tested: the inputs' JSON Schema, next to
  every leaf (each simple value, choice and collection length) with its type
  and range and the parameters derived from it — distance or category, AHP
  weight, and threshold at θ.
- **Distance heatmap** — every pair of inputs, coloured by weighted mean
  distance. Light cells off the diagonal are near-duplicates.
- **Corpus map** — inputs laid out in 2-D (classical multidimensional
  scaling) so dot distance approximates input distance. An even cloud means a
  well-spread set. Generated inputs are coloured by why they were kept;
  analysed ones by whether they're part of a near-duplicate pair.
- **What makes inputs different** — an icicle chart of input → leaf →
  parameter, each block sized by its share of all differences.
- **Growth** (generated corpora) — corpus size against attempts. A flat tail
  means saturation.
- **Near-duplicates** — pairs that test the same thing, with the parameters
  that came closest to telling them apart (distance / threshold).
- **All inputs** — with each one's nearest neighbour.

## JSON Schema

A JSON Schema describes inputs without any source code, so sengi works for
anything that takes JSON: other languages, services, config files. Schemas
follow draft 2020-12; what [`schemars`](https://crates.io/crates/schemars)
generates for serde types works as-is, and inputs come out in exactly the
layout the schema describes.

### Supported keywords

| JSON Schema | Generates |
|---|---|
| `"type": "boolean"` | `true` / `false` |
| `"type": "integer"`, `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` | integers in range (default: the `i64` range) |
| integer `format`: `int8`…`int128`, `uint8`…`uint128`, `uint` | that type's range, unless bounds are given |
| `"type": "number"`, the same bounds, `format: "float"` | finite numbers in range |
| `"type": "string"`, `minLength`, `maxLength` | strings of those lengths (default: 0–64 characters) |
| `"type": "object"`, `properties`, `required` | objects; properties not in `required` are sometimes left out |
| `"type": "object"`, `additionalProperties: {...}` (no `properties`), `minProperties`, `maxProperties` | maps with string keys |
| `"type": "array"`, `items`, `minItems`, `maxItems` | arrays (default: 0–16 items) |
| `prefixItems` (or an `items` array) | tuples |
| `enum`, `const`, `"type": "null"` | one of the values |
| `oneOf`, `anyOf` | one alternative's value; `[X, null]` is treated like `Option` |
| `"type": ["string", "null"]` and other type lists | one of the types |
| `$ref` to `#/$defs/...` or `#/definitions/...` | the referenced schema |
| `allOf` with one schema | that schema |
| a missing `type` | inferred from the keywords (`properties` → object, ...) |

### Not generated

These are reported (`skipped: ...` / `not measured: ...`) and produce nothing,
while the rest of the input is still generated. Give them an
[override](#overrides):

- `pattern` and string `format` (`email`, `uuid`, `date-time`, ...);
- `multipleOf`;
- `allOf` with several schemas, `not`, `if`/`then`/`else`;
- recursive or external `$ref`s;
- the `true` schema (any value).

An optional property that can't be generated is always left out, so the
input stays valid. `uniqueItems`, `patternProperties` and `propertyNames` are
not enforced.

### sengi keywords

| Keyword | Meaning |
|---|---|
| `"x-sengi-override": "dna"` | generate this part with the override named `dna` |
| `"x-sengi-ignored": true` | don't generate this part (a Rust generic) |
| `"x-sengi-unsupported": "why"` | report this part as unsupported |

`sengi schema` writes the last two for Rust types it can't generate, so a
round trip keeps them.

### Choice labels

`oneOf`/`anyOf` alternatives are told apart by a label, used in paths,
coverage and reports. It is the alternative's `title`; otherwise its `const`,
its single required property (serde's enum layout: `{"Fast": ...}` is
`Fast`), its `$ref` name, its `type`, or its position.

### From Rust to a schema and back

`sengi schema` (or `schema::document` in the library) exports a function's
inputs. Reading the schema back gives the same inputs, except that `char`
becomes a one-character string, Rust floats lose `NaN`/`±∞`, and map keys
become strings.

## Analysing existing inputs

`sengi analyze` reads a set of inputs — a hand-written test suite, fixtures,
production samples, an earlier corpus — through a schema or Rust types, then
measures them with the same [parameters](#built-in-parameters) that decide
what goes into a generated corpus:

- **categories** of each input (e.g. `age / boundary = zero`, `role / variant
  = "guest"`);
- **coverage** — how many single categories and pairs of categories the set
  covers together;
- **pair distances** — the weighted mean for every pair, and min/mean
  overall;
- **near-duplicates** — pairs with the same categories that aren't clearly
  different on any distance at θ. sengi would never keep both, because they
  exercise the same thing. Each pair lists its biggest differences, all
  below their thresholds.

`<name>.analysis.json`:

```json
{
  "name": "tests",
  "theta": 0.5,
  "stats": { "size": 4, "covered": 11, "near_duplicates": 3, "min_pair_distance": 0.0, "mean_pair_distance": 0.408 },
  "inputs": [
    { "value": { "name": "Alice", "age": 30, "role": "admin" },
      "categories": { "age / boundary": "other", "role / variant": "\"admin\"", "tags / variant": "None" } }
  ],
  "near_duplicates": [
    { "a": 1, "b": 2, "distance": 0.119,
      "closest": [{ "leaf": "name", "parameter": "bytes", "distance": 0.33, "threshold": 0.5 }] }
  ]
}
```

Inputs are numbered from 1, as in the CLI and report. Comparing every pair is
quadratic, so thousands of inputs take a while.

## How inputs are chosen

sengi generates random candidates and keeps one if **either**:

1. **it covers something new** — a category, or a pair of categories, that
   no kept input has (e.g. `retries` = `max` together with `mode` = `Off`); or
2. **it is clearly different from every kept input** — on at least one
   parameter of at least one leaf, the distance reaches that parameter's
   threshold.

```text
keep  ⇔  new category or category pair
      or  ∀ kept t: ∃ (leaf, parameter) p with d_p(candidate, t) ≥ θ_p
```

### Parameters: distances and categories

Every simple type is described by a few **parameters**
([table](#built-in-parameters)). Each is one of:

- **distance** — continuous, normalised to 0–1 (0 = same, 1 = as different as
  it gets), e.g. string length. Checked against θ.
- **category** — a label, e.g. an integer's boundary class
  `zero / one / min / max / other`. Tracked as **pairwise coverage**.

Categories aren't treated as distances because a mismatch would score 1 and
pass any θ — θ couldn't control them, and the corpus would explode with
combinations. Pairwise coverage is bounded: two `bool` arguments need exactly
4 inputs. List elements share a dimension (`xs[0]` and `xs[3]` both count as
`xs[]`).

### Distances are never averaged

A candidate is compared leaf by leaf and parameter by parameter. One very
different string among twenty identical numbers is a real difference;
averaging would hide it. Averages appear only in statistics and reports.

### Structure counts too

Two structural leaves are generated alongside the values:

- **choice** — which enum variant, `oneOf` alternative or `enum` value, or
  `Some`/`None`, was picked (`cfg.mode`). Categorised by the choice.
- **length** — how many elements a list or map has (`xs.len`). Measured as a
  relative distance.

So inputs that differ only in variant or length are still told apart. Leaves
present in only one of two inputs (fields of a variant the other didn't pick)
are skipped — the choice leaf already measures that difference.

### θ

θ is how big a difference the **most important** distance parameter of a type
needs. Less important parameters need proportionally more (see
[thresholds](#thresholds)).

- **Lower θ** → inputs may be closer → bigger, denser corpus.
- **Higher θ** → only clearly distinct inputs → smaller, more spread-out corpus.

θ only affects the distance rule. A new category or pair is always kept. In
`analyze`, θ decides what counts as a near-duplicate.

### Stopping

| Reason | When | Default limit |
|---|---|---|
| `Full` | the corpus has `max_size` inputs | 200 |
| `Saturated` | `max_rejections_in_a_row` candidates in a row added nothing | 2 000 |
| `OutOfAttempts` | `max_attempts` candidates generated in total | 100 000 |

`Saturated` means the space is covered as well as θ allows. `Full` means
there was more to find — raise `max_size` or θ.

### Reproducibility

Generation uses a seeded RNG (SplitMix64). The same input definition, seed and
config always produce the same corpus. The seed is recorded in the JSON.

The corpus depends on the order candidates arrive in — early inputs get in
easily, later ones must be far from all of them — so it is well spread, not
optimally spread.

## Supported Rust types

| Type | Generated as |
|---|---|
| `bool`, `char`, all integers, `f32`, `f64`, `&str`, `String` | a simple value over the type's whole range |
| structs, tuples, tuple structs, unit structs | every field |
| enums | one variant, then its fields |
| `Option<T>` | `None`, or `Some` with a `T` |
| `Vec`, `[T]`, `VecDeque`, `HashSet`, `BTreeSet`, ... | 0–16 elements |
| `[T; N]` (literal `N`) | exactly `N` elements |
| `HashMap<K, V>`, `BTreeMap<K, V>` | 0–16 key/value pairs |
| `&T`, `&mut T`, `Box<T>`, `Rc<T>`, `Arc<T>` | `T` |
| type aliases | their target |
| generic structs (`Wrapper<bool>`) | expanded with the arguments substituted |
| any type or path with an [override](#overrides) | the override's generator |

Not generated:

| Type | Why | Reported as |
|---|---|---|
| generic parameters (`T`), `impl Trait` | no concrete type | ignored silently |
| structs/enums defined in another file or crate | definition not visible | `skipped: ...` |
| recursive types (`struct Node { next: Option<Box<Node>> }`) | unbounded | `skipped: ...` |
| `dyn Trait`, function pointers, raw pointers, arrays with non-literal length | no sensible value | `skipped: ...` |

To narrow a range or length that Rust types can't express (a `u32` that is
really 1–10), export a schema with `sengi schema`, edit it, and generate from
the schema.

Functions found: free functions, `impl` methods (named `Type::method`, with
`self` as a parameter of the impl type) and trait methods (`Trait::method`),
including those nested in modules and function bodies.

## Built-in parameters

Every simple value has a range — a Rust type's whole range, or what a schema
allows. Integer and float parameters are measured relative to it.

| Type | Distances | Categories |
|---|---|---|
| `bool` | – | value |
| unsigned integers | magnitude (log scale, from the minimum), bits (Hamming) | boundary: `zero`, `one`, `min`, `min + 1`, `max - 1`, `max`, `other` |
| signed integers | magnitude (log scale, from zero or the end nearest it), bits (Hamming) | sign; boundary: as above plus `minus one` |
| floats | magnitude: exponents for whole float types and unbounded numbers; position within the range for bounded ones | class: `nan`, `infinite`, `zero`, `subnormal`, `normal`; sign |
| `char` | codepoint | class (below) |
| strings | length, bytes (edit distance), character classes (Jaccard) | – |
| choice (variant, alternative, `enum` value, `Option`) | – | which one |
| collection length | length (relative) | – |

Character classes: lowercase, uppercase, digit, punctuation, whitespace,
control, non-ASCII. `usize`/`isize` are treated as 64-bit.

### Generation

Values are biased towards variety, not drawn uniformly (a uniform `u64` is
almost always huge), and always stay in range:

- **integers** — 25% edge cases (0, 1, −1, both ends of the range and one
  step in from each); otherwise a random bit width first, so small and huge
  are equally likely.
- **floats** — whole float types: 20% specials (±0, ±∞, NaN, MIN_POSITIVE,
  MAX, smallest subnormal), otherwise a random exponent. Schema numbers: 20%
  range ends and 0, otherwise half evenly over the range, half over the
  exponents below its largest magnitude.
- **strings, collections** — a length on a log scale (0, 1, 2–3, 4–7, …)
  within the allowed lengths; each string gets a random mix of character
  classes.
- **choices** — uniformly random.

## Library

The CLI is a thin wrapper over the library. Use the library for overrides,
seeds, size limits, or to feed inputs straight into tests.

```rust
use sengi::corpus::{CorpusConfig, build_corpus};
use sengi::overrides::Overrides;
use sengi::registry::Registry;
use sengi::save::{input_to_json, save_corpus};
use sengi::schema::shape_from_schema;
use sengi::shape::function_shape;
use sengi::stats::stats;

let overrides = Overrides::new();
let registry = Registry::with_overrides(&overrides)?;
let config = CorpusConfig { theta: 0.4, seed: 42, ..CorpusConfig::default() };

// From Rust source: one shape per function.
let src = std::fs::read_to_string("src/config.rs")?;
for function in &sengi::function_inputs(&src)? {
    let shape = function_shape(function, &overrides);
    let corpus = build_corpus(&shape, &registry, &overrides, &config);
    let stats = stats(&corpus, &registry);
    save_corpus("corpus".as_ref(), &function.name, &shape, &corpus, &config, &stats)?;
}

// From a JSON Schema.
let schema: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("user.schema.json")?)?;
let shape = shape_from_schema(&schema, &overrides);
let corpus = build_corpus(&shape, &registry, &overrides, &config);
for input in &corpus.inputs {
    let json = input_to_json(&shape, input); // serde_json::Value in the schema's layout
}
```

Analysing existing inputs:

```rust
use sengi::analysis::{analyze, read_inputs};

let values: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string("tests.json")?)?;
let inputs = read_inputs(&shape, &values)?; // Err names the input and path that don't fit
let analysis = analyze(inputs, &registry, 0.5);
for pair in &analysis.near_duplicates {
    println!("#{} ~ #{}: {:.3}", pair.a + 1, pair.b + 1, pair.distance);
}
```

### `CorpusConfig`

| Field | Default | Meaning |
|---|---|---|
| `theta` | `0.5` | Distance threshold for the most important parameter. |
| `seed` | `0` | RNG seed. Same seed → same corpus. |
| `max_size` | `200` | Stop at this many inputs. |
| `max_attempts` | `100_000` | Stop after this many candidates. |
| `max_rejections_in_a_row` | `2_000` | Stop after this many useless candidates in a row. |

### `Corpus`

| Field | Meaning |
|---|---|
| `inputs` | Kept inputs, in acceptance order. Each is a flat map from leaf path (`cfg.mode.Fast.0`) to value. |
| `kept_for` | Why each was kept: `NewCategories` or `Distance`. |
| `accepted_at` | Attempt number at which each was kept. |
| `attempts` | Candidates generated in total. |
| `stop_reason` | `Full`, `Saturated` or `OutOfAttempts`. |
| `covered` | Single categories plus category pairs covered. |

### `Analysis`

| Field | Meaning |
|---|---|
| `inputs` | The inputs, as read. |
| `categories` | Each input's `(dimension, category)` set. |
| `covered` | Single categories plus category pairs covered. |
| `near_duplicates` | `NearDuplicate { a, b, distance, closest }`, indices from 0. |
| `min_pair_distance`, `mean_pair_distance` | Over all pairs. |

### Modules

| Module / item | Purpose |
|---|---|
| `function_inputs`, `parsed_types` | Parse Rust source: functions with resolved argument types; struct/enum/alias definitions. |
| `schema` | JSON Schema → shape (`shape_from_schema`) and shape → schema (`schema_of`, `document`). |
| `shape` | `Shape` and `Simple` (a value with its range): what every front end produces; `function_shape`, `type_paths`. |
| `params` | Built-in parameters per family of simple value. |
| `ahp`, `registry` | Weights from comparison matrices; thresholds. |
| `distance` | `clearly_different` (the keep rule), `compare` (per-parameter breakdown), `weighted_mean` (summary only). |
| `coverage` | Pairwise category coverage. |
| `fuzz` | Generate one input from a shape. |
| `corpus` | `build_corpus`, `CorpusConfig`, `Corpus`. |
| `analysis` | `read_inputs`, `analyze`, `near_duplicates`. |
| `overrides` | Custom generators and parameters. |
| `save` | Corpus JSON; `input_to_json` and its inverse `json_to_input`. |
| `stats`, `report` | Corpus summary; HTML report (`report_data` with or without a `Generation`). |
| `value`, `rng`, `char_class` | Input representation, seeded RNG, character classes. |

`distance::compare` answers "why are these two inputs (not) different?" — it
returns every `(leaf, parameter, distance, threshold)`.

## Overrides

Built-in generators know nothing about your domain: a `DnaSequence(String)`
gets random strings. An override replaces both **generation** and
**comparison** for a type or path:

```rust
use sengi::overrides::{Override, Overrides};
use sengi::rng::Rng;
use serde_json::Value;

fn dna(rng: &mut Rng) -> Value {
    let len = rng.range(1, 64) as usize;
    Value::String((0..len).map(|_| *rng.pick(&['A', 'C', 'G', 'T'])).collect())
}

fn gc(s: &str) -> f64 {
    s.chars().filter(|c| matches!(c, 'G' | 'C')).count() as f64 / s.len().max(1) as f64
}

let mut overrides = Overrides::new();
overrides.add(
    Override::new("dna", dna)
        .distance("length", |a, b| {
            let (a, b) = (a.as_str().unwrap().len(), b.as_str().unwrap().len());
            a.abs_diff(b) as f64 / a.max(b).max(1) as f64
        })
        .distance("gc_content", |a, b| {
            (gc(a.as_str().unwrap()) - gc(b.as_str().unwrap())).abs()
        })
        .category("starts_with_atg", |v| v.as_str().unwrap().starts_with("ATG").to_string())
        .comparisons(vec![            // optional; default: all equally important
            vec![1.0, 1.0, 0.5],
            vec![1.0, 1.0, 0.5],
            vec![2.0, 2.0, 1.0],
        ]),
);
overrides.use_for_type("DnaSequence", "dna");

// The registry must know about the override's parameters.
let registry = Registry::with_overrides(&overrides)?;
```

- **`Override::new(name, generate)`** — `generate` returns a whole value as
  JSON, in the [output layout](#corpus-json).
- **`.distance(name, fn(&Value, &Value) -> f64)`** — must return 0–1.
- **`.category(name, fn(&Value) -> String)`** — joins pairwise coverage.
- **`.comparisons(matrix)`** — [AHP](#weights-and-thresholds) matrix, one
  row/column per parameter in the order added.

An override's parameters **replace** the built-in ones, and the overridden
part becomes a single leaf — it is not broken down further. The CLI has no
way to register overrides yet; use the library.

### Attaching

| How | Applies to |
|---|---|
| `use_for_path("reads[].sequence", "dna")` | one place in the inputs (wins over the others) |
| `"x-sengi-override": "dna"` in a schema | that part of the schema |
| `use_for_type("DnaSequence", "dna")` | every occurrence of a Rust type, or of a schema's `$defs/DnaSequence` |

Path syntax: fields with `.` (`cfg.name`), list elements `[]` (`reads[]`),
map entries `[].key` / `[].value`, enum variant fields `variant.field`, the
inside of an `Option` or nullable `Some`. References and `oneOf`/`anyOf`
alternatives are see-through; an optional property's value is at the
property's own path.

Find what can be overridden in Rust source:

```rust
for t in sengi::parsed_types(&src)? {
    println!("{:?} {}", t.kind, t.name);             // Struct DnaSequence
}
for (path, ty) in sengi::shape::type_paths(&function) {
    println!("{path}: {ty}");                         // seq: DnaSequence, seq.0: String
}
```

Overrides are also how you supply what sengi can't generate: external
structs, recursive types, trait objects, string patterns and formats.

## Weights and thresholds

### AHP

Parameters within a type are weighted with the Analytic Hierarchy Process.
Instead of picking weights directly, you state pairwise judgements on Saaty's
1–9 scale (1 = equal, 3 = moderately, 5 = strongly more important):

```text
               length  gc  atg
length       [  1      1   1/2 ]
gc_content   [  1      1   1/2 ]
starts_atg   [  2      2   1   ]
```

`matrix[i][j]` is how much more important `i` is than `j`; the matrix must be
reciprocal (`matrix[j][i] = 1 / matrix[i][j]`). The weights are its principal
eigenvector. Matrices whose judgements contradict each other (A > B > C > A)
are **rejected** when the registry is built — consistency ratio above 0.1.

### Thresholds

```text
θ_p = θ · (largest distance weight / w_p)
```

The most important distance parameter needs θ; one half as important needs
2θ. A threshold above 1 can never be reached, so that parameter alone never
makes an input different — intentional, an unimportant parameter shouldn't
decide. Category weights don't affect thresholds.

Built-in unsigned integers at θ = 0.5:

| Parameter | Kind | Weight | Threshold |
|---|---|---|---|
| magnitude | distance | 0.309 | 0.500 |
| bits | distance | 0.109 | 1.412 (never alone) |
| boundary | category | 0.582 | – |

`cargo run --example weights` prints this for every built-in type.

## Limitations

- **One Rust file at a time.** Types defined in other files or crates aren't
  expanded; give them an [override](#overrides), or describe the inputs with
  a JSON Schema instead.
- **Same-named types** in different modules of one file aren't told apart.
- **Generics** (`T`, `impl Trait`) are skipped, not instantiated.
- **JSON Schema** `pattern`, string `format`s, `multipleOf` and multi-schema
  `allOf` need overrides; `uniqueItems` isn't enforced.
- **No domain knowledge** without overrides: a "DNA" string gets arbitrary
  strings.
- **Greedy selection**: well spread, not optimally spread.
- **The CLI** has no flags for overrides, seed or size limits yet — use the
  library.

## Examples

```sh
cargo run --example dna       # the same function with and without a DNA override
cargo run --example weights   # AHP weights and thresholds of the built-in types
```

## License

Licensed under either of [MIT](https://opensource.org/licenses/MIT) or
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
