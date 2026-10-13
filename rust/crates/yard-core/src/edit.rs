//! Edits to one entity: what a click, a drag or an agent tool call does to the
//! blocks and links, and how it is undone.
//!
//! The browser and the server each run this code on their own copy of a design.
//! An [`Op`] carries everything needed to apply it, so the copies stay identical
//! as long as both apply the same ops in the same order.

use crate::entity::{Entity, Pos, SEG};
use crate::links::{Rules, Slot};
use crate::Block;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// The ship core. It sits at the origin of every ship and can be neither
/// removed, replaced nor placed a second time.
pub const CORE_TYPE: u16 = 1;
/// Most cells the ops of one step may cover.
pub const MAX_CELLS: u64 = 2_000_000;
/// Positions further than this from the core are refused.
pub const MAX_COORD: i32 = 4000;

/// Which cells of a box a fill writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillMode {
    #[default]
    Solid,
    /// The six faces only.
    Hollow,
    /// The twelve edges only.
    Frame,
}

/// Which existing cells an op may write to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    #[default]
    Always,
    /// Only where there is no block yet. This is how placing works in the game.
    Empty,
    /// Only where there is a block: painting.
    Occupied,
    /// Only where there is a block of this type.
    Type(u16),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Writes the listed cells. Block 0 removes.
    Set {
        cells: Vec<(Pos, Block)>,
        #[serde(default)]
        when: When,
    },
    /// Writes a box given by two opposite corners, both included. Block 0 removes.
    Fill {
        min: Pos,
        max: Pos,
        block: Block,
        #[serde(default)]
        mode: FillMode,
        #[serde(default)]
        when: When,
        /// Links every block this fill puts into an empty cell to the block
        /// here, where the game allows that pair: as the game links what is
        /// placed to the ship core, or to the block selected with C.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<Pos>,
    },
    /// Connects a controller to blocks it runs, the way pressing V on them
    /// does: pairs the game does not allow are left out, and a block that can
    /// have one controller only leaves the one it had.
    Link { controller: Pos, targets: Vec<Pos> },
    Unlink { controller: Pos, targets: Vec<Pos> },
    /// Gives every block of one type another type, all over the entity,
    /// keeping each block's orientation: one material swapped for another,
    /// shape by shape. Each pair is (from, to). The core is never touched.
    Retype { pairs: Vec<(u16, u16)> },
    /// Takes back the latest step. Must be the only op of its step.
    Undo,
    Redo,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum EditError {
    #[error("that edit covers {0} blocks; the most allowed in one step is {max}", max = MAX_CELLS)]
    TooLarge(u64),
    #[error("position ({0}, {1}, {2}) is too far from the core; the limit is {max} blocks", max = MAX_COORD)]
    OutOfRange(i32, i32, i32),
    #[error("there is nothing to undo")]
    NothingToUndo,
    #[error("there is nothing to redo")]
    NothingToRedo,
    #[error("undo and redo cannot be combined with other edits")]
    MixedUndo,
}

/// What one step did, in enough detail to repeat or reverse it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// (position, block before, block after)
    pub cells: Vec<(Pos, Block, Block)>,
    /// (controller, target type, target)
    pub unlinked: Vec<(Pos, u16, Pos)>,
    pub linked: Vec<(Pos, u16, Pos)>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.unlinked.is_empty() && self.linked.is_empty()
    }

    pub fn inverse(&self) -> Change {
        Change {
            cells: self.cells.iter().map(|&(p, before, after)| (p, after, before)).collect(),
            unlinked: self.linked.clone(),
            linked: self.unlinked.clone(),
        }
    }

    /// Replays the change on an entity that is in the "before" state.
    pub fn apply_to(&self, entity: &mut Entity) {
        for &(p, _, after) in &self.cells {
            entity.set(p, after);
        }
        for &(c, ty, t) in &self.unlinked {
            entity.unlink(c, t, ty);
        }
        for &(c, ty, t) in &self.linked {
            entity.link(c, t, ty);
        }
    }

    /// The segments whose appearance changed: those holding a changed cell, and
    /// their neighbours where the cell sits on a segment border (a block there
    /// can hide or reveal a face next door).
    pub fn dirty_segments(&self) -> HashSet<(i32, i32, i32)> {
        let mut out = HashSet::new();
        for &(p, _, _) in &self.cells {
            let (key, _) = Entity::locate(p);
            out.insert(key);
            let local = [
                (p.x + crate::entity::CORE_OFFSET).rem_euclid(SEG),
                (p.y + crate::entity::CORE_OFFSET).rem_euclid(SEG),
                (p.z + crate::entity::CORE_OFFSET).rem_euclid(SEG),
            ];
            for axis in 0..3 {
                let mut k = [key.0, key.1, key.2];
                if local[axis] == 0 {
                    k[axis] -= 1;
                } else if local[axis] == SEG - 1 {
                    k[axis] += 1;
                } else {
                    continue;
                }
                out.insert((k[0], k[1], k[2]));
            }
        }
        out
    }
}

fn check_range(p: Pos) -> Result<(), EditError> {
    if p.x.abs() > MAX_COORD || p.y.abs() > MAX_COORD || p.z.abs() > MAX_COORD {
        Err(EditError::OutOfRange(p.x, p.y, p.z))
    } else {
        Ok(())
    }
}

/// Cells an op may touch at most, after checking its positions.
fn budget(op: &Op) -> Result<u64, EditError> {
    match op {
        Op::Set { cells, .. } => {
            cells.iter().try_for_each(|&(p, _)| check_range(p))?;
            Ok(cells.len() as u64)
        }
        Op::Fill { min, max, .. } => {
            check_range(*min)?;
            check_range(*max)?;
            let (lo, hi) = (min.lo(*max), min.hi(*max));
            Ok((hi.x - lo.x + 1) as u64 * (hi.y - lo.y + 1) as u64 * (hi.z - lo.z + 1) as u64)
        }
        Op::Link { controller, targets } | Op::Unlink { controller, targets } => {
            check_range(*controller)?;
            targets.iter().try_for_each(|&p| check_range(p))?;
            Ok(targets.len() as u64)
        }
        // As many blocks as the entity has of those types: bounded by the
        // entity, not by the step, so it counts nothing against the step.
        Op::Retype { pairs } => {
            if pairs.len() > MAX_RETYPE_PAIRS {
                return Err(EditError::TooLarge(pairs.len() as u64));
            }
            Ok(0)
        }
        Op::Undo | Op::Redo => Err(EditError::MixedUndo),
    }
}

