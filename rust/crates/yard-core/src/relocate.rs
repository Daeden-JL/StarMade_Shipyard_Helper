//! Moving a ship's core to another cell of the design.
//!
//! The game keeps every ship's core in one fixed cell of its file, so the
//! core cannot simply be placed elsewhere: it is moved by moving the whole
//! design around it. The core and the block at the chosen cell swap places,
//! and every position in the entity (blocks, links, and everything the
//! blueprint stores by position) then shifts so that the core is at the
//! origin again.
//!
//! Everything stored by position moves with it: blocks, links, the rail
//! dockers, cargo blocks and wireless modules the meta file lists, the
//! records of the entities docked to this one, this entity's own record in
//! its parent, and the wireless links other entities have to it. The one
//! thing that cannot move is the manager tag, per-block data (storage
//! contents, display text, settings) that the editor cannot read; it is
//! dropped, and the game starts those blocks afresh.

use crate::blueprint::BlueprintEntity;
use crate::dock::{Dock, ROTATORS, TURRET_AXIS};
use crate::dockrec::DockRecord;
use crate::edit::CORE_TYPE;
use crate::entity::Entity;
use crate::metafile::{MetaFile, Tail};
use crate::catalog::{Catalog, Shape};
use crate::shape::{turn_step, GridTurn, ShapeSet, NO_GRID_TURN};
use crate::{Blueprint, Error, Pos, Result};
use std::collections::{BTreeMap, BTreeSet};

/// What came of a relocation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relocated {
    /// Things the player should know, in words: what was left out or reset.
    pub notes: Vec<String>,
}

/// Positions in links and meta files are 16-bit numbers counted from the
/// core at 16; stay well inside them.
const LIMIT: i32 = 30_000;

fn refuse(detail: impl Into<String>) -> Error {
    Error::Unsupported { what: "relocating the core", detail: detail.into() }
}

/// The meta file of an entity, which must be one this can change.
fn read_meta(e: &BlueprintEntity, which: &str) -> Result<Option<MetaFile>> {
    if e.meta.is_empty() {
        return Ok(None);
    }
    let meta = MetaFile::parse(&e.meta).map_err(|err| refuse(format!("the meta file of {which} cannot be read ({err})")))?;
    if meta.version < 4 {
        return Err(refuse(format!("the meta file of {which} is from an old version of the game (version {})", meta.version)));
    }
    if meta.has_legacy_docks() {
        return Err(refuse(format!("{which} has docks of the old kind, whose positions are not understood")));
    }
    Ok(Some(meta))
}

/// What the entity at `path` is called in words.
pub(crate) fn called(path: &str) -> String {
    if path.is_empty() { "the main entity".to_string() } else { format!("the docked entity {path}") }
}

/// The blocks and links of `old` with every position taken through `f` and
/// every block's orientation through `orient`, less the blocks at the cells
/// `gone` says yes to and the links that end on them.
fn translated(old: &Entity, f: &dyn Fn(Pos) -> Pos, orient: &dyn Fn(u16, u8) -> u8, gone: &dyn Fn(Pos) -> bool) -> Entity {
    let mut new = Entity::new();
    old.for_each(|p, mut b| {
        if gone(p) {
            return;
        }
        b.set_orientation(orient(b.ty(), b.orientation()));
        new.set(f(p), b)
    });
    for (controller, groups) in old.links() {
        for (ty, targets) in groups {
            for target in targets {
                if !gone(*controller) && !gone(*target) {
                    new.link(f(*controller), f(*target), *ty);
                }
            }
        }
    }
    new
}

/// What a relocation needs to know about block types: how a block's
/// orientation turns with its entity, and which types keep data by
/// position in the manager tag (so that a swap that moves one of them
/// resets that data). The block catalog knows both.
pub trait Aid {
    /// The orientation of a block of type `ty` that has `orientation` now,
    /// once its entity turns by `turn`.
    fn turned(&self, ty: u16, orientation: u8, turn: &GridTurn) -> u8;
    fn keeps_data(&self, ty: u16) -> bool;
}

impl Aid for Catalog {
    fn turned(&self, ty: u16, o: u8, turn: &GridTurn) -> u8 {
        self.get(ty).map_or(o, |d| ShapeSet::turned(d.shape, o, d.shape == Shape::Cube && (d.slab > 0 || d.sides == 6), turn))
    }

    fn keeps_data(&self, ty: u16) -> bool {
        crate::turret::keeps_data(self, ty)
    }
}

/// Without a catalog: nothing turns, and any block may keep data.
struct NoAid;

impl Aid for NoAid {
    fn turned(&self, _: u16, o: u8, _: &GridTurn) -> u8 {
        o
    }

    fn keeps_data(&self, ty: u16) -> bool {
        ty != 0
    }
}

/// How a core is to move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    /// The cell the core goes to, in the entity's coordinates as they are.
    pub to: Pos,
    /// The way (a step along one axis, in the entity's coordinates as they
    /// are) that is to become the core's forward, +Z, and the way that is to
    /// become its up, +Y. None leaves the entity facing as it is.
    pub facing: Option<([i32; 3], [i32; 3])>,
    /// On a turret, the dock goes with the core (see [`relocate`]).
    pub dock_follows: bool,
}

/// Moves the core of the entity at `path` ("" for the root, `ATTACHED_0` ...)
/// to the cell `to` of that entity (in its design coordinates, the core being
/// at the origin now), and shifts the whole design so the core is at the
/// origin again.
///
/// Refuses, leaving the blueprint as it was, when the entity has no core at
/// its origin, when its links file could not be read, or when a meta file
/// that holds positions is of an old version, has docks of the old kind or
/// cannot be read; and when the new design would not fit the 16-bit
/// positions the files use.
pub fn relocate_core(bp: &mut Blueprint, path: &str, to: Pos) -> Result<Relocated> {
    relocate(bp, path, Move { to, facing: None, dock_follows: false }, &NoAid)
}

/// Moves the core as [`relocate_core`] does and turns the entity round it, so
/// that the way `forward` points now (a step along one axis, in the entity's
/// coordinates as they are) becomes the core's forward, +Z, and `up` its up,
/// +Y. The core is what a ship is steered by, so this is how a design built
/// facing another way is made to fly the way it looks. Every block turns
/// with the entity (its shape or facing turned to match, as the catalog
/// says it turns); docked entities turn with it on their rails; a docked
/// entity that is turned stays where it is docked, turned on its docker.
pub fn relocate_and_turn(bp: &mut Blueprint, catalog: &Catalog, path: &str, to: Pos, forward: [i32; 3], up: [i32; 3]) -> Result<Relocated> {
    relocate(bp, path, Move { to, facing: Some((forward, up)), dock_follows: false }, catalog)
}

/// The turn that takes what points `forward` to +Z and what points `up` to
/// +Y; refused unless they are two different axes.
fn grid_turn_to(forward: [i32; 3], up: [i32; 3]) -> Result<GridTurn> {
    let unit = |v: [i32; 3]| v.iter().map(|c| c.abs()).sum::<i32>() == 1;
    if !unit(forward) || !unit(up) || forward.iter().zip(up).map(|(a, b)| a * b).sum::<i32>() != 0 {
        return Err(refuse("forward and up must be two different axes"));
    }
    // The rows take a step to the new axes: right is up × forward.
    let right = [up[1] * forward[2] - up[2] * forward[1], up[2] * forward[0] - up[0] * forward[2], up[0] * forward[1] - up[1] * forward[0]];
    Ok([right, up, forward])
}

/// A dock's turn at rest as a grid turn, if it is one (every dock at rest
/// keeps blocks on the grid).
pub(crate) fn grid(turn: &crate::dock::Turn) -> Option<GridTurn> {
    let mut out = NO_GRID_TURN;
    for (r, row) in turn.iter().enumerate() {
        let mut ones = 0;
        for (c, v) in row.iter().enumerate() {
            let near = v.round();
            if (v - near).abs() > 1e-3 || near.abs() > 1.0 {
                return None;
            }
            out[r][c] = near as i32;
            ones += near.abs() as i32;
        }
        if ones != 1 {
            return None;
        }
    }
    Some(out)
}

pub(crate) fn transposed(t: &GridTurn) -> GridTurn {
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| t[j][i]))
}

/// The cell in front of a rail block's arrow side, where a docker rests.
pub(crate) fn in_front(rail: &crate::dock::Piece) -> Pos {
    let (up, _) = crate::shape::turned_axes(rail.orientation);
    rail.pos.offset(up[0], up[1], up[2])
}

/// The path of the parent of the entity at `path`, and the folder the
/// entity is in; None for the root.
pub(crate) fn parent_of(path: &str) -> Option<(&str, &str)> {
    path.rsplit_once('/').or(if path.is_empty() { None } else { Some(("", path)) })
}

