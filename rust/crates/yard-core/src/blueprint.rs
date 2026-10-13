//! Blueprints: what the game exports as `.sment` and stores in its `blueprints/`
//! folder. A blueprint is a directory:
//!
//! ```text
//! Name/header.smbph         type, bounds, block counts
//! Name/logic.smbpl          links between blocks
//! Name/meta.smbpm           docking, AI and other entity settings
//! Name/modmappings.smbmm    ids of modded blocks
//! Name/DATA/Name.x.y.z.smd3 block data, one file per region
//! Name/ATTACHED_0/...       each docked entity, same layout, nested
//! ```
//!
//! and a `.sment` is that directory zipped.

use crate::bytes::{Reader, Writer};
use crate::catalog::BlockInfo;
use crate::entity::{Entity, Pos, CORE_OFFSET};
use crate::{smd3, Error, Result};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

/// Files by '/'-separated relative path.
pub type FileTree = BTreeMap<String, Vec<u8>>;

/// Marks a logic file whose positions are in current file space (core at 16).
const LOGIC_TAG_CURRENT: i32 = -1026;

/// The header tail of a new blueprint: one byte, "has no score table".
const NEW_HEADER_TAIL: &[u8] = &[0];
/// The smallest meta file the game accepts: version 5, then the tag that ends
/// it. Docks, rails, cargo and AI settings are all optional sections before it.
const NEW_META: &[u8] = &[0, 0, 0, 5, 1];
/// Longest name given to a blueprint here. The game's own commands stop at
/// this length.
pub const MAX_NAME: usize = 48;
/// Longest name accepted from the game. Its save dialog is not held to
/// [`MAX_NAME`], so blueprints with longer names exist.
pub const MAX_GAME_NAME: usize = 128;

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' '
}

/// Whether the game takes `name` for a blueprint: letters, digits, `_`, `-`
/// and spaces, with no space at either end (its `EntityRequest.isShipNameValid`),
/// and not absurdly long. A dot in particular is not allowed: the game drops
/// it from the name but not from the folder, and the two then disagree.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_GAME_NAME && name.chars().all(name_char) && !name.starts_with(' ') && !name.ends_with(' ')
}

/// The nearest name the game takes: anything it does not allow becomes a
/// space, runs of spaces become one, and the result is cut to length. Empty if
/// nothing usable is left.
pub fn game_name(raw: &str) -> String {
    let spaced: String = raw.chars().map(|c| if name_char(c) { c } else { ' ' }).collect();
    let words: Vec<&str> = spaced.split(' ').filter(|w| !w.is_empty()).collect();
    let cut: String = words.join(" ").chars().take(MAX_NAME).collect();
    cut.trim_end().to_string()
}

/// Written into new blueprints when the install does not say which build it is.
pub const FALLBACK_GAME_VERSION: &str = "0.304.7_20260628_193755";

/// Turns the contents of an install's `version.txt` (`0.304.7#20260628_193755`)
/// into the spelling blueprint headers use (`0.304.7_20260628_193755`).
pub fn header_game_version(version_txt: &str) -> String {
    let line = version_txt.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        FALLBACK_GAME_VERSION.to_string()
    } else {
        line.replace('#', "_")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Header layout version. Written back as read: 5 is current, 2 to 4 are
    /// older layouts with fewer fields.
    pub version: i32,
    /// Game build that saved the blueprint, e.g. `0.203.175_20250426_044402`.
    /// Only version 5 headers have it.
    pub game_version: String,
    /// 0 is a ship, 1 a shop, 2 a space station; then asteroids and planets.
    pub entity_type: i32,
    /// The role given in the game's catalog. Headers before version 3 have none.
    pub classification: i32,
    /// Min x,y,z then max x,y,z in design space; max is one past the last block.
    pub bounds: [f32; 6],
    /// Blocks per type, ascending by type.
    pub counts: Vec<(i16, i32)>,
    /// Whatever follows the count table, kept verbatim.
    pub tail: Vec<u8>,
}

