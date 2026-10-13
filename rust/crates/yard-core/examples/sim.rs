//! Runs the game's logic and rails on every design in a folder: lets each
//! settle, then presses its buttons and switches one after another. Reports
//! what moved and switched, and how long a tick takes.
//!
//!     cargo run -q --release -p yard-core --example sim -- <designs dir> <starmade config dir> [-v]

use std::time::Instant;
use yard_core::catalog::Catalog;
use yard_core::collide::CatalogShapes;
use yard_core::links::{Kind, Rules};
use yard_core::rig::Rig;
use yard_core::sim::{Press, Sim, TICK_MS};
use yard_core::{Blueprint, Entity, Pos};

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
    let (mut designs, mut pressed_all, mut switched_all, mut slowest) = (0, 0, 0, (0.0f64, String::new()));
    for entry in std::fs::read_dir(&dir).expect("readable directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(mut bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        designs += 1;
        let mut rig = Rig::new(&bp, |ty| rules.is_track(ty), &shapes);
        let world = rig.entities(&bp);
        let mut sim = Sim::new(&mut rig, &world, &catalog, &bp.wireless());
        drop(world);
        // What can be pressed: logic blocks a player can activate.
        let mut switches: Vec<(usize, Pos)> = Vec::new();
        for (i, (_, e)) in bp.entities().iter().enumerate() {
            e.entity.for_each(|p, b| {
                if catalog.get(b.ty()).is_some_and(|d| d.logic && d.can_activate) && switches.len() < 40 {
                    switches.push((i, p));
                }
            });
        }
        let started = Instant::now();
        let mut ticks = 0;
        let mut run = |sim: &mut Sim, rig: &mut Rig, bp: &mut Blueprint, seconds: f32| {
            let mut world: Vec<&mut Entity> = bp.blocks_mut();
            let mut left = seconds;
            while left > 0.0 && sim.busy() {
                sim.tick(rig, &mut world, &catalog, &shapes);
                left -= (TICK_MS / 1000.0) as f32;
                ticks += 1;
            }
        };
        run(&mut sim, &mut rig, &mut bp, 8.0);
        let moved_at_start = rig.parts.iter().filter(|p| p.driven.is_some_and(|m| Some(m) != p.dock.as_ref().map(|d| d.moved))).count();
        let mut pressed = 0;
        for (entity, pos) in &switches {
            let world: Vec<&Entity> = bp.entities().into_iter().map(|(_, e)| &e.entity).collect();
            if sim.press(&world, &catalog, *entity, *pos) == Press::Done {
                pressed += 1;
            }
            run(&mut sim, &mut rig, &mut bp, 3.0);
        }
        let switched = sim.changed.len();
        let per_tick = started.elapsed().as_secs_f64() * 1000.0 / ticks.max(1) as f64;
        if per_tick > slowest.0 && ticks > 20 {
            slowest = (per_tick, bp.name.clone());
        }
        pressed_all += pressed;
        switched_all += switched;
        if verbose && (pressed > 0 || moved_at_start > 0) {
            let moved = rig.parts.iter().filter(|p| p.driven.is_some_and(|m| Some(m) != p.dock.as_ref().map(|d| d.moved))).count();
            println!(
                "{}: {} parts, {moved_at_start} moved by themselves, {pressed} pressed, {switched} switched, {moved} moved in the end, {} notes, {ticks} ticks, {per_tick:.2} ms a tick",
                bp.name,
                rig.parts.len() - 1,
                sim.notes.len()
            );
        }
        sim.undo(&mut bp.blocks_mut());
    }
    println!("{designs} designs, {pressed_all} presses, {switched_all} blocks switched; slowest tick {:.2} ms ({})", slowest.0, slowest.1);
}
