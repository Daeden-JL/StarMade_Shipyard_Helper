//! Presses one block of a design and tells what the game's logic and rails
//! then do, tick by tick.
//!
//!     cargo run -q -p yard-core --example press -- <file.sment> <starmade config dir> <entity number> <x> <y> <z> [seconds]

use yard_core::catalog::Catalog;
use yard_core::collide::CatalogShapes;
use yard_core::links::{Kind, Rules};
use yard_core::rig::Rig;
use yard_core::sim::{Sim, TICK_MS};
use yard_core::{Blueprint, Entity, Pos};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = std::path::PathBuf::from(&args[1]);
    let number = |i: usize| args[i].parse::<i32>().expect("a number");
    let (entity, pos) = (number(2) as usize, Pos::new(number(3), number(4), number(5)));
    let seconds: f32 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).expect("BlockConfig.xml");
    let props = std::fs::read_to_string(cfg.join("BlockTypes.properties")).expect("BlockTypes.properties");
    let catalog = Catalog::parse(&xml, &props).expect("catalog");
    let rules = Rules::new(&catalog, Kind::Ship);
    let shapes = CatalogShapes::new(&catalog);
    let mut bp = Blueprint::from_sment(&std::fs::read(&args[0]).expect("readable file")).expect("a blueprint");
    let mut rig = Rig::new(&bp, |ty| rules.is_track(ty), &shapes);
    let world = rig.entities(&bp);
    let mut sim = Sim::new(&mut rig, &world, &catalog, &bp.wireless());
    println!("pressing {} in entity {entity}: {:?}", catalog.name(world[entity].get(pos).ty()), sim.press(&world, &catalog, entity, pos));
    drop(world);
    let places = |rig: &Rig| rig.parts.iter().skip(1).map(|p| p.placed.at([0.0; 3]).map(|c| (c * 10.0).round() / 10.0)).collect::<Vec<_>>();
    let start = places(&rig);
    println!("at the start parts at {start:?}");
    let mut last = places(&rig);
    let mut time = 0.0;
    while time < seconds {
        let mut world: Vec<&mut Entity> = bp.blocks_mut();
        sim.tick(&mut rig, &mut world, &catalog, &shapes);
        time += (TICK_MS / 1000.0) as f32;
        for (e, p) in sim.changed.drain(..) {
            let b = world[e].get(p);
            println!("{time:5.2}s  entity {e} {:?} {} is {} (turn {})", p.to_array(), catalog.name(b.ty()), if b.active_bit() { "on" } else { "off" }, b.orientation());
        }
        for note in sim.notes.drain(..) {
            println!("{time:5.2}s  {note:?}");
        }
        let now = places(&rig);
        if now != last && (time * 1000.0) as i32 % 400 < 40 {
            println!("{time:5.2}s  parts at {now:?}");
        }
        last = now;
    }
    println!("in the end parts at {last:?}; busy: {}", sim.busy());
}
