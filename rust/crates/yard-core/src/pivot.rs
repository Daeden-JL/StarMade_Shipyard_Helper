//! Finding the place for a turret's pivot where it can turn furthest.
//!
//! A turret turns about the line through its rail docker, and [`crate::relocate`]
//! can move that line (with the core, and the axes and dockers of what is
//! docked to the turret) to another cell of the turret without moving the
//! turret itself. This tries every such cell at the core's own level and
//! sweeps the turret about it in the design's rig, as the whole-design view
//! does, to see how far it gets before it meets the ship or another part.
//! A cell whose move would leave a block floating outside the structure
//! (the axis beyond the deck's edge, say) is marked as such and comes last.
//! [`fit_chain`] does this for a turret and every section docked to it, in
//! turn.

use crate::collide::Shapes;
use crate::dock::Motion;
use crate::entity::Bounds;
use crate::relocate::{called, grid, parent_of, relocate, shuffles, transposed, turret_docks, Aid, Move, Shuffle};
use crate::rig::Rig;
use crate::shape::{turn_step, turned_axes, SIDE_DIRS};
use crate::{Block, Blueprint, Error, Pos, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A cell for the core of a turret, and how far the turret turns with its
/// pivot there.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// The cell, in the entity's coordinates as they are; the origin is
    /// where the core is now.
    pub to: Pos,
    /// How far the turret turns the negative way before meeting something,
    /// in degrees (zero or less, -180 at most).
    pub back: f32,
    /// The same the positive way (zero or more, 180 at most).
    pub ahead: f32,
    /// Why the core should not go there after all: the move would leave
    /// blocks floating outside the structure, or cannot be made (it would
    /// displace a core or a block a dock rests on). None when it is sound.
    pub fault: Option<String>,
}

impl Fit {
    /// The whole sweep, in degrees: 360 for a turret nothing stops.
    pub fn range(&self) -> f32 {
        self.ahead - self.back
    }
}

/// Most cells swept at full resolution. A level with more is tried on a
/// grid first, and the cells round the best of those after.
const MOST_SWEPT: usize = 200;

fn refuse(detail: impl Into<String>) -> Error {
    Error::Unsupported { what: "fitting the turret's pivot", detail: detail.into() }
}

/// What an error says, without the name of what was being done.
fn detail(e: Error) -> String {
    match e {
        Error::Unsupported { detail, .. } => detail,
        other => other.to_string(),
    }
}

/// Fits best first: sound ones before faulty, then the widest sweep, the
/// most even both ways, and the nearest the core's cell now.
fn order(a: &Fit, b: &Fit) -> std::cmp::Ordering {
    let from_origin = |p: Pos| p.x * p.x + p.y * p.y + p.z * p.z;
    a.fault
        .is_some()
        .cmp(&b.fault.is_some())
        .then(b.range().partial_cmp(&a.range()).unwrap_or(std::cmp::Ordering::Equal))
        .then((a.ahead + a.back).abs().partial_cmp(&(b.ahead + b.back).abs()).unwrap_or(std::cmp::Ordering::Equal))
        .then(from_origin(a.to).cmp(&from_origin(b.to)))
}