/// Most type pairs one retyping may carry: a material comes in a few dozen shapes.
const MAX_RETYPE_PAIRS: usize = 256;

/// Notes a link made or broken in a step. Making and breaking the same link
/// within one step cancel out.
fn note(change: &mut Change, made: bool, link: (Pos, u16, Pos)) {
    let (same, opposite) = if made { (&mut change.linked, &mut change.unlinked) } else { (&mut change.unlinked, &mut change.linked) };
    match opposite.iter().position(|l| *l == link) {
        Some(i) => {
            opposite.swap_remove(i);
        }
        None => same.push(link),
    }
}

fn break_link(entity: &mut Entity, change: &mut Change, controller: Pos, target: Pos) -> bool {
    match entity.unlink_any(controller, target) {
        Some(ty) => {
            note(change, false, (controller, ty, target));
            true
        }
        None => false,
    }
}

fn make_link(entity: &mut Entity, change: &mut Change, controller: Pos, target: Pos) -> bool {
    let ty = entity.get(target).ty();
    let made = ty != 0 && target != controller && entity.link(controller, target, ty);
    if made {
        note(change, true, (controller, ty, target));
    }
    made
}

/// Where the ship core sits, if this is a ship with its core in place.
fn core_of(entity: &Entity, rules: &Rules) -> Option<Pos> {
    let origin = Pos::new(0, 0, 0);
    (rules.kind == crate::links::Kind::Ship && entity.get(origin).ty() == CORE_TYPE).then_some(origin)
}

/// Links `targets` to `controller` the way the game does when V is pressed
/// on each: only pairs the game allows, and with the game's housekeeping.
fn link_all(entity: &mut Entity, change: &mut Change, rules: &Rules, controller: Pos, targets: &[Pos]) {
    let a = entity.get(controller).ty();
    if a == 0 {
        return;
    }
    // Other controllers that hold blocks of a type, looked up once per type:
    // a weapon of thousands of modules is linked in one step.
    let mut holders: HashMap<u16, Vec<Pos>> = HashMap::new();
    for &t in targets {
        let b = entity.get(t).ty();
        if b == 0 || t == controller || crate::links::is_linked(entity.links(), controller, t) || !rules.can_link(a, b) {
            continue;
        }
        if !rules.is_unchecked() {
            // A block that can have one controller leaves the one it had.
            let others = holders.entry(b).or_insert_with(|| {
                entity.links().iter().filter(|(c, groups)| **c != controller && groups.contains_key(&b)).map(|(c, _)| *c).collect()
            });
            for &other in others.iter() {
                if rules.displaces(a, b, entity.get(other).ty()) {
                    break_link(entity, change, other, t);
                }
            }
            // A weapon computer has one place each for a second weapon, an
            // effect and a light; the ship core has one for a light.
            if let Some(slot) = rules.slot(a, b) {
                let held: Vec<Pos> = entity
                    .links()
                    .get(&controller)
                    .into_iter()
                    .flat_map(|groups| groups.iter())
                    .filter(|(ty, _)| rules.slot(a, **ty) == Some(slot))
                    .flat_map(|(_, set)| set.iter().copied())
                    .collect();
                for old in held {
                    break_link(entity, change, controller, old);
                    // On a ship a computer let go of answers to the core again.
                    if slot != Slot::Light {
                        if let Some(core) = core_of(entity, rules).filter(|_| rules.can_link(CORE_TYPE, entity.get(old).ty())) {
                            make_link(entity, change, core, old);
                        }
                    }
                }
                // A computer taken as second weapon or effect gives up its own.
                if slot != Slot::Light {
                    let own: Vec<Pos> = entity
                        .links()
                        .get(&t)
                        .into_iter()
                        .flat_map(|groups| groups.iter())
                        .filter(|(ty, _)| rules.slot(b, **ty).is_some())
                        .flat_map(|(_, set)| set.iter().copied())
                        .collect();
                    for held in own {
                        break_link(entity, change, t, held);
                    }
                }
            }
        }
        make_link(entity, change, controller, t);
    }
}

/// Applies the ops of one step, with no linking rules: every link asked for
/// is made. See [`apply_with`].
pub fn apply(entity: &mut Entity, ops: &[Op]) -> Result<Change, EditError> {
    apply_with(entity, ops, &Rules::unchecked())
}

