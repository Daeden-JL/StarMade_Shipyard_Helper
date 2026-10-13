//! Puts every design in a folder together with its docked parts and plays
//! the motion demonstration for a few seconds: how many parts overlap as
//! saved, what they overlap with, and how long a frame of motion takes.
//!
//!     cargo run -q --release -p yard-core --example rig -- <designs dir> <starmade config dir> [-v]

use std::time::Instant;
use yard_core::catalog::Catalog;
use yard_core::collide::CatalogShapes;
use yard_core::links::{Kind, Rules};
use yard_core::rig::Rig;
use yard_core::Blueprint;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("designs directory");
    let cfg = std::path::PathBuf::from(args.next().expect("config directory"));
    let verbose = args.next().is_some();
    let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).expect("BlockConfig.xml");
    let props = std::fs::read_to_string(cfg.join("BlockTypes.properties")).expect("BlockTypes.properties");
    let catalog = Catalog::parse(&xml, &props).expect("catalog");
    let rules = Rules::new(&catalog, Kind::Ship);
    let shapes = CatalogShapes::new(&catalog);
    let (mut parts, mut movers, mut overlapping, mut slowest) = (0, 0, 0, (0.0f64, String::new()));
    for entry in std::fs::read_dir(&dir).expect("readable directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        if bp.root.children.is_empty() {
            continue;
        }
        let started = Instant::now();
        let mut rig = Rig::new(&bp, |ty| rules.is_track(ty), &shapes);
        let built = started.elapsed().as_secs_f64() * 1000.0;
        let entities = rig.entities(&bp);
        parts += rig.parts.len() - 1;
        movers += rig.parts.iter().filter(|p| p.can_move()).count();
        for (i, part) in rig.parts.iter().enumerate().skip(1) {
            if !part.overlapping {
                continue;
            }
            overlapping += 1;
            if verbose {
                let mut probe = rig.clone();
                probe.parts[i].overlapping = false;
                let hit = probe.hit(&entities, i, &shapes);
                let what = hit.map(|c| {
                    format!(
                        "{} at {:?} in {} at {:?} of [{}]",
                        catalog.name(entities[c.mover].get(c.mover_cell).ty()),
                        c.mover_cell.to_array(),
                        catalog.name(entities[c.still].get(c.still_cell).ty()),
                        c.still_cell.to_array(),
                        rig.parts[c.still].path
                    )
                });
                println!("{} [{}] {:?}: overlaps as saved: {}", bp.name, part.path, part.motion(), what.unwrap_or_default());
            }
        }
        rig.start_demo();
        let started = Instant::now();
        let frames = 240;
        let mut contacts = 0;
        for _ in 0..frames {
            contacts += rig.play(&entities, 1.0 / 60.0, true, &shapes).contacts.len();
        }
        let frame = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
        if frame > slowest.0 {
            slowest = (frame, bp.name.clone());
        }
        if verbose || frame > 2.0 {
            println!("{}: {} parts, built in {built:.1} ms, {frame:.2} ms a frame, {contacts} contacts in 4 s", bp.name, rig.parts.len() - 1);
        }
    }
    println!("{parts} docked parts, {movers} can move, {overlapping} overlap as saved; slowest frame {:.2} ms ({})", slowest.0, slowest.1);
}