/// Every cell of the entity at `path` where its core could go with the dock
/// following (see [`crate::relocate::relocate`]) and the turret keeping its
/// place, with how far the turret turns about each, best first (see
/// [`order`]). The cells tried are those at the core's level across the
/// turn axis that hold a block, the core's own among them. Refused for an
/// entity that is not a turret that turns, or that the design cannot place.
pub fn best_pivots(bp: &Blueprint, path: &str, is_track: impl Fn(u16) -> bool, shapes: &dyn Shapes) -> Result<Vec<Fit>> {
    let (own, children) = turret_docks(bp, path);
    let own = own.ok_or_else(|| refuse("the entity does not hang from a turret axis"))?;
    if own.motion() != Motion::Turret {
        return Err(refuse("the turret's docker faces front or back, so the game never turns it"));
    }
    let e = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
    let mut rig = Rig::new(bp, is_track, shapes);
    let part = rig.parts.iter().position(|p| p.path == path).ok_or_else(|| refuse("the design has no place for the entity"))?;
    let entities = rig.entities(bp);
    let to_parent = grid(&own.at_rest().turn).ok_or_else(|| refuse("the dock is not square to its parent"))?;
    let child_parts: Vec<(usize, crate::shape::GridTurn)> = children
        .iter()
        .filter_map(|(child, dock)| {
            let j = rig.parts.iter().position(|p| p.path == *child)?;
            Some((j, transposed(&grid(&dock.at_rest().turn)?)))
        })
        .collect();

    // The cells at the core's level across the axis the turret turns about.
    let (axis, _) = turned_axes(own.docker.orientation);
    let level = |p: Pos| p.x * axis[0] + p.y * axis[1] + p.z * axis[2] == 0;
    let mut cells: Vec<Pos> = Vec::new();
    e.entity.for_each(|p, _| {
        if level(p) {
            cells.push(p);
        }
    });
    let origin = Pos::new(0, 0, 0);
    if !cells.contains(&origin) {
        cells.push(origin);
    }
    cells.sort();

    // Sweeping one cell: the docks shift as the relocation would shift them,
    // which leaves every part where it is and moves only the line.
    let docks_at_rest: Vec<(usize, crate::dock::Dock)> = std::iter::once(part).chain(child_parts.iter().map(|(j, _)| *j)).filter_map(|i| Some((i, rig.parts[i].dock.clone()?))).collect();
    let mut bounds: BTreeMap<String, Option<Bounds>> = BTreeMap::new();
    let mut sweep = |rig: &mut Rig, d: Pos| -> Fit {
        let in_parent = turn_step(&to_parent, d.to_array());
        if let Some(dock) = rig.parts[part].dock.as_mut() {
            dock.docker.pos = dock.docker.pos.offset(d.x, d.y, d.z);
            dock.rail.pos = dock.rail.pos.offset(in_parent[0], in_parent[1], in_parent[2]);
        }
        for (j, into_child) in &child_parts {
            let in_child = turn_step(into_child, d.to_array());
            if let Some(dock) = rig.parts[*j].dock.as_mut() {
                dock.rail.pos = dock.rail.pos.offset(d.x, d.y, d.z);
                dock.docker.pos = dock.docker.pos.offset(in_child[0], in_child[1], in_child[2]);
            }
        }
        rig.parts[part].value = 0.0;
        rig.place();
        rig.move_to(&entities, part, 180.0, shapes);
        let ahead = rig.parts[part].value;
        rig.parts[part].value = 0.0;
        rig.place();
        rig.move_to(&entities, part, -180.0, shapes);
        let back = rig.parts[part].value;
        for (i, dock) in &docks_at_rest {
            rig.parts[*i].dock = Some(dock.clone());
        }
        rig.parts[part].value = 0.0;
        rig.place();
        Fit { to: d, back, ahead, fault: fault_of(bp, path, d, &mut bounds) }
    };

    // Across the two axes of the level: a grid first where there are many.
    let across = |p: Pos| {
        let v = p.to_array();
        let mut out = [0; 2];
        let mut k = 0;
        for a in 0..3 {
            if axis[a] == 0 {
                out[k] = v[a];
                k += 1;
            }
        }
        out
    };
    let stride = ((cells.len() as f32 / MOST_SWEPT as f32).sqrt().ceil() as i32).max(1);
    let on_grid = |p: Pos| across(p).iter().all(|c| c.rem_euclid(stride) == 0);
    let mut fits: Vec<Fit> = cells.iter().filter(|p| stride == 1 || on_grid(**p)).map(|p| sweep(&mut rig, *p)).collect();
    if stride > 1 {
        if let Some(best) = fits.iter().min_by(|a, b| order(a, b)).map(|f| across(f.to)) {
            let near = |p: Pos| across(p).iter().zip(best).all(|(c, b)| (c - b).abs() < stride);
            let more: Vec<Fit> = cells.iter().filter(|p| !on_grid(**p) && near(**p)).map(|p| sweep(&mut rig, *p)).collect();
            fits.extend(more);
        }
    }
    fits.sort_by(order);
    Ok(fits)
}