/// The docks that go with the core of the entity at `path` when the dock
/// follows it: the dock the entity hangs from a turret axis by, and the
/// docks of the entities that hang from turret axes of this one (by path).
/// Nothing for an entity that does not hang from a turret axis itself: it
/// is not a turret, and its own turret axes stay where they are.
pub fn turret_docks(bp: &Blueprint, path: &str) -> (Option<Dock>, Vec<(String, Dock)>) {
    let docks = bp.docks();
    let own = docks.get(path).filter(|d| d.rail.ty == TURRET_AXIS).cloned();
    if own.is_none() {
        return (None, Vec::new());
    }
    let children = docks
        .iter()
        .filter(|(p, d)| parent_of(p).is_some_and(|(parent, _)| parent == path) && d.rail.ty == TURRET_AXIS)
        .map(|(p, d)| (p.clone(), d.clone()))
        .collect();
    (own, children)
}

/// The cells of the entity at `path` that no swap may displace, and what is
/// there: its core, the docker it hangs by, and every rail a docked entity
/// hangs from.
pub(crate) fn pinned(bp: &Blueprint, docks: &BTreeMap<String, Dock>, path: &str) -> BTreeMap<Pos, String> {
    let mut out = BTreeMap::new();
    if bp.entity(path).is_some_and(|e| e.entity.get(Pos::new(0, 0, 0)).ty() == CORE_TYPE) {
        out.insert(Pos::new(0, 0, 0), "the ship core".to_string());
    }
    if let Some(d) = docks.get(path) {
        out.insert(d.docker.pos, "the rail docker it hangs by".to_string());
    }
    for (child, d) in docks {
        if parent_of(child).is_some_and(|(parent, _)| parent == path) {
            let rail = if d.rail.ty == TURRET_AXIS { "turret axis" } else { "rail" };
            out.insert(d.rail.pos, format!("the {rail} {child} hangs from"));
        }
    }
    out
}

/// Where the cells of an entity go when the blocks in `moving` shift by
/// `by`: each moving block takes the cell `by` away, and a block that was
/// there goes back to the cell the block that took its place came from.
/// Where that cell is taken again in the world (`retaken`: by a block of
/// this column or another entity's, as when the column shifts along its own
/// line) the displaced block is dropped, as the game drops what a turret
/// axis is placed over; those cells make up the second result. Refuses to
/// displace a cell in `pinned` (the blocks the design's docks rest on),
/// naming what is there.
fn shuffled(moving: &BTreeSet<Pos>, by: Pos, pinned: &BTreeMap<Pos, String>, which: &str, retaken: &dyn Fn(Pos) -> bool) -> Result<(BTreeMap<Pos, Pos>, BTreeSet<Pos>)> {
    let shift = |p: Pos| p.offset(by.x, by.y, by.z);
    let mut map: BTreeMap<Pos, Pos> = moving.iter().map(|&c| (c, shift(c))).collect();
    let mut dropped = BTreeSet::new();
    for x in moving.iter().map(|&c| shift(c)).filter(|x| !moving.contains(x)) {
        if let Some(what) = pinned.get(&x) {
            return Err(refuse(format!("the dock cannot move that way: it would displace {what} in {which}")));
        }
        let back = Pos::new(x.x - by.x, x.y - by.y, x.z - by.z);
        if retaken(back) {
            dropped.insert(x);
        }
        map.insert(x, back);
    }
    Ok((map, dropped))
}

/// The shift of the rail block the entity at `child` hangs from, in the
/// coordinates of the entity that holds it, for a cell `to` chosen in the
/// coordinates of the entity at `entity`: the child itself (then `to` is
/// where the child's rail docker is to be, the axis going in front of it on
/// the parent) or the parent (then `to` is the axis's own new cell).
pub fn rail_shift(bp: &Blueprint, entity: &str, child: &str, to: Pos) -> Result<Pos> {
    let docks = bp.docks();
    let dock = docks.get(child).ok_or_else(|| refuse(format!("{} is not docked", called(child))))?;
    if entity == child {
        let turn = grid(&dock.at_rest().turn).ok_or_else(|| refuse(format!("the dock of {} is not square to its parent", called(child))))?;
        let [x, y, z] = turn_step(&turn, [to.x - dock.docker.pos.x, to.y - dock.docker.pos.y, to.z - dock.docker.pos.z]);
        Ok(Pos::new(x, y, z))
    } else if parent_of(child).is_some_and(|(parent, _)| parent == entity) {
        Ok(Pos::new(to.x - dock.rail.pos.x, to.y - dock.rail.pos.y, to.z - dock.rail.pos.z))
    } else {
        Err(refuse(format!("{} does not hang from {}", called(child), called(entity))))
    }
}

/// Moves the rail block the entity at `child` hangs from (a turret axis or
/// a rotator: one block, unlike a run of rail) by `by`, in its parent's
/// coordinates, with the cell in front of it where the child's docker
/// rests. With `carried`, the child comes along, docked as it was, so it
/// shifts by as much in the world; without, the child keeps its place and
/// its rail docker moves by as much within it to meet the axis. Blocks in
/// the way take the cells left, or are replaced where those are taken
/// again, as for [`relocate`]; a core or a block another dock rests on is
/// never displaced.
pub fn move_rail(bp: &mut Blueprint, child: &str, by: Pos, carried: bool, aid: &dyn Aid) -> Result<Relocated> {
    let refuse = |detail: String| Error::Unsupported { what: "moving the rail", detail };
    let docks = bp.docks();
    let dock = docks.get(child).ok_or_else(|| refuse(format!("{} is not docked", called(child))))?.clone();
    let (parent_path, _) = parent_of(child).ok_or_else(|| refuse("the main entity hangs from nothing".to_string()))?;
    let rail_name = if dock.rail.ty == TURRET_AXIS {
        "turret axis"
    } else if ROTATORS.contains(&dock.rail.ty) {
        "rotator"
    } else {
        return Err(refuse(format!("{} hangs from a run of rail, which is moved block by block, not a turret axis or a rotator", called(child))));
    };
    if by == Pos::new(0, 0, 0) {
        return Err(refuse("the rail is there already".to_string()));
    }
    let parent = bp.entity(parent_path).ok_or_else(|| refuse("its parent is missing".to_string()))?;
    if parent.logic_unreadable {
        return Err(refuse(format!("the links of {} are in a file this editor cannot read", called(parent_path))));
    }
    let mut shifts: Vec<(String, BTreeSet<Pos>, Pos)> = vec![(parent_path.to_string(), [dock.rail.pos, in_front(&dock.rail)].into(), by)];
    if !carried {
        let rest = grid(&dock.at_rest().turn).ok_or_else(|| refuse(format!("the dock of {} is not square to its parent", called(child))))?;
        let [x, y, z] = turn_step(&transposed(&rest), by.to_array());
        shifts.push((child.to_string(), [dock.docker.pos].into(), Pos::new(x, y, z)));
    }
    for (entity, cells, by) in &shifts {
        for c in cells {
            let there = c.offset(by.x, by.y, by.z);
            if there.x.abs().max(there.y.abs()).max(there.z.abs()) * 2 > LIMIT {
                return Err(refuse(format!("the rail would go too far from the core of {}", called(entity))));
            }
        }
    }
    let at_rest: BTreeMap<String, crate::dock::Placement> = bp.layout(|_, d| d.at_rest()).into_iter().collect();
    let world_of = |entity: &str, p: Pos| at_rest.get(entity).and_then(|placement| placement.cell(p));
    let column: BTreeSet<Pos> = shifts.iter().flat_map(|(entity, cells, _)| cells.iter().filter_map(|c| world_of(entity, *c))).collect();
    let retaken = |entity: &str, by: Pos, v: Pos| world_of(entity, Pos::new(v.x - by.x, v.y - by.y, v.z - by.z)).is_some_and(|w| column.contains(&w));
    let mut plan = Plan::default();
    let same = |_: u16, o: u8| o;
    let mut replaced = 0;
    for (entity, cells, by) in &shifts {
        let (moved, dropped) = shuffled(cells, *by, &pinned(bp, &docks, entity), &called(entity), &|v| retaken(entity, *by, v))?;
        let e = bp.entity(entity).ok_or_else(|| refuse(format!("there is no entity {entity:?}")))?;
        let data_moves = moved.keys().filter(|p| !cells.contains(p)).map(|p| e.entity.get(*p)).any(|b| !b.is_air() && aid.keeps_data(b.ty()));
        replaced += dropped.iter().filter(|p| !e.entity.get(**p).is_air()).count();
        let g = |p: Pos| moved.get(&p).copied().unwrap_or(p);
        plan.remap(bp, entity, &g, &NO_GRID_TURN, &same, data_moves, &|p| dropped.contains(&p))?;
    }
    let there = dock.rail.pos.offset(by.x, by.y, by.z);
    let mut notes = vec![format!(
        "The {rail_name} {child} hangs from moved from ({}, {}, {}) to ({}, {}, {}) in {}; {}",
        dock.rail.pos.x,
        dock.rail.pos.y,
        dock.rail.pos.z,
        there.x,
        there.y,
        there.z,
        called(parent_path),
        if carried { format!("{child} came along with it, docked as it was.") } else { format!("{child} stays where it is, its rail docker moved to meet the {rail_name}.") }
    )];
    if replaced > 0 {
        notes.push(format!("{replaced} {} where the moving blocks went {} replaced.", if replaced == 1 { "block" } else { "blocks" }, if replaced == 1 { "was" } else { "were" }));
    }
    notes.extend(plan.commit(bp));
    Ok(Relocated { notes })
}

