//! Relocating cores and making turrets on the dev designs (`rust/data/designs`):
//! the lossless tag and meta round trips, and that a core moved and moved back
//! leaves a design as it was. Skipped where the designs are not checked out.

use std::collections::BTreeMap;
use std::path::PathBuf;
use yard_core::metafile::{MetaFile, Tail};
use yard_core::rawtag::RawTag;
use yard_core::relocate::{relocate_and_turn, relocate_core};
use yard_core::{Blueprint, Pos};

/// Designs with more blocks than this are left out of the editing tests, which run in debug builds.
const SMALL: u64 = 2500;

fn designs() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/designs");
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sment"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

/// Every meta file of every design, by where it is in the archive, without reading any blocks.
fn meta_files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (name, data) in designs() {
        let Ok(files) = yard_core::blueprint::unzip(&data) else { continue };
        out.extend(files.into_iter().filter(|(path, _)| path.ends_with("meta.smbpm")).map(|(path, meta)| (format!("{name} {path}"), meta)));
    }
    out
}

#[test]
fn every_dock_record_survives_a_lossless_round_trip() {
    let mut n = 0;
    for (name, meta) in meta_files() {
        let Ok(records) = yard_core::meta::dock_records(&meta) else { continue };
        for (child, record) in records.into_iter().filter(|(_, r)| !r.is_empty()) {
            let tag = RawTag::parse(&record).unwrap_or_else(|err| panic!("{name} {child}: {err}"));
            let unpacked = if tag.gzipped {
                use std::io::Read;
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(&record[..]).read_to_end(&mut out).unwrap();
                out
            } else {
                record.clone()
            };
            assert_eq!(tag.write_plain(), unpacked, "{name} {child}");
            if !tag.gzipped {
                assert_eq!(tag.write(), record, "{name} {child}");
            }
            n += 1;
        }
    }
    eprintln!("{n} dock records round-tripped");
    assert!(n == 0 || n > 1000);
}

#[test]
fn every_meta_file_survives_a_lossless_round_trip() {
    for (name, meta) in meta_files() {
        // A few old files have a meta this does not read; those are refused, not guessed at.
        if let Ok(parsed) = MetaFile::parse(&meta) {
            assert_eq!(parsed.write(), meta, "{name}");
        }
    }
}

/// Everything of an entity that a relocation must bring back: path, blocks, links, meta.
type State = Vec<(String, Vec<(Pos, u32)>, Vec<(Pos, u16, Pos)>, Option<MetaFile>)>;

fn state(bp: &Blueprint) -> State {
    bp.entities()
        .into_iter()
        .map(|(path, e)| {
            let mut blocks = Vec::new();
            e.entity.for_each(|p, b| blocks.push((p, b.0)));
            blocks.sort();
            let mut links = Vec::new();
            for (c, groups) in e.entity.links() {
                for (ty, targets) in groups {
                    targets.iter().for_each(|t| links.push((*c, *ty, *t)));
                }
            }
            let meta = MetaFile::parse(&e.meta).ok().map(|mut m| {
                // The manager tag is the one thing a relocation drops.
                m.tail = Tail::Finish;
                m
            });
            (path, blocks, links, meta)
        })
        .collect()
}

/// Where each entity's blocks are in the root's coordinates, as the game
/// places them: (entity path, world cell) to the stored block. Entities that
/// sit off the grid (a turret left aimed between two cells) are left out.
fn world(bp: &Blueprint) -> BTreeMap<(String, Pos), u32> {
    let mut out = BTreeMap::new();
    for (path, placement) in bp.layout(|_, d| d.placement()) {
        let e = &bp.entity(&path).unwrap().entity;
        let mut cells = Vec::new();
        e.for_each(|p, b| cells.push((placement.cell(p), b.0)));
        if cells.iter().any(|(cell, _)| cell.is_none()) {
            continue;
        }
        out.extend(cells.into_iter().map(|(cell, block)| ((path.clone(), cell.unwrap()), block)));
    }
    out
}

/// The cell a core moves to in these tests: a block next to the core where
/// there is one, and failing that a cell outside the design.
fn target(e: &yard_core::Entity) -> Option<Pos> {
    for d in [(1, 0, 0), (0, 0, 1), (0, 1, 0), (-1, 0, 0), (0, -1, 0), (0, 0, -1)] {
        let p = Pos::new(d.0, d.1, d.2);
        if !e.get(p).is_air() {
            return Some(p);
        }
    }
    let b = e.bounds()?;
    Some(Pos::new(b.max.x + 3, b.min.y - 2, b.max.z))
}

