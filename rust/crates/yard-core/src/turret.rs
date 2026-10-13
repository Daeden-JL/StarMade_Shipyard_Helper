//! Cutting a part out of an entity and docking it back as a turret.
//!
//! The commonest turret there is, as the game builds it by hand: a turret
//! axis block on the parent, a rail docker on the new entity right in front
//! of the axis (arrow side to arrow side, arrows pointing the same way, so
//! nothing needs turning), and the new entity's own ship core in the cell
//! behind the docker. The parent's meta file gets a record of the dock; the
//! new entity gets a meta file listing its docker.

use crate::blueprint::BlueprintEntity;
use crate::catalog::Catalog;
use crate::dock::{facing_back, TURRET_AXIS};
use crate::dockrec::{DockRecord, Piece};
use crate::edit::CORE_TYPE;
use crate::links::{Kind, Rules};
use crate::metafile::{self, MetaFile, Rails, Section, Tail};
use crate::shape::{turned_axes, SIDE_DIRS};
use crate::{Block, Blueprint, Error, Pos, Result};
use std::collections::BTreeSet;

const RAIL_DOCKER: u16 = 663;
/// The display module; its text is kept by position in the manager tag.
const DISPLAY: u16 = 479;
const CARGO: u16 = 689;
/// Positions in links and meta files are 16-bit numbers counted from the
/// core at 16; stay well inside them.
const LIMIT: i32 = 30_000;

/// What came of making a turret.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Made {
    /// The path of the new docked entity (`ATTACHED_3`, `ATTACHED_0/ATTACHED_1` ...).
    pub child: String,
    /// Things the player should know, in words.
    pub notes: Vec<String>,
}

fn refuse(detail: impl Into<String>) -> Error {
    Error::Unsupported { what: "making a turret", detail: detail.into() }
}

/// Whether a block of this type keeps data by position in the manager tag
/// (inventories, display text, computer settings). Types the block list does
/// not know are taken to.
pub(crate) fn keeps_data(catalog: &Catalog, ty: u16) -> bool {
    match catalog.get(ty) {
        None => true,
        Some(d) => {
            d.system || d.logic || d.inventory != 0 || d.factory || d.chamber || d.sensor || d.main_computer || d.support_computer || d.effect_computer || ty == DISPLAY || ty == CARGO
        }
    }
}

/// The orientation of a 24-way block whose arrow side faces `up`, with its
/// arrow pointing forward where the entity's own front, up or right allow.
pub(crate) fn orientation_facing(up: [i32; 3]) -> u8 {
    for forward in [[0, 0, 1], [0, 1, 0], [1, 0, 0]] {
        if let Some(o) = (0..24u8).find(|o| turned_axes(*o) == (up, forward)) {
            return o;
        }
    }
    unreachable!("every direction has an orientation with an arrow beside it")
}

/// A meta file that can be changed here.
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

fn called(path: &str) -> String {
    if path.is_empty() { "the main entity".to_string() } else { format!("the docked entity {path}") }
}

/// The number in `ATTACHED_<n>`.
pub(crate) fn attached_number(dir: &str) -> Option<u32> {
    dir.strip_prefix("ATTACHED_")?.parse().ok()
}

/// Cuts the blocks inside the box `min`..=`max` (design coordinates of the
/// entity at `path`) out of that entity and makes them a new entity docked to
/// it on a turret axis. `base` is the side of the box (the game's numbering:
/// 0 front +Z, 1 back -Z, 2 top +Y, 3 bottom -Y, 4 right +X, 5 left -X)
/// that rests on the parent: the turret axis goes in the cell outside the
/// middle of that face.
///
/// The rail docker takes the cell of the box next to the axis and the new
/// ship core the one beyond it, replacing whatever was there; the axis
/// replaces whatever block was in its cell. The box must be at least three
/// cells deep, hold a block, and leave the parent's core alone. Links inside
/// the box move with the blocks, links across its edge are dropped, and the
/// parent's core links to blocks that moved are replaced by links from the
/// new core where the game's rules have the core control that block.
///
/// Refuses, leaving the blueprint as it was, for a box that does not fit
/// that, one that holds a rail or turret axis that has something docked to
/// it (or the parent's own docker), a links file that could not be read, and
/// a meta file that holds positions but is of an old version, has docks of
/// the old kind or cannot be read.
///
/// The parent's manager tag (storage contents, display text, computer
/// settings, kept by position) is dropped when the cut takes a block that
/// could have such data, and left alone otherwise: it holds nothing about
/// plain hull. The new entity starts without a manager tag and without AI
/// settings.
pub fn make_turret(bp: &mut Blueprint, catalog: &Catalog, path: &str, min: Pos, max: Pos, base: usize) -> Result<Made> {
    make_turret_at(bp, catalog, path, min, max, base, None, &BTreeSet::new())
}

/// The cell of the box the rail docker takes when none is chosen: the
/// middle of the base face. `None` where `base` is not a side.
pub fn default_docker(min: Pos, max: Pos, base: usize) -> Option<Pos> {
    let dir = *SIDE_DIRS.get(base)?;
    let (l, h) = (min.lo(max).to_array(), min.hi(max).to_array());
    let mut cell = [0; 3];
    for i in 0..3 {
        cell[i] = match dir[i] {
            1 => h[i],
            -1 => l[i],
            _ => (l[i] + h[i]).div_euclid(2),
        };
    }
    Some(Pos::new(cell[0], cell[1], cell[2]))
}

