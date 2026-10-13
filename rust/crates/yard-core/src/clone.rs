//! Copying a docked entity, with everything docked to it, onto another dock
//! of the design: in place of another docked entity, which is removed, or
//! onto a new rail block placed where the player chose.
//!
//! The copy is the entity as it is: the same blocks about its core, the same
//! links, rail docker, storage and AI settings, and the same docked entities
//! below it. Only what names it changes: its folder, the name it goes by
//! among the design's entities (which must be unique, and which wireless
//! links use) and the dock records, which are written anew at rest.

use crate::blueprint::BlueprintEntity;
use crate::dock::{Dock, Piece as DockPiece, ROTATORS, TURRET_AXIS};
use crate::dockrec::{DockRecord, Piece};
use crate::metafile::{MetaFile, Rails, Tail};
use crate::relocate::{called, in_front, parent_of, pinned, Aid};
use crate::turret::{attached_number, orientation_facing, Made};
use crate::{Block, Blueprint, Error, Pos, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Where a copy goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// In place of another docked entity, which is removed with everything
    /// docked to it; the copy hangs from the rail that entity hung from.
    Replace { entity: String },
    /// On a new rail block (of the kind the original hangs from) at `at` in
    /// the entity at `parent`, replacing the block there, with its arrow
    /// side `facing` (a step along one axis): the copy's rail docker rests
    /// in the cell that way, which must be empty.
    New { parent: String, at: Pos, facing: [i32; 3] },
}

fn refuse(detail: impl Into<String>) -> Error {
    Error::Unsupported { what: "cloning a docked entity", detail: detail.into() }
}

/// The meta file of a docked entity, which must be one this can rewrite.
fn read_meta(e: &BlueprintEntity, which: &str) -> Result<MetaFile> {
    if e.meta.is_empty() {
        return Ok(MetaFile { version: 5, sections: Vec::new(), tail: Tail::Finish });
    }
    let meta = MetaFile::parse(&e.meta).map_err(|err| refuse(format!("the meta file of {which} cannot be read ({err})")))?;
    if meta.version < 4 {
        return Err(refuse(format!("the meta file of {which} is from an old version of the game (version {})", meta.version)));
    }
    if meta.has_legacy_docks() {
        return Err(refuse(format!("{which} has docks of the old kind, whose positions are not understood")));
    }
    Ok(meta)
}

/// The names every entity of the design goes by.
fn names_in(bp: &Blueprint) -> BTreeSet<String> {
    bp.entities().into_iter().filter_map(|(_, e)| MetaFile::parse(&e.meta).ok()?.rails().map(|r| r.name.clone())).collect()
}

/// A name for a docked entity, as the game makes them: its parent's name
/// and its number, lengthened until no entity has it.
fn fresh_name(parent_name: &str, n: u32, taken: &mut BTreeSet<String>) -> String {
    let mut name = format!("{}{n}", if parent_name.is_empty() { "rl" } else { parent_name });
    while taken.contains(&name) {
        name.push('0');
    }
    taken.insert(name.clone());
    name
}

/// The lowest number no docked entity of `parent` has, by folder or by record.
fn free_number(parent: &BlueprintEntity, meta: &MetaFile) -> u32 {
    let taken: BTreeSet<u32> = parent
        .children
        .iter()
        .filter_map(|c| attached_number(&c.dir))
        .chain(meta.rails().into_iter().flat_map(|r| r.children.iter().filter_map(|(c, _)| attached_number(c.rsplit('/').next()?))))
        .collect();
    (0..).find(|n| !taken.contains(n)).expect("some number is free")
}