#[test]
fn a_core_moved_and_moved_back_leaves_the_design_as_it_was() {
    let (mut done, mut refused) = (0, 0);
    for (name, data) in designs().into_iter().filter(|(_, d)| d.len() < 300_000) {
        let Ok(mut bp) = Blueprint::from_sment(&data) else { continue };
        if bp.entities().iter().map(|(_, e)| e.entity.block_count()).sum::<u64>() > SMALL {
            continue;
        }
        let before = state(&bp);
        let paths: Vec<String> = bp.entities().into_iter().map(|(p, _)| p).collect();
        for path in paths {
            let Some(to) = target(&bp.entity(&path).unwrap().entity) else { continue };
            match relocate_core(&mut bp, &path, to) {
                Ok(_) => {
                    let moved = &bp.entity(&path).unwrap().entity;
                    assert_eq!(moved.get(Pos::new(0, 0, 0)).ty(), 1, "{name} {path}");
                    relocate_core(&mut bp, &path, Pos::new(-to.x, -to.y, -to.z)).unwrap_or_else(|err| panic!("{name} {path}: back: {err}"));
                    assert_eq!(state(&bp), before, "{name} {path} to {to:?}");
                    done += 1;
                }
                Err(_) => {
                    // Refused: nothing may have changed.
                    assert_eq!(state(&bp), before, "{name} {path}: a refusal changed the design");
                    refused += 1;
                }
            }
        }
    }
    eprintln!("{done} relocations round-tripped, {refused} refused");
}

#[test]
fn relocating_keeps_every_block_where_it_is_in_the_world() {
    let mut done = 0;
    for (name, data) in designs().into_iter().filter(|(_, d)| d.len() < 300_000) {
        let Ok(bp) = Blueprint::from_sment(&data) else { continue };
        let entities = bp.entities();
        if entities.len() < 2 || entities.iter().map(|(_, e)| e.entity.block_count()).sum::<u64>() > SMALL {
            continue;
        }
        let paths: Vec<String> = entities.into_iter().map(|(p, _)| p).collect();
        let before = world(&bp);
        let placements = bp.layout(|_, d| d.placement());
        for path in paths {
            let Some((_, placement)) = placements.iter().find(|(p, _)| *p == path) else { continue };
            let Some(to) = target(&bp.entity(&path).unwrap().entity) else { continue };
            // An entity docked by its core hangs on that core, which stays the core, and a
            // docker or rail block that the core swaps places with takes what hangs on it
            // along: in those cases the design moves in the world on purpose.
            let docks = bp.docks();
            let hangs_on_it = |child: &str, d: &yard_core::dock::Dock| {
                (child == path && (d.docker.ty == 1 || d.docker.pos == to)) || (child.rsplit_once('/').map_or("", |(p, _)| p) == path && d.rail.pos == to)
            };
            if docks.iter().any(|(child, d)| hangs_on_it(child, d)) {
                continue;
            }
            let mut bp = Blueprint::from_sment(&data).unwrap();
            let (Some(old_core), Some(taken), Some(_)) = (placement.cell(Pos::new(0, 0, 0)), placement.cell(to), placement.cell(Pos::new(1, 2, 3))) else { continue };
            // (An entity with a block off the grid is not in the world map.)
            let Some(&core_block) = before.get(&(path.clone(), old_core)) else { continue };
            if relocate_core(&mut bp, &path, to).is_err() {
                continue;
            }
            // The root's whole world moves by -to; a docked entity's stays put.
            let shift = |p: Pos| if path.is_empty() { Pos::new(p.x - to.x, p.y - to.y, p.z - to.z) } else { p };
            let mut expected = BTreeMap::new();
            for ((e, cell), block) in &before {
                if *e != path || (*cell != old_core && *cell != taken) {
                    expected.insert((e.clone(), shift(*cell)), *block);
                }
            }
            // The core and the block that was at `to` have swapped places.
            expected.insert((path.clone(), shift(taken)), core_block);
            if let Some(block) = before.get(&(path.clone(), taken)) {
                expected.insert((path.clone(), shift(old_core)), *block);
            }
            let got = world(&bp);
            if got != expected {
                let missing: Vec<_> = expected.iter().filter(|(k, v)| got.get(*k) != Some(*v)).take(5).collect();
                let extra: Vec<_> = got.iter().filter(|(k, v)| expected.get(*k) != Some(*v)).take(5).collect();
                panic!("{name} {path} to {to:?}: expected but not there or different {missing:?}; found but not expected {extra:?}");
            }
            done += 1;
        }
    }
    eprintln!("{done} relocations kept the world as it was");
}