impl Header {
    pub fn read(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "blueprint header");
        let version = r.i32()?;
        // Versions 0 and 1 belong to the pre-2017 format with 16-block chunks.
        if !(2..=5).contains(&version) {
            return Err(Error::Unsupported { what: "blueprint header version", detail: version.to_string() });
        }
        let game_version = if version >= 5 { r.utf()? } else { String::new() };
        let entity_type = r.i32()?;
        let classification = if version >= 3 { r.i32()? } else { 0 };
        let mut bounds = [0f32; 6];
        for b in &mut bounds {
            *b = r.f32()?;
        }
        let n = r.i32()?;
        if n < 0 || n as usize * 6 > r.remaining() {
            return Err(Error::Malformed { what: "blueprint header", detail: format!("block type count {n}") });
        }
        let mut counts = Vec::with_capacity(n as usize);
        for _ in 0..n {
            counts.push((r.i16()?, r.i32()?));
        }
        Ok(Self { version, game_version, entity_type, classification, bounds, counts, tail: r.rest().to_vec() })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.i32(self.version);
        if self.version >= 5 {
            w.utf(&self.game_version);
        }
        w.i32(self.entity_type);
        if self.version >= 3 {
            w.i32(self.classification);
        }
        for b in self.bounds {
            w.f32(b);
        }
        w.i32(self.counts.len() as i32);
        for &(ty, n) in &self.counts {
            w.i16(ty);
            w.i32(n);
        }
        w.bytes(&self.tail);
        w.out
    }

    /// Brings bounds and block counts in line with the entity's actual blocks.
    ///
    /// The game does not write a tight box: game-saved ships have one block of
    /// margin below the lowest block and two above the highest, so that is what
    /// is written here. (Docked entities in game files often carry a stale box,
    /// which is why an unchanged header is never recomputed.)
    pub fn refresh(&mut self, entity: &Entity) {
        self.counts = entity.counts_by_type().into_iter().map(|(t, n)| (t as i16, n as i32)).collect();
        if let Some(b) = entity.bounds() {
            self.bounds = [
                (b.min.x - 1) as f32, (b.min.y - 1) as f32, (b.min.z - 1) as f32,
                (b.max.x + 2) as f32, (b.max.y + 2) as f32, (b.max.z + 2) as f32,
            ];
        }
    }
}

/// One entity of a blueprint with everything needed to write it back.
#[derive(Clone)]
pub struct BlueprintEntity {
    /// Directory name: the blueprint name for the root, `ATTACHED_n` for docked entities.
    pub dir: String,
    pub header: Header,
    pub entity: Entity,
    /// Kept verbatim until the editor understands them.
    pub meta: Vec<u8>,
    pub modmappings: Vec<u8>,
    /// The logic file as loaded. Written back verbatim while links are unchanged.
    pub logic_raw: Option<Vec<u8>>,
    /// True when the logic file was in a format this crate cannot read; links
    /// then show as empty and the file is always written back verbatim.
    pub logic_unreadable: bool,
    /// Docked entities.
    pub children: Vec<BlueprintEntity>,
}

#[derive(Clone)]
pub struct Blueprint {
    pub name: String,
    pub root: BlueprintEntity,
    /// Things that loaded only partly, in words a player can read.
    pub warnings: Vec<String>,
    /// True when block data from an older game version was converted to the
    /// current layout while reading, so what is in memory is no longer what the
    /// file holds. Writing it out gives a current-format blueprint.
    pub converted: bool,
}