/// Applies the ops of one step. Nothing is changed if any of them is refused.
/// The caller records the returned change in a [`Journal`] to make it undoable.
pub fn apply_with(entity: &mut Entity, ops: &[Op], rules: &Rules) -> Result<Change, EditError> {
    let mut total = 0u64;
    for op in ops {
        total = total.saturating_add(budget(op)?);
    }
    if total > MAX_CELLS {
        return Err(EditError::TooLarge(total));
    }

    // Position -> (block before the step, block now). Ops of one step may overlap.
    let mut cells: HashMap<Pos, (Block, Block)> = HashMap::new();
    let mut change = Change::default();
    // Returns whether the cell was written.
    let mut write = |entity: &mut Entity, p: Pos, block: Block, when: When| -> bool {
        let old = entity.get(p);
        let new = if block.is_air() { Block::AIR } else { block };
        if old.ty() == CORE_TYPE || new.ty() == CORE_TYPE || old == new {
            return false;
        }
        let allowed = match when {
            When::Always => true,
            When::Empty => old.is_air(),
            When::Occupied => !old.is_air(),
            When::Type(t) => old.ty() == t,
        };
        if allowed {
            entity.set(p, new);
            cells.entry(p).and_modify(|e| e.1 = new).or_insert((old, new));
        }
        allowed
    };

    for op in ops {
        match op {
            Op::Set { cells: list, when } => {
                for &(p, block) in list {
                    write(entity, p, block, *when);
                }
            }
            Op::Fill { min, max, block, mode, when, link } => {
                let (lo, hi) = (min.lo(*max), min.hi(*max));
                let mut placed = Vec::new();
                for z in lo.z..=hi.z {
                    for y in lo.y..=hi.y {
                        for x in lo.x..=hi.x {
                            let on_faces = (x == lo.x || x == hi.x) as u8
                                + (y == lo.y || y == hi.y) as u8
                                + (z == lo.z || z == hi.z) as u8;
                            let wanted = match mode {
                                FillMode::Solid => true,
                                FillMode::Hollow => on_faces >= 1,
                                FillMode::Frame => on_faces >= 2,
                            };
                            if wanted {
                                let p = Pos::new(x, y, z);
                                let was_empty = entity.get(p).is_air();
                                if write(entity, p, *block, *when) && was_empty && link.is_some() {
                                    placed.push(p);
                                }
                            }
                        }
                    }
                }
                if let (Some(controller), false) = (link, placed.is_empty()) {
                    link_all(entity, &mut change, rules, *controller, &placed);
                }
            }
            Op::Link { controller, targets } => link_all(entity, &mut change, rules, *controller, targets),
            Op::Unlink { controller, targets } => {
                let a = entity.get(*controller).ty();
                for &t in targets {
                    if !break_link(entity, &mut change, *controller, t) {
                        continue;
                    }
                    // On a ship, a second weapon or effect computer let go of
                    // answers to the core again, as it did before it was taken.
                    let b = entity.get(t).ty();
                    if matches!(rules.slot(a, b), Some(Slot::Slave | Slot::Effect)) {
                        if let Some(core) = core_of(entity, rules).filter(|core| *core != *controller && rules.can_link(CORE_TYPE, b)) {
                            make_link(entity, &mut change, core, t);
                        }
                    }
                }
            }
            Op::Retype { pairs } => {
                let to_of: HashMap<u16, u16> = pairs.iter().copied().filter(|(a, b)| a != b).collect();
                if to_of.is_empty() {
                    continue;
                }
                let mut found = Vec::new();
                entity.for_each(|p, b| {
                    if let Some(&ty) = to_of.get(&b.ty()) {
                        let mut new = b;
                        new.set_type(ty);
                        found.push((p, new));
                    }
                });
                for (p, new) in found {
                    write(entity, p, new, When::Occupied);
                }
            }
            Op::Undo | Op::Redo => unreachable!("refused by budget()"),
        }
    }

    // A block that is gone or has become another type keeps none of its links,
    // neither as a controller nor as something controlled.
    let retyped: HashSet<Pos> = cells.iter().filter(|(_, (a, b))| a.ty() != b.ty()).map(|(p, _)| *p).collect();
    let mut dropped = Vec::new();
    if !retyped.is_empty() && !entity.links().is_empty() {
        for (c, groups) in entity.links() {
            let controller_gone = retyped.contains(c);
            for (ty, targets) in groups {
                for t in targets {
                    // A link this step made to a block it placed is kept, of course.
                    let fresh = change.linked.contains(&(*c, *ty, *t)) && entity.get(*t).ty() == *ty && !entity.get(*c).is_air();
                    if (controller_gone || retyped.contains(t)) && !fresh {
                        dropped.push((*c, *ty, *t));
                    }
                }
            }
        }
    }
    // Nor does a link made in this step outlive a block the same step took
    // away again, even where the cell ends as it began.
    dropped.extend(change.linked.iter().filter(|(c, ty, t)| entity.get(*t).ty() != *ty || entity.get(*c).is_air()).copied());
    for (c, ty, t) in dropped {
        if entity.unlink(c, t, ty) {
            note(&mut change, false, (c, ty, t));
        }
    }

    change.cells = cells.into_iter().filter(|(_, (a, b))| a != b).map(|(p, (a, b))| (p, a, b)).collect();
    Ok(change)
}

/// The undo history of one entity.
#[derive(Default)]
pub struct Journal {
    undo: VecDeque<Change>,
    redo: Vec<Change>,
}

impl Journal {
    pub const MAX_STEPS: usize = 200;
    /// Remembered cell changes across all steps; bounds memory after huge fills.
    pub const MAX_CELLS: usize = 8_000_000;

    pub fn record(&mut self, change: Change) {
        if change.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push_back(change);
        let mut cells: usize = self.undo.iter().map(|c| c.cells.len()).sum();
        while self.undo.len() > Self::MAX_STEPS || (cells > Self::MAX_CELLS && self.undo.len() > 1) {
            cells -= self.undo.pop_front().map_or(0, |c| c.cells.len());
        }
    }

    /// Reverses the latest step and returns what that did to the entity.
    pub fn undo(&mut self, entity: &mut Entity) -> Result<Change, EditError> {
        let step = self.undo.pop_back().ok_or(EditError::NothingToUndo)?;
        let reversal = step.inverse();
        reversal.apply_to(entity);
        self.redo.push(step);
        Ok(reversal)
    }

    pub fn redo(&mut self, entity: &mut Entity) -> Result<Change, EditError> {
        let step = self.redo.pop().ok_or(EditError::NothingToRedo)?;
        step.apply_to(entity);
        self.undo.push_back(step.clone());
        Ok(step)
    }

    /// (steps that can be undone, steps that can be redone)
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

/// Runs one step against an entity and its history, with no linking rules.
pub fn run(entity: &mut Entity, journal: &mut Journal, ops: &[Op]) -> Result<Change, EditError> {
    run_with(entity, journal, ops, &Rules::unchecked())
}

/// Runs one step against an entity and its history: either a lone undo or redo,
/// or a group of edits that become a single undoable step.
pub fn run_with(entity: &mut Entity, journal: &mut Journal, ops: &[Op], rules: &Rules) -> Result<Change, EditError> {
    match ops {
        [Op::Undo] => journal.undo(entity),
        [Op::Redo] => journal.redo(entity),
        _ => {
            let change = apply_with(entity, ops, rules)?;
            journal.record(change.clone());
            Ok(change)
        }
    }
}

/// Mirror planes, as in the game's build mode. Each value is twice the plane's
/// coordinate on that axis: an even value runs the plane through the middle of a
/// row of blocks, an odd one between two rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symmetry {
    /// Mirrors left and right.
    pub x: Option<i32>,
    /// Mirrors top and bottom.
    pub y: Option<i32>,
    /// Mirrors front and back.
    pub z: Option<i32>,
}

impl Symmetry {
    pub fn is_on(&self) -> bool {
        self.x.is_some() || self.y.is_some() || self.z.is_some()
    }

