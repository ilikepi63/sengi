//! Prints every kind's AHP weights, and each distance parameter's threshold.
use sengi::Primitive;
use sengi::registry::Registry;
use sengi::value::Kind;

fn main() {
    let registry = Registry::new().unwrap();
    let theta = 0.5;
    for kind in [
        Kind::Primitive(Primitive::U64),
        Kind::Primitive(Primitive::I64),
        Kind::Primitive(Primitive::F64),
        Kind::Primitive(Primitive::Char),
        Kind::Primitive(Primitive::String),
    ] {
        println!("{kind:?}");
        for p in registry.distances(&kind) {
            println!(
                "  distance {:<10} weight {:.3}  threshold at θ={theta}: {:.3}",
                p.name,
                p.weight,
                theta * p.threshold_scale
            );
        }
        for p in registry.categories(&kind) {
            println!("  category {:<10} weight {:.3}", p.name, p.weight);
        }
    }
}