impl Blueprint {
    /// A new ship: just the core, as the game makes one. `game_version` is the
    /// build the blueprint claims to come from, in the header's own spelling
    /// (`0.304.7_20260628_193755`); see [`header_game_version`].
    pub fn new_ship(name: &str, game_version: &str) -> Self {
        let mut entity = Entity::new();
        entity.set(crate::Pos::new(0, 0, 0), crate::Block::new(crate::catalog::Catalog::CORE, 0, false));
        let mut header = Header {
            version: 5,
            game_version: game_version.to_string(),
            entity_type: 0,
            classification: 0,
            bounds: [0.0; 6],
            counts: Vec::new(),
            tail: NEW_HEADER_TAIL.to_vec(),
        };
        header.refresh(&entity);
        // Mark it changed so the first save writes everything from the entity.
        let root = BlueprintEntity {
            dir: name.to_string(),
            header,
            entity,
            meta: NEW_META.to_vec(),
            modmappings: Vec::new(),
            logic_raw: None,
            logic_unreadable: false,
            children: Vec::new(),
        };
        Self { name: name.to_string(), root, warnings: Vec::new(), converted: false }
    }

    /// Reads a `.sment` file as it is stored. Fine for anything saved by a
    /// current game; blueprints from older versions need [`Blueprint::from_sment_with`].
    pub fn from_sment(data: &[u8]) -> Result<Self> {
        Self::from_files_with(&unzip(data)?, None)
    }

    /// Reads a `.sment` file the way the game does, given the game's block list:
    /// block data from older versions is converted, and links to block types the
    /// game no longer has are dropped.
    pub fn from_sment_with(data: &[u8], blocks: &dyn BlockInfo) -> Result<Self> {
        Self::from_files_with(&unzip(data)?, Some(blocks))
    }

    pub fn from_files(files: &FileTree) -> Result<Self> {
        Self::from_files_with(files, None)
    }

    /// Reads a blueprint directory given as a file tree. The blueprint may sit at
    /// any depth; the shallowest `header.smbph` marks its root.
    pub fn from_files_with(files: &FileTree, blocks: Option<&dyn BlockInfo>) -> Result<Self> {
        let header_path = files
            .keys()
            .filter(|p| p.ends_with("header.smbph"))
            .min_by_key(|p| p.matches('/').count())
            .ok_or_else(|| Error::NotABlueprint("no header.smbph inside".into()))?;
        let dir = header_path.strip_suffix("header.smbph").unwrap().trim_end_matches('/');
        let name = dir.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("Blueprint").to_string();
        let mut warnings = Vec::new();
        let mut converted = false;
        let root = read_entity(files, dir, &name, blocks, &mut warnings, &mut converted)?;
        if converted {
            warnings.push("Saved by an older version of the game; converted to the current format the way the game does when it loads it.".to_string());
        }
        Ok(Self { name, root, warnings, converted })
    }

    /// Gives the blueprint another name. The name is also the folder the game
    /// keeps the blueprint in, and every parent names its docked entities by
    /// paths that start with it, so those are rewritten to match.
    pub fn rename(&mut self, name: &str) {
        fn walk(e: &mut BlueprintEntity, root: &str, stuck: &mut bool) {
            match crate::meta::with_root(&e.meta, root) {
                Ok(meta) => e.meta = meta,
                // A meta file that cannot be read matters only if it names children.
                Err(_) => *stuck |= !e.children.is_empty(),
            }
            for child in &mut e.children {
                walk(child, root, stuck);
            }
        }
        let mut stuck = false;
        walk(&mut self.root, name, &mut stuck);
        if stuck {
            self.warnings.push(format!(
                "Renamed from \"{}\", but its docked entities are filed under the old name in a format that could not be rewritten. The game will not find them.",
                self.name
            ));
        }
        self.name = name.to_string();
        self.root.dir = name.to_string();
    }

    /// The blueprint as a directory tree, rooted at its name.
    pub fn to_files(&self) -> Result<FileTree> {
        let mut out = FileTree::new();
        write_entity(&self.root, &self.name, &mut out)?;
        Ok(out)
    }

    pub fn to_sment(&self) -> Result<Vec<u8>> {
        zip_up(&self.to_files()?)
    }