    /// A box and its mirror images, the original first. Each comes with the axes
    /// it was flipped on, which callers need to mirror block orientations too.
    /// Images that coincide are returned once.
    pub fn boxes(&self, min: Pos, max: Pos) -> Vec<(Pos, Pos, [bool; 3])> {
        let (lo, hi) = (min.lo(max), min.hi(max));
        let mut out: Vec<(Pos, Pos, [bool; 3])> = Vec::with_capacity(8);
        for mask in 0..8u8 {
            let flip = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
            if (flip[0] && self.x.is_none()) || (flip[1] && self.y.is_none()) || (flip[2] && self.z.is_none()) {
                continue;
            }
            let m = |v: i32, plane: Option<i32>, on: bool| if on { plane.unwrap_or(0) - v } else { v };
            let a = Pos::new(m(lo.x, self.x, flip[0]), m(lo.y, self.y, flip[1]), m(lo.z, self.z, flip[2]));
            let b = Pos::new(m(hi.x, self.x, flip[0]), m(hi.y, self.y, flip[1]), m(hi.z, self.z, flip[2]));
            let image = (a.lo(b), a.hi(b));
            if !out.iter().any(|(p, q, _)| (*p, *q) == image) {
                out.push((image.0, image.1, flip));
            }
        }
        out
    }

    pub fn points(&self, p: Pos) -> Vec<(Pos, [bool; 3])> {
        self.boxes(p, p).into_iter().map(|(a, _, flip)| (a, flip)).collect()
    }
}

/// Whether the block at `p` has an empty cell beside it at one of its six
/// faces: it is on a surface, not in the filling behind one.
pub fn on_surface(entity: &Entity, p: Pos) -> bool {
    crate::shape::SIDE_DIRS.iter().any(|d| entity.get(Pos::new(p.x + d[0], p.y + d[1], p.z + d[2])).is_air())
}

/// Whether an empty cell is at most `depth` cells from `p`, straight along
/// one of the axes: 1 is a block on the surface.
pub fn near_surface(entity: &Entity, p: Pos, depth: i32) -> bool {
    crate::shape::SIDE_DIRS.iter().any(|d| (1..=depth.max(1)).any(|k| entity.get(Pos::new(p.x + d[0] * k, p.y + d[1] * k, p.z + d[2] * k)).is_air()))
}

