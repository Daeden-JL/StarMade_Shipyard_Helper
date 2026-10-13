//! Tries every cell of a turret's row as its pivot and prints how far the
//! turret turns about each and what is wrong with it, then fits the chain.
//!
//!     cargo run -q --release -p yard-core --example pivots -- <file.sment> <starmade config dir> <entity>

use std::time::Instant;
use yard_core::catalog::Catalog;
use yard_core::collide::CatalogShapes;
use yard_core::links::{Kind, Rules};
use yard_core::pivot::{best_pivots, fit_chain};
use yard_core::Blueprint;

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("a .sment file");
    let cfg = std::path::PathBuf::from(args.next().expect("config directory"));
    let entity = args.next().unwrap_or_default();
    let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).expect("BlockConfig.xml");
    let props = std::fs::read_to_string(cfg.join("BlockTypes.properties")).expect("BlockTypes.properties");
    let catalog = Catalog::parse(&xml, &props).expect("catalog");
    let rules = Rules::new(&catalog, Kind::Ship);
    let shapes = CatalogShapes::new(&catalog);
    let mut bp = Blueprint::from_sment(&std::fs::read(&file).expect("readable file")).expect("a blueprint");
    let started = Instant::now();
    let fits = best_pivots(&bp, &entity, |ty| rules.is_track(ty), &shapes).expect("a turret");
    println!("{} fits in {:.1} s; {} faulty", fits.len(), started.elapsed().as_secs_f64(), fits.iter().filter(|f| f.fault.is_some()).count());
    for f in fits.iter().take(12).chain(fits.iter().filter(|f| f.fault.is_some()).take(12)) {
        println!("  {:?} {:.0}° ({:.0}/{:.0}) {}", f.to.to_array(), f.range(), -f.back, f.ahead, f.fault.as_deref().unwrap_or("sound"));
    }
    let mut reasons = std::collections::BTreeMap::new();
    for f in &fits {
        if let Some(why) = &f.fault {
            *reasons.entry(why.clone()).or_insert(0) += 1;
        }
    }
    for (why, n) in reasons {
        println!("  {n} x {why}");
    }
    let started = Instant::now();
    let chain = fit_chain(&mut bp, &entity, |ty| rules.is_track(ty), &shapes, &catalog).expect("a chain");
    println!("chain in {:.1} s", started.elapsed().as_secs_f64());
    for s in &chain.steps {
        println!("  step {}: to {:?}, {:.0}° against {:.0}°", s.path, s.fit.to.to_array(), s.fit.range(), s.now);
    }
    for (p, now) in &chain.kept {
        println!("  kept {p}: {now:.0}°");
    }
    for (p, why) in &chain.skipped {
        println!("  skipped {p}: {why}");
    }
    for n in &chain.notes {
        println!("  note: {n}");
    }
}