    /// The entity a path from [`Blueprint::entities`] names.
    pub fn entity_mut(&mut self, path: &str) -> Option<&mut BlueprintEntity> {
        let mut entity = &mut self.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            entity = entity.children.iter_mut().find(|c| c.dir == part)?;
        }
        Some(entity)
    }

    pub fn entity(&self, path: &str) -> Option<&BlueprintEntity> {
        let mut entity = &self.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            entity = entity.children.iter().find(|c| c.dir == part)?;
        }
        Some(entity)
    }

    /// The root entity followed by every docked entity, depth first, with the
    /// path that names each one ("" for the root, "ATTACHED_0/ATTACHED_1"...).
    /// How each docked entity is docked, by its path ("ATTACHED_0",
    /// "ATTACHED_0/ATTACHED_1"...). An entity is left out if its parent keeps
    /// no rail dock for it: one docked the way the game docked before rails,
    /// or a leftover folder the game itself would ignore.
    pub fn docks(&self) -> std::collections::BTreeMap<String, crate::dock::Dock> {
        let mut out = std::collections::BTreeMap::new();
        for (path, e) in self.entities() {
            let (Ok(version), Ok(records)) = (crate::meta::version(&e.meta), crate::meta::dock_records(&e.meta)) else { continue };
            for (child, record) in records {
                // The meta names a child by its path below the catalog folder;
                // the last part is its folder.
                let Some(dir) = child.rsplit('/').next().filter(|d| e.children.iter().any(|c| c.dir == *d)) else { continue };
                if let Ok(Some(dock)) = crate::dock::Dock::read(&record, version) {
                    out.insert(if path.is_empty() { dir.to_string() } else { format!("{path}/{dir}") }, dock);
                }
            }
        }
        out
    }

    /// Where every entity sits in the root entity's coordinates, by path, the
    /// root itself included. `pose` gives each dock's placement on its
    /// parent: [`crate::dock::Dock::placement`] for the pose the blueprint
    /// was saved in. Entities with no dock on record, and everything docked
    /// to them, are left out.
    pub fn layout(&self, pose: impl Fn(&str, &crate::dock::Dock) -> crate::dock::Placement) -> Vec<(String, crate::dock::Placement)> {
        let docks = self.docks();
        let mut placed: Vec<(String, crate::dock::Placement)> = vec![(String::new(), crate::dock::Placement::SAME)];
        // Paths come parents first.
        for (path, _) in self.entities().into_iter().skip(1) {
            let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            let (Some(dock), Some((_, on_parent))) = (docks.get(&path), placed.iter().find(|(p, _)| p == parent)) else { continue };
            let here = on_parent.then(&pose(&path, dock));
            placed.push((path, here));
        }
        placed
    }

    /// The pairs of wireless logic modules that speak to each other within
    /// the design: (entity, module) to (entity, module), with entities
    /// counted in the order [`Blueprint::entities`] lists them.
    pub fn wireless(&self) -> Vec<((usize, crate::Pos), (usize, crate::Pos))> {
        let read: Vec<_> = self.entities().iter().map(|(_, e)| crate::meta::wireless(&e.meta).unwrap_or_default()).collect();
        let mut out = Vec::new();
        for (from_entity, (_, pairs)) in read.iter().enumerate() {
            for (from, other, to) in pairs {
                if let Some(to_entity) = read.iter().position(|(name, _)| name == other) {
                    out.push(((from_entity, *from), (to_entity, *to)));
                }
            }
        }
        out
    }

    /// The blocks of every entity, to be changed in place, in the order
    /// [`Blueprint::entities`] lists them.
    pub fn blocks_mut(&mut self) -> Vec<&mut Entity> {
        fn walk<'a>(e: &'a mut BlueprintEntity, out: &mut Vec<&'a mut Entity>) {
            out.push(&mut e.entity);
            for c in &mut e.children {
                walk(c, out);
            }
        }
        let mut out = Vec::new();
        walk(&mut self.root, &mut out);
        out
    }

    pub fn entities(&self) -> Vec<(String, &BlueprintEntity)> {
        fn walk<'a>(e: &'a BlueprintEntity, path: String, out: &mut Vec<(String, &'a BlueprintEntity)>) {
            out.push((path.clone(), e));
            for c in &e.children {
                let child = if path.is_empty() { c.dir.clone() } else { format!("{path}/{}", c.dir) };
                walk(c, child, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.root, String::new(), &mut out);
        out
    }
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_string() } else { format!("{dir}/{rest}") }
}