/// `make_turret` with the rail docker in a cell of the base face the player
/// chose (`None` for the middle of it): the turret axis goes in the cell
/// outside it. Refuses a cell that is not on the base face. The blocks in
/// the cells `exclude` names stay where they are, on the entity the turret
/// is cut from (a ridge of the hull the box takes in by the way); the
/// docker's cell and the new core's may not be among them.
#[allow(clippy::too_many_arguments)]
pub fn make_turret_at(bp: &mut Blueprint, catalog: &Catalog, path: &str, min: Pos, max: Pos, base: usize, docker: Option<Pos>, exclude: &BTreeSet<Pos>) -> Result<Made> {
    if base > 5 {
        return Err(refuse(format!("there is no side {base}")));
    }
    let (lo, hi) = (min.lo(max), min.hi(max));
    let which = called(path);
    let parent = bp.entity(path).ok_or_else(|| refuse(format!("there is no entity {path:?}")))?;
    if parent.logic_unreadable {
        return Err(refuse(format!("the links of {which} are in a file this editor cannot read")));
    }

    // The geometry: the axis in the cell outside the middle of the base face,
    // the docker in the box cell beside it, the core one cell further.
    let dir = SIDE_DIRS[base];
    let axis = dir.iter().position(|d| *d != 0).expect("a side has a direction");
    let (l, h) = (lo.to_array(), hi.to_array());
    if h[axis] - l[axis] + 1 < 3 {
        return Err(refuse("the box must be at least three cells deep: room for the turret axis' neighbour, the core and something beyond"));
    }
    if l.iter().chain(h.iter()).any(|c| c.abs() > LIMIT / 2) {
        return Err(refuse("the box reaches too far from the core"));
    }
    let in_box = |p: Pos| p.lo(lo) == lo && p.hi(hi) == hi;
    // The cells the turret takes: the box, less those left out.
    let inside = |p: Pos| in_box(p) && !exclude.contains(&p);
    let docker_cell = match docker {
        Some(cell) => cell,
        None => default_docker(lo, hi, base).expect("the side was checked"),
    };
    let face = if dir[axis] > 0 { h[axis] } else { l[axis] };
    if !in_box(docker_cell) || docker_cell.to_array()[axis] != face {
        return Err(refuse("the rail docker must be on the side of the box that rests on the ship"));
    }
    let up = dir.map(|c| -c);
    let rail_cell = docker_cell.offset(dir[0], dir[1], dir[2]);
    let core_cell = docker_cell.offset(up[0], up[1], up[2]);
    if exclude.contains(&docker_cell) || exclude.contains(&core_cell) {
        return Err(refuse("the cells of the rail docker and of the turret's core above it cannot be left out"));
    }
    let origin = Pos::new(0, 0, 0);
    if inside(origin) || rail_cell == origin {
        return Err(refuse("the box would take the core of the ship"));
    }
    let mut cut: Vec<(Pos, Block)> = Vec::new();
    parent.entity.for_each(|p, b| {
        if inside(p) {
            cut.push((p, b));
        }
    });
    if cut.is_empty() {
        return Err(refuse("there are no blocks in the box"));
    }
    if cut.iter().any(|(_, b)| b.ty() == CORE_TYPE) {
        return Err(refuse("the box holds a ship core"));
    }
    cut.sort_by_key(|(p, _)| *p);
    // Cells whose blocks are replaced rather than moved.
    let replaced = |p: Pos| p == docker_cell || p == core_cell;
    // Cells that stop being what they were, which anything kept by position must forget.
    let gone = |p: Pos| inside(p) || p == rail_cell;

    // Everything that can fail goes first: the meta files and the dock
    // records they hold.
    let mut meta = read_meta(parent, &which)?;
    let version = meta.version;
    if let Some(rails) = meta.rails() {
        for (child, record) in rails.children.iter().filter(|(_, r)| !r.is_empty()) {
            let mut record = DockRecord::parse(record).map_err(|err| refuse(format!("the dock record of {child} cannot be read ({err})")))?;
            if gone(record.positions(version).0) {
                return Err(refuse(format!("{child} is docked to a block in the box or where the turret axis goes")));
            }
        }
    }
    if let Some((grand, dir)) = path.rsplit_once('/').or(if path.is_empty() { None } else { Some(("", path)) }) {
        let grandparent = bp.entity(grand).ok_or_else(|| refuse("its parent is missing"))?;
        let grand_meta = read_meta(grandparent, &called(grand))?;
        let record = grand_meta.rails().and_then(|r| r.children.iter().find(|(c, r)| c.rsplit('/').next() == Some(dir) && !r.is_empty()));
        if let Some((child, record)) = record {
            let mut record = DockRecord::parse(record).map_err(|err| refuse(format!("the dock record of {child} cannot be read ({err})")))?;
            if gone(record.positions(grand_meta.version).1) {
                return Err(refuse(format!("{which} is docked by a block in the box or where the turret axis goes")));
            }
        }
    }
    let parent_name = meta.rails().map_or(String::new(), |r| r.name.clone());
    // Wireless links other entities have to blocks that are leaving.
    let mut others: Vec<(String, MetaFile)> = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let (mut twin, mut dropped_wireless) = (false, 0);
    for (other_path, other) in bp.entities() {
        if other.meta.is_empty() || crate::meta::version(&other.meta).is_ok_and(|v| v < 2) {
            continue;
        }
        let mut m = if other_path == path {
            meta.clone()
        } else {
            MetaFile::parse(&other.meta).map_err(|err| refuse(format!("the meta file of {} cannot be read ({err})", called(&other_path))))?
        };
        if let Some(r) = m.rails() {
            names.insert(r.name.clone());
            twin |= other_path != path && r.name == parent_name;
        }
        if other_path != path {
            let n = m.drop_wireless(false, &parent_name, &gone);
            if n > 0 {
                dropped_wireless += n;
                others.push((other_path, m));
            }
        }
    }
    if twin && dropped_wireless > 0 {
        return Err(refuse(format!("wireless links name this entity as {parent_name:?}, a name another entity of the design shares")));
    }

    // The new entity's name among the docked ones, and its folder.
    let taken: BTreeSet<u32> = parent
        .children
        .iter()
        .filter_map(|c| attached_number(&c.dir))
        .chain(meta.rails().into_iter().flat_map(|r| r.children.iter().filter_map(|(c, _)| attached_number(c.rsplit('/').next()?))))
        .collect();
    let n = (0..).find(|n| !taken.contains(n)).expect("some number is free");
    let dir_name = format!("ATTACHED_{n}");
    let child_path = if path.is_empty() { dir_name.clone() } else { format!("{path}/{dir_name}") };
    let mut child_name = format!("{}{n}", if parent_name.is_empty() { "rl" } else { &parent_name });
    while names.contains(&child_name) {
        child_name.push('0');
    }

    // The two new blocks and the dock record.
    let rail_orientation = orientation_facing(up);
    let docker_orientation = facing_back(rail_orientation);
    let record = DockRecord::new_turret(
        Piece { entity: &format!("{}{parent_name}", bp.name), pos: rail_cell, ty: TURRET_AXIS, orientation: rail_orientation },
        Piece { entity: &format!("{}{child_name}", bp.name), pos: Pos::new(-up[0], -up[1], -up[2]), ty: RAIL_DOCKER, orientation: docker_orientation },
        docker_cell,
    );
    let old_rail_block = parent.entity.get(rail_cell);
    let rules = Rules::new(catalog, Kind::Ship);
    let to_child = |p: Pos| Pos::new(p.x - core_cell.x, p.y - core_cell.y, p.z - core_cell.z);

    // Nothing can fail from here on. First the links, while the blocks are in place.
    let mut child_entity = crate::Entity::new();
    let (mut moved_links, mut dropped_links, mut core_links, mut lost_core_links) = (0, 0, 0, 0);
    let all_links: Vec<(Pos, u16, Pos)> = parent
        .entity
        .links()
        .iter()
        .flat_map(|(c, groups)| groups.iter().flat_map(move |(ty, targets)| targets.iter().map(move |t| (*c, *ty, *t))))
        .collect();
    let parent = bp.entity_mut(path).expect("checked above");
    for (c, ty, t) in all_links {
        let (c_in, t_in) = (inside(c), inside(t));
        if !(c_in || t_in || c == rail_cell || t == rail_cell) {
            continue;
        }
        parent.entity.unlink(c, t, ty);
        if c_in && t_in && !replaced(c) && !replaced(t) {
            child_entity.link(to_child(c), to_child(t), ty);
            moved_links += 1;
        } else if c == origin && t_in && !replaced(t) {
            // The parent's core ran this block; the new core takes its place where the game would have it.
            if rules.core_takes(ty) {
                child_entity.link(origin, to_child(t), ty);
                core_links += 1;
            } else {
                lost_core_links += 1;
            }
        } else {
            dropped_links += 1;
        }
    }

    // The blocks: the box is cut out, the axis goes in, and the child gets
    // its blocks with its core at the origin.
    let mut manager_data = keeps_data(catalog, old_rail_block.ty()) && !old_rail_block.is_air();
    let mut replaced_blocks = 0;
    for &(p, b) in &cut {
        manager_data |= keeps_data(catalog, b.ty());
        parent.entity.set(p, Block::AIR);
        if replaced(p) {
            replaced_blocks += 1;
        } else {
            child_entity.set(to_child(p), b);
        }
    }
    child_entity.set(to_child(docker_cell), Block::new(RAIL_DOCKER, docker_orientation, false));
    child_entity.set(origin, Block::new(CORE_TYPE, 0, false));
    parent.entity.set(rail_cell, Block::new(TURRET_AXIS, rail_orientation, false));
    parent.header.refresh(&parent.entity);

    // The parent's meta: what the cut took goes to the child's, and the dock is recorded.
    let mut notes = Vec::new();
    let mut child_dockers = vec![metafile::docker_record(5, to_child(docker_cell), RAIL_DOCKER, docker_orientation)];
    for (mut r, p) in meta.take_dockers(&gone) {
        if !replaced(p) && inside(p) {
            metafile::move_docker(&mut r, version, to_child(p));
            child_dockers.push(r);
        }
    }
    let mut child_cargo = Vec::new();
    for (mut r, p) in meta.take_cargo(&gone) {
        if !replaced(p) && inside(p) {
            metafile::move_cargo(&mut r, version, to_child(p));
            child_cargo.push(r);
        }
    }
    dropped_wireless += meta.drop_wireless(true, &parent_name, &gone);
    if meta.rails().is_none() {
        meta.add_rails(Rails { head: [0; 24], name: parent_name.clone(), wireless: Vec::new(), children: Vec::new() });
    }
    meta.rails_mut().expect("added above").children.push((format!("{}/{child_path}", bp.name), record.write()));
    if manager_data && meta.has_manager_tag() {
        meta.tail = Tail::Finish;
        notes.push("Storage contents, display text and other block settings stored by position were reset on the part the turret was cut from.".to_string());
    }
    let parent = bp.entity_mut(path).expect("checked above");
    parent.meta = meta.write();
    let (game_version, modmappings) = (parent.header.game_version.clone(), parent.modmappings.clone());
    for (other_path, m) in others {
        bp.entity_mut(&other_path).expect("checked above").meta = m.write();
    }

    // The new entity.
    let game_version = if game_version.is_empty() { crate::blueprint::FALLBACK_GAME_VERSION.to_string() } else { game_version };
    let mut header = Blueprint::new_ship(&bp.name, &game_version).root.header;
    header.refresh(&child_entity);
    let child_meta = MetaFile {
        version: 5,
        sections: vec![
            Section::Legacy(Vec::new()),
            Section::Dockers(Some(child_dockers)),
            Section::Cargo(Some(child_cargo)),
            Section::Rails(Rails { head: [0; 24], name: child_name, wireless: Vec::new(), children: Vec::new() }),
        ],
        tail: Tail::Finish,
    };
    let blocks = child_entity.block_count();
    let child = BlueprintEntity {
        dir: dir_name,
        header,
        entity: child_entity,
        meta: child_meta.write(),
        modmappings,
        logic_raw: None,
        logic_unreadable: false,
        children: Vec::new(),
    };
    let parent = bp.entity_mut(path).expect("checked above");
    let at = parent.children.iter().position(|c| attached_number(&c.dir).is_some_and(|k| k > n)).unwrap_or(parent.children.len());
    parent.children.insert(at, child);

    notes.insert(
        0,
        format!(
            "Cut {} blocks out as {child_path}, a new docked entity of {blocks} blocks with its own ship core and a rail docker, on a turret axis placed on the {} side.",
            cut.len() - replaced_blocks,
            // +X is the pilot's left in StarMade, so the sides across X go by their axis.
            ["front", "back", "top", "bottom", "+X", "-X"][base]
        ),
    );
    if replaced_blocks > 0 {
        notes.push(format!("{replaced_blocks} of the blocks in the box were replaced by the rail docker and the core."));
    }
    if !old_rail_block.is_air() {
        notes.push("The block where the turret axis went was replaced.".to_string());
    }
    if moved_links > 0 || core_links > 0 {
        notes.push(format!("{moved_links} links moved with the blocks; {core_links} links from the ship core to its computers were made again from the new core."));
    }
    if dropped_links > 0 || lost_core_links > 0 {
        notes.push(format!("{} links that crossed the edge of the box or ended on replaced blocks were dropped.", dropped_links + lost_core_links));
    }
    if dropped_wireless > 0 {
        notes.push(format!("{dropped_wireless} wireless links to blocks that left were dropped."));
    }
    notes.push("The turret has no AI settings yet; set them in the game.".to_string());
    Ok(Made { child: child_path, notes })
}