/// Gives every entity of a copied subtree its new folder, name and dock
/// records, so that nothing in it names the original. `path` is the copy's
/// new path, `name` its new name; docked entities below keep their folders.
/// Docked entities whose record is missing are dropped and counted.
fn relabel(bp_name: &str, e: &mut BlueprintEntity, path: &str, name: &str, taken: &mut BTreeSet<String>, renamed: &mut BTreeMap<String, String>, dropped: &mut usize) -> Result<()> {
    let which = format!("the copy of {}", called(path));
    let mut meta = read_meta(e, &which)?;
    let version = meta.version;
    if meta.rails().is_none() {
        meta.add_rails(Rails { head: [0; 24], name: name.to_string(), wireless: Vec::new(), children: Vec::new() });
    }
    let old_name = meta.rails().map(|r| r.name.clone()).unwrap_or_default();
    renamed.insert(old_name, name.to_string());
    let records: Vec<(String, Vec<u8>)> = meta.rails().map(|r| r.children.clone()).unwrap_or_default();
    let mut children = std::mem::take(&mut e.children);
    let mut new_records = Vec::new();
    let mut kept = Vec::new();
    for mut child in children.drain(..) {
        let record = records.iter().find(|(c, r)| c.rsplit('/').next() == Some(child.dir.as_str()) && !r.is_empty());
        let dock = match record.map(|(_, r)| Dock::read(r, version)) {
            Some(Ok(Some(dock))) => dock,
            _ => {
                *dropped += 1;
                continue;
            }
        };
        let n = attached_number(&child.dir).unwrap_or(0);
        let child_name = fresh_name(name, n, taken);
        let child_path = format!("{path}/{}", child.dir);
        relabel(bp_name, &mut child, &child_path, &child_name, taken, renamed, dropped)?;
        new_records.push((format!("{bp_name}/{child_path}"), new_record(bp_name, name, &dock.rail, &child_name, &dock.docker).write()));
        kept.push(child);
    }
    e.children = kept;
    let rails = meta.rails_mut().expect("added above");
    rails.name = name.to_string();
    rails.children = new_records;
    e.meta = meta.write();
    Ok(())
}

/// A dock record at rest for a docker of the entity called `child` on a
/// rail of the entity called `parent`.
fn new_record(bp_name: &str, parent: &str, rail: &DockPiece, child: &str, docker: &DockPiece) -> DockRecord {
    DockRecord::new_turret(
        Piece { entity: &format!("{bp_name}{parent}"), pos: rail.pos, ty: rail.ty, orientation: rail.orientation },
        Piece { entity: &format!("{bp_name}{child}"), pos: docker.pos, ty: docker.ty, orientation: docker.orientation },
        in_front(rail),
    )
}

/// Every entity of a subtree, with the names they go by, for links to them to be dropped.
fn names_below(e: &BlueprintEntity, out: &mut BTreeSet<String>) {
    if let Some(name) = MetaFile::parse(&e.meta).ok().and_then(|m| m.rails().map(|r| r.name.clone())) {
        out.insert(name);
    }
    for c in &e.children {
        names_below(c, out);
    }
}

fn count_below(e: &BlueprintEntity) -> (usize, u64) {
    e.children.iter().fold((e.children.len(), e.entity.block_count()), |(n, b), c| {
        let (cn, cb) = count_below(c);
        (n + cn, b + cb)
    })
}