/// What a paint bucket covers: the blocks that touch, face to face, the one
/// at `start` and are of the same material (`material` names a block's
/// material, or nothing for a block that is not painted). With a `depth`,
/// only blocks that far or less from an empty cell, straight along an axis,
/// count: 1 is the surface and not the filling behind it, so that a fill on
/// a solid hull does not turn the inside of the hull too. Without one, every
/// touching block of the material counts, all the way through. At most
/// `limit` blocks are found; the answer says whether there were more.
pub fn paint_region(entity: &Entity, start: Pos, material: impl Fn(u16) -> Option<u16>, depth: Option<i32>, limit: usize) -> (Vec<Pos>, bool) {
    const SIDES: [[i32; 3]; 6] = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];
    let first = entity.get(start);
    let Some(wanted) = (!first.is_air()).then(|| material(first.ty())).flatten() else { return (Vec::new(), false) };
    let step = |p: Pos, d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
    let open = |p: Pos| depth.is_none_or(|d| near_surface(entity, p, d));
    let (mut found, mut seen, mut waiting) = (Vec::new(), HashSet::new(), VecDeque::new());
    seen.insert(start);
    waiting.push_back(start);
    while let Some(p) = waiting.pop_front() {
        if found.len() >= limit {
            return (found, true);
        }
        found.push(p);
        for d in SIDES {
            let next = step(p, d);
            let block = entity.get(next);
            if block.is_air() || material(block.ty()) != Some(wanted) || !open(next) || !seen.insert(next) {
                continue;
            }
            waiting.push_back(next);
        }
    }
    (found, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HULL: u16 = 5;
    const COMPUTER: u16 = 6;
    const BARREL: u16 = 16;

    fn ship() -> Entity {
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(CORE_TYPE, 0, false));
        e
    }

    fn fill(min: (i32, i32, i32), max: (i32, i32, i32), ty: u16, mode: FillMode, when: When) -> Op {
        Op::Fill {
            min: Pos::new(min.0, min.1, min.2),
            max: Pos::new(max.0, max.1, max.2),
            block: if ty == 0 { Block::AIR } else { Block::new(ty, 0, false) },
            mode,
            when,
            link: None,
        }
    }

    #[test]
    fn placing_fills_only_empty_cells_and_never_touches_the_core() {
        let mut e = ship();
        e.set(Pos::new(1, 0, 0), Block::new(BARREL, 0, false));
        let change = apply(&mut e, &[fill((-1, 0, 0), (1, 0, 0), HULL, FillMode::Solid, When::Empty)]).unwrap();
        assert_eq!(change.cells.len(), 1);
        assert_eq!(e.get(Pos::new(-1, 0, 0)).ty(), HULL);
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert_eq!(e.get(Pos::new(1, 0, 0)).ty(), BARREL);

        // Removing or overwriting everything still leaves the core.
        apply(&mut e, &[fill((-5, -5, -5), (5, 5, 5), 0, FillMode::Solid, When::Always)]).unwrap();
        assert_eq!(e.block_count(), 1);
        apply(&mut e, &[fill((0, 0, 0), (0, 0, 0), HULL, FillMode::Solid, When::Always)]).unwrap();
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        // A second core cannot be placed.
        apply(&mut e, &[fill((3, 0, 0), (3, 0, 0), CORE_TYPE, FillMode::Solid, When::Always)]).unwrap();
        assert_eq!(e.block_count(), 1);
    }

    #[test]
    fn fill_modes_cover_volume_faces_or_edges() {
        for (mode, expected) in [(FillMode::Solid, 64), (FillMode::Hollow, 56), (FillMode::Frame, 32)] {
            let mut e = Entity::new();
            let change = apply(&mut e, &[fill((10, 10, 10), (13, 13, 13), HULL, mode, When::Always)]).unwrap();
            assert_eq!(change.cells.len(), expected, "{mode:?}");
            assert_eq!(e.block_count(), expected as u64);
        }
        // Corners may come in any order.
        let mut e = Entity::new();
        apply(&mut e, &[fill((3, 0, -2), (1, 0, -4), HULL, FillMode::Solid, When::Always)]).unwrap();
        assert_eq!(e.block_count(), 9);
    }

    #[test]
    fn painting_changes_only_matching_blocks() {
        let mut e = ship();
        apply(&mut e, &[fill((1, 0, 0), (4, 0, 0), HULL, FillMode::Solid, When::Empty)]).unwrap();
        e.set(Pos::new(2, 0, 0), Block::new(BARREL, 0, false));
        apply(&mut e, &[fill((-3, 0, 0), (9, 0, 0), 77, FillMode::Solid, When::Type(HULL))]).unwrap();
        assert_eq!(e.get(Pos::new(1, 0, 0)).ty(), 77);
        assert_eq!(e.get(Pos::new(2, 0, 0)).ty(), BARREL);
        assert_eq!(e.get(Pos::new(-1, 0, 0)), Block::AIR);
        apply(&mut e, &[fill((-3, 0, 0), (9, 0, 0), 99, FillMode::Solid, When::Occupied)]).unwrap();
        assert_eq!(e.get(Pos::new(2, 0, 0)).ty(), 99);
        assert_eq!(e.block_count(), 5);
    }

    #[test]
    fn undo_and_redo_restore_blocks_and_links() {
        let mut e = ship();
        let mut j = Journal::default();
        let (computer, barrel) = (Pos::new(0, 1, 0), Pos::new(0, 1, 1));
        run(&mut e, &mut j, &[Op::Set {
            cells: vec![(computer, Block::new(COMPUTER, 0, false)), (barrel, Block::new(BARREL, 0, false))],
            when: When::Empty,
        }])
        .unwrap();
        let linked = run(&mut e, &mut j, &[Op::Link { controller: computer, targets: vec![barrel] }]).unwrap();
        assert_eq!(linked.linked, vec![(computer, BARREL, barrel)]);
        assert_eq!(e.link_count(), 1);

        // Removing the barrel takes its link with it, and undo brings both back.
        let removed = run(&mut e, &mut j, &[fill((0, 1, 1), (0, 1, 1), 0, FillMode::Solid, When::Always)]).unwrap();
        assert_eq!(removed.unlinked, vec![(computer, BARREL, barrel)]);
        assert_eq!(e.link_count(), 0);
        let back = run(&mut e, &mut j, &[Op::Undo]).unwrap();
        assert_eq!(back.linked, vec![(computer, BARREL, barrel)]);
        assert_eq!(e.get(barrel).ty(), BARREL);
        assert_eq!(e.link_count(), 1);
        assert_eq!(j.depth(), (2, 1));

        run(&mut e, &mut j, &[Op::Redo]).unwrap();
        assert_eq!(e.get(barrel), Block::AIR);
        assert_eq!(e.link_count(), 0);

        // Back to an empty ship, then nothing more to undo.
        for _ in 0..3 {
            run(&mut e, &mut j, &[Op::Undo]).unwrap();
        }
        assert_eq!(e.block_count(), 1);
        assert_eq!(run(&mut e, &mut j, &[Op::Undo]), Err(EditError::NothingToUndo));
        // A new edit discards what could have been redone.
        run(&mut e, &mut j, &[fill((5, 5, 5), (5, 5, 5), HULL, FillMode::Solid, When::Empty)]).unwrap();
        assert_eq!(run(&mut e, &mut j, &[Op::Redo]), Err(EditError::NothingToRedo));
    }

    #[test]
    fn removing_a_controller_drops_everything_it_ran() {
        let mut e = ship();
        let computer = Pos::new(2, 0, 0);
        e.set(computer, Block::new(COMPUTER, 0, false));
        for z in 1..4 {
            e.set(Pos::new(2, 0, z), Block::new(BARREL, 0, false));
        }
        apply(&mut e, &[Op::Link { controller: computer, targets: (1..4).map(|z| Pos::new(2, 0, z)).collect() }]).unwrap();
        assert_eq!(e.link_count(), 3);
        let change = apply(&mut e, &[Op::Set { cells: vec![(computer, Block::AIR)], when: When::Always }]).unwrap();
        assert_eq!(change.unlinked.len(), 3);
        assert_eq!(e.link_count(), 0);
    }

    #[test]
    fn overlapping_ops_in_one_step_record_first_before_and_last_after() {
        let mut e = Entity::new();
        e.set(Pos::new(1, 1, 1), Block::new(HULL, 0, false));
        let change = apply(
            &mut e,
            &[
                fill((1, 1, 1), (2, 1, 1), 40, FillMode::Solid, When::Always),
                fill((1, 1, 1), (1, 1, 1), 50, FillMode::Solid, When::Always),
                // Putting a block back to what it was is no change at all.
                fill((7, 7, 7), (7, 7, 7), 60, FillMode::Solid, When::Always),
                fill((7, 7, 7), (7, 7, 7), 0, FillMode::Solid, When::Always),
            ],
        )
        .unwrap();
        let mut cells = change.cells.clone();
        cells.sort_by_key(|c| c.0);
        assert_eq!(
            cells,
            vec![
                (Pos::new(1, 1, 1), Block::new(HULL, 0, false), Block::new(50, 0, false)),
                (Pos::new(2, 1, 1), Block::AIR, Block::new(40, 0, false)),
            ]
        );
        change.inverse().apply_to(&mut e);
        assert_eq!(e.block_count(), 1);
        assert_eq!(e.get(Pos::new(1, 1, 1)).ty(), HULL);
    }

    #[test]
    fn oversized_or_far_away_edits_change_nothing() {
        let mut e = ship();
        let small = fill((1, 0, 0), (1, 0, 0), HULL, FillMode::Solid, When::Empty);
        let huge = fill((0, 0, 0), (200, 200, 200), HULL, FillMode::Solid, When::Empty);
        assert!(matches!(apply(&mut e, &[small.clone(), huge]), Err(EditError::TooLarge(_))));
        let far = fill((0, 0, 0), (0, 0, MAX_COORD + 1), HULL, FillMode::Solid, When::Empty);
        assert!(matches!(apply(&mut e, &[small.clone(), far]), Err(EditError::OutOfRange(..))));
        assert_eq!(apply(&mut e, &[small, Op::Undo]), Err(EditError::MixedUndo));
        assert_eq!(e.block_count(), 1);
    }

    #[test]
    fn dirty_segments_include_neighbours_at_borders() {
        // Design position 15 is file position 31: the last row of segment 0.
        let at = |p: Pos| Change { cells: vec![(p, Block::AIR, Block::new(HULL, 0, false))], ..Default::default() };
        assert_eq!(at(Pos::new(0, 0, 0)).dirty_segments().len(), 1);
        let border = at(Pos::new(15, 0, 0)).dirty_segments();
        assert!(border.contains(&(0, 0, 0)) && border.contains(&(1, 0, 0)) && border.len() == 2);
        assert_eq!(at(Pos::new(-16, -16, -16)).dirty_segments().len(), 4);
    }

    #[test]
    fn symmetry_mirrors_boxes_across_each_plane() {
        // Plane through the core's column (x = 0) and one between y = 2 and y = 3.
        let s = Symmetry { x: Some(0), y: Some(5), z: None };
        let images = s.boxes(Pos::new(2, 0, 7), Pos::new(4, 1, 7));
        let boxes: Vec<_> = images.iter().map(|(a, b, _)| (*a, *b)).collect();
        assert_eq!(
            boxes,
            vec![
                (Pos::new(2, 0, 7), Pos::new(4, 1, 7)),
                (Pos::new(-4, 0, 7), Pos::new(-2, 1, 7)),
                (Pos::new(2, 4, 7), Pos::new(4, 5, 7)),
                (Pos::new(-4, 4, 7), Pos::new(-2, 5, 7)),
            ]
        );
        assert_eq!(images[3].2, [true, true, false]);
        // A block on the plane is its own image.
        assert_eq!(Symmetry { x: Some(0), ..Default::default() }.points(Pos::new(0, 3, 3)).len(), 1);
        assert_eq!(Symmetry::default().points(Pos::new(9, 9, 9)).len(), 1);
    }

    #[test]
    fn ops_have_a_stable_json_form() {
        let op = Op::Fill {
            min: Pos::new(1, 2, 3),
            max: Pos::new(4, 5, 6),
            block: Block(7),
            mode: FillMode::Hollow,
            when: When::Type(5),
            link: None,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"fill","min":[1,2,3],"max":[4,5,6],"block":7,"mode":"hollow","when":{"type":5}}"#);
        assert_eq!(serde_json::from_str::<Op>(&json).unwrap(), op);
        // Defaults keep hand-written and model-written ops short.
        let short: Op = serde_json::from_str(r#"{"op":"set","cells":[[[0,1,0],5]]}"#).unwrap();
        assert_eq!(short, Op::Set { cells: vec![(Pos::new(0, 1, 0), Block(5))], when: When::Always });
        assert_eq!(serde_json::from_str::<Op>(r#"{"op":"undo"}"#).unwrap(), Op::Undo);
    }

    /// A small block list for the linking rules: core, two cannon computers
    /// and a beam computer (all "main"), an effect computer, barrels, a
    /// light, an activation module, and hull.
    struct Blocks(HashMap<u16, crate::links::LinkTraits>);

    impl crate::links::BlockLinks for Blocks {
        fn link_traits(&self, ty: u16) -> Option<&crate::links::LinkTraits> {
            self.0.get(&ty)
        }
    }

    const BEAM_COMPUTER: u16 = 414;
    const EFFECT: u16 = 349;
    const LIGHT: u16 = 55;
    const ACTIVATION: u16 = 405;

    fn blocks() -> Blocks {
        use crate::links::{flag, LinkTraits};
        let traits = |controls: &[u16], controlled_by: &[u16], flags: u16| LinkTraits { controls: controls.to_vec(), controlled_by: controlled_by.to_vec(), flags, inventory: 0 };
        Blocks(HashMap::from([
            (CORE_TYPE, traits(&[BARREL], &[], 0)),
            (COMPUTER, traits(&[BARREL], &[CORE_TYPE], flag::MAIN | flag::CAN_ACTIVATE)),
            (BEAM_COMPUTER, traits(&[415], &[CORE_TYPE], flag::MAIN | flag::CAN_ACTIVATE)),
            (EFFECT, traits(&[350], &[CORE_TYPE], flag::EFFECT)),
            (BARREL, traits(&[], &[COMPUTER, CORE_TYPE], flag::CAN_ACTIVATE)),
            (LIGHT, traits(&[], &[], flag::LIGHT | flag::CAN_ACTIVATE)),
            (ACTIVATION, traits(&[], &[], flag::SIGNAL | flag::CAN_ACTIVATE)),
            (HULL, traits(&[], &[], 0)),
        ]))
    }

    /// A ship with its core, the listed blocks, and the game's rules.
    fn ship_with(parts: &[(Pos, u16)]) -> Entity {
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(CORE_TYPE, 0, false));
        for &(p, ty) in parts {
            e.set(p, Block::new(ty, 0, false));
        }
        e
    }

    fn linked(e: &Entity, controller: Pos, target: Pos) -> bool {
        crate::links::is_linked(e.links(), controller, target)
    }

    #[test]
    fn only_pairs_the_game_allows_are_linked() {
        let blocks = blocks();
        let rules = Rules::new(&blocks, crate::links::Kind::Ship);
        let (computer, barrel, hull, light) = (Pos::new(1, 0, 0), Pos::new(1, 0, 1), Pos::new(1, 0, 2), Pos::new(1, 0, 3));
        let mut e = ship_with(&[(computer, COMPUTER), (barrel, BARREL), (hull, HULL), (light, LIGHT)]);
        let change = apply_with(&mut e, &[Op::Link { controller: computer, targets: vec![barrel, hull, light, computer] }], &rules).unwrap();
        assert!(linked(&e, computer, barrel) && linked(&e, computer, light));
        assert!(!linked(&e, computer, hull) && !linked(&e, computer, computer));
        assert_eq!(change.linked.len(), 2);
        // The other way round is not a link the game knows.
        let change = apply_with(&mut e, &[Op::Link { controller: barrel, targets: vec![computer] }], &rules).unwrap();
        assert!(change.is_empty());
        // Without rules, as for replaying a recorded step, anything goes.
        assert!(!apply(&mut e, &[Op::Link { controller: barrel, targets: vec![hull] }]).unwrap().is_empty());
    }

    #[test]
    fn a_module_has_one_computer_and_a_logic_block_many_controllers() {
        let blocks = blocks();
        let rules = Rules::new(&blocks, crate::links::Kind::Ship);
        let (first, second, barrel) = (Pos::new(1, 0, 0), Pos::new(2, 0, 0), Pos::new(1, 0, 1));
        let (switch_a, switch_b, lamp) = (Pos::new(3, 0, 0), Pos::new(4, 0, 0), Pos::new(3, 0, 1));
        let mut e = ship_with(&[(first, COMPUTER), (second, COMPUTER), (barrel, BARREL), (switch_a, ACTIVATION), (switch_b, ACTIVATION), (lamp, LIGHT)]);
        let mut journal = Journal::default();
        run_with(&mut e, &mut journal, &[Op::Link { controller: first, targets: vec![barrel] }], &rules).unwrap();
        // Linked to the second computer, the barrel leaves the first: the
        // game treats a module under two computers as cheating.
        let change = run_with(&mut e, &mut journal, &[Op::Link { controller: second, targets: vec![barrel] }], &rules).unwrap();
        assert!(linked(&e, second, barrel) && !linked(&e, first, barrel));
        assert_eq!((change.linked.len(), change.unlinked.len()), (1, 1));
        // Undo gives it back to the first.
        journal.undo(&mut e).unwrap();
        assert!(linked(&e, first, barrel) && !linked(&e, second, barrel));
        // A light, too, answers to one switch only; a switch to as many as link to it.
        apply_with(&mut e, &[Op::Link { controller: switch_a, targets: vec![lamp, switch_b] }], &rules).unwrap();
        apply_with(&mut e, &[Op::Link { controller: switch_b, targets: vec![lamp] }], &rules).unwrap();
        assert!(linked(&e, switch_b, lamp) && !linked(&e, switch_a, lamp));
        apply_with(&mut e, &[Op::Link { controller: first, targets: vec![] }, Op::Link { controller: switch_b, targets: vec![switch_a] }, Op::Link { controller: lamp, targets: vec![switch_a] }], &rules).unwrap();
        let third = Pos::new(5, 0, 0);
        e.set(third, Block::new(ACTIVATION, 0, false));
        apply_with(&mut e, &[Op::Link { controller: third, targets: vec![switch_b] }], &rules).unwrap();
        assert!(linked(&e, switch_a, switch_b) && linked(&e, third, switch_b), "logic blocks keep every controller");
    }

    #[test]
    fn a_weapon_computer_has_one_second_weapon_and_the_core_takes_back_what_is_let_go() {
        let blocks = blocks();
        let rules = Rules::new(&blocks, crate::links::Kind::Ship);
        let core = Pos::new(0, 0, 0);
        let (cannon, beam, other, effect, lamp) = (Pos::new(1, 0, 0), Pos::new(2, 0, 0), Pos::new(3, 0, 0), Pos::new(4, 0, 0), Pos::new(5, 0, 0));
        let mut e = ship_with(&[(cannon, COMPUTER), (beam, BEAM_COMPUTER), (other, COMPUTER), (effect, EFFECT), (lamp, LIGHT)]);
        // As the game links computers when they are placed: all answer to the core.
        apply_with(&mut e, &[Op::Link { controller: core, targets: vec![cannon, beam, other, effect] }], &rules).unwrap();
        // The beam computer becomes the cannon's second weapon: it leaves the core.
        apply_with(&mut e, &[Op::Link { controller: cannon, targets: vec![beam, effect, lamp] }], &rules).unwrap();
        assert!(linked(&e, cannon, beam) && !linked(&e, core, beam));
        assert!(linked(&e, cannon, effect) && !linked(&e, core, effect) && linked(&e, cannon, lamp));
        assert!(linked(&e, core, cannon), "the master stays on the core");
        // Another second weapon takes its place, and the beam computer goes back to the core.
        apply_with(&mut e, &[Op::Link { controller: cannon, targets: vec![other] }], &rules).unwrap();
        assert!(linked(&e, cannon, other) && !linked(&e, cannon, beam) && linked(&e, core, beam));
        assert!(linked(&e, cannon, effect), "the effect has its own place");
        // A computer taken as second weapon gives up its own second weapon.
        apply_with(&mut e, &[Op::Link { controller: beam, targets: vec![cannon] }], &rules).unwrap();
        assert!(linked(&e, beam, cannon) && !linked(&e, cannon, other) && !linked(&e, cannon, effect) && !linked(&e, cannon, lamp));
        assert!(!linked(&e, core, cannon));
        // Unlinked, it answers to the core again.
        apply_with(&mut e, &[Op::Unlink { controller: beam, targets: vec![cannon] }], &rules).unwrap();
        assert!(linked(&e, core, cannon) && !linked(&e, beam, cannon));
        // On a station there is no core to go back to.
        let station = Rules::new(&blocks, crate::links::Kind::Station);
        let mut s = Entity::new();
        s.set(cannon, Block::new(COMPUTER, 0, false));
        s.set(beam, Block::new(BEAM_COMPUTER, 0, false));
        apply_with(&mut s, &[Op::Link { controller: cannon, targets: vec![beam] }], &station).unwrap();
        let change = apply_with(&mut s, &[Op::Unlink { controller: cannon, targets: vec![beam] }], &station).unwrap();
        assert_eq!((change.unlinked.len(), change.linked.len()), (1, 0));
    }

    #[test]
    fn blocks_placed_with_a_link_are_linked_as_the_game_links_what_is_placed() {
        let blocks = blocks();
        let rules = Rules::new(&blocks, crate::links::Kind::Ship);
        let computer = Pos::new(1, 0, 0);
        let mut e = ship_with(&[(computer, COMPUTER), (Pos::new(1, 0, 2), BARREL), (Pos::new(1, 0, 3), HULL)]);
        let mut journal = Journal::default();
        // A row of barrels from z = 1 to 4, placed with the computer selected.
        // z = 2 holds a barrel already and z = 3 hull: neither is placed, so
        // neither is linked.
        let place = Op::Fill { min: Pos::new(1, 0, 1), max: Pos::new(1, 0, 4), block: Block::new(BARREL, 0, false), mode: FillMode::Solid, when: When::Empty, link: Some(computer) };
        let change = run_with(&mut e, &mut journal, &[place], &rules).unwrap();
        assert_eq!((change.cells.len(), change.linked.len()), (2, 2));
        assert!(linked(&e, computer, Pos::new(1, 0, 1)) && linked(&e, computer, Pos::new(1, 0, 4)));
        assert!(!linked(&e, computer, Pos::new(1, 0, 2)));
        // Hull placed with the computer selected is just hull.
        let hull = Op::Fill { min: Pos::new(2, 0, 1), max: Pos::new(2, 0, 2), block: Block::new(HULL, 0, false), mode: FillMode::Solid, when: When::Empty, link: Some(computer) };
        assert!(run_with(&mut e, &mut journal, &[hull], &rules).unwrap().linked.is_empty());
        // Undo takes the hull, then the barrels and their links.
        journal.undo(&mut e).unwrap();
        journal.undo(&mut e).unwrap();
        assert_eq!(e.link_count(), 0);
        assert!(e.get(Pos::new(1, 0, 1)).is_air() && e.get(Pos::new(1, 0, 2)).ty() == BARREL);
        journal.redo(&mut e).unwrap();
        assert!(linked(&e, computer, Pos::new(1, 0, 4)));
        // Placing and removing in one step leaves no link behind.
        let both = [
            Op::Fill { min: Pos::new(1, 1, 1), max: Pos::new(1, 1, 1), block: Block::new(BARREL, 0, false), mode: FillMode::Solid, when: When::Empty, link: Some(computer) },
            Op::Fill { min: Pos::new(1, 1, 1), max: Pos::new(1, 1, 1), block: Block::AIR, mode: FillMode::Solid, when: When::Always, link: None },
        ];
        assert!(apply_with(&mut e, &both, &rules).unwrap().is_empty());
    }

    #[test]
    fn a_fill_covers_the_touching_blocks_of_one_material_on_the_surface() {
        // A flat 5 by 5 plate of material 5 (types 5 and 7 are the same
        // material), with a stripe of material 9 across the middle and a
        // block of 5 hanging off a corner by an edge only.
        let mut e = ship();
        let material = |ty: u16| match ty {
            5 | 7 => Some(5),
            9 => Some(9),
            _ => None,
        };
        for x in -2..=2 {
            for z in -2..=2 {
                let ty = if x == 0 { 9 } else if (x + z) % 2 == 0 { 5 } else { 7 };
                e.set(Pos::new(x, 1, z), Block::new(ty, 0, false));
            }
        }
        e.set(Pos::new(3, 2, 3), Block::new(5, 0, false));
        let (left, more) = paint_region(&e, Pos::new(-2, 1, -2), material, Some(1), 1000);
        assert!(!more);
        // The 10 blocks left of the stripe; the stripe stops it, and the one
        // on the corner touches nothing by a face.
        assert_eq!(left.len(), 10);
        assert!(left.iter().all(|p| p.x < 0));
        let (stripe, _) = paint_region(&e, Pos::new(0, 1, 0), material, Some(1), 1000);
        assert_eq!(stripe.len(), 5);
        // A block that is not painted material, or air, starts nothing.
        assert!(paint_region(&e, Pos::new(0, 0, 0), material, Some(1), 1000).0.is_empty());
        assert!(paint_region(&e, Pos::new(9, 9, 9), material, Some(1), 1000).0.is_empty());
        // The limit cuts it short and says so.
        let (some, more) = paint_region(&e, Pos::new(-2, 1, -2), material, Some(1), 4);
        assert_eq!((some.len(), more), (4, true));
    }

    #[test]
    fn a_fill_leaves_the_filling_inside_a_solid_hull_alone() {
        let mut e = ship();
        for x in 0..5 {
            for y in 1..6 {
                for z in 0..5 {
                    e.set(Pos::new(x, y, z), Block::new(HULL, 0, false));
                }
            }
        }
        let (found, _) = paint_region(&e, Pos::new(2, 5, 2), |ty| (ty == HULL).then_some(1), Some(1), 10_000);
        // The 5 by 5 by 5 block has 125 cells, 27 of them inside (3 by 3 by 3).
        assert_eq!(found.len(), 125 - 27);
        assert!(found.iter().all(|p| !(1..=3).contains(&p.x) || !(2..=4).contains(&p.y) || !(1..=3).contains(&p.z)));
        // Two deep, it takes the next layer in too: all but the middle cell.
        let (two, _) = paint_region(&e, Pos::new(2, 5, 2), |ty| (ty == HULL).then_some(1), Some(2), 10_000);
        assert_eq!(two.len(), 124);
        assert!(!two.contains(&Pos::new(2, 3, 2)));
        // Asked to go through, it paints the filling too.
        let (all, _) = paint_region(&e, Pos::new(2, 5, 2), |ty| (ty == HULL).then_some(1), None, 10_000);
        assert_eq!(all.len(), 125);
    }

    #[test]
    fn retyping_swaps_one_material_for_another_keeping_each_block_turned() {
        let mut e = ship();
        let (grey, grey_wedge, red, red_wedge) = (5u16, 7u16, 9u16, 11u16);
        e.set(Pos::new(1, 0, 0), Block::new(grey, 0, false));
        e.set(Pos::new(2, 0, 0), Block::new(grey_wedge, 13, false));
        e.set(Pos::new(3, 0, 0), Block::new(BARREL, 0, false));
        let mut j = Journal::default();
        let change = run(&mut e, &mut j, &[Op::Retype { pairs: vec![(grey, red), (grey_wedge, red_wedge), (CORE_TYPE, red)] }]).unwrap();
        assert_eq!(change.cells.len(), 2);
        assert_eq!(e.get(Pos::new(1, 0, 0)).ty(), red);
        assert_eq!((e.get(Pos::new(2, 0, 0)).ty(), e.get(Pos::new(2, 0, 0)).orientation()), (red_wedge, 13));
        assert_eq!(e.get(Pos::new(3, 0, 0)).ty(), BARREL, "other types are left alone");
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE, "the core is never touched");
        // One undo puts every block back.
        run(&mut e, &mut j, &[Op::Undo]).unwrap();
        assert_eq!(e.get(Pos::new(1, 0, 0)).ty(), grey);
        assert_eq!((e.get(Pos::new(2, 0, 0)).ty(), e.get(Pos::new(2, 0, 0)).orientation()), (grey_wedge, 13));
        // Too many pairs is refused.
        let many: Vec<(u16, u16)> = (0..300).map(|i| (i, i + 1)).collect();
        assert!(matches!(run(&mut e, &mut j, &[Op::Retype { pairs: many }]), Err(EditError::TooLarge(_))));
    }
}