fn read_entity(
    files: &FileTree,
    dir: &str,
    dir_name: &str,
    blocks: Option<&dyn BlockInfo>,
    warnings: &mut Vec<String>,
    converted: &mut bool,
) -> Result<BlueprintEntity> {
    let header_bytes = files
        .get(&join(dir, "header.smbph"))
        .ok_or_else(|| Error::NotABlueprint(format!("{dir_name} has no header.smbph")))?;
    let header = Header::read(header_bytes)?;

    let mut entity = Entity::new();
    let data_prefix = join(dir, "DATA/");
    // Region files are named <stem>.<x>.<y>.<z>.smd3. A DATA folder can hold
    // leftovers under a second stem (the entity's in-world id) from an earlier
    // save; the game reads the set named after the directory, so prefer that.
    let mut by_stem: BTreeMap<&str, Vec<((i32, i32, i32), &Vec<u8>)>> = BTreeMap::new();
    let mut old_format = false;
    for (path, data) in files.range(data_prefix.clone()..) {
        let Some(file) = path.strip_prefix(&data_prefix) else { break };
        if let Some(rest) = file.strip_suffix(".smd3") {
            // <stem>.<x>.<y>.<z>: the numbers say which region of space the file holds.
            let mut parts = rest.rsplitn(4, '.');
            let numbers = [parts.next(), parts.next(), parts.next()].map(|p| p.and_then(|n| n.parse::<i32>().ok()));
            let (Some(stem), [Some(z), Some(y), Some(x)]) = (parts.next(), numbers) else {
                warnings.push(format!("{dir_name}: ignored the block data file \"{file}\", whose name does not say where it belongs."));
                continue;
            };
            by_stem.entry(stem).or_default().push(((x, y, z), data));
        } else if file.ends_with(".smd2") {
            old_format = true;
        }
    }
    let chosen = if by_stem.contains_key(dir_name) {
        by_stem.get(dir_name)
    } else {
        if by_stem.len() > 1 {
            warnings.push(format!(
                "{dir_name}: found block data under several names and none matches; using the largest set."
            ));
        }
        by_stem.values().max_by_key(|files| files.len())
    };
    let mut report = smd3::ReadReport::default();
    for (region, data) in chosen.into_iter().flatten() {
        report.add(smd3::read_region(data, *region, &mut entity, blocks)?);
    }
    *converted |= report.converted_segments > 0;
    if report.old_segments > 0 {
        warnings.push(format!(
            "{dir_name}: saved by an older version of the game and shown as stored. The game resets hit points when it loads it."
        ));
    }
    if report.skipped_segments > 0 {
        warnings.push(format!("{dir_name}: {} parts hold planet data, which is not supported and was left out.", report.skipped_segments));
    }
    if old_format {
        warnings.push(format!(
            "{dir_name}: block data is in the pre-2017 .smd2 format, which is not supported yet. Load it in the game and save it again to convert."
        ));
    }

    let mut logic_raw = files.get(&join(dir, "logic.smbpl")).cloned();
    let mut logic_unreadable = false;
    if let Some(logic) = &logic_raw {
        match read_logic(logic, &mut entity, blocks) {
            // Old layout or links to block types the game no longer has: the
            // stored file no longer says what was read, so it is regenerated.
            Ok(true) => {
                logic_raw = None;
                *converted = true;
            }
            Ok(false) => {}
            Err(e) => {
                warnings.push(format!("{dir_name}: links could not be read ({e}); they are kept as they are."));
                entity.clear_links();
                logic_unreadable = true;
            }
        }
    }
    entity.mark_unchanged();

    // Docked entities live in ATTACHED_<n> directories directly below this one.
    let mut child_dirs: Vec<(u32, String)> = Vec::new();
    let attached_prefix = join(dir, "ATTACHED_");
    for path in files.range(attached_prefix.clone()..).map(|(p, _)| p) {
        if !path.starts_with(&attached_prefix) {
            break;
        }
        let rest = &path[attached_prefix.len()..];
        if let Some((n, tail)) = rest.split_once('/') {
            if tail == "header.smbph" {
                if let Ok(n) = n.parse::<u32>() {
                    child_dirs.push((n, format!("ATTACHED_{n}")));
                }
            }
        }
    }
    child_dirs.sort();
    let mut children = Vec::new();
    for (_, child) in child_dirs {
        children.push(read_entity(files, &join(dir, &child), &child, blocks, warnings, converted)?);
    }

    Ok(BlueprintEntity {
        dir: dir_name.to_string(),
        header,
        entity,
        meta: files.get(&join(dir, "meta.smbpm")).cloned().unwrap_or_default(),
        modmappings: files.get(&join(dir, "modmappings.smbmm")).cloned().unwrap_or_default(),
        logic_raw,
        logic_unreadable,
        children,
    })
}

