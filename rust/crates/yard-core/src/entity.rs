//! The blocks and links of one entity (a ship, a turret, a station).

use crate::Block;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Blocks along one edge of a segment.
pub const SEG: i32 = 32;
pub const SEG_VOLUME: usize = (SEG * SEG * SEG) as usize;
/// Files store positions with the ship core at (16,16,16). Everything in this
/// crate uses design space instead, where the core is the origin, +Z is forward,
/// +Y is up and +X is right.
pub const CORE_OFFSET: i32 = 16;

/// A block position in design space.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl serde::Serialize for Pos {
    /// Travels as `[x, y, z]`.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.x, self.y, self.z).serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for Pos {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (x, y, z) = <(i32, i32, i32)>::deserialize(d)?;
        Ok(Pos { x, y, z })
    }
}

impl Pos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The lower value on each axis: the low corner of the box two positions span.
    pub fn lo(self, o: Pos) -> Pos {
        Pos::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    /// The higher value on each axis.
    pub fn hi(self, o: Pos) -> Pos {
        Pos::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }

    pub fn to_array(self) -> [i32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.z + dz)
    }
}

/// A 32x32x32 cube of blocks. Indexed x + y*32 + z*1024, the order the game
/// stores them in.
#[derive(Clone)]
pub struct Segment {
    pub blocks: Box<[Block]>,
    /// Non-air blocks, kept up to date by [`Entity::set`].
    pub count: u32,
    /// The game's per-segment data version and last-change time, carried through
    /// unchanged when the file is written again.
    pub version: u8,
    pub timestamp: i64,
}

impl Segment {
    pub fn empty() -> Self {
        Self { blocks: vec![Block::AIR; SEG_VOLUME].into_boxed_slice(), count: 0, version: 6, timestamp: 0 }
    }

    pub fn index(lx: i32, ly: i32, lz: i32) -> usize {
        (lx + ly * SEG + lz * SEG * SEG) as usize
    }
}

/// Inclusive bounds in design space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    pub min: Pos,
    pub max: Pos,
}

impl Bounds {
    pub fn size(&self) -> (i32, i32, i32) {
        (self.max.x - self.min.x + 1, self.max.y - self.min.y + 1, self.max.z - self.min.z + 1)
    }
}

/// Controller position -> controlled block type -> controlled positions. This is
/// how StarMade records "this computer runs those modules" and every logic wire.
pub type Links = BTreeMap<Pos, BTreeMap<u16, BTreeSet<Pos>>>;

#[derive(Clone, Default)]
pub struct Entity {
    /// Keyed by segment coordinate: file-space block position divided by 32.
    segments: HashMap<(i32, i32, i32), Segment>,
    links: Links,
    /// Set once blocks or links differ from what was loaded. Unchanged parts of a
    /// blueprint are written back exactly as the game wrote them.
    blocks_changed: bool,
    links_changed: bool,
}