fn catalog() -> Option<yard_core::catalog::Catalog> {
    let dir = std::env::var_os("STARMADE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?
        .join("data/config");
    let xml = std::fs::read_to_string(dir.join("BlockConfig.xml")).ok()?;
    let props = std::fs::read_to_string(dir.join("BlockTypes.properties")).ok()?;
    yard_core::catalog::Catalog::parse(&xml, &props).ok()
}

/// A box of `size` cubed cells that is mostly blocks, away from the core, with a block under the
/// middle of one of its faces for the turret axis; the one with the most in it.
fn turret_box(e: &yard_core::Entity, size: i32) -> Option<(Pos, Pos, usize)> {
    let mut cells = Vec::new();
    e.for_each(|p, _| cells.push(p));
    cells.sort();
    let mut best: Option<(i32, Pos, Pos, usize)> = None;
    for lo in cells {
        let hi = lo.offset(size - 1, size - 1, size - 1);
        let mut n = 0;
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    n += !e.get(Pos::new(x, y, z)).is_air() as i32;
                }
            }
        }
        let origin = Pos::new(0, 0, 0);
        if n < size * size * size * 2 / 3 || (lo.lo(origin) == lo && hi.hi(origin) == hi) {
            continue;
        }
        for base in [3usize, 2, 0, 1, 4, 5] {
            let d = yard_core::shape::SIDE_DIRS[base];
            let axis = d.iter().position(|c| *c != 0).unwrap();
            let (l, h) = (lo.to_array(), hi.to_array());
            let mut r = [0; 3];
            for i in 0..3 {
                r[i] = if i == axis { (if d[axis] > 0 { h[i] } else { l[i] }) + d[axis] } else { (l[i] + h[i]).div_euclid(2) };
            }
            if e.get(Pos::new(r[0], r[1], r[2])).is_air() || Pos::new(r[0], r[1], r[2]) == origin {
                continue;
            }
            if best.as_ref().is_none_or(|b| n > b.0) {
                best = Some((n, lo, hi, base));
            }
            break;
        }
    }
    best.map(|(_, lo, hi, base)| (lo, hi, base))
}

#[test]
fn turrets_cut_from_real_designs_dock_where_they_were_cut() {
    let Some(catalog) = catalog() else { return };
    let (mut made, mut refused) = (0, 0);
    for (name, data) in designs().into_iter().filter(|(_, d)| d.len() < 300_000) {
        let Ok(bp) = Blueprint::from_sment(&data) else { continue };
        let entities = bp.entities();
        if entities.iter().map(|(_, e)| e.entity.block_count()).sum::<u64>() > SMALL {
            continue;
        }
        let before = world(&bp);
        let paths: Vec<String> = entities.into_iter().map(|(p, _)| p).collect();
        for path in paths {
            let mut bp = Blueprint::from_sment(&data).unwrap();
            let Some((lo, hi, base)) = turret_box(&bp.entity(&path).unwrap().entity, 3) else { continue };
            let count = bp.entities().len();
            let mut in_box = 0;
            bp.entity(&path).unwrap().entity.for_each(|p, _| in_box += (p.lo(lo) == lo && p.hi(hi) == hi) as usize);
            let snapshot = bp.to_files().unwrap();
            let child = match yard_core::turret::make_turret(&mut bp, &catalog, &path, lo, hi, base) {
                Ok(m) => m.child,
                Err(_) => {
                    assert_eq!(bp.to_files().unwrap(), snapshot, "{name} {path}: a refusal changed the design");
                    refused += 1;
                    continue;
                }
            };
            assert_eq!(bp.entities().len(), count + 1, "{name}");
            let dock = bp.docks().remove(&child).unwrap_or_else(|| panic!("{name} {child}: no dock"));
            let up = yard_core::shape::turned_axes(dock.rail.orientation).0;
            assert_eq!(dock.placement().cell(Pos::new(0, 0, 0)), Some(dock.rail.pos.offset(2 * up[0], 2 * up[1], 2 * up[2])), "{name} {child}");
            // Written and read again, the same.
            let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
            assert_eq!(again.entities().len(), count + 1);
            assert_eq!(world(&again), world(&bp), "{name} {child}");
            assert_eq!(again.docks()[&child], dock);
            // Every block that was in the box is still in its world cell, now on the child; the rest did not move.
            let after = world(&bp);
            let (mut same, mut cut) = (0, 0);
            for ((e, cell), block) in &before {
                if after.get(&(e.clone(), *cell)) == Some(block) {
                    same += 1;
                } else if after.get(&(child.clone(), *cell)) == Some(block) {
                    cut += 1;
                }
            }
            let _ = same;
            // (An entity left aimed between two cells is not in the world map at all.)
            if !before.keys().any(|(e, _)| *e == path) {
                continue;
            }
            // At most two of the blocks in the box were replaced, by the docker and the core.
            assert!(cut + 2 >= in_box && cut <= in_box, "{name} {path}: {cut} of the {in_box} blocks in the box moved to the child");
            let child_blocks = bp.entity(&child).unwrap().entity.block_count() as usize;
            assert!(child_blocks >= cut, "{name}");
            made += 1;
        }
    }
    eprintln!("{made} turrets made, {refused} refused");
    assert!(made > 20);
}

