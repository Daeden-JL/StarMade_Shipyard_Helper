//! Places every docked entity of the designs in a folder the way the game
//! does and reports what does not fit: a docker that is not on its rail, or
//! blocks that land inside the parent.
//!
//!     cargo run -q -p yard-core --example docks -- <designs dir> [-v]

use yard_core::dock::Placement;
use yard_core::{Blueprint, Pos};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("designs directory");
    let verbose = args.next().is_some();
    let (mut designs, mut docked, mut no_dock, mut moved, mut off_grid, mut clean, mut overlapping, mut stale) = (0, 0, 0, 0, 0, 0, 0, 0);
    let mut kinds = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("readable directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        let docks = bp.docks();
        let entities = bp.entities();
        if entities.len() < 2 {
            continue;
        }
        designs += 1;
        no_dock += entities.len() - 1 - docks.len();
        for (child_path, dock) in &docks {
            docked += 1;
            *kinds.entry(format!("{:?} (rail type {})", dock.motion(), dock.rail.ty)).or_insert(0) += 1;
            let parent_path = child_path.rsplit_once('/').map_or("", |(p, _)| p);
            let (parent, child) = (&bp.entity(parent_path).unwrap().entity, &bp.entity(child_path).unwrap().entity);
            let travelled = dock.moved != Placement::SAME;
            moved += travelled as usize;
            let rail_there = parent.get(dock.rail.pos).ty() == dock.rail.ty && parent.get(dock.rail.pos).orientation() == dock.rail.orientation;
            let docker_there = child.get(dock.docker.pos).ty() == dock.docker.ty && child.get(dock.docker.pos).orientation() == dock.docker.orientation;
            if !docker_there || (!rail_there && !travelled) {
                stale += 1;
                if verbose {
                    println!("{} [{child_path}]: record names blocks that are not there (rail {rail_there}, docker {docker_there})", bp.name);
                }
            }
            let saved = dock.placement();
            let on_grid = saved.cell(Pos::new(0, 0, 0)).is_some() && saved.cell(Pos::new(1, 2, 3)).is_some();
            // Off the grid (a turret left aimed somewhere): check it at rest instead.
            let checked = if on_grid { saved } else { dock.at_rest() };
            off_grid += !on_grid as usize;
            let mut inside = 0;
            child.for_each(|p, _| {
                if checked.cell(p).is_some_and(|at| !parent.get(at).is_air()) {
                    inside += 1;
                }
            });
            if inside == 0 {
                clean += 1;
            } else {
                overlapping += 1;
                if verbose || !travelled {
                    println!(
                        "{} [{child_path}]: {inside} of {} blocks inside the parent ({}{}, rail {:?} o{}, docker {:?} o{})",
                        bp.name,
                        child.block_count(),
                        if on_grid { "as saved" } else { "at rest" },
                        if travelled { ", travelled" } else { "" },
                        dock.rail.pos.to_array(),
                        dock.rail.orientation,
                        dock.docker.pos.to_array(),
                        dock.docker.orientation,
                    );
                }
            }
        }
    }
    println!("{designs} designs with docked entities: {docked} docks, {no_dock} folders without a dock record");
    println!("{clean} sit clear of their parent, {overlapping} overlap it; {moved} travelled along a rail, {off_grid} saved off the grid, {stale} with stale records");
    for (kind, n) in kinds {
        println!("  {n:4}  {kind}");
    }
}