fn write_entity(e: &BlueprintEntity, dir: &str, out: &mut FileTree) -> Result<()> {
    let mut header = e.header.clone();
    if e.entity.blocks_changed() {
        header.refresh(&e.entity);
    }
    out.insert(join(dir, "header.smbph"), header.write());
    let logic = match &e.logic_raw {
        Some(raw) if e.logic_unreadable || !e.entity.links_changed() => raw.clone(),
        _ => write_logic(&e.entity),
    };
    out.insert(join(dir, "logic.smbpl"), logic);
    out.insert(join(dir, "meta.smbpm"), e.meta.clone());
    out.insert(join(dir, "modmappings.smbmm"), e.modmappings.clone());
    // Region files carry the name of the directory they belong to.
    let file_stem = dir.rsplit('/').next().unwrap_or(dir);
    for ((x, y, z), data) in smd3::write_regions(&e.entity)? {
        out.insert(join(dir, &format!("DATA/{file_stem}.{x}.{y}.{z}.smd3")), data);
    }
    for child in &e.children {
        write_entity(child, &join(dir, &child.dir), out)?;
    }
    Ok(())
}

/// Logic file layout:
///
/// ```text
/// i32 version
/// i32 tag (negative; -1026 = positions in current file space)
/// i32 controller count
/// per controller: i16 x,y,z, i32 group count
///   per group: i16 controlled block type, i32 n, n times i16 x,y,z
/// ```
///
/// The oldest files have no tag: the controller count stands in its place, and
/// positions are eight blocks lower on each axis, from when the core sat at
/// (8,8,8). Tags -1 to -1024 mark a network form that blueprints do not use.
///
/// Given the block list, links to a block type the game does not have are left
/// out, as the game leaves them out (`ControlElementMapper.deserialize`). That
/// is the only thing it checks: not whether the blocks at either end exist.
///
/// Returns whether what was read differs from what the file says: an old
/// layout, or links left out. Such a file is written back in current form.
fn read_logic(data: &[u8], entity: &mut Entity, blocks: Option<&dyn BlockInfo>) -> Result<bool> {
    let mut rewritten = false;
    let mut r = Reader::new(data, "logic file");
    let _version = r.i32()?;
    let tag = r.i32()?;
    let (controllers, shift) = match tag {
        count if count >= 0 => (count, 8i16),
        disk if disk < -1024 => (r.i32()?, 0),
        other => return Err(Error::Unsupported { what: "logic file format", detail: format!("tag {other}") }),
    };
    // Positions are 16-bit and the shift wraps like the game's short arithmetic.
    let read_pos = |r: &mut Reader| -> Result<Pos> {
        let mut c = [0i32; 3];
        for v in &mut c {
            *v = r.i16()?.wrapping_add(shift) as i32 - CORE_OFFSET;
        }
        Ok(Pos::new(c[0], c[1], c[2]))
    };
    for _ in 0..controllers {
        let controller = read_pos(&mut r)?;
        let groups = r.i32()?;
        for _ in 0..groups {
            let ty = r.i16()? as u16;
            let n = r.i32()?;
            if n < 0 || n as usize * 6 > r.remaining() {
                return Err(Error::Malformed { what: "logic file", detail: format!("link count {n}") });
            }
            let known = ty > 0 && blocks.is_none_or(|b| b.style(ty).is_some());
            rewritten |= !known;
            for _ in 0..n {
                let target = read_pos(&mut r)?;
                if known {
                    entity.link(controller, target, ty);
                }
            }
        }
    }
    if r.remaining() != 0 {
        return Err(Error::Malformed { what: "logic file", detail: format!("{} trailing bytes", r.remaining()) });
    }
    // Only a reading with the block list is the game's reading; without it the
    // file is taken as it is and goes back out as it is.
    Ok(blocks.is_some() && (rewritten || shift != 0))
}