/// Turns tried on the real designs: half turns (done with the mirror tables),
/// quarter turns about each axis and a roll, as (forward, up).
const TURNS: [([i32; 3], [i32; 3]); 5] =
    [([0, 0, -1], [0, 1, 0]), ([1, 0, 0], [0, 1, 0]), ([0, 0, 1], [-1, 0, 0]), ([0, 1, 0], [0, 0, -1]), ([0, -1, 0], [1, 0, 0])];

/// The turn [`relocate_and_turn`] makes, as rows.
fn rows(forward: [i32; 3], up: [i32; 3]) -> [[i32; 3]; 3] {
    let right = [up[1] * forward[2] - up[2] * forward[1], up[2] * forward[0] - up[0] * forward[2], up[0] * forward[1] - up[1] * forward[0]];
    [right, up, forward]
}

/// The state with each shaped block's orientation brought to the lowest value
/// of the same solid: values past a shape's choices repeat lower ones, and a
/// turn may hand back either.
fn solid_state(bp: &Blueprint, catalog: &yard_core::catalog::Catalog) -> State {
    use yard_core::catalog::Shape;
    let set = yard_core::shape::ShapeSet::new();
    let solid = |shape: Shape, o: u8| {
        let mut out: Vec<[i32; 3]> = set.geom(shape, 0, o).faces.iter().flat_map(|f| f.corners.iter()).map(|c| c.map(|v| (v * 4.0).round() as i32)).collect();
        out.sort();
        out.dedup();
        out
    };
    let mut s = state(bp);
    for (_, blocks, _, _) in &mut s {
        for (_, raw) in blocks.iter_mut() {
            let mut b = yard_core::Block(*raw);
            let Some(def) = catalog.get(b.ty()) else { continue };
            if matches!(def.shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta) {
                let want = solid(def.shape, b.orientation());
                if let Some(low) = (0..24u8).find(|&o| solid(def.shape, o) == want) {
                    b.set_orientation(low);
                    *raw = b.0;
                }
            }
        }
    }
    s
}

/// The first few things that differ between two states, in words.
fn difference(a: &State, b: &State) -> String {
    let mut out = Vec::new();
    for ((path, blocks, links, meta), (_, blocks2, links2, meta2)) in a.iter().zip(b) {
        let one: BTreeMap<_, _> = blocks.iter().copied().collect();
        let two: BTreeMap<_, _> = blocks2.iter().copied().collect();
        for (p, v) in &one {
            if two.get(p) != Some(v) {
                let w = two.get(p).map(|w| (yard_core::Block(*w).ty(), yard_core::Block(*w).orientation(), *w));
                out.push(format!("{path} {p:?}: {:?} became {w:?}", (yard_core::Block(*v).ty(), yard_core::Block(*v).orientation(), *v)));
            }
        }
        if links != links2 {
            out.push(format!("{path}: links differ"));
        }
        if meta != meta2 {
            out.push(format!("{path}: meta differs"));
        }
    }
    out.truncate(6);
    out.join("; ")
}