/// Small ships to cut turrets from, shared with the tests of
/// [`crate::relocate`].
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const HULL: u16 = 598;
    pub const PLATE: u16 = 599;
    pub const COMPUTER: u16 = 6;
    pub const BARREL: u16 = 16;
    pub const WIRELESS: u16 = 668;

    const XML: &str = r#"<Config><Element>
      <General><Hulls>
        <Block name="Hull" textureId="1,1,1,1,1,1" type="HULL"><Placable>true</Placable></Block>
        <Block name="Plate" textureId="1,1,1,1,1,1" type="PLATE"><Placable>true</Placable></Block>
      </Hulls></General>
      <Ship><Weapons>
        <Block name="Ship Core" textureId="1,1,1,1,1,1" type="SHIP_CORE"><Controlling><Element>CANNON_COMPUTER</Element></Controlling></Block>
        <Block name="Cannon Computer" textureId="1,1,1,1,1,1" type="CANNON_COMPUTER">
          <ControlledBy><Element>SHIP_CORE</Element></ControlledBy>
          <Controlling><Element>CANNON_BARREL</Element></Controlling>
          <Placable>true</Placable><SystemBlock>true</SystemBlock>
        </Block>
        <Block name="Cannon Barrel" textureId="1,1,1,1,1,1" type="CANNON_BARREL">
          <ControlledBy><Element>CANNON_COMPUTER</Element></ControlledBy><Placable>true</Placable>
        </Block>
        <Block name="Wireless" textureId="1,1,1,1,1,1" type="WIRELESS"><Placable>true</Placable><LogicBlock>true</LogicBlock></Block>
      </Weapons></Ship>
    </Element></Config>"#;
    const PROPS: &str = "SHIP_CORE=1\nCANNON_COMPUTER=6\nCANNON_BARREL=16\nHULL=598\nPLATE=599\nWIRELESS=668\n";

    pub fn catalog() -> Catalog {
        Catalog::parse(XML, PROPS).unwrap()
    }

    /// A ship named "Test" with its core and a hull block (type `HULL`) in each of
    /// the cells `solid` lists.
    pub fn ship(solid: &[(Pos, Pos)]) -> Blueprint {
        let mut bp = Blueprint::new_ship("Test", "0.304.7_20260628_193755");
        for (lo, hi) in solid {
            for x in lo.x..=hi.x {
                for y in lo.y..=hi.y {
                    for z in lo.z..=hi.z {
                        bp.root.entity.set(Pos::new(x, y, z), Block::new(HULL, 0, false));
                    }
                }
            }
        }
        bp
    }

    /// Where the blocks of every entity are in the root's coordinates: (entity path, world cell) to the block.
    pub fn world(bp: &Blueprint) -> std::collections::BTreeMap<(String, Pos), u32> {
        let mut out = std::collections::BTreeMap::new();
        for (path, placement) in bp.layout(|_, d| d.placement()) {
            bp.entity(&path).unwrap().entity.for_each(|p, b| {
                out.insert((path.clone(), placement.cell(p).expect("on the grid")), b.0);
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::dock::Dock;

    /// A hull slab on the `base` side of a 3x3x3 box of plates, as the turret test ships.
    fn ship_with_box(base: usize) -> (Blueprint, Pos, Pos) {
        let d = SIDE_DIRS[base];
        let axis = d.iter().position(|c| *c != 0).unwrap();
        let (mut lo, mut hi) = ([9; 3], [13; 3]);
        (lo[axis], hi[axis]) = if d[axis] > 0 { (13, 15) } else { (7, 9) };
        let mut bp = ship(&[(Pos::new(lo[0], lo[1], lo[2]), Pos::new(hi[0], hi[1], hi[2]))]);
        let (min, max) = (Pos::new(10, 10, 10), Pos::new(12, 12, 12));
        for x in 10..=12 {
            for y in 10..=12 {
                for z in 10..=12 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 5, false));
                }
            }
        }
        (bp, min, max)
    }

    #[test]
    fn the_docker_can_go_anywhere_on_the_base_face_and_nowhere_else() {
        let catalog = catalog();
        let (mut moved, min, max) = ship_with_box(3);
        // The bottom face of the 3 by 3 by 3 box at 10..=12: a corner cell of it.
        let made = make_turret_at(&mut moved, &catalog, "", min, max, 3, Some(Pos::new(10, 10, 12)), &BTreeSet::new()).unwrap();
        let dock = &moved.docks()[made.child.as_str()];
        assert_eq!(dock.rail.pos, Pos::new(10, 9, 12), "the axis is under the chosen cell");
        assert_eq!(dock.placement().cell(Pos::new(0, 0, 0)), Some(Pos::new(10, 11, 12)), "the core is above the docker");
        // Not off the face, nor inside the box above it.
        for cell in [Pos::new(9, 10, 11), Pos::new(11, 11, 11)] {
            let (mut tried, _, _) = ship_with_box(3);
            assert!(make_turret_at(&mut tried, &catalog, "", min, max, 3, Some(cell), &BTreeSet::new()).is_err(), "{cell:?}");
        }
        assert_eq!(default_docker(min, max, 3), Some(Pos::new(11, 10, 11)));
    }

    #[test]
    fn the_docker_lands_in_front_of_the_axis_and_every_block_stays_in_its_cell_on_every_side() {
        let catalog = catalog();
        for (base, d) in SIDE_DIRS.into_iter().enumerate() {
            let (mut bp, min, max) = ship_with_box(base);
            let before = world(&bp);
            let made = make_turret(&mut bp, &catalog, "", min, max, base).unwrap();
            assert_eq!(made.child, "ATTACHED_0");
            let docks = bp.docks();
            let dock = &docks["ATTACHED_0"];
            let child = &bp.entity("ATTACHED_0").unwrap().entity;
            // The axis is outside the middle of the base face, the docker beside it, the core beyond.
            let axis = d.iter().position(|c| *c != 0).unwrap();
            let mut rail = [11; 3];
            rail[axis] = if d[axis] > 0 { 13 } else { 9 };
            let rail = Pos::new(rail[0], rail[1], rail[2]);
            assert_eq!(dock.rail.pos, rail);
            assert_eq!((dock.rail.ty, dock.docker.ty), (TURRET_AXIS, RAIL_DOCKER), "base {base}");
            let up = d.map(|c| -c);
            assert_eq!(turned_axes(dock.rail.orientation).0, up);
            assert_eq!(turned_axes(dock.docker.orientation).0, d, "the docker's arrow side faces the axis");
            assert_eq!(turned_axes(dock.rail.orientation).1, turned_axes(dock.docker.orientation).1, "arrows point the same way");
            let at_rest = dock.placement();
            assert_eq!(at_rest.turn, crate::dock::NO_TURN, "base {base}: nothing needs turning");
            assert_eq!(at_rest.cell(dock.docker.pos), Some(rail.offset(up[0], up[1], up[2])));
            assert_eq!(at_rest.cell(Pos::new(0, 0, 0)), Some(rail.offset(2 * up[0], 2 * up[1], 2 * up[2])), "the core is one cell beyond the docker");
            assert_eq!(dock.docker.pos, Pos::new(-up[0], -up[1], -up[2]));
            assert_eq!(child.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
            assert_eq!(child.get(dock.docker.pos), Block::new(RAIL_DOCKER, dock.docker.orientation, false));
            // 27 plates, two of them replaced by the docker and the core.
            assert_eq!(child.block_count(), 27);
            let parent = &bp.root.entity;
            assert_eq!(parent.get(rail), Block::new(TURRET_AXIS, dock.rail.orientation, false));
            assert_eq!(parent.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
            // Every block is where it was in the world, except the three that were replaced.
            let after = world(&bp);
            let changed: Vec<_> = before.keys().filter(|k| after.get(*k) != before.get(*k)).collect();
            assert_eq!(changed.len(), 28, "base {base}: the 27 cut cells and the axis cell changed owner or block");
            let moved: Vec<_> = after.iter().filter(|((e, _), _)| e == "ATTACHED_0").collect();
            assert_eq!(moved.len(), 27);
            for ((_, cell), block) in moved {
                let was = before[&(String::new(), *cell)];
                let (b, a) = (Block(*block), Block(was));
                if b.ty() == PLATE {
                    assert_eq!(b, a, "base {base} {cell:?}");
                }
            }
            // The record names the child the way existing ones name theirs, and the game's reading agrees.
            assert_eq!(crate::meta::children(&bp.root.meta).unwrap(), vec!["Test/ATTACHED_0"]);
            let records = crate::meta::dock_records(&bp.root.meta).unwrap();
            let read = Dock::read(&records[0].1, crate::meta::version(&bp.root.meta).unwrap()).unwrap().unwrap();
            assert_eq!(&read, dock);
            // The child's meta lists its docker.
            let child_meta = MetaFile::parse(&bp.entity("ATTACHED_0").unwrap().meta).unwrap();
            let Section::Dockers(Some(list)) = &child_meta.sections[1] else { panic!("no docker list") };
            assert_eq!(list.len(), 1);
            assert_eq!(list[0], metafile::docker_record(5, dock.docker.pos, RAIL_DOCKER, dock.docker.orientation));
            assert_eq!(child_meta.rails().unwrap().name, "rl0");
            // The whole thing survives being written and read.
            let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
            assert_eq!(again.entities().len(), 2);
            assert_eq!(again.docks()["ATTACHED_0"], *dock);
            assert_eq!(world(&again), after);
            assert_eq!(again.entity("ATTACHED_0").unwrap().entity.block_count(), 27);
            assert_eq!(again.root.header.bounds, bp.root.header.bounds);
        }
    }

    #[test]
    fn links_move_with_the_blocks_and_the_core_takes_its_computer_again() {
        let catalog = catalog();
        let (mut bp, min, max) = ship_with_box(3);
        // A computer and two barrels in the box, one barrel linked from outside it; the core runs the computer.
        let (computer, barrel, other, outside) = (Pos::new(10, 11, 10), Pos::new(11, 11, 10), Pos::new(12, 11, 10), Pos::new(11, 13, 11));
        let e = &mut bp.root.entity;
        e.set(computer, Block::new(COMPUTER, 0, false));
        e.set(barrel, Block::new(BARREL, 0, false));
        e.set(other, Block::new(BARREL, 0, false));
        e.set(outside, Block::new(COMPUTER, 0, false));
        e.link(Pos::new(0, 0, 0), computer, COMPUTER);
        e.link(Pos::new(0, 0, 0), outside, COMPUTER);
        e.link(computer, barrel, BARREL);
        e.link(computer, other, BARREL);
        e.link(outside, barrel, BARREL);
        let made = make_turret(&mut bp, &catalog, "", min, max, 3).unwrap();
        let core_cell = Pos::new(11, 11, 11);
        let t = |p: Pos| Pos::new(p.x - core_cell.x, p.y - core_cell.y, p.z - core_cell.z);
        let child = &bp.entity(&made.child).unwrap().entity;
        let mut links: Vec<_> = child
            .links()
            .iter()
            .flat_map(|(c, g)| g.iter().flat_map(move |(ty, ts)| ts.iter().map(move |x| (*c, *ty, *x))))
            .collect();
        links.sort();
        let mut want = vec![(Pos::new(0, 0, 0), COMPUTER, t(computer)), (t(computer), BARREL, t(barrel)), (t(computer), BARREL, t(other))];
        want.sort();
        assert_eq!(links, want, "the new core runs the computer; the computer runs the barrels");
        // The parent keeps only what never touched the box.
        let parent: Vec<_> = bp.root.entity.links().iter().flat_map(|(c, g)| g.iter().flat_map(move |(ty, ts)| ts.iter().map(move |x| (*c, *ty, *x)))).collect();
        assert_eq!(parent, vec![(Pos::new(0, 0, 0), COMPUTER, outside)]);
        assert!(made.notes.iter().any(|n| n.contains("3 links moved") || n.contains("2 links moved")), "{:?}", made.notes);
        assert!(made.notes.iter().any(|n| n.contains("dropped")), "{:?}", made.notes);
        // Written out, the links file is the one made from the entity, not a stale copy.
        let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
        assert_eq!(again.entity(&made.child).unwrap().entity.link_count(), 3);
        assert_eq!(again.root.entity.link_count(), 1);
    }

    #[test]
    fn a_turret_can_be_cut_from_a_docked_entity_and_gets_the_next_free_number() {
        let catalog = catalog();
        // A tall turret on top, from which a second one is cut.
        let mut bp = ship(&[(Pos::new(9, 5, 9), Pos::new(15, 9, 15))]);
        for x in 10..=14 {
            for y in 10..=14 {
                for z in 10..=16 {
                    bp.root.entity.set(Pos::new(x, y, z), Block::new(PLATE, 5, false));
                }
            }
        }
        let first = make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(14, 14, 16), 3).unwrap();
        assert_eq!(first.child, "ATTACHED_0");
        let before = world(&bp);
        // Its core is at (12, 12, 13) in the root's coordinates, so these cells in its own coordinates ...
        let made = make_turret(&mut bp, &catalog, "ATTACHED_0", Pos::new(-1, 0, 1), Pos::new(1, 2, 3), 0).unwrap();
        assert_eq!(made.child, "ATTACHED_0/ATTACHED_0");
        assert_eq!(bp.entities().len(), 3);
        let docks = bp.docks();
        assert_eq!(docks.len(), 2);
        let inner = &docks["ATTACHED_0/ATTACHED_0"];
        // The axis went into the empty cell outside the middle of the front face.
        assert_eq!(inner.rail.pos, Pos::new(0, 1, 4));
        assert_eq!(bp.entity("ATTACHED_0").unwrap().entity.get(Pos::new(0, 1, 4)).ty(), TURRET_AXIS);
        assert_eq!(crate::meta::children(&bp.entity("ATTACHED_0").unwrap().meta).unwrap(), vec!["Test/ATTACHED_0/ATTACHED_0"]);
        assert_eq!(crate::meta::children(&bp.root.meta).unwrap(), vec!["Test/ATTACHED_0"]);
        // Placed in the world, the inner turret's blocks are in the cells they were cut from.
        let after = world(&bp);
        let mut moved = 0;
        for ((e, cell), block) in &after {
            if e == "ATTACHED_0/ATTACHED_0" && Block(*block).ty() == PLATE {
                assert_eq!(Block(before[&("ATTACHED_0".to_string(), *cell)]), Block(*block));
                moved += 1;
            }
        }
        assert_eq!(moved, 25);
        // Its name among the docked entities follows its parent's.
        assert_eq!(MetaFile::parse(&bp.entity("ATTACHED_0/ATTACHED_0").unwrap().meta).unwrap().rails().unwrap().name, "rl00");
    }

    #[test]
    fn a_second_turret_gets_the_next_free_number_and_a_gap_is_filled() {
        let catalog = catalog();
        let mut bp = ship(&[(Pos::new(9, 5, 9), Pos::new(20, 9, 20))]);
        for (x, z) in [(10, 10), (17, 17), (13, 13)] {
            for (dx, dy, dz) in (0..27).map(|i| (i % 3, i / 3 % 3, i / 9)) {
                bp.root.entity.set(Pos::new(x + dx, 10 + dy, z + dz), Block::new(PLATE, 0, false));
            }
        }
        make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(12, 12, 12), 3).unwrap();
        let second = make_turret(&mut bp, &catalog, "", Pos::new(17, 10, 17), Pos::new(19, 12, 19), 3).unwrap();
        assert_eq!(second.child, "ATTACHED_1");
        // Taking the first one away again leaves a gap, which the next turret fills, in its place in the order.
        bp.root.children.remove(0);
        let mut meta = MetaFile::parse(&bp.root.meta).unwrap();
        meta.rails_mut().unwrap().children.remove(0);
        bp.root.meta = meta.write();
        let third = make_turret(&mut bp, &catalog, "", Pos::new(13, 10, 13), Pos::new(15, 12, 15), 3).unwrap();
        assert_eq!(third.child, "ATTACHED_0");
        assert_eq!(bp.root.children.iter().map(|c| c.dir.as_str()).collect::<Vec<_>>(), vec!["ATTACHED_0", "ATTACHED_1"]);
        assert_eq!(crate::meta::children(&bp.root.meta).unwrap(), vec!["Test/ATTACHED_1", "Test/ATTACHED_0"]);
        let names: Vec<_> = bp.entities().iter().map(|(_, e)| MetaFile::parse(&e.meta).unwrap().rails().map(|r| r.name.clone())).collect();
        assert_eq!(names, vec![Some(String::new()), Some("rl0".to_string()), Some("rl1".to_string())]);
    }

    #[test]
    fn the_manager_tag_goes_only_when_the_cut_takes_something_that_could_hold_data() {
        let catalog = catalog();
        let with_tag = |bp: &mut Blueprint| {
            bp.root.meta = MetaFile { version: 5, sections: vec![Section::Legacy(Vec::new())], tail: Tail::Manager(vec![1, 2, 3]) }.write();
        };
        // Plain hull and plates: the tag is untouched.
        let (mut bp, min, max) = ship_with_box(3);
        with_tag(&mut bp);
        let made = make_turret(&mut bp, &catalog, "", min, max, 3).unwrap();
        assert_eq!(MetaFile::parse(&bp.root.meta).unwrap().tail, Tail::Manager(vec![1, 2, 3]));
        assert!(!made.notes.iter().any(|n| n.contains("Storage contents")));
        // A computer in the box: its settings are by position, so the tag goes.
        let (mut bp, min, max) = ship_with_box(3);
        bp.root.entity.set(Pos::new(10, 11, 10), Block::new(COMPUTER, 0, false));
        with_tag(&mut bp);
        let made = make_turret(&mut bp, &catalog, "", min, max, 3).unwrap();
        assert_eq!(MetaFile::parse(&bp.root.meta).unwrap().tail, Tail::Finish);
        assert!(made.notes.iter().any(|n| n.contains("Storage contents")), "{:?}", made.notes);
    }

    #[test]
    fn dockers_cargo_and_wireless_modules_in_the_box_follow_their_blocks() {
        let catalog = catalog();
        let (mut bp, min, max) = ship_with_box(3);
        let file = |p: Pos| (p.x + 16, p.y + 16, p.z + 16);
        // A docker and a cargo block in the box, one of each outside it, one docker where the core will be.
        let (docker_in, cargo_in, wl_in, wl_out) = (Pos::new(10, 11, 10), Pos::new(12, 11, 12), Pos::new(10, 12, 12), Pos::new(9, 9, 9));
        let (docker_out, cargo_out, under_core) = (Pos::new(9, 9, 10), Pos::new(9, 10, 9), Pos::new(11, 11, 11));
        for p in [docker_in, docker_out, under_core] {
            bp.root.entity.set(p, Block::new(RAIL_DOCKER, 0, false));
        }
        for p in [wl_in, wl_out] {
            bp.root.entity.set(p, Block::new(WIRELESS, 0, false));
        }
        let docker = |p: Pos| metafile::docker_record(5, p, RAIL_DOCKER, 0);
        let cargo = |p: Pos| {
            let mut r = [7u8; 16];
            let f = file(p);
            r[..8].copy_from_slice(&metafile::pack(f.0, f.1, f.2).to_be_bytes());
            r
        };
        let pair = |entity: &str, to: Pos, from: Pos| {
            let (t, f) = (file(to), file(from));
            metafile::WirelessPair { entity: entity.to_string(), to: metafile::pack(t.0, t.1, t.2), from: metafile::pack(f.0, f.1, f.2) }
        };
        bp.root.meta = MetaFile {
            version: 5,
            sections: vec![
                Section::Legacy(Vec::new()),
                Section::Dockers(Some(vec![docker(docker_in), docker(docker_out), docker(under_core)])),
                Section::Cargo(Some(vec![cargo(cargo_in), cargo(cargo_out)])),
                Section::Rails(Rails {
                    head: [0; 24],
                    name: String::new(),
                    // One link inside the design, one from a module that leaves, one to a module that leaves.
                    wireless: vec![pair("", wl_out, wl_out), pair("", wl_in, wl_out), pair("far", Pos::new(1, 1, 1), wl_in)],
                    children: Vec::new(),
                }),
            ],
            tail: Tail::Finish,
        }
        .write();
        let made = make_turret(&mut bp, &catalog, "", min, max, 3).unwrap();
        let core_cell = Pos::new(11, 11, 11);
        let t = |p: Pos| Pos::new(p.x - core_cell.x, p.y - core_cell.y, p.z - core_cell.z);
        let parent = MetaFile::parse(&bp.root.meta).unwrap();
        let child = MetaFile::parse(&bp.entity(&made.child).unwrap().meta).unwrap();
        let list = |m: &MetaFile| -> Vec<Pos> {
            let Section::Dockers(Some(l)) = m.sections.iter().find(|s| matches!(s, Section::Dockers(_))).unwrap() else { panic!() };
            let mut out = l.iter().map(|r| Pos::new(i32::from_be_bytes(r[0..4].try_into().unwrap()) - 16, i32::from_be_bytes(r[4..8].try_into().unwrap()) - 16, i32::from_be_bytes(r[8..12].try_into().unwrap()) - 16)).collect::<Vec<_>>();
            out.sort();
            out
        };
        assert_eq!(list(&parent), vec![docker_out]);
        // The child's own docker first, then the one that came with the blocks; the one under the core is gone.
        let mut want = vec![t(docker_in), t(Pos::new(11, 10, 11))];
        want.sort();
        assert_eq!(list(&child), want);
        let Section::Cargo(Some(c)) = parent.sections.iter().find(|s| matches!(s, Section::Cargo(_))).unwrap() else { panic!() };
        assert_eq!(c.as_slice(), &[cargo(cargo_out)]);
        let Section::Cargo(Some(c)) = child.sections.iter().find(|s| matches!(s, Section::Cargo(_))).unwrap() else { panic!() };
        let moved = metafile::unpack(i64::from_be_bytes(c[0][..8].try_into().unwrap()));
        assert_eq!((c.len(), moved), (1, [t(cargo_in).x + 16, t(cargo_in).y + 16, t(cargo_in).z + 16]));
        assert_eq!(&c[0][8..], &[7; 8], "the rest of the record is carried over");
        // Of the three wireless links the one between modules that stay remains.
        assert_eq!(parent.rails().unwrap().wireless, vec![pair("", wl_out, wl_out)]);
        assert!(made.notes.iter().any(|n| n.contains("2 wireless links")), "{:?}", made.notes);
        // Every docker the parent still lists is a docker block.
        for p in list(&parent) {
            assert_eq!(bp.root.entity.get(p).ty(), RAIL_DOCKER);
        }
    }

    #[test]
    fn boxes_that_do_not_make_a_turret_are_refused_and_change_nothing() {
        let catalog = catalog();
        let (bp, min, max) = ship_with_box(3);
        let snapshot = |bp: &Blueprint| (bp.to_files().unwrap(), bp.entities().len());
        let want = snapshot(&bp);
        let mut bp = bp;
        let o = Pos::new(0, 0, 0);
        let cases: [(Pos, Pos, usize); 6] = [
            (min, Pos::new(12, 11, 12), 3),                    // two cells deep
            (min, max, 6),                                     // no such side
            (Pos::new(30, 30, 30), Pos::new(32, 32, 32), 3),   // nothing in it
            (Pos::new(-1, -1, -1), Pos::new(1, 1, 1), 3),      // the core is in it
            (Pos::new(-1, 1, -1), Pos::new(1, 3, 1), 3),       // the axis would go on the core
            (Pos::new(-1, -1, 1), Pos::new(1, 1, 3), 1),       // the axis would go on the core from the other side
        ];
        for (lo, hi, base) in cases {
            assert!(make_turret(&mut bp, &catalog, "", lo, hi, base).is_err(), "{lo:?} {hi:?} {base}");
            assert_eq!(snapshot(&bp), want);
        }
        assert!(make_turret(&mut bp, &catalog, "ATTACHED_9", min, max, 3).is_err());
        let _ = o;
        // Swapped corners are the same box.
        assert!(make_turret(&mut bp, &catalog, "", max, min, 3).is_ok());
    }

    #[test]
    fn a_rail_that_would_be_cut_away_with_something_docked_to_it_is_refused() {
        let catalog = catalog();
        let (mut bp, min, max) = ship_with_box(3);
        // Dock a turret to the middle of the plates first: then cutting the box around its axis would orphan it.
        make_turret(&mut bp, &catalog, "", Pos::new(10, 10, 10), Pos::new(12, 12, 12), 3).unwrap();
        let want = bp.to_files().unwrap();
        // A box around the hull slab that holds the axis.
        let r = make_turret(&mut bp, &catalog, "", Pos::new(10, 5, 10), Pos::new(12, 9, 12), 2);
        assert!(r.is_err());
        assert_eq!(bp.to_files().unwrap(), want);
        let _ = (min, max);
    }
    #[test]
    fn blocks_left_out_stay_on_the_ship_and_the_rest_go() {
        let catalog = catalog();
        let (mut bp, min, max) = ship_with_box(3);
        // A ridge along one side of the box, and a cell the docker needs.
        let ridge: BTreeSet<Pos> = (10..=12).map(|z| Pos::new(12, 12, z)).collect();
        let made = make_turret_at(&mut bp, &catalog, "", min, max, 3, None, &ridge).unwrap();
        for p in &ridge {
            assert_eq!(bp.root.entity.get(*p).ty(), PLATE, "{p:?} stays");
        }
        assert!(bp.root.entity.get(Pos::new(10, 12, 10)).is_air(), "the rest of the box goes");
        let child = &bp.entity(&made.child).unwrap().entity;
        // 27 cells, less the ridge, less the docker's and the core's cells (replaced), plus those two.
        assert_eq!(child.block_count(), 27 - 3);
        // The docker's cell cannot be left out.
        let (mut tried, _, _) = ship_with_box(3);
        let docker: BTreeSet<Pos> = [default_docker(min, max, 3).unwrap()].into();
        assert!(make_turret_at(&mut tried, &catalog, "", min, max, 3, None, &docker).is_err());
    }
}