/// Copies the docked entity at `source`, with everything docked to it, to
/// `place` (see [`Place`]). Returns the copy's path. Refuses, leaving the
/// blueprint as it was, a source or target that is not docked, a place that
/// is the source itself or holds it, a cell that is a core or that a dock
/// rests on, and meta files this cannot rewrite. `aid` tells which block
/// types keep data by position (the block catalog does).
pub fn clone_docked(bp: &mut Blueprint, aid: &dyn Aid, source: &str, place: &Place) -> Result<Made> {
    let bp_name = bp.name.clone();
    let docks = bp.docks();
    if source.is_empty() {
        return Err(refuse("the main entity is docked to nothing; choose a docked entity to copy"));
    }
    let original = bp.entity(source).ok_or_else(|| refuse(format!("there is no entity {source:?}")))?;
    let dock = docks.get(source).ok_or_else(|| refuse(format!("{} has no dock on record", called(source))))?.clone();
    if original.logic_unreadable {
        return Err(refuse(format!("the links of {} are in a file this editor cannot read", called(source))));
    }
    let (parts, blocks) = count_below(original);

    // Where it goes: the rail, in its entity's coordinates, and that entity.
    let (parent_path, rail, replaced_dir): (String, DockPiece, Option<String>) = match place {
        Place::Replace { entity } => {
            if entity == source {
                return Err(refuse("an entity cannot take its own place"));
            }
            if source.starts_with(&format!("{entity}/")) {
                return Err(refuse(format!("{} holds {}, so the copy cannot take its place", called(entity), called(source))));
            }
            let old = docks.get(entity).ok_or_else(|| refuse(format!("{} has no dock on record", called(entity))))?;
            let (parent, dir) = parent_of(entity).ok_or_else(|| refuse("the main entity cannot be replaced"))?;
            (parent.to_string(), old.rail, Some(dir.to_string()))
        }
        Place::New { parent, at, facing } => {
            if facing.iter().map(|c| c.abs()).sum::<i32>() != 1 {
                return Err(refuse("the rail must face along one axis"));
            }
            let e = bp.entity(parent).ok_or_else(|| refuse(format!("there is no entity {parent:?}")))?;
            if e.logic_unreadable {
                return Err(refuse(format!("the links of {} are in a file this editor cannot read", called(parent))));
            }
            let front = at.offset(facing[0], facing[1], facing[2]);
            let pins = pinned(bp, &docks, parent);
            for (cell, what) in [(*at, "the rail"), (front, "the copy's rail docker")] {
                if let Some(there) = pins.get(&cell) {
                    return Err(refuse(format!("{what} would take the cell of {there} in {}", called(parent))));
                }
            }
            if !e.entity.get(front).is_air() {
                return Err(refuse(format!("the cell in front of the rail, ({}, {}, {}) of {}, where the copy's rail docker rests, holds a block", front.x, front.y, front.z, called(parent))));
            }
            (parent.clone(), DockPiece { pos: *at, ty: dock.rail.ty, orientation: orientation_facing(*facing) }, None)
        }
    };
    let parent = bp.entity(&parent_path).ok_or_else(|| refuse(format!("there is no entity {parent_path:?}")))?;
    let mut parent_meta = read_meta(parent, &called(&parent_path))?;
    let parent_name = parent_meta.rails().map_or(String::new(), |r| r.name.clone());

    // The copy, named afresh throughout.
    let mut copy = original.clone();
    let mut taken = names_in(bp);
    let mut removed_names = BTreeSet::new();
    if let Some(dir) = &replaced_dir {
        if let Some(old) = parent.children.iter().find(|c| c.dir == *dir) {
            names_below(old, &mut removed_names);
        }
        for name in &removed_names {
            taken.remove(name);
        }
    }
    let n = match &replaced_dir {
        Some(dir) => attached_number(dir).unwrap_or_else(|| free_number(parent, &parent_meta)),
        None => free_number(parent, &parent_meta),
    };
    let dir_name = format!("ATTACHED_{n}");
    let child_path = if parent_path.is_empty() { dir_name.clone() } else { format!("{parent_path}/{dir_name}") };
    let name = fresh_name(&parent_name, n, &mut taken);
    let mut renamed = BTreeMap::new();
    let mut dropped_parts = 0;
    copy.dir = dir_name.clone();
    relabel(&bp_name, &mut copy, &child_path, &name, &mut taken, &mut renamed, &mut dropped_parts)?;
    // Wireless links inside the copy follow the new names; links to other
    // entities stay (those entities keep their names).
    fn rename_wireless(e: &mut BlueprintEntity, renamed: &BTreeMap<String, String>) {
        if let Ok(mut meta) = MetaFile::parse(&e.meta) {
            if let Some(rails) = meta.rails_mut() {
                for pair in &mut rails.wireless {
                    if let Some(new) = renamed.get(&pair.entity) {
                        pair.entity = new.clone();
                    }
                }
            }
            e.meta = meta.write();
        }
        for c in &mut e.children {
            rename_wireless(c, renamed);
        }
    }
    rename_wireless(&mut copy, &renamed);
    let record = new_record(&bp_name, &parent_name, &rail, &name, &dock.docker).write();

    // Nothing can fail from here on.
    let mut notes = Vec::new();
    let parent = bp.entity_mut(&parent_path).expect("checked above");
    let mut old_rail_block = Block::AIR;
    if let Some(dir) = &replaced_dir {
        parent.children.retain(|c| c.dir != *dir);
        if let Some(rails) = parent_meta.rails_mut() {
            rails.children.retain(|(c, _)| c.rsplit('/').next() != Some(dir.as_str()));
        }
    } else {
        old_rail_block = parent.entity.get(rail.pos);
        let links: Vec<(Pos, u16, Pos)> = parent
            .entity
            .links()
            .iter()
            .flat_map(|(c, groups)| groups.iter().flat_map(move |(ty, targets)| targets.iter().map(move |t| (*c, *ty, *t))))
            .filter(|(c, _, t)| *c == rail.pos || *t == rail.pos)
            .collect();
        for (c, ty, t) in &links {
            parent.entity.unlink(*c, *t, *ty);
        }
        if !links.is_empty() {
            notes.push(format!("{} {} of the block the rail replaced {} dropped.", links.len(), if links.len() == 1 { "link" } else { "links" }, if links.len() == 1 { "was" } else { "were" }));
        }
        parent.entity.set(rail.pos, Block::new(rail.ty, rail.orientation, false));
        parent.header.refresh(&parent.entity);
        let gone = |p: Pos| p == rail.pos;
        parent_meta.take_dockers(&gone);
        parent_meta.take_cargo(&gone);
        let mut dropped_wireless = parent_meta.drop_wireless(true, &parent_name, &gone);
        for (other_path, other) in bp.entities().into_iter().map(|(p, e)| (p, e.meta.clone())).collect::<Vec<_>>() {
            if other_path == parent_path || other.is_empty() || crate::meta::version(&other).is_ok_and(|v| v < 2) {
                continue;
            }
            if let Ok(mut m) = MetaFile::parse(&other) {
                let k = m.drop_wireless(false, &parent_name, &gone);
                if k > 0 {
                    dropped_wireless += k;
                    bp.entity_mut(&other_path).expect("listed").meta = m.write();
                }
            }
        }
        if dropped_wireless > 0 {
            notes.push(format!("{dropped_wireless} wireless {} to the block the rail replaced {} dropped.", if dropped_wireless == 1 { "link" } else { "links" }, if dropped_wireless == 1 { "was" } else { "were" }));
        }
        if !old_rail_block.is_air() && aid.keeps_data(old_rail_block.ty()) && parent_meta.has_manager_tag() {
            parent_meta.tail = Tail::Finish;
            notes.push(format!("Storage contents, display text and other block settings stored by position were reset on {}.", called(&parent_path)));
        }
    }
    if parent_meta.rails().is_none() {
        parent_meta.add_rails(Rails { head: [0; 24], name: parent_name.clone(), wireless: Vec::new(), children: Vec::new() });
    }
    parent_meta.rails_mut().expect("added above").children.push((format!("{bp_name}/{child_path}"), record));
    let parent = bp.entity_mut(&parent_path).expect("checked above");
    parent.meta = parent_meta.write();
    let at = parent.children.iter().position(|c| attached_number(&c.dir).is_some_and(|k| k > n)).unwrap_or(parent.children.len());
    parent.children.insert(at, copy);

    // Wireless links other entities had to what was removed.
    if !removed_names.is_empty() {
        let mut dropped = 0;
        for (other_path, other) in bp.entities().into_iter().map(|(p, e)| (p, e.meta.clone())).collect::<Vec<_>>() {
            let Ok(mut m) = MetaFile::parse(&other) else { continue };
            let Some(rails) = m.rails_mut() else { continue };
            let before = rails.wireless.len();
            rails.wireless.retain(|p| !removed_names.contains(&p.entity));
            if rails.wireless.len() != before {
                dropped += before - rails.wireless.len();
                bp.entity_mut(&other_path).expect("listed").meta = m.write();
            }
        }
        if dropped > 0 {
            notes.push(format!("{dropped} wireless {} to the entity replaced {} dropped.", if dropped == 1 { "link" } else { "links" }, if dropped == 1 { "was" } else { "were" }));
        }
    }

    // What the copy sits in, as placed at rest.
    let overlaps = overlapping(bp, &child_path);
    let rail_name = if rail.ty == TURRET_AXIS {
        "turret axis"
    } else if ROTATORS.contains(&rail.ty) {
        "rotator"
    } else {
        "rail"
    };
    notes.insert(
        0,
        match place {
            Place::Replace { entity } => format!(
                "Copied {} as {child_path}: {blocks} blocks{}, in place of {}, which was removed with what was docked to it; the copy hangs from the {rail_name} it hung from.",
                called(source),
                if parts > 0 { format!(" with {parts} docked {}", if parts == 1 { "entity" } else { "entities" }) } else { String::new() },
                called(entity)
            ),
            Place::New { .. } => format!(
                "Copied {} as {child_path}: {blocks} blocks{}, on a new {rail_name} at ({}, {}, {}) of {}{}.",
                called(source),
                if parts > 0 { format!(" with {parts} docked {}", if parts == 1 { "entity" } else { "entities" }) } else { String::new() },
                rail.pos.x,
                rail.pos.y,
                rail.pos.z,
                called(&parent_path),
                if old_rail_block.is_air() { "" } else { ", which replaced the block there" }
            ),
        },
    );
    if dropped_parts > 0 {
        notes.push(format!("{dropped_parts} docked {} of the original had no dock on record and {} left out of the copy.", if dropped_parts == 1 { "entity" } else { "entities" }, if dropped_parts == 1 { "was" } else { "were" }));
    }
    for (other, n) in overlaps {
        notes.push(format!("{n} {} of the copy {} where {} has blocks; move one or the other.", if n == 1 { "block" } else { "blocks" }, if n == 1 { "sits" } else { "sit" }, called(&other)));
    }
    Ok(Made { child: child_path, notes })
}