/// Why the core of the entity at `path` should not go to `to` with the
/// dock following: None when the move can be made and leaves no block
/// floating outside the structure.
fn fault_of(bp: &Blueprint, path: &str, to: Pos, bounds: &mut BTreeMap<String, Option<Bounds>>) -> Option<String> {
    if to == Pos::new(0, 0, 0) {
        return None;
    }
    match shuffles(bp, path, to, true) {
        Err(e) => Some(detail(e)),
        Ok((shuffles, _)) => {
            let adrift = left_floating(bp, &shuffles, bounds);
            if adrift.is_empty() {
                return None;
            }
            let named: Vec<String> = adrift.iter().take(2).map(|(entity, p)| format!("({}, {}, {}) of {}", p.x, p.y, p.z, called(entity))).collect();
            Some(format!(
                "it would leave {} floating outside the structure, at {}{}",
                if adrift.len() == 1 { "a block".to_string() } else { format!("{} blocks", adrift.len()) },
                named.join(" and "),
                if adrift.len() > 2 { " among others" } else { "" }
            ))
        }
    }
}

/// The blocks the shuffles of a core move would leave floating outside
/// the structure, by entity and cell (as the cells are before the move):
/// touching no block of their entity, with open space on some side all
/// the way out. A block floating inside a hull, with blocks on every side
/// of it somewhere, is left alone, as is one that floated before the move.
/// Only the cells the move touches and their neighbours change, so only
/// those are looked at.
fn left_floating(bp: &Blueprint, shuffles: &[Shuffle], bounds: &mut BTreeMap<String, Option<Bounds>>) -> Vec<(String, Pos)> {
    let mut adrift = Vec::new();
    for s in shuffles {
        let Some(e) = bp.entity(&s.entity) else { continue };
        let e = &e.entity;
        let Some(b) = *bounds.entry(s.entity.clone()).or_insert_with(|| e.bounds()) else { continue };
        let landed: BTreeMap<Pos, Pos> = s.moved.iter().map(|(from, to)| (*to, *from)).collect();
        let after = |q: Pos| match landed.get(&q) {
            Some(from) if s.dropped.contains(from) => Block::AIR,
            Some(from) => e.get(*from),
            None => e.get(q),
        };
        let before = |q: Pos| e.get(q);
        let mut cells = BTreeSet::new();
        for c in s.moved.keys().chain(s.moved.values()) {
            cells.insert(*c);
            for n in SIDE_DIRS {
                cells.insert(c.offset(n[0], n[1], n[2]));
            }
        }
        adrift.extend(cells.iter().filter(|c| floating(**c, &b, &after) && !floating(**c, &b, &before)).map(|c| (s.entity.clone(), *c)));
    }
    adrift
}

/// Whether the block at `c`, if there is one, touches nothing of its
/// entity and has open space all the way out on some side. The entity's
/// bounds say where the open is: a cell beyond them has nothing further.
fn floating(c: Pos, bounds: &Bounds, at: &dyn Fn(Pos) -> Block) -> bool {
    if at(c).is_air() || SIDE_DIRS.iter().any(|n| !at(c.offset(n[0], n[1], n[2])).is_air()) {
        return false;
    }
    SIDE_DIRS.iter().any(|n| {
        let mut q = c;
        loop {
            q = q.offset(n[0], n[1], n[2]);
            if q.x < bounds.min.x || q.y < bounds.min.y || q.z < bounds.min.z || q.x > bounds.max.x || q.y > bounds.max.y || q.z > bounds.max.z {
                return true;
            }
            if !at(q).is_air() {
                return false;
            }
        }
    })
}

/// One section of a dock chain whose core moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub path: String,
    /// Where its core goes, in its coordinates as they are, and how far it
    /// turns there.
    pub fit: Fit,
    /// How far it turns where its core is now.
    pub now: f32,
}

/// What fitting a dock chain found, section by section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chain {
    /// The sections whose core moves, in the order the moves are made.
    pub steps: Vec<Step>,
    /// Sections at their best place already, with how far they turn.
    pub kept: Vec<(String, f32)>,
    /// Sections not fitted, and why: not a turret that turns, or no cell
    /// of their row without a fault.
    pub skipped: Vec<(String, String)>,
    /// What the moves did, in words, each starting with the section's
    /// path and a colon (or "the main entity").
    pub notes: Vec<String>,
}