/// The changes a relocation makes, worked out on copies: every meta file
/// touched, by path, and every entity's blocks and links anew.
#[derive(Default)]
struct Plan {
    metas: BTreeMap<String, MetaFile>,
    entities: BTreeMap<String, Entity>,
    notes: Vec<String>,
}

impl Plan {
    /// The meta file of the entity at `path`, as changed so far. Strictly,
    /// it must be one this can change (see [`read_meta`]); otherwise any
    /// that can be parsed, old files without wireless logic left out.
    fn meta(&mut self, bp: &Blueprint, path: &str, strict: bool) -> Result<Option<MetaFile>> {
        if let Some(meta) = self.metas.remove(path) {
            return Ok(Some(meta));
        }
        let e = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
        if strict {
            return read_meta(e, &called(path));
        }
        if e.meta.is_empty() || crate::meta::version(&e.meta).is_ok_and(|v| v < 2) {
            return Ok(None);
        }
        MetaFile::parse(&e.meta).map(Some).map_err(|err| refuse(format!("the meta file of {} cannot be read ({err})", called(path))))
    }

    fn keep(&mut self, path: &str, meta: MetaFile) {
        self.metas.insert(path.to_string(), meta);
    }

    /// Takes the cells of the entity at `path` through `f` (a bijection of
    /// positions) and turns its blocks by `turn`: its blocks and links, the
    /// positions its meta file keeps of its own blocks, the rails its docked
    /// entities hang from, its own docker in its parent's record, and the
    /// wireless links other entities have to it. `data_moves` says the
    /// manager tag, kept by position, no longer fits and is dropped.
    /// Returns how many docked entities followed.
    #[allow(clippy::too_many_arguments)]
    fn remap(&mut self, bp: &Blueprint, path: &str, f: &dyn Fn(Pos) -> Pos, turn: &GridTurn, orient: &dyn Fn(u16, u8) -> u8, data_moves: bool, gone: &dyn Fn(Pos) -> bool) -> Result<usize> {
        let which = called(path);
        let turning = *turn != NO_GRID_TURN;
        let e = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
        if e.logic_unreadable {
            return Err(refuse(format!("the links of {which} are in a file this editor cannot read")));
        }
        let mut records = 0;
        let mut name = None;
        if let Some(mut meta) = self.meta(bp, path, true)? {
            let version = meta.version;
            meta.map_own_blocks(f);
            if turning {
                meta.turn_own_dockers(orient);
            }
            if let Some(rails) = meta.rails_mut() {
                for (child, record) in rails.children.iter_mut().filter(|(_, r)| !r.is_empty()) {
                    let mut parsed = DockRecord::parse(record).map_err(|err| refuse(format!("the dock record of {child} cannot be read ({err})")))?;
                    parsed.map_rail(version, f);
                    if turning {
                        parsed.turn_rail(turn, orient).map_err(|err| refuse(format!("the dock record of {child} cannot be turned ({err})")))?;
                    }
                    *record = parsed.write();
                    records += 1;
                }
            }
            if data_moves && meta.has_manager_tag() {
                self.notes.push(if path.is_empty() {
                    "Storage contents, display text and other block settings stored by position were reset.".to_string()
                } else {
                    format!("Storage contents, display text and other block settings stored by position were reset on {which}.")
                });
                meta.tail = Tail::Finish;
            }
            name = meta.rails().map(|r| r.name.clone());
            self.keep(path, meta);
        }

        // This entity's record in its parent's meta file.
        if let Some((parent_path, dir)) = parent_of(path) {
            if let Some(mut meta) = self.meta(bp, parent_path, true)? {
                let version = meta.version;
                let record = meta.rails_mut().and_then(|r| r.children.iter_mut().find(|(c, _)| c.rsplit('/').next() == Some(dir) && !c.is_empty()));
                if let Some((child, record)) = record.filter(|(_, r)| !r.is_empty()) {
                    let mut parsed = DockRecord::parse(record).map_err(|err| refuse(format!("the dock record of {child} cannot be read ({err})")))?;
                    parsed.map_docker(version, f);
                    if turning {
                        if Dock::read(record, version).ok().flatten().is_some_and(|d| d.docker.ty == CORE_TYPE) {
                            return Err(refuse(format!("{which} is docked by its core, which cannot turn on its rail")));
                        }
                        parsed.turn_docker(turn, orient).map_err(|err| refuse(format!("the dock record of {child} cannot be turned ({err})")))?;
                    }
                    *record = parsed.write();
                }
                self.keep(parent_path, meta);
            }
        }

        // Wireless links other entities have to this one, by the name it goes by.
        let mut named_by_others = 0;
        let mut twin = false;
        if let Some(name) = &name {
            for (other_path, _) in bp.entities() {
                if other_path == path {
                    continue;
                }
                let Some(mut meta) = self.meta(bp, &other_path, false)? else { continue };
                twin |= meta.rails().is_some_and(|r| r.name == *name);
                named_by_others += meta.map_wireless_to(name, f);
                self.keep(&other_path, meta);
            }
            let own_refs = self.metas.get(path).and_then(|m| m.rails()).map_or(0, |r| r.wireless.iter().filter(|p| p.entity == *name).count());
            if twin && named_by_others + own_refs > 0 {
                return Err(refuse(format!("wireless links name {which} as {name:?}, a name another entity of the design shares")));
            }
        }
        if named_by_others > 0 {
            self.notes.push(format!("{named_by_others} wireless {} on other entities followed the blocks of {which}.", if named_by_others == 1 { "link" } else { "links" }));
        }

        let source = self.entities.get(path).unwrap_or(&e.entity);
        let entity = translated(source, f, orient, gone);
        self.entities.insert(path.to_string(), entity);
        Ok(records)
    }

    /// Writes everything into the blueprint. Nothing here can fail.
    fn commit(self, bp: &mut Blueprint) -> Vec<String> {
        for (path, entity) in self.entities {
            let e = bp.entity_mut(&path).expect("checked when planned");
            e.entity = entity;
            // The links file is written from the entity, not from the copy that was read.
            e.logic_raw = None;
            e.header.refresh(&e.entity);
        }
        for (path, meta) in self.metas {
            bp.entity_mut(&path).expect("checked when planned").meta = meta.write();
        }
        self.notes
    }
}