/// How many blocks of the entity at `path` and of what is docked to it sit
/// in cells other entities' blocks take, with the design at rest, by the
/// other entity's path.
fn overlapping(bp: &Blueprint, path: &str) -> Vec<(String, usize)> {
    let placed: BTreeMap<String, crate::dock::Placement> = bp.layout(|_, d| d.at_rest()).into_iter().collect();
    let below = format!("{path}/");
    let mine = |p: &str| p == path || p.starts_with(&below);
    let others: Vec<(String, &BlueprintEntity, crate::dock::Placement)> = bp.entities().into_iter().filter(|(p, _)| !mine(p)).filter_map(|(p, e)| Some((p.clone(), e, placed.get(&p)?.back()))).collect();
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for (p, e) in bp.entities().into_iter().filter(|(p, _)| mine(p)) {
        let Some(place) = placed.get(&p) else { continue };
        e.entity.for_each(|cell, _| {
            let Some(world) = place.cell(cell) else { return };
            for (other, oe, back) in &others {
                if back.cell(world).is_some_and(|q| !oe.entity.get(q).is_air()) {
                    *out.entry(other.clone()).or_insert(0) += 1;
                }
            }
        });
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turret::fixtures::*;
    use crate::turret;

    /// A slab (x and z 4..=20, y 8..=9) with a turret base cut from a 3 by 6
    /// by 3 box on it at x and z 10..=12 and a barrel cut from the base's
    /// top three layers. The base's core is at (11, 11, 11) of the ship, on
    /// an axis at (11, 9, 11).
    fn with_turret_and_barrel() -> Blueprint {
        let catalog = catalog();
        let mut bp = ship(&[(Pos::new(4, 8, 4), Pos::new(20, 9, 20))]);
        for x in 10..=12 {
            for y in 10..=15 {
                for z in 10..=12 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 5, false));
                }
            }
        }
        let made = turret::make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(12, 15, 12), 3).unwrap();
        assert_eq!(made.child, "ATTACHED_0");
        let barrel = turret::make_turret(&mut bp, &catalog, "ATTACHED_0", Pos::new(-1, 2, -1), Pos::new(1, 4, 1), 3).unwrap();
        assert_eq!(barrel.child, "ATTACHED_0/ATTACHED_0");
        bp
    }

    /// A second, plain turret: a 3 by 3 by 4 box at x 14..=16, z 14..=17 on the slab.
    fn with_two_turrets() -> Blueprint {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        for x in 14..=16 {
            for y in 10..=12 {
                for z in 14..=17 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(HULL, 0, false));
                }
            }
        }
        let made = turret::make_turret(&mut bp, &catalog, "", Pos::new(14, 10, 14), Pos::new(16, 12, 17), 3).unwrap();
        assert_eq!(made.child, "ATTACHED_1");
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

    fn same_blocks(a: &crate::Entity, b: &crate::Entity) -> bool {
        let mut same = a.block_count() == b.block_count();
        a.for_each(|p, block| same &= b.get(p) == block);
        same
    }

    #[test]
    fn a_turret_is_copied_onto_a_new_axis_with_its_barrel_and_the_names_stay_unique() {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        let before = bp.docks();
        let made = clone_docked(&mut bp, &catalog, "ATTACHED_0", &Place::New { parent: String::new(), at: Pos::new(15, 9, 15), facing: [0, 1, 0] }).unwrap();
        assert_eq!(made.child, "ATTACHED_1");
        assert!(made.notes[0].contains("with 1 docked entity") && made.notes[0].contains("replaced the block there"), "{:?}", made.notes);
        assert!(!made.notes.iter().any(|n| n.contains("sit where")), "{:?}", made.notes);
        docks_hold(&bp);
        let docks = bp.docks();
        assert_eq!(docks.len(), 4, "{:?}", docks.keys().collect::<Vec<_>>());
        let copy = &docks["ATTACHED_1"];
        assert_eq!((copy.rail.pos, copy.rail.ty), (Pos::new(15, 9, 15), TURRET_AXIS));
        assert_eq!(copy.rail.orientation, before["ATTACHED_0"].rail.orientation, "an axis facing up, as the original's");
        assert_eq!(copy.docker, before["ATTACHED_0"].docker);
        assert_eq!(docks["ATTACHED_1/ATTACHED_0"].rail, before["ATTACHED_0/ATTACHED_0"].rail);
        assert_eq!(docks["ATTACHED_1/ATTACHED_0"].docker, before["ATTACHED_0/ATTACHED_0"].docker);
        // The same blocks, in both the base and the barrel; the ship got an axis.
        assert!(same_blocks(&bp.entity("ATTACHED_1").unwrap().entity, &bp.entity("ATTACHED_0").unwrap().entity));
        assert!(same_blocks(&bp.entity("ATTACHED_1/ATTACHED_0").unwrap().entity, &bp.entity("ATTACHED_0/ATTACHED_0").unwrap().entity));
        assert_eq!(bp.root.entity.get(Pos::new(15, 9, 15)).ty(), TURRET_AXIS);
        // Every entity goes by its own name.
        assert_eq!(names_in(&bp).len(), 5);
        // Placed at rest, the copy's core sits two above its axis, like the original's.
        let placed: BTreeMap<String, crate::dock::Placement> = bp.layout(|_, d| d.at_rest()).into_iter().collect();
        assert_eq!(placed["ATTACHED_0"].cell(Pos::new(0, 0, 0)), Some(Pos::new(11, 11, 11)));
        assert_eq!(placed["ATTACHED_1"].cell(Pos::new(0, 0, 0)), Some(Pos::new(15, 11, 15)));
        assert!(placed.contains_key("ATTACHED_1/ATTACHED_0"));
        // Written out and read back, it is all still there.
        let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
        assert_eq!(again.docks(), docks);
        assert_eq!(names_in(&again).len(), 5);
    }

    #[test]
    fn a_copy_takes_the_place_of_another_turret_which_goes_with_what_hung_from_it() {
        let catalog = catalog();
        let mut bp = with_two_turrets();
        let plain = bp.entity("ATTACHED_1").unwrap().entity.block_count();
        let base = bp.entity("ATTACHED_0").unwrap().entity.block_count();
        assert_ne!(plain, base);
        let rail_before = bp.docks()["ATTACHED_1"].rail;
        let made = clone_docked(&mut bp, &catalog, "ATTACHED_0", &Place::Replace { entity: "ATTACHED_1".into() }).unwrap();
        assert_eq!(made.child, "ATTACHED_1");
        assert!(made.notes[0].contains("in place of the docked entity ATTACHED_1"), "{:?}", made.notes);
        docks_hold(&bp);
        assert_eq!(bp.root.children.len(), 2);
        assert_eq!(bp.entity("ATTACHED_1").unwrap().entity.block_count(), base);
        assert!(same_blocks(&bp.entity("ATTACHED_1/ATTACHED_0").unwrap().entity, &bp.entity("ATTACHED_0/ATTACHED_0").unwrap().entity));
        let docks = bp.docks();
        assert_eq!(docks["ATTACHED_1"].rail, rail_before, "the copy hangs from the rail the old turret hung from");
        assert_eq!(docks["ATTACHED_1"].docker, docks["ATTACHED_0"].docker);
        assert_eq!(names_in(&bp).len(), 5);
        let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
        assert_eq!(again.docks(), docks);
    }

    #[test]
    fn the_copy_in_the_way_of_another_part_is_said_so() {
        let catalog = catalog();
        let mut bp = with_turret_and_barrel();
        // An axis one cell over from the original's: the copy's body overlaps the original's.
        let made = clone_docked(&mut bp, &catalog, "ATTACHED_0", &Place::New { parent: String::new(), at: Pos::new(12, 9, 11), facing: [0, 1, 0] }).unwrap();
        assert!(made.notes.iter().any(|n| n.contains("of the copy sit where the docked entity ATTACHED_0 has blocks")), "{:?}", made.notes);
    }

    #[test]
    fn what_cannot_be_copied_is_refused_and_nothing_changes() {
        let catalog = catalog();
        let mut bp = with_two_turrets();
        let before = bp.to_files().unwrap();
        let new = |at: Pos, facing: [i32; 3]| Place::New { parent: String::new(), at, facing };
        let refused = [
            ("", Place::Replace { entity: "ATTACHED_1".into() }, "the main entity"),
            ("ATTACHED_0", Place::Replace { entity: "ATTACHED_0".into() }, "its own place"),
            ("ATTACHED_0/ATTACHED_0", Place::Replace { entity: "ATTACHED_0".into() }, "holds"),
            ("ATTACHED_0", Place::Replace { entity: "ATTACHED_7".into() }, "no dock on record"),
            ("ATTACHED_0", new(Pos::new(0, 0, 0), [0, 1, 0]), "the ship core"),
            ("ATTACHED_0", new(Pos::new(11, 8, 11), [0, 1, 0]), "the turret axis"),
            ("ATTACHED_0", new(Pos::new(10, 8, 10), [0, 1, 0]), "holds a block"),
            ("ATTACHED_0", new(Pos::new(10, 9, 10), [1, 1, 0]), "along one axis"),
        ];
        for (source, place, why) in refused {
            let err = clone_docked(&mut bp, &catalog, source, &place).unwrap_err().to_string();
            assert!(err.contains(why), "{source} {place:?}: {err}");
        }
        assert_eq!(bp.to_files().unwrap(), before);
    }
}