impl Entity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Splits a design-space position into (segment coordinate, index in segment).
    pub fn locate(p: Pos) -> ((i32, i32, i32), usize) {
        let (ax, ay, az) = (p.x + CORE_OFFSET, p.y + CORE_OFFSET, p.z + CORE_OFFSET);
        let key = (ax.div_euclid(SEG), ay.div_euclid(SEG), az.div_euclid(SEG));
        (key, Segment::index(ax.rem_euclid(SEG), ay.rem_euclid(SEG), az.rem_euclid(SEG)))
    }

    pub fn get(&self, p: Pos) -> Block {
        let (key, idx) = Self::locate(p);
        self.segments.get(&key).map_or(Block::AIR, |s| s.blocks[idx])
    }

    /// Writes a block; [`Block::AIR`] removes. Links are not touched here.
    pub fn set(&mut self, p: Pos, block: Block) {
        let (key, idx) = Self::locate(p);
        if block.is_air() && !self.segments.contains_key(&key) {
            return;
        }
        let seg = self.segments.entry(key).or_insert_with(Segment::empty);
        let was_air = seg.blocks[idx].is_air();
        let new = if block.is_air() { Block::AIR } else { block };
        if seg.blocks[idx] != new {
            self.blocks_changed = true;
        }
        seg.blocks[idx] = new;
        match (was_air, block.is_air()) {
            (true, false) => seg.count += 1,
            (false, true) => seg.count -= 1,
            _ => {}
        }
    }

    pub fn block_count(&self) -> u64 {
        self.segments.values().map(|s| s.count as u64).sum()
    }

    /// The design-space position of block `index` of segment `key`.
    pub fn position(key: (i32, i32, i32), index: usize) -> Pos {
        let i = index as i32;
        Pos::new(
            key.0 * SEG + i % SEG - CORE_OFFSET,
            key.1 * SEG + (i / SEG) % SEG - CORE_OFFSET,
            key.2 * SEG + i / (SEG * SEG) - CORE_OFFSET,
        )
    }

    pub fn segment(&self, key: (i32, i32, i32)) -> Option<&Segment> {
        self.segments.get(&key)
    }

    /// Segment coordinates and data, for file writing and meshing.
    pub fn segments(&self) -> impl Iterator<Item = (&(i32, i32, i32), &Segment)> {
        self.segments.iter()
    }

    /// Installs a whole segment as read from a file.
    pub fn insert_segment(&mut self, key: (i32, i32, i32), mut segment: Segment) {
        segment.count = segment.blocks.iter().filter(|b| !b.is_air()).count() as u32;
        self.segments.insert(key, segment);
    }

    /// Calls `f` for every non-air block.
    pub fn for_each(&self, mut f: impl FnMut(Pos, Block)) {
        for (&(sx, sy, sz), seg) in &self.segments {
            if seg.count == 0 {
                continue;
            }
            for (i, &b) in seg.blocks.iter().enumerate() {
                if b.is_air() {
                    continue;
                }
                f(Self::position((sx, sy, sz), i), b);
            }
        }
    }

    pub fn bounds(&self) -> Option<Bounds> {
        let mut out: Option<Bounds> = None;
        self.for_each(|p, _| {
            let b = out.get_or_insert(Bounds { min: p, max: p });
            b.min = b.min.lo(p);
            b.max = b.max.hi(p);
        });
        out
    }

    /// Number of blocks of each type.
    pub fn counts_by_type(&self) -> BTreeMap<u16, u32> {
        let mut out = BTreeMap::new();
        self.for_each(|_, b| *out.entry(b.ty()).or_insert(0) += 1);
        out
    }

    pub fn links(&self) -> &Links {
        &self.links
    }

    /// Returns whether the link is new.
    pub fn link(&mut self, controller: Pos, target: Pos, target_type: u16) -> bool {
        let added = self.links.entry(controller).or_default().entry(target_type).or_default().insert(target);
        self.links_changed |= added;
        added
    }

    /// Returns whether there was such a link.
    pub fn unlink(&mut self, controller: Pos, target: Pos, target_type: u16) -> bool {
        let Some(groups) = self.links.get_mut(&controller) else { return false };
        let Some(targets) = groups.get_mut(&target_type) else { return false };
        let removed = targets.remove(&target);
        if removed {
            self.links_changed = true;
            if targets.is_empty() {
                groups.remove(&target_type);
            }
            if groups.is_empty() {
                self.links.remove(&controller);
            }
        }
        removed
    }

    /// Removes the link from `controller` to `target` whatever type it was
    /// filed under, and returns that type.
    pub fn unlink_any(&mut self, controller: Pos, target: Pos) -> Option<u16> {
        let ty = self.links.get(&controller)?.iter().find(|(_, set)| set.contains(&target)).map(|(ty, _)| *ty)?;
        self.unlink(controller, target, ty).then_some(ty)
    }

    pub fn clear_links(&mut self) {
        self.links_changed |= !self.links.is_empty();
        self.links.clear();
    }

    pub fn blocks_changed(&self) -> bool {
        self.blocks_changed
    }

    pub fn links_changed(&self) -> bool {
        self.links_changed
    }

    /// Declares the current state to be the loaded one. Called after reading a file.
    pub fn mark_unchanged(&mut self) {
        self.blocks_changed = false;
        self.links_changed = false;
    }

    pub fn link_count(&self) -> usize {
        self.links.values().flat_map(|by_type| by_type.values()).map(|set| set.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_across_segment_borders_and_negative_space() {
        let mut e = Entity::new();
        let spots = [Pos::new(0, 0, 0), Pos::new(15, 15, 15), Pos::new(16, 0, 0), Pos::new(-17, -40, 100)];
        for (i, &p) in spots.iter().enumerate() {
            e.set(p, Block::new(100 + i as u16, 0, false));
        }
        for (i, &p) in spots.iter().enumerate() {
            assert_eq!(e.get(p).ty(), 100 + i as u16);
        }
        assert_eq!(e.block_count(), 4);
        assert_eq!(e.get(Pos::new(1, 0, 0)), Block::AIR);
        // 15 and 16 sit either side of a segment border: the core is at file position 16.
        assert_eq!(e.segments().count(), 3);
    }

    #[test]
    fn removing_updates_counts_and_bounds() {
        let mut e = Entity::new();
        e.set(Pos::new(-3, 0, 5), Block::new(5, 0, false));
        e.set(Pos::new(4, 2, -1), Block::new(5, 0, false));
        assert_eq!(e.bounds().unwrap(), Bounds { min: Pos::new(-3, 0, -1), max: Pos::new(4, 2, 5) });
        e.set(Pos::new(4, 2, -1), Block::AIR);
        assert_eq!(e.block_count(), 1);
        assert_eq!(e.bounds().unwrap().size(), (1, 1, 1));
        e.set(Pos::new(900, 900, 900), Block::AIR);
        assert_eq!(e.segments().count(), 1, "removing air must not allocate a segment");
    }
}