/// Moves the core of the entity at `path` as `mv` says: to another cell (see
/// [`relocate_core`]), turned to face another way (see [`relocate_and_turn`]),
/// or both at once.
///
/// With `dock_follows`, on a turret, the dock goes with the core: the rail
/// docker the entity hangs by and the turret axis it rests on (a block of
/// the parent), and every turret axis of this entity that another entity
/// hangs from, with that entity's rail docker, all shift by as much as the
/// core does, each within its own entity. The turret keeps its place, and
/// so does everything docked to it; only the line it turns about moves. The
/// blocks that were in the way take the cells the moving blocks left, as
/// the block in the core's new cell takes the core's old one. It is refused
/// when a block that another dock rests on, or a core, would be displaced.
/// An entity that hangs from a plain rail or a rotator, or has such docks,
/// is moved as without it: those docks stay where they are.
pub fn relocate(bp: &mut Blueprint, path: &str, mv: Move, aid: &dyn Aid) -> Result<Relocated> {
    let origin = Pos::new(0, 0, 0);
    let which = called(path);
    let to = mv.to;
    let turn = match mv.facing {
        Some((forward, up)) => grid_turn_to(forward, up)?,
        None => NO_GRID_TURN,
    };
    let turning = turn != NO_GRID_TURN;
    let orient = |ty: u16, o: u8| if turning { aid.turned(ty, o, &turn) } else { o };
    let e = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
    if e.entity.get(origin).ty() != CORE_TYPE {
        return Err(refuse(format!("{which} has no ship core at its origin")));
    }
    if to == origin && !turning {
        return Err(refuse("the core is already there"));
    }
    if e.logic_unreadable {
        return Err(refuse(format!("the links of {which} are in a file this editor cannot read")));
    }
    let mut far = to.x.abs().max(to.y.abs()).max(to.z.abs());
    e.entity.for_each(|p, _| far = far.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
    if far * 2 > LIMIT {
        return Err(refuse("the design would reach too far from the new core"));
    }

    let (shuffles, stayed) = shuffles(bp, path, to, mv.dock_follows)?;
    let own = &shuffles[0];
    let (moving, moved, dropped) = (&own.cells, &own.moved, &own.dropped);
    // Where a cell of the design goes: the moving blocks shift and the
    // blocks in their way take their old cells, then everything moves so
    // that the core is at the origin, and turns round it.
    let f = |p: Pos| -> Pos {
        let q = moved.get(&p).copied().unwrap_or(p);
        let [x, y, z] = turn_step(&turn, [q.x - to.x, q.y - to.y, q.z - to.z]);
        Pos::new(x, y, z)
    };
    let block_there = e.entity.get(to);
    let mut notes = vec![if to == origin {
        "The core stays where it is.".to_string()
    } else if block_there.is_air() {
        format!("The core now sits at the cell that was ({}, {}, {}), which was empty; its old cell is empty now.", to.x, to.y, to.z)
    } else if dropped.contains(&to) {
        format!("The core now sits at the cell that was ({}, {}, {}), in place of the block there.", to.x, to.y, to.z)
    } else if moving.contains(&to) {
        format!("The core now sits at the cell that was ({}, {}, {}), a cell of the dock, which moved on by as much.", to.x, to.y, to.z)
    } else {
        format!("The core now sits at the cell that was ({}, {}, {}); the block that was there took the old core's place.", to.x, to.y, to.z)
    }];
    let mut replaced = dropped.iter().filter(|p| !e.entity.get(**p).is_air()).count();

    let mut plan = Plan::default();
    let records = plan.remap(bp, path, &f, &turn, &orient, true, &|p| dropped.contains(&p))?;
    let same = |_: u16, o: u8| o;

    // The parent's turret axis and the cell its docker rests in, and each
    // docked entity's rail docker, by as much in their own coordinates.
    let mut followed = Vec::new();
    for s in shuffles.iter().skip(1) {
        let entity = s.entity.as_str();
        let e = bp.entity(entity).ok_or_else(|| refuse(format!("there is no entity {entity:?}")))?;
        // Rails and dockers keep nothing by position; the blocks they displace may.
        let data_moves = s.moved.keys().filter(|p| !s.cells.contains(p)).map(|p| e.entity.get(*p)).any(|b| !b.is_air() && aid.keeps_data(b.ty()));
        replaced += s.dropped.iter().filter(|p| !e.entity.get(**p).is_air()).count();
        let g = |p: Pos| s.moved.get(&p).copied().unwrap_or(p);
        plan.remap(bp, entity, &g, &NO_GRID_TURN, &same, data_moves, &|p| s.dropped.contains(&p))?;
        followed.push(if s.cells.len() > 1 {
            format!("the rail docker and the turret axis it rests on in {}", called(entity))
        } else {
            format!("the turret axis {entity} hangs from, with that entity's rail docker")
        });
    }

    if !followed.is_empty() {
        notes.push(format!(
            "The dock followed the core: {}. The turret and what is docked to it stay where they are; the blocks that were in the way took the cells the moving ones left.",
            followed.join(", and ")
        ));
    }
    for child in &stayed {
        notes.push(format!("The turret axis {child} hangs from stayed where it is: moved with the core it would have touched nothing. {child} keeps its dock; Move rail can put the axis elsewhere."));
    }
    if replaced > 0 {
        notes.push(format!("{replaced} {} where the moving blocks went {} replaced.", if replaced == 1 { "block" } else { "blocks" }, if replaced == 1 { "was" } else { "were" }));
    }
    if records > 0 {
        notes.push(format!("{records} docked {} followed the core.", if records == 1 { "entity" } else { "entities" }));
    }
    if turning {
        notes.push("The entity turned with the core, so that the core faces the way chosen.".to_string());
    }
    notes.extend(plan.commit(bp));
    Ok(Relocated { notes })
}

/// One entity's part in a core move: the cells of it that move (`cells`),
/// where every cell touched goes (`moved`: the moving cells and the ones
/// they displace, in the entity's own coordinates), and the displaced cells
/// whose blocks are dropped (see [`shuffled`]).
pub(crate) struct Shuffle {
    pub entity: String,
    pub cells: BTreeSet<Pos>,
    pub moved: BTreeMap<Pos, Pos>,
    pub dropped: BTreeSet<Pos>,
}

/// What moves when the core of the entity at `path` goes to `to`, the dock
/// following if `dock_follows` (see [`relocate`]): the entity's own shuffle
/// first, then its parent's and each docked entity's whose dock follows.
/// With them, the docked entities whose turret axis stays where it is.
/// Refuses what [`shuffled`] refuses, and a dock that would go too far.
pub(crate) fn shuffles(bp: &Blueprint, path: &str, to: Pos, dock_follows: bool) -> Result<(Vec<Shuffle>, Vec<String>)> {
    let origin = Pos::new(0, 0, 0);
    let which = called(path);
    let e = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
    // Which cells of this entity go with the core: the docker, and each
    // turret axis a child hangs from with the cell in front of it, where
    // that child's docker rests (empty here, and it must stay so).
    let docks = bp.docks();
    let (own_dock, mut children) = if dock_follows && to != origin { turret_docks(bp, path) } else { (None, Vec::new()) };
    let following = own_dock.is_some();
    let mut moving: BTreeSet<Pos> = [origin].into();
    if let Some(d) = &own_dock {
        moving.insert(d.docker.pos);
    }
    for (_, d) in &children {
        moving.insert(d.rail.pos);
        moving.insert(in_front(&d.rail));
    }
    // An axis that would touch nothing of the entity once moved (it stood
    // on a part of the body the core's shift leaves behind) stays where it
    // is, with what hangs from it; a floating axis is no use to anyone.
    let occupied_after = |q: Pos| {
        let from = Pos::new(q.x - to.x, q.y - to.y, q.z - to.z);
        if moving.contains(&from) {
            !e.entity.get(from).is_air()
        } else if moving.contains(&q) {
            !e.entity.get(q.offset(to.x, to.y, to.z)).is_air()
        } else {
            !e.entity.get(q).is_air()
        }
    };
    let mut stayed = Vec::new();
    children.retain(|(child, d)| {
        let there = d.rail.pos.offset(to.x, to.y, to.z);
        let touches = crate::shape::SIDE_DIRS.iter().any(|n| occupied_after(there.offset(n[0], n[1], n[2])));
        if !touches {
            stayed.push(child.clone());
        }
        touches
    });
    for child in &stayed {
        if let Some(d) = docks.get(child) {
            moving.remove(&d.rail.pos);
            moving.remove(&in_front(&d.rail));
        }
    }
    // The shift of every moving block in each entity's own coordinates (the
    // docks at rest are square to one another), and all their cells in the
    // main entity's coordinates, to tell which cells the column takes again.
    let mut shifts: Vec<(String, BTreeSet<Pos>, Pos)> = vec![(path.to_string(), moving.clone(), to)];
    if let (Some(dock), Some((parent_path, _))) = (&own_dock, parent_of(path)) {
        let rest = grid(&dock.at_rest().turn).ok_or_else(|| refuse(format!("the dock of {which} is not square to its parent")))?;
        let [x, y, z] = turn_step(&rest, to.to_array());
        shifts.push((parent_path.to_string(), [dock.rail.pos, in_front(&dock.rail)].into(), Pos::new(x, y, z)));
    }
    for (child_path, dock) in &children {
        let rest = grid(&dock.at_rest().turn).ok_or_else(|| refuse(format!("the dock of {child_path} is not square to {which}")))?;
        let [x, y, z] = turn_step(&transposed(&rest), to.to_array());
        shifts.push((child_path.clone(), [dock.docker.pos].into(), Pos::new(x, y, z)));
    }
    let at_rest: BTreeMap<String, crate::dock::Placement> = if following { bp.layout(|_, d| d.at_rest()).into_iter().collect() } else { BTreeMap::new() };
    let world_of = |entity: &str, p: Pos| at_rest.get(entity).and_then(|placement| placement.cell(p));
    let column: BTreeSet<Pos> = shifts.iter().flat_map(|(entity, cells, _)| cells.iter().filter_map(|c| world_of(entity, *c))).collect();
    for (entity, cells, by) in &shifts {
        for c in cells {
            let there = c.offset(by.x, by.y, by.z);
            if there.x.abs().max(there.y.abs()).max(there.z.abs()) * 2 > LIMIT {
                return Err(refuse(format!("the dock would go too far from the core of {}", called(entity))));
            }
        }
    }
    // Whether the world cell of `v` (a cell of `entity`) is taken again once the column has shifted by `by`.
    let retaken = |entity: &str, by: Pos, v: Pos| world_of(entity, Pos::new(v.x - by.x, v.y - by.y, v.z - by.z)).is_some_and(|w| column.contains(&w));
    let mut out = Vec::with_capacity(shifts.len());
    for (entity, cells, by) in shifts {
        let pins = if following { pinned(bp, &docks, &entity) } else { BTreeMap::new() };
        let (moved, dropped) = shuffled(&cells, by, &pins, &called(&entity), &|v| retaken(&entity, by, v))?;
        out.push(Shuffle { entity, cells, moved, dropped });
    }
    Ok((out, stayed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::TURRET_AXIS;
    use crate::metafile::{self, Section};
    use crate::turret::fixtures::*;
    use crate::{turret, Block};

    const RAIL_DOCKER: u16 = 663;

    fn snapshot(bp: &Blueprint) -> crate::blueprint::FileTree {
        bp.to_files().unwrap()
    }

    fn links_of(e: &Entity) -> Vec<(Pos, u16, Pos)> {
        e.links().iter().flat_map(|(c, g)| g.iter().flat_map(move |(ty, ts)| ts.iter().map(move |t| (*c, *ty, *t)))).collect()
    }

    /// A ship with a hull block at (1, 0, 0), another at (0, 2, 0) and a computer
    /// at (0, 1, 0) the core runs, which runs a barrel at (2, 0, 0).
    fn small() -> Blueprint {
        let mut bp = ship(&[]);
        let e = &mut bp.root.entity;
        e.set(Pos::new(1, 0, 0), Block::new(HULL, 3, false));
        e.set(Pos::new(0, 2, 0), Block::new(PLATE, 0, false));
        e.set(Pos::new(0, 1, 0), Block::new(COMPUTER, 0, false));
        e.set(Pos::new(2, 0, 0), Block::new(BARREL, 0, false));
        e.link(Pos::new(0, 0, 0), Pos::new(0, 1, 0), COMPUTER);
        e.link(Pos::new(0, 1, 0), Pos::new(2, 0, 0), BARREL);
        e.mark_unchanged();
        bp
    }

    #[test]
    fn the_core_and_the_block_at_the_target_swap_and_everything_moves_with_them() {
        let mut bp = small();
        let hull = bp.root.entity.get(Pos::new(1, 0, 0));
        let r = relocate_core(&mut bp, "", Pos::new(1, 0, 0)).unwrap();
        assert!(r.notes[0].contains("(1, 0, 0)"), "{:?}", r.notes);
        let e = &bp.root.entity;
        // The core is at the origin; the hull block that was at (1,0,0) took its place at (-1,0,0), turned the way it was.
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert_eq!(e.get(Pos::new(-1, 0, 0)), hull);
        assert_eq!(e.get(Pos::new(-1, 1, 0)).ty(), COMPUTER);
        assert_eq!(e.get(Pos::new(-1, 2, 0)).ty(), PLATE);
        assert_eq!(e.get(Pos::new(1, 0, 0)).ty(), BARREL);
        assert_eq!(e.block_count(), 5);
        let mut links = links_of(e);
        links.sort();
        // The core's link, keyed at the old core cell, is keyed at the origin still; the others moved.
        assert_eq!(links, vec![(Pos::new(-1, 1, 0), BARREL, Pos::new(1, 0, 0)), (Pos::new(0, 0, 0), COMPUTER, Pos::new(-1, 1, 0))]);
        assert_eq!(bp.root.header.bounds, [-2.0, -1.0, -1.0, 3.0, 4.0, 2.0]);
        assert_eq!(bp.root.header.counts.iter().map(|c| c.1).sum::<i32>(), 5);
        // Written out, the links file is the changed one.
        let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
        assert_eq!(again.root.entity.get(Pos::new(-1, 0, 0)), hull);
        assert_eq!(links_of(&again.root.entity).len(), 2);
        assert_eq!(links_of(&again.root.entity).into_iter().filter(|l| l.0 == Pos::new(-1, 1, 0)).count(), 1);
        // And moving it back restores the design.
        relocate_core(&mut bp, "", Pos::new(-1, 0, 0)).unwrap();
        let back = small();
        for p in [(0, 0, 0), (1, 0, 0), (0, 2, 0), (0, 1, 0), (2, 0, 0), (-1, 0, 0)] {
            assert_eq!(bp.root.entity.get(Pos::new(p.0, p.1, p.2)), back.root.entity.get(Pos::new(p.0, p.1, p.2)));
        }
        assert_eq!(links_of(&bp.root.entity), links_of(&back.root.entity));
    }

    #[test]
    fn turning_the_core_turns_the_entity_round_it_with_its_links() {
        let catalog = catalog();
        let mut bp = small();
        let want = snapshot(&bp);
        assert!(relocate_and_turn(&mut bp, &catalog, "", Pos::new(0, 0, 0), [0, 0, 1], [0, 0, 1]).is_err(), "up along forward");
        assert!(relocate_and_turn(&mut bp, &catalog, "", Pos::new(0, 0, 0), [0, 0, 2], [0, 1, 0]).is_err(), "not a step");
        assert_eq!(snapshot(&bp), want);
        // What points +X now becomes the nose: the turn takes +X to +Z and +Z to -X.
        let r = relocate_and_turn(&mut bp, &catalog, "", Pos::new(0, 0, 0), [1, 0, 0], [0, 1, 0]).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("turned")), "{:?}", r.notes);
        let e = &bp.root.entity;
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert_eq!(e.get(Pos::new(0, 0, 1)).ty(), HULL);
        assert_eq!(e.get(Pos::new(0, 0, 2)).ty(), BARREL);
        assert_eq!(e.get(Pos::new(0, 2, 0)).ty(), PLATE, "up stays up");
        let mut links = links_of(e);
        links.sort();
        assert_eq!(links, vec![(Pos::new(0, 0, 0), COMPUTER, Pos::new(0, 1, 0)), (Pos::new(0, 1, 0), BARREL, Pos::new(0, 0, 2))]);
        // Moved and turned at once, and back the same way.
        relocate_and_turn(&mut bp, &catalog, "", Pos::new(0, 0, 1), [0, 1, 0], [0, 0, -1]).unwrap();
        assert_eq!(bp.root.entity.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert_eq!(bp.root.entity.block_count(), 5);
    }

    #[test]
    fn an_empty_target_leaves_an_empty_cell_where_the_core_was() {
        let mut bp = small();
        relocate_core(&mut bp, "", Pos::new(0, 3, 0)).unwrap();
        let e = &bp.root.entity;
        assert_eq!(e.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert!(e.get(Pos::new(0, -3, 0)).is_air(), "the old core cell is empty");
        assert_eq!(e.get(Pos::new(1, -3, 0)).ty(), HULL);
        assert_eq!(e.block_count(), 5);
    }

    #[test]
    fn it_refuses_what_it_cannot_do_and_changes_nothing() {
        let mut bp = small();
        let want = snapshot(&bp);
        assert!(relocate_core(&mut bp, "", Pos::new(0, 0, 0)).is_err(), "already there");
        assert!(relocate_core(&mut bp, "ATTACHED_0", Pos::new(1, 0, 0)).is_err(), "no such entity");
        assert!(relocate_core(&mut bp, "", Pos::new(40_000, 0, 0)).is_err(), "too far for the files");
        bp.root.entity.set(Pos::new(0, 0, 0), Block::AIR);
        assert!(relocate_core(&mut bp, "", Pos::new(1, 0, 0)).is_err(), "no core");
        bp.root.entity.set(Pos::new(0, 0, 0), Block::new(CORE_TYPE, 0, false));
        bp.root.entity.mark_unchanged();
        assert_eq!(snapshot(&bp), want);
        // An old meta file, one with docks of the old kind, a links file that could not be read.
        let meta = bp.root.meta.clone();
        bp.root.meta = vec![0, 0, 0, 3, 1];
        assert!(relocate_core(&mut bp, "", Pos::new(1, 0, 0)).is_err());
        bp.root.meta = MetaFile { version: 5, sections: vec![Section::Legacy(vec![("Test/ATTACHED_0".into(), [0; 27])])], tail: Tail::Finish }.write();
        assert!(relocate_core(&mut bp, "", Pos::new(1, 0, 0)).is_err());
        bp.root.meta = vec![0, 0, 0, 5, 9];
        assert!(relocate_core(&mut bp, "", Pos::new(1, 0, 0)).is_err());
        bp.root.meta = meta;
        bp.root.logic_unreadable = true;
        assert!(relocate_core(&mut bp, "", Pos::new(1, 0, 0)).is_err());
        bp.root.logic_unreadable = false;
        assert_eq!(snapshot(&bp), want);
        // A docked entity whose record cannot be read: refused whichever way it is reached.
        let catalog = catalog();
        let (mut bp, min, max) = {
            let mut bp = ship(&[(Pos::new(9, 9, 9), Pos::new(13, 13, 13))]);
            for (x, y, z) in (0..27).map(|i| (10 + i % 3, 10 + i / 3 % 3, 10 + i / 9)) {
                bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 0, false));
            }
            (bp, Pos::new(10, 10, 10), Pos::new(12, 12, 12))
        };
        turret::make_turret(&mut bp, &catalog, "", min, max, 3).unwrap();
        let good = bp.root.meta.clone();
        let mut meta = MetaFile::parse(&good).unwrap();
        meta.rails_mut().unwrap().children[0].1 = vec![0, 0, 13, 0, 0];
        bp.root.meta = meta.write();
        let want = snapshot(&bp);
        assert!(relocate_core(&mut bp, "", Pos::new(9, 9, 9)).is_err(), "the record of the child is damaged");
        assert!(relocate_core(&mut bp, "ATTACHED_0", Pos::new(1, 0, 0)).is_err(), "so is the parent's record of this entity");
        assert_eq!(snapshot(&bp), want);
    }

    /// A ship with a turret cut from it, and a wireless module on each, linked both ways.
    fn with_turret() -> (Blueprint, Pos, Pos) {
        let catalog = catalog();
        let mut bp = ship(&[(Pos::new(9, 9, 9), Pos::new(13, 13, 13))]);
        for (x, y, z) in (0..27).map(|i| (10 + i % 3, 10 + i / 3 % 3, 10 + i / 9)) {
            bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 5, false));
        }
        let (parent_module, child_module) = (Pos::new(9, 9, 10), Pos::new(10, 11, 11));
        bp.root.entity.set(parent_module, Block::new(WIRELESS, 0, false));
        bp.root.entity.set(child_module, Block::new(WIRELESS, 0, false));
        let made = turret::make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(12, 12, 12), 3).unwrap();
        assert_eq!(made.child, "ATTACHED_0");
        // The module in the box is at (-1, 0, 0) in the child's own coordinates.
        let child_module = Pos::new(-1, 0, 0);
        assert_eq!(bp.entity("ATTACHED_0").unwrap().entity.get(child_module).ty(), WIRELESS);
        let file = |p: Pos| metafile::pack(p.x + 16, p.y + 16, p.z + 16);
        let mut meta = MetaFile::parse(&bp.root.meta).unwrap();
        meta.rails_mut().unwrap().wireless.push(metafile::WirelessPair { entity: "rl0".into(), to: file(child_module), from: file(parent_module) });
        meta.rails_mut().unwrap().wireless.push(metafile::WirelessPair { entity: String::new(), to: file(parent_module), from: file(parent_module) });
        bp.root.meta = meta.write();
        let mut meta = MetaFile::parse(&bp.entity("ATTACHED_0").unwrap().meta).unwrap();
        meta.rails_mut().unwrap().wireless.push(metafile::WirelessPair { entity: String::new(), to: file(parent_module), from: file(child_module) });
        meta.rails_mut().unwrap().wireless.push(metafile::WirelessPair { entity: "rl0".into(), to: file(child_module), from: file(child_module) });
        bp.entity_mut("ATTACHED_0").unwrap().meta = meta.write();
        (bp, parent_module, child_module)
    }

    /// The wireless pairs of a design as (entity path, block type) ends.
    fn wireless_ends(bp: &Blueprint) -> Vec<((String, u16), (String, u16))> {
        let entities = bp.entities();
        let mut out: Vec<_> = bp
            .wireless()
            .into_iter()
            .map(|((a, p), (b, q))| ((entities[a].0.clone(), entities[a].1.entity.get(p).ty()), (entities[b].0.clone(), entities[b].1.entity.get(q).ty())))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn docked_entities_their_records_and_wireless_links_follow_the_core_of_the_parent() {
        let (mut bp, ..) = with_turret();
        let ends = wireless_ends(&bp);
        assert_eq!(ends.len(), 4);
        assert!(ends.iter().all(|(a, b)| a.1 == WIRELESS && b.1 == WIRELESS));
        let world_before = world(&bp);
        let to = Pos::new(13, 13, 13);
        let r = relocate_core(&mut bp, "", to).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("docked entity followed")), "{:?}", r.notes);
        // All the wireless links still join wireless modules.
        assert_eq!(wireless_ends(&bp), ends);
        // The world moved by the shift and the core swapped with the block at the target.
        let shifted: std::collections::BTreeMap<_, _> = world_before
            .iter()
            .filter(|((e, c), _)| !(e.is_empty() && (*c == Pos::new(0, 0, 0) || *c == to)))
            .map(|((e, c), b)| ((e.clone(), Pos::new(c.x - 13, c.y - 13, c.z - 13)), *b))
            .collect();
        let mut after = world(&bp);
        after.remove(&(String::new(), Pos::new(0, 0, 0)));
        after.remove(&(String::new(), Pos::new(-13, -13, -13)));
        assert_eq!(after, shifted);
        // The dock record is the one the game reads, the rail on the same block.
        let dock = &bp.docks()["ATTACHED_0"];
        assert_eq!(bp.root.entity.get(dock.rail.pos).ty(), TURRET_AXIS);
        assert_eq!(bp.entity("ATTACHED_0").unwrap().entity.get(dock.docker.pos).ty(), RAIL_DOCKER);
        let up = turned_axes_of(dock.rail.orientation);
        assert_eq!(dock.placement().cell(Pos::new(0, 0, 0)), Some(dock.rail.pos.offset(2 * up[0], 2 * up[1], 2 * up[2])), "the child's core is still beyond the docker");
        // The cell the docker must reach moved with the rail.
        let record = crate::meta::dock_records(&bp.root.meta).unwrap().remove(0).1;
        let tag = crate::rawtag::RawTag::parse(&record).unwrap();
        let request = tag.value.members().unwrap()[1].value.members().unwrap()[0].value.members().unwrap();
        let crate::rawtag::Value::Vec3i(target) = request[4].value else { panic!() };
        assert_eq!(target, [dock.rail.pos.x + 16 + up[0], dock.rail.pos.y + 16 + up[1], dock.rail.pos.z + 16 + up[2]]);
    }

    fn turned_axes_of(orientation: u8) -> [i32; 3] {
        crate::shape::turned_axes(orientation).0
    }

    #[test]
    fn a_docked_entity_that_moves_its_core_stays_where_it_is_docked_and_others_find_its_modules() {
        let (mut bp, ..) = with_turret();
        let ends = wireless_ends(&bp);
        let before = world(&bp);
        // The block next to the child's core, away from its docker.
        let to = Pos::new(0, 1, 0);
        assert_ne!(bp.entity("ATTACHED_0").unwrap().entity.get(to).ty(), 0);
        relocate_core(&mut bp, "ATTACHED_0", to).unwrap();
        assert_eq!(wireless_ends(&bp), ends);
        // The world is as it was, but for the two swapped blocks.
        let after = world(&bp);
        let differing: Vec<_> = after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        assert_eq!(differing.len(), 2, "{differing:?}");
        // The child's section 06 and the parent's record of the child name the docker where it is now.
        let child = bp.entity("ATTACHED_0").unwrap();
        let meta = MetaFile::parse(&child.meta).unwrap();
        let Section::Dockers(Some(list)) = &meta.sections[1] else { panic!() };
        let shift = 16;
        let at = Pos::new(i32::from_be_bytes(list[0][0..4].try_into().unwrap()) - shift, i32::from_be_bytes(list[0][4..8].try_into().unwrap()) - shift, i32::from_be_bytes(list[0][8..12].try_into().unwrap()) - shift);
        assert_eq!(child.entity.get(at).ty(), RAIL_DOCKER);
        assert_eq!(bp.docks()["ATTACHED_0"].docker.pos, at);
        assert_eq!(bp.docks()["ATTACHED_0"].rail.pos, Pos::new(11, 9, 11));
    }

    #[test]
    fn what_hangs_from_a_block_that_swaps_places_goes_with_that_block() {
        let (mut bp, ..) = with_turret();
        // The docker block of the child is at (0, -1, 0): swapping it with the core moves the entity in the world.
        let docker = bp.docks()["ATTACHED_0"].docker.pos;
        relocate_core(&mut bp, "ATTACHED_0", docker).unwrap();
        let dock = &bp.docks()["ATTACHED_0"];
        assert_eq!(dock.docker.pos, Pos::new(0, 1, 0), "the docker took the core's place");
        assert_eq!(bp.entity("ATTACHED_0").unwrap().entity.get(dock.docker.pos).ty(), RAIL_DOCKER);
        assert_eq!(dock.placement().cell(dock.docker.pos), Some(Pos::new(11, 10, 11)), "and still rests against the axis");
    }

    /// A ship with a turret base cut from it (ATTACHED_0, 3 wide, 6 tall and
    /// 3 long, docked at the bottom) and a barrel cut from the top of the
    /// base (ATTACHED_0/ATTACHED_0), both on turret axes.
    fn with_turret_and_barrel() -> Blueprint {
        let catalog = catalog();
        let mut bp = ship(&[(Pos::new(8, 8, 8), Pos::new(14, 9, 14))]);
        for x in 10..=12 {
            for y in 10..=15 {
                for z in 10..=12 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 5, false));
                }
            }
        }
        let made = turret::make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(12, 15, 12), 3).unwrap();
        assert_eq!(made.child, "ATTACHED_0");
        // The base's core is at world (11, 11, 11); its cells go from (-1, -1, -1) to (1, 4, 1).
        let barrel = turret::make_turret(&mut bp, &catalog, "ATTACHED_0", Pos::new(-1, 2, -1), Pos::new(1, 4, 1), 3).unwrap();
        assert_eq!(barrel.child, "ATTACHED_0/ATTACHED_0");
        bp
    }

    /// The dock records read back, with the blocks at the cells they name.
    fn docks_hold(bp: &Blueprint) {
        for (path, dock) in bp.docks() {
            let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
            assert_eq!(bp.entity(parent).unwrap().entity.get(dock.rail.pos).ty(), dock.rail.ty, "{path}: the rail block is where its record says");
            assert_eq!(bp.entity(&path).unwrap().entity.get(dock.docker.pos).ty(), dock.docker.ty, "{path}: the docker block is where its record says");
        }
    }

    #[test]
    fn with_the_dock_following_the_turret_keeps_its_place_and_only_its_pivot_moves() {
        let mut bp = with_turret_and_barrel();
        docks_hold(&bp);
        let before = world(&bp);
        let base = bp.docks()["ATTACHED_0"].clone();
        let barrel = bp.docks()["ATTACHED_0/ATTACHED_0"].clone();
        assert_eq!((base.rail.pos, base.docker.pos), (Pos::new(11, 9, 11), Pos::new(0, -1, 0)));
        assert_eq!((barrel.rail.pos, barrel.docker.pos), (Pos::new(0, 1, 0), Pos::new(0, -1, 0)));
        let by = Pos::new(1, 0, -1);
        let r = relocate(&mut bp, "ATTACHED_0", Move { to: by, facing: None, dock_follows: true }, &catalog()).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("The dock followed the core") && n.contains("ATTACHED_0/ATTACHED_0")), "{:?}", r.notes);
        docks_hold(&bp);
        // The axis on the ship, the docker, core and axis of the base and the
        // barrel's docker all moved by as much; in each entity's own
        // coordinates the base's column keeps its cells, since the core is the origin.
        let base_now = &bp.docks()["ATTACHED_0"];
        let barrel_now = &bp.docks()["ATTACHED_0/ATTACHED_0"];
        assert_eq!(base_now.rail.pos, Pos::new(12, 9, 10));
        assert_eq!(base_now.docker.pos, Pos::new(0, -1, 0));
        assert_eq!(barrel_now.rail.pos, Pos::new(0, 1, 0));
        assert_eq!(barrel_now.docker.pos, Pos::new(1, -1, -1));
        assert_eq!(bp.entity("ATTACHED_0/ATTACHED_0").unwrap().entity.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE, "the barrel's core stays");
        // In the world, nothing moved but the five blocks of the column and
        // the five hull blocks that took their cells.
        let after = world(&bp);
        let moved: Vec<_> = before.iter().filter(|(k, v)| after.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        let column = |path: &str, cells: &[Pos]| cells.iter().map(|c| (path.to_string(), *c)).collect::<Vec<_>>();
        // The cells in front of the axes are empty on both sides, so they do not show.
        let mut expect = column("", &[Pos::new(11, 9, 11), Pos::new(12, 9, 10)]);
        expect.extend(column("ATTACHED_0", &[Pos::new(11, 10, 11), Pos::new(12, 10, 10), Pos::new(11, 11, 11), Pos::new(12, 11, 10), Pos::new(11, 12, 11), Pos::new(12, 12, 10)]));
        expect.extend(column("ATTACHED_0/ATTACHED_0", &[Pos::new(11, 13, 11), Pos::new(12, 13, 10)]));
        expect.sort();
        assert_eq!(moved, expect);
        assert_eq!(after.len(), before.len());
        // Each column block is where the shift put it, and the hull it displaced is where it was.
        assert_eq!(after[&(String::new(), Pos::new(12, 9, 10))], before[&(String::new(), Pos::new(11, 9, 11))]);
        assert_eq!(after[&(String::new(), Pos::new(11, 9, 11))], before[&(String::new(), Pos::new(12, 9, 10))]);
        assert_eq!(after[&("ATTACHED_0/ATTACHED_0".to_string(), Pos::new(12, 13, 10))], before[&("ATTACHED_0/ATTACHED_0".to_string(), Pos::new(11, 13, 11))]);
        // The barrel still rests on the base's axis, which is a cell further along.
        assert_eq!(barrel_now.placement().cell(barrel_now.docker.pos), Some(Pos::new(0, 2, 0)));
        assert_eq!(barrel_now.placement().cell(Pos::new(0, 0, 0)), Some(barrel.placement().cell(Pos::new(0, 0, 0)).unwrap().offset(-1, 0, 1)), "its core is where it was, in the base's shifted coordinates");
        // And moving it back by as much undoes it all.
        relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(-1, 0, 1), facing: None, dock_follows: true }, &catalog()).unwrap();
        assert_eq!(world(&bp), before);
        docks_hold(&bp);
    }

    #[test]
    fn the_barrel_moves_its_own_dock_and_the_shift_is_refused_where_a_core_or_a_rail_is_in_the_way() {
        let mut bp = with_turret_and_barrel();
        let before = world(&bp);
        let want = snapshot(&bp);
        let catalog = catalog();
        // Up one: the barrel's docker would land on the barrel's core.
        let err = relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(0, 1, 0), facing: None, dock_follows: true }, &catalog).unwrap_err();
        assert!(err.to_string().contains("ship core") && err.to_string().contains("ATTACHED_0/ATTACHED_0"), "{err}");
        assert_eq!(snapshot(&bp), want);
        // Down one: the base's docker takes the core's cell and the core the
        // axis's; the ship's axis sinks into the hull, replacing the block
        // there, since the cell it leaves is where the docker now rests.
        let r = relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(0, -1, 0), facing: None, dock_follows: true }, &catalog).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("a cell of the dock, which moved on")), "{:?}", r.notes);
        assert!(r.notes.iter().any(|n| n == "1 block where the moving blocks went was replaced."), "{:?}", r.notes);
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0"].rail.pos, Pos::new(11, 8, 11));
        assert!(bp.root.entity.get(Pos::new(11, 9, 11)).is_air(), "the docker rests where the axis was");
        assert_eq!(bp.root.entity.block_count(), 7 * 2 * 7, "the slab, the core, less the hull block the axis replaced and the one it sank into");
        let after = world(&bp);
        assert_eq!(after.len(), before.len() - 1);
        assert!(after.values().filter(|b| **b == before[&(String::new(), Pos::new(11, 8, 11))]).count() < before.values().filter(|b| **b == before[&(String::new(), Pos::new(11, 8, 11))]).count());
        // Nothing overlaps: every world cell is one entity's.
        let cells: BTreeSet<Pos> = after.keys().map(|(_, c)| *c).collect();
        assert_eq!(cells.len(), after.len());
        // Back up: the same but for the lost hull block.
        relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(0, 1, 0), facing: None, dock_follows: true }, &catalog).unwrap();
        let mut less = before.clone();
        less.remove(&(String::new(), Pos::new(11, 8, 11)));
        assert_eq!(world(&bp), less);
        // The barrel's own core: its docker and the base's axis follow.
        let before = world(&bp);
        let r = relocate(&mut bp, "ATTACHED_0/ATTACHED_0", Move { to: Pos::new(1, 0, 0), facing: None, dock_follows: true }, &catalog).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("the turret axis it rests on in the docked entity ATTACHED_0")), "{:?}", r.notes);
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].rail.pos, Pos::new(1, 1, 0));
        let after = world(&bp);
        let moved = before.iter().filter(|(k, v)| after.get(*k) != Some(*v)).count();
        assert_eq!(moved, 6, "the core, the docker and the axis, and the three plates they displaced");
        // Without the dock following, the old behaviour: the core alone swaps.
        let mut plain = with_turret_and_barrel();
        relocate(&mut plain, "ATTACHED_0", Move { to: Pos::new(1, 0, -1), facing: None, dock_follows: false }, &catalog).unwrap();
        assert_eq!(plain.docks()["ATTACHED_0"].rail.pos, Pos::new(11, 9, 11), "the ship's axis stays");
        assert_eq!(plain.docks()["ATTACHED_0"].docker.pos, Pos::new(-1, -1, 1), "the docker keeps its cell, which has another name now");
        // The main entity has no dock of its own; asking for it to follow changes nothing.
        let mut root = with_turret_and_barrel();
        let mut root_plain = with_turret_and_barrel();
        relocate(&mut root, "", Move { to: Pos::new(9, 8, 9), facing: None, dock_follows: true }, &catalog).unwrap();
        relocate(&mut root_plain, "", Move { to: Pos::new(9, 8, 9), facing: None, dock_follows: false }, &catalog).unwrap();
        assert_eq!(snapshot(&root), snapshot(&root_plain));
    }

    #[test]
    fn an_axis_that_would_float_stays_behind_with_what_hangs_from_it() {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        // Thin the base down to its column, a plate beside the core and the
        // barrel's axis on that plate: moved along Z the axis would float.
        let base = &mut bp.entity_mut("ATTACHED_0").unwrap().entity;
        let keep = [Pos::new(0, -1, 0), Pos::new(0, 0, 0), Pos::new(1, 0, 0), Pos::new(0, 1, 0)];
        let mut gone = Vec::new();
        base.for_each(|p, _| {
            if !keep.contains(&p) {
                gone.push(p);
            }
        });
        for p in gone {
            base.set(p, Block::AIR);
        }
        let e = bp.entity_mut("ATTACHED_0").unwrap();
        e.header.refresh(&e.entity);
        // The axis is at (0, 1, 0), over the core: Move rail puts it on the plate beside, the barrel coming along.
        let r = move_rail(&mut bp, "ATTACHED_0/ATTACHED_0", Pos::new(1, 0, 0), true, &catalog).unwrap();
        assert!(r.notes[0].contains("came along"), "{:?}", r.notes);
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].rail.pos, Pos::new(1, 1, 0));
        let before = world(&bp);
        let r = relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(0, 0, 1), facing: None, dock_follows: true }, &catalog).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("stayed where it is")), "{:?}", r.notes);
        docks_hold(&bp);
        // The barrel and its axis are where they were; the base's docker and core moved.
        let after = world(&bp);
        let moved: Vec<_> = before.iter().filter(|(k, v)| after.get(*k) != Some(*v)).map(|(k, _)| k.0.clone()).collect();
        assert!(moved.iter().all(|path| path.is_empty() || path == "ATTACHED_0"), "{moved:?}");
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].rail.pos, Pos::new(1, 1, -1), "the axis keeps its cell, named from the moved core");
        assert_eq!(bp.entity("ATTACHED_0").unwrap().entity.get(Pos::new(1, 1, -1)).ty(), TURRET_AXIS);
    }

    #[test]
    fn a_rail_moves_with_or_without_what_hangs_from_it() {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        let before = world(&bp);
        // The barrel's axis, one along X, the barrel coming along.
        let r = move_rail(&mut bp, "ATTACHED_0/ATTACHED_0", Pos::new(1, 0, 0), true, &catalog).unwrap();
        assert!(r.notes[0].contains("moved from (0, 1, 0) to (1, 1, 0) in the docked entity ATTACHED_0") && r.notes[0].contains("came along"), "{:?}", r.notes);
        docks_hold(&bp);
        let after = world(&bp);
        let barrel = |w: &std::collections::BTreeMap<(String, Pos), u32>| w.iter().filter(|((p, _), _)| p == "ATTACHED_0/ATTACHED_0").map(|((_, c), b)| (*c, *b)).collect::<Vec<_>>();
        let shifted: Vec<_> = barrel(&before).into_iter().map(|(c, b)| (c.offset(1, 0, 0), b)).collect();
        assert_eq!(barrel(&after), shifted, "every block of the barrel moved one along X");
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].docker.pos, Pos::new(0, -1, 0), "its docker is the same block");
        // Back, the barrel staying: its docker moves instead.
        let r = move_rail(&mut bp, "ATTACHED_0/ATTACHED_0", Pos::new(-1, 0, 0), false, &catalog).unwrap();
        assert!(r.notes[0].contains("stays where it is"), "{:?}", r.notes);
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].rail.pos, Pos::new(0, 1, 0));
        assert_eq!(bp.docks()["ATTACHED_0/ATTACHED_0"].docker.pos, Pos::new(-1, -1, 0));
        // The barrel is still where the first move put it: only its docker and
        // the plate it swapped with differ, and the base's axis went back.
        let again = world(&bp);
        let differing: Vec<_> = again.iter().filter(|(k, v)| after.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect();
        assert_eq!(differing.len(), 4, "{differing:?}");
        let base_cells = |w: &std::collections::BTreeMap<(String, Pos), u32>| w.iter().filter(|((p, _), _)| p == "ATTACHED_0").map(|((_, c), b)| (*c, *b)).collect::<Vec<_>>();
        assert_eq!(base_cells(&again), base_cells(&before), "the base is as it started");
        // The base's own axis, chosen from the base's side: the cell for its docker.
        let by = rail_shift(&bp, "ATTACHED_0", "ATTACHED_0", Pos::new(0, -1, 1)).unwrap();
        assert_eq!(by, Pos::new(0, 0, 1));
        assert_eq!(rail_shift(&bp, "", "ATTACHED_0", Pos::new(11, 9, 12)).unwrap(), Pos::new(0, 0, 1));
        move_rail(&mut bp, "ATTACHED_0", by, false, &catalog).unwrap();
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0"].rail.pos, Pos::new(11, 9, 12));
        assert_eq!(bp.docks()["ATTACHED_0"].docker.pos, Pos::new(0, -1, 1));
        // Refusals: no shift, a stranger, the main entity.
        assert!(move_rail(&mut bp, "ATTACHED_0", Pos::new(0, 0, 0), true, &catalog).is_err());
        assert!(rail_shift(&bp, "ATTACHED_0/ATTACHED_0", "ATTACHED_0", Pos::new(0, 0, 0)).is_err());
        assert!(move_rail(&mut bp, "", Pos::new(1, 0, 0), true, &catalog).is_err());
    }

    #[test]
    fn the_dock_follows_a_turned_core_too() {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        let before = world(&bp);
        // Moved along X and turned so that +X becomes the nose.
        let r = relocate(&mut bp, "ATTACHED_0", Move { to: Pos::new(1, 0, 0), facing: Some(([1, 0, 0], [0, 1, 0])), dock_follows: true }, &catalog).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("turned")), "{:?}", r.notes);
        docks_hold(&bp);
        assert_eq!(bp.docks()["ATTACHED_0"].rail.pos, Pos::new(12, 9, 11));
        // The base turned round its new column, which is on the ship's new axis; the barrel rests on the base and turned with it.
        let after = world(&bp);
        assert_eq!(after.len(), before.len());
        let base = &bp.docks()["ATTACHED_0"];
        assert_eq!(base.placement().cell(Pos::new(0, 0, 0)), Some(Pos::new(12, 11, 11)));
        let barrel = &bp.docks()["ATTACHED_0/ATTACHED_0"];
        assert_eq!(barrel.placement().cell(barrel.docker.pos), Some(barrel.rail.pos.offset(0, 1, 0)));
        let barrel_core = base.placement().then(&barrel.placement()).cell(Pos::new(0, 0, 0)).unwrap();
        assert_eq!(barrel_core.y, 14, "{barrel_core:?}");
        assert_eq!(before[&("ATTACHED_0/ATTACHED_0".to_string(), Pos::new(11, 14, 11))], after[&("ATTACHED_0/ATTACHED_0".to_string(), barrel_core)]);
    }

    #[test]
    fn the_manager_tag_is_dropped_and_said_so_and_a_name_two_entities_share_is_refused() {
        let (mut bp, ..) = with_turret();
        let mut meta = MetaFile::parse(&bp.root.meta).unwrap();
        meta.tail = Tail::Manager(vec![1, 2, 3, 4]);
        bp.root.meta = meta.write();
        let r = relocate_core(&mut bp, "", Pos::new(11, 11, 11)).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("Storage contents")), "{:?}", r.notes);
        assert_eq!(MetaFile::parse(&bp.root.meta).unwrap().tail, Tail::Finish);
        // Two entities with the same wireless name, one of them named by a link.
        let (mut bp, ..) = with_turret();
        let mut meta = MetaFile::parse(&bp.entity("ATTACHED_0").unwrap().meta).unwrap();
        meta.rails_mut().unwrap().name = String::new();
        bp.entity_mut("ATTACHED_0").unwrap().meta = meta.write();
        let want = snapshot(&bp);
        assert!(relocate_core(&mut bp, "", Pos::new(11, 11, 11)).is_err());
        assert_eq!(snapshot(&bp), want);
    }
}
