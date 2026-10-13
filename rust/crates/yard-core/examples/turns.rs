//! For every docked entity saved with a turret turn: which axis the turn is
//! about, compared with the docker's and the rail's own axes.
//!     cargo run -q -p yard-core --example turns -- <designs dir>
use yard_core::dock::{turned, Placement, NO_TURN};
use yard_core::shape::turned_axes;
use yard_core::Blueprint;

fn main() {
    let dir = std::env::args().nth(1).expect("designs directory");
    let mut tally = std::collections::BTreeMap::new();
    let mut moved_turns = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        for (_, dock) in bp.docks() {
            let t = dock.turret;
            if t != NO_TURN {
                // The axis of a rotation matrix, from its antisymmetric part.
                let axis = [t[2][1] - t[1][2], t[0][2] - t[2][0], t[1][0] - t[0][1]];
                let len = axis.iter().map(|c| c * c).sum::<f32>().sqrt();
                let (docker_up, docker_forward) = turned_axes(dock.docker.orientation);
                let along = |d: [i32; 3]| if len < 1e-4 { 9.0 } else { (0..3).map(|i| axis[i] / len * d[i] as f32).sum::<f32>().abs() };
                let kind = if len < 1e-4 {
                    "half turn or none (no axis from the matrix)"
                } else if along(docker_up) > 0.999 {
                    "about the docker's arrow-side axis"
                } else if along(docker_forward) > 0.999 {
                    "about the docker's arrow"
                } else if (0..3).any(|a| (axis[a] / len).abs() > 0.999) {
                    "about another block axis"
                } else {
                    "about a slanted axis"
                };
                *tally.entry((dock.rail.ty, kind)).or_insert(0) += 1;
            }
            if dock.moved != Placement::SAME {
                let m = dock.moved;
                let (rail_up, rail_forward) = turned_axes(dock.rail.orientation);
                let shift_len = m.shift.iter().map(|c| c * c).sum::<f32>().sqrt();
                let dir = |d: [i32; 3]| if shift_len < 1e-4 { 0.0 } else { (0..3).map(|i| m.shift[i] / shift_len * d[i] as f32).sum::<f32>() };
                let whole = m.shift.iter().all(|c| (c - c.round()).abs() < 1e-3);
                let kind = format!(
                    "turn {}, shift {} ({})",
                    if m.turn == NO_TURN { "none" } else if turned(&m.turn, rail_up.map(|c| c as f32)).iter().zip(rail_up).all(|(a, b)| (a - b as f32).abs() < 1e-3) { "about the rail's arrow-side axis" } else { "other" },
                    if shift_len < 1e-4 { "none" } else if dir(rail_forward).abs() > 0.999 { "along the rail's arrow" } else if dir(rail_up).abs() > 0.999 { "along the rail's arrow-side axis" } else { "elsewhere" },
                    if whole { "whole blocks" } else { "between blocks" },
                );
                *moved_turns.entry((dock.rail.ty, kind)).or_insert(0) += 1;
            }
        }
    }
    println!("turret turns:");
    for ((ty, kind), n) in tally {
        println!("  {n:4}  rail type {ty}: {kind}");
    }
    println!("rail movement:");
    for ((ty, kind), n) in moved_turns {
        println!("  {n:4}  rail type {ty}: {kind}");
    }
}