#[test]
fn a_core_turned_and_turned_back_leaves_the_design_as_it_was() {
    let Some(catalog) = catalog() else { return };
    let (mut done, mut refused) = (0, 0);
    for (name, data) in designs().into_iter().filter(|(_, d)| d.len() < 300_000) {
        let Ok(mut bp) = Blueprint::from_sment(&data) else { continue };
        if bp.entities().iter().map(|(_, e)| e.entity.block_count()).sum::<u64>() > SMALL {
            continue;
        }
        let before = solid_state(&bp, &catalog);
        let exact = state(&bp);
        let paths: Vec<String> = bp.entities().into_iter().map(|(p, _)| p).collect();
        for path in paths {
            let Some(to) = target(&bp.entity(&path).unwrap().entity) else { continue };
            for (forward, up) in TURNS {
                match relocate_and_turn(&mut bp, &catalog, &path, to, forward, up) {
                    Ok(_) => {
                        assert_eq!(bp.entity(&path).unwrap().entity.get(Pos::new(0, 0, 0)).ty(), 1, "{name} {path}");
                        // Back: the block that was at `to` sits at turn · (-to), and the reverse turn's
                        // forward and up are the columns of this one.
                        let m = rows(forward, up);
                        let back_to = Pos::new(-(m[0][0] * to.x + m[0][1] * to.y + m[0][2] * to.z), -(m[1][0] * to.x + m[1][1] * to.y + m[1][2] * to.z), -(m[2][0] * to.x + m[2][1] * to.y + m[2][2] * to.z));
                        let column = |c: usize| [m[0][c], m[1][c], m[2][c]];
                        relocate_and_turn(&mut bp, &catalog, &path, back_to, column(2), column(1)).unwrap_or_else(|err| panic!("{name} {path}: back: {err}"));
                        let after = solid_state(&bp, &catalog);
                        if after != before {
                            panic!("{name} {path} to {to:?} facing {forward:?} up {up:?}: {}", difference(&before, &after));
                        }
                        done += 1;
                    }
                    Err(err) => {
                        assert_eq!(state(&bp), exact, "{name} {path}: a refusal changed the design");
                        eprintln!("refused {name} {path}: {err}");
                        refused += 1;
                    }
                }
                bp = Blueprint::from_sment(&data).unwrap();
            }
        }
    }
    eprintln!("{done} turns round-tripped, {refused} refused");
    assert!(done == 0 || done > 50);
}

#[test]
fn turning_the_core_turns_the_world_round_it() {
    let Some(catalog) = catalog() else { return };
    let mut done = 0;
    let types = |bp: &Blueprint| -> BTreeMap<(String, Pos), u16> { world(bp).into_iter().map(|(k, b)| (k, yard_core::Block(b).ty())).collect() };
    for (name, data) in designs().into_iter().filter(|(_, d)| d.len() < 300_000) {
        let Ok(bp) = Blueprint::from_sment(&data) else { continue };
        let entities = bp.entities();
        if entities.len() < 2 || entities.iter().map(|(_, e)| e.entity.block_count()).sum::<u64>() > SMALL {
            continue;
        }
        let before = types(&bp);
        let placements = bp.layout(|_, d| d.placement());
        let paths: Vec<String> = entities.into_iter().map(|(p, _)| p).collect();
        for path in paths {
            if !before.keys().any(|(e, _)| *e == path) {
                continue;
            }
            let Some((_, placement)) = placements.iter().find(|(p, _)| *p == path) else { continue };
            let Some(old_core) = placement.cell(Pos::new(0, 0, 0)) else { continue };
            for (forward, up) in TURNS {
                let mut bp = Blueprint::from_sment(&data).unwrap();
                // Turned only, so no block changes places within the entity.
                if relocate_and_turn(&mut bp, &catalog, &path, Pos::new(0, 0, 0), forward, up).is_err() {
                    continue;
                }
                let m = rows(forward, up);
                // The root's world turns round its core; a docked entity's stays put.
                let moved = |p: Pos| {
                    if !path.is_empty() {
                        return p;
                    }
                    let v = [p.x, p.y, p.z];
                    let r = m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);
                    Pos::new(r[0], r[1], r[2])
                };
                let expected: BTreeMap<_, _> = before.iter().map(|((e, cell), ty)| ((e.clone(), moved(*cell)), *ty)).collect();
                let got = types(&bp);
                if got != expected {
                    let missing: Vec<_> = expected.iter().filter(|(k, v)| got.get(*k) != Some(*v)).take(5).collect();
                    let extra: Vec<_> = got.iter().filter(|(k, v)| expected.get(*k) != Some(*v)).take(5).collect();
                    panic!("{name} {path} facing {forward:?} up {up:?} (core at {old_core:?}): expected but not there {missing:?}; found but not expected {extra:?}");
                }
                done += 1;
            }
        }
    }
    eprintln!("{done} turns kept every block where the turn put it");
    assert!(done == 0 || done > 20);
}