fn write_logic(entity: &Entity) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(0);
    w.i32(LOGIC_TAG_CURRENT);
    w.i32(entity.links().len() as i32);
    for (controller, groups) in entity.links() {
        write_pos(&mut w, *controller);
        w.i32(groups.len() as i32);
        for (ty, targets) in groups {
            w.i16(*ty as i16);
            w.i32(targets.len() as i32);
            for t in targets {
                write_pos(&mut w, *t);
            }
        }
    }
    w.out
}

fn write_pos(w: &mut Writer, p: Pos) {
    w.i16((p.x + CORE_OFFSET) as i16);
    w.i16((p.y + CORE_OFFSET) as i16);
    w.i16((p.z + CORE_OFFSET) as i16);
}

/// Unpacks a zip into a file tree, with a cap on total size so a small upload
/// cannot expand into gigabytes.
pub fn unzip(data: &[u8]) -> Result<FileTree> {
    const MAX_TOTAL: u64 = 2 * 1024 * 1024 * 1024;
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| Error::NotABlueprint(format!("not a zip file ({e})")))?;
    let mut out = FileTree::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| Error::Archive(e.to_string()))?;
        if file.is_dir() {
            continue;
        }
        total += file.size();
        if total > MAX_TOTAL {
            return Err(Error::Archive("archive expands to more than 2 GB".into()));
        }
        // Windows-made archives use backslashes.
        let name = file.name().replace('\\', "/").trim_start_matches('/').to_string();
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes).map_err(|e| Error::Archive(e.to_string()))?;
        out.insert(name, bytes);
    }
    Ok(out)
}

fn zip_up(files: &FileTree) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (path, data) in files {
        writer.start_file(path.as_str(), options).map_err(|e| Error::Archive(e.to_string()))?;
        writer.write_all(data).map_err(|e| Error::Archive(e.to_string()))?;
    }
    Ok(writer.finish().map_err(|e| Error::Archive(e.to_string()))?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_games_rule() {
        for good in ["Scout", "Isanth Type-PNR-25-B", "a", "x_1 - y", "9"] {
            assert!(valid_name(good), "{good}");
            assert_eq!(game_name(good), good);
        }
        assert!(valid_name(&"x".repeat(100)), "the game has names longer than the ones made here");
        for bad in ["", " Scout", "Scout ", "My.Ship", "a/b", "Ship (2)", "Schiff\u{e4}", &"x".repeat(129)] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(game_name("  My.Ship  v1.2 "), "My Ship v1 2");
        assert_eq!(game_name("../../etc"), "etc");
        assert_eq!(game_name("Ship (2)"), "Ship 2");
        assert_eq!(game_name("..."), "");
        let long = game_name(&format!("{} tail", "x".repeat(47)));
        assert_eq!(long, "x".repeat(47));
        assert!(valid_name(&long));
    }
}