/// Fits the pivot of the entity at `path` and of every entity docked to it,
/// down the chain (a base, then its barrels, then what hangs from those),
/// making each move in the blueprint before the next section is tried:
/// the best sound cell of each section's row (see [`best_pivots`]) where
/// that widens the sweep. A section that is not a turret that turns, or
/// has no sound cell, is skipped and the chain goes on below it.
pub fn fit_chain(bp: &mut Blueprint, path: &str, is_track: impl Fn(u16) -> bool, shapes: &dyn Shapes, aid: &dyn Aid) -> Result<Chain> {
    if bp.entity(path).is_none() {
        return Err(refuse(format!("there is no entity {path:?}")));
    }
    let origin = Pos::new(0, 0, 0);
    let mut chain = Chain::default();
    let mut queue = VecDeque::from([path.to_string()]);
    while let Some(p) = queue.pop_front() {
        match best_pivots(bp, &p, &is_track, shapes) {
            Err(e) => chain.skipped.push((p.clone(), detail(e))),
            Ok(fits) => {
                let now = fits.iter().find(|f| f.to == origin).map_or(0.0, |f| f.range());
                match fits.iter().find(|f| f.fault.is_none()) {
                    Some(best) if best.to != origin && best.range() > now => match relocate(bp, &p, Move { to: best.to, facing: None, dock_follows: true }, aid) {
                        Ok(done) => {
                            let who = if p.is_empty() { "the main entity".to_string() } else { p.clone() };
                            chain.notes.extend(done.notes.into_iter().map(|n| format!("{who}: {n}")));
                            chain.steps.push(Step { path: p.clone(), fit: best.clone(), now });
                        }
                        Err(e) => chain.skipped.push((p.clone(), detail(e))),
                    },
                    Some(_) => chain.kept.push((p.clone(), now)),
                    None => chain.skipped.push((p.clone(), "no cell of its row keeps every block on the structure".to_string())),
                }
            }
        }
        queue.extend(bp.docks().keys().filter(|c| parent_of(c).is_some_and(|(parent, _)| parent == p)).cloned());
    }
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turret::fixtures::*;
    use crate::dock::TURRET_AXIS;
    use crate::{turret, Block};

    /// Every block a full cube.
    struct Solid(std::sync::LazyLock<crate::shape::Geom>);
    impl Shapes for Solid {
        fn geom(&self, _: Block) -> Option<&crate::shape::Geom> {
            Some(&self.0)
        }
    }
    static SOLID: Solid = Solid(std::sync::LazyLock::new(crate::shape::Geom::cube));

    /// A deck (y = 3, above the ship's core) reaching `deck_to` along +X
    /// from x = -8, with a turret cut from a 5 by `top - 3` by 13 box on it
    /// (x 2..=6, y 4..=top, z -2..=10), and a wall along the deck at x = -3.
    /// Swung about the middle of the box, the box's far corners (7 blocks
    /// away) reach the wall; swung about its +X edge they do not.
    fn deck_turret(deck_to: i32, top: i32) -> Blueprint {
        let catalog = catalog();
        let mut bp = ship(&[(Pos::new(-8, 3, -8), Pos::new(deck_to, 3, 12))]);
        for x in 2..=6 {
            for y in 4..=top {
                for z in -2..=10 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 0, false));
                }
            }
        }
        for y in 4..=8 {
            for z in -8..=12 {
                bp.root.entity.set(Pos::new(-3, y, z), Block::new(HULL, 0, false));
            }
        }
        let made = turret::make_turret(&mut bp, &catalog, "", Pos::new(2, 4, -2), Pos::new(6, top, 10), 3).unwrap();
        assert_eq!(made.child, "ATTACHED_0");
        bp
    }

    fn deck_turret_and_wall() -> Blueprint {
        deck_turret(8, 6)
    }

    /// The dock records read back, with the blocks at the cells they name.
    fn docks_hold(bp: &Blueprint) {
        for (path, dock) in bp.docks() {
            let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
            assert_eq!(bp.entity(parent).unwrap().entity.get(dock.rail.pos).ty(), dock.rail.ty, "{path}: the rail block is where its record says");
            assert_eq!(bp.entity(&path).unwrap().entity.get(dock.docker.pos).ty(), dock.docker.ty, "{path}: the docker block is where its record says");
        }
    }

    /// The blocks of every entity that float outside the structure.
    fn floating_outside(bp: &Blueprint) -> Vec<(String, Pos)> {
        let mut out = Vec::new();
        for (path, e) in bp.entities() {
            let Some(b) = e.entity.bounds() else { continue };
            e.entity.for_each(|p, _| {
                if floating(p, &b, &|q| e.entity.get(q)) {
                    out.push((path.clone(), p));
                }
            });
        }
        out
    }

    #[test]
    fn the_pivot_furthest_from_the_wall_lets_the_turret_turn_all_the_way_round() {
        let bp = deck_turret_and_wall();
        // The core is at the middle of the base face: (4, 5, 4) of the ship.
        let dock = &bp.docks()["ATTACHED_0"];
        assert_eq!(dock.rail.pos, Pos::new(4, 3, 4));
        let fits = best_pivots(&bp, "ATTACHED_0", |ty| ty == 662, &SOLID).unwrap();
        // The level across the turn axis is the core's own row: 5 by 13 cells.
        assert_eq!(fits.len(), 65);
        let now = fits.iter().find(|f| f.to == Pos::new(0, 0, 0)).unwrap();
        assert!(now.range() < 300.0, "{now:?}: swung round the middle, the turret's far corners reach the wall");
        let best = &fits[0];
        assert_eq!(best.range(), 360.0, "{:?}", &fits[..6]);
        assert!(best.to.x > 0, "{best:?}: the pivot moves away from the wall (+X in the turret is +X on the ship)");
        // Every fit is a block of the turret's row, and the list is best first.
        let e = &bp.entity("ATTACHED_0").unwrap().entity;
        assert!(fits.iter().all(|f| f.to.y == 0 && !e.get(f.to).is_air()));
        assert!(fits.windows(2).all(|w| w[0].range() >= w[1].range()));
        // Not a turret: refused.
        assert!(best_pivots(&bp, "", |ty| ty == 662, &SOLID).is_err());
        // Every cell is sound: the deck is under the whole of the turret's row.
        assert!(fits.iter().all(|f| f.fault.is_none()), "{:?}", fits.iter().filter(|f| f.fault.is_some()).collect::<Vec<_>>());
    }

    #[test]
    fn a_pivot_whose_axis_would_float_beyond_the_deck_is_faulted_and_comes_last() {
        // The deck ends at x = 4, under the turret's middle row. The axis
        // moved straight out to x = 5 would hang off the hole it left at
        // x = 4 (the empty cell it took goes back there), touching nothing;
        // moved out and along it touches the deck's edge beside the hole.
        // At x = 6 it touches nothing whichever way.
        let bp = deck_turret(4, 6);
        let fits = best_pivots(&bp, "ATTACHED_0", |ty| ty == 662, &SOLID).unwrap();
        assert_eq!(fits.len(), 65);
        for f in &fits {
            if f.to.x == 2 || (f.to.x == 1 && f.to.z == 0) {
                assert!(f.fault.as_deref().is_some_and(|why| why.contains("floating outside the structure")), "{f:?}");
            } else {
                assert_eq!(f.fault, None, "{f:?}");
            }
        }
        // Sound fits first, the widest among them; the faulty ones last.
        let first_fault = fits.iter().position(|f| f.fault.is_some()).unwrap();
        assert_eq!(first_fault, 65 - 14);
        assert!(fits[..first_fault].windows(2).all(|w| w[0].range() >= w[1].range()));
        assert!(fits[first_fault..].windows(2).all(|w| w[0].range() >= w[1].range()));
        let best = &fits[0];
        assert_eq!(best.fault, None);
        assert!(best.to.x <= 1, "{best:?}");
        let now = fits.iter().find(|f| f.to == Pos::new(0, 0, 0)).unwrap();
        assert!(best.range() >= now.range());
    }

    #[test]
    fn a_block_floating_inside_a_hull_is_not_floating_outside_it() {
        let mut e = crate::entity::Entity::new();
        for x in 0..=4 {
            for y in 0..=4 {
                for z in 0..=4 {
                    if [x, y, z].iter().any(|c| *c == 0 || *c == 4) {
                        e.set(Pos::new(x, y, z), Block::new(HULL, 0, false));
                    }
                }
            }
        }
        e.set(Pos::new(2, 2, 2), Block::new(PLATE, 0, false));
        e.set(Pos::new(2, 2, 8), Block::new(PLATE, 0, false));
        e.set(Pos::new(2, 5, 2), Block::new(PLATE, 0, false));
        let b = e.bounds().unwrap();
        let at = |q: Pos| e.get(q);
        assert!(!floating(Pos::new(2, 2, 2), &b, &at), "inside the hull, touching nothing");
        assert!(floating(Pos::new(2, 2, 8), &b, &at), "beyond the hull, touching nothing");
        assert!(!floating(Pos::new(2, 5, 2), &b, &at), "on the hull's top");
        assert!(!floating(Pos::new(0, 0, 0), &b, &at), "a corner of the hull");
        assert!(!floating(Pos::new(2, 2, 6), &b, &at), "an empty cell");
    }

    #[test]
    fn the_chain_fits_the_base_and_then_the_barrel_and_leaves_nothing_floating() {
        let catalog = catalog();
        // A turret five high; its top three layers (y 1..=3 of it, 5 by 13)
        // become a barrel turning about a vertical axis at (0, 0, 3) of the base.
        let mut bp = deck_turret(8, 8);
        let barrel = turret::make_turret_at(&mut bp, &catalog, "ATTACHED_0", Pos::new(-2, 1, -6), Pos::new(2, 3, 6), 3, Some(Pos::new(0, 1, 3)), &Default::default()).unwrap();
        assert_eq!(barrel.child, "ATTACHED_0/ATTACHED_0");
        docks_hold(&bp);
        // The ship's core floats alone in this fixture; nothing else does.
        let floating_before = floating_outside(&bp);
        assert_eq!(floating_before, vec![(String::new(), Pos::new(0, 0, 0))]);
        let before = best_pivots(&bp, "ATTACHED_0/ATTACHED_0", |ty| ty == 662, &SOLID).unwrap();
        let barrel_now = before.iter().find(|f| f.to == Pos::new(0, 0, 0)).unwrap().range();
        assert!(barrel_now < 360.0, "the barrel's far end reaches the wall");

        let chain = fit_chain(&mut bp, "ATTACHED_0", |ty| ty == 662, &SOLID, &catalog).unwrap();
        assert_eq!(chain.skipped, vec![], "{chain:?}");
        // The base first: its core goes to the +X edge and it turns all the way round.
        assert_eq!(chain.steps[0].path, "ATTACHED_0");
        assert_eq!(chain.steps[0].fit.range(), 360.0);
        assert!(chain.steps[0].now < 360.0);
        assert!(chain.steps[0].fit.to.x > 0);
        // Then the barrel, tried after the base's move (which took the
        // barrel's axis along, so the barrel turns about a new line too):
        // it turns no less than it did.
        let about_barrel = chain.steps.iter().find(|s| s.path == "ATTACHED_0/ATTACHED_0").map(|s| (s.now, s.fit.range())).or_else(|| chain.kept.iter().find(|k| k.0 == "ATTACHED_0/ATTACHED_0").map(|k| (k.1, k.1)));
        let (now, then) = about_barrel.expect("the barrel was tried");
        assert!(then >= now && then >= barrel_now, "{chain:?}");
        assert!(chain.notes.iter().any(|n| n.starts_with("ATTACHED_0: ")), "{:?}", chain.notes);
        assert!(chain.notes.iter().any(|n| n.starts_with("ATTACHED_0/ATTACHED_0: ")), "{:?}", chain.notes);
        // Everything is where its records say and nothing floats; the barrel still rests on its axis.
        docks_hold(&bp);
        assert_eq!(floating_outside(&bp), floating_before);
        let docks = bp.docks();
        let base = &bp.entity("ATTACHED_0").unwrap().entity;
        assert_eq!(base.get(docks["ATTACHED_0/ATTACHED_0"].rail.pos).ty(), TURRET_AXIS);
        // Fitted again, every section stays: the chain has nothing to do.
        let again = fit_chain(&mut bp, "ATTACHED_0", |ty| ty == 662, &SOLID, &catalog).unwrap();
        assert_eq!(again.steps, vec![], "{again:?}");
        assert_eq!(again.kept.len(), 2, "{again:?}");
    }
}
