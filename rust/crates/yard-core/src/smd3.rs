//! `.smd3` region files: the block data of a blueprint.
//!
//! A region file holds up to 16x16x16 segments. Layout, all big-endian:
//!
//! ```text
//! 4 bytes     file version
//! 4096 x 4    index, one entry per segment position in the region:
//!             (i16 slot number starting at 1, u16 stored size); 0 = absent
//! then 49152-byte slots, in slot-number order, each holding one record:
//!   u8  segment version
//!   i64 timestamp
//!   i32 x, y, z     file-space position of the segment's first block
//!   u8  data type   1 = zlib block array, 2 = empty, 3 = one value for the
//!                   whole segment, 4 = palette bitmap, 5 = planet data
//!   type 1: i32 compressed size, then zlib data of 32768 blocks, 3 bytes each
//!           (little-endian within a block)
//!   type 3: i32 block value
//!   type 4: i32 shift (3, 4 or 5), i32 n, n x i32 palette values, then
//!           32768 >> shift words of i32, each packing 1 << shift blocks at
//!           32 >> shift bits per block
//!   zero padding to the slot size
//! ```
//!
//! Segments carry their own version. 6 is current and 5 reads the same. The
//! game converts older ones when it loads them, and so does this reader when it
//! is given the block list ([`BlockInfo`]): versions 3 and 4 have their hit
//! points reset, version 2 has another bit layout altogether (see
//! [`convert_v2`]). Versions 0 and 1 are from the 16-block-chunk era and never
//! valid in a `.smd3`.
//!
//! The index is what counts. The game finds segments through it alone, never by
//! scanning slots, and ignores the position stored inside a record. A slot the
//! index no longer points at can still hold the blocks of a segment that was
//! emptied long ago; reading those would bring deleted parts of a ship back.

use crate::bytes::{Reader, Writer};
use crate::catalog::BlockInfo;
use crate::entity::{Entity, Segment, SEG, SEG_VOLUME};
use crate::{Block, Error, Result};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::collections::BTreeMap;
use std::io::{Read, Write};

/// Segments along one edge of a region file.
pub const REGION: i32 = 16;
const INDEX_ENTRIES: usize = (REGION * REGION * REGION) as usize;
const HEADER_SIZE: usize = 4 + INDEX_ENTRIES * 4;
const SLOT_SIZE: usize = 49152;
const SEGMENT_HEADER: usize = 26;
/// What current game versions write at the start of the file.
pub const FILE_VERSION: [u8; 4] = [6, 0, 0, 0];
/// The segment version whose block layout this crate reads and writes.
pub const SEGMENT_VERSION: u8 = 6;

const TYPE_ARRAY: u8 = 1;
const TYPE_EMPTY: u8 = 2;
const TYPE_SINGLE: u8 = 3;
const TYPE_BITMAP: u8 = 4;
const TYPE_PLANET: u8 = 5;

/// What a region file held besides readable segments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    /// Segments saved by an older game version and left as stored, because no
    /// block list was given to convert them with. The game converts them on load.
    pub old_segments: usize,
    /// Segments saved by an older game version and converted to the current
    /// layout, as the game does on load.
    pub converted_segments: usize,
    /// Segments in a form this crate cannot read (planet data).
    pub skipped_segments: usize,
}

impl ReadReport {
    pub fn add(&mut self, other: ReadReport) {
        self.old_segments += other.old_segments;
        self.converted_segments += other.converted_segments;
        self.skipped_segments += other.skipped_segments;
    }
}

fn malformed(detail: String) -> Error {
    Error::Malformed { what: "smd3 segment", detail }
}

/// A version 2 block, converted the way the game converts it
/// (`SegmentData3Byte.migrateTo`). The three bytes are big-endian: type in bits
/// 0-10, eight bits of hit points from bit 11, the active flag stored inverted
/// at bit 19 and four bits of orientation from bit 20. Blocks that turn 24 ways
/// (corners, tetras, heptas, rails) used the active flag as a fifth orientation
/// bit. Every cell is converted, empty ones included.
pub fn convert_v2(bytes: [u8; 3], blocks: &dyn BlockInfo) -> Block {
    let v = (bytes[0] as u32) << 16 | (bytes[1] as u32) << 8 | bytes[2] as u32;
    let ty = v & 0x7FF;
    let active = (v >> 19) & 1 == 0;
    let mut orientation = (v >> 20) & 0xF;
    let style = blocks.style(ty as u16);
    if matches!(style, Some(2 | 4 | 5 | 6)) {
        orientation = (orientation + if active { 0 } else { 16 }) % 24;
    }
    // Known blocks come out at full hit points, like every old segment.
    let hp = if style.is_some() { Block::FULL_HP as u32 } else { ((v >> 11) & 0xFF).min(127) };
    Block(ty | hp << 11 | (active as u32) << 18 | orientation << 19)
}

/// Brings one freshly decoded segment up to the current layout. Returns false
/// if it was left as stored for want of a block list, which is only possible
/// where the stored layout is the current one.
fn convert(segment: &mut Segment, raw_v2: Option<&[u8]>, blocks: Option<&dyn BlockInfo>) -> Result<bool> {
    let Some(blocks) = blocks else {
        if raw_v2.is_some() {
            return Err(Error::Unsupported {
                what: "version 2 block data",
                detail: "it can only be converted with the game's block list at hand".into(),
            });
        }
        return Ok(false);
    };
    if let Some(raw) = raw_v2 {
        for (i, chunk) in raw.chunks_exact(3).enumerate() {
            segment.blocks[i] = convert_v2([chunk[0], chunk[1], chunk[2]], blocks);
        }
    } else {
        // Versions 3 and 4: the layout is current, hit points are reset.
        for block in segment.blocks.iter_mut() {
            if blocks.style(block.ty()).is_some() {
                block.set_hp(Block::FULL_HP);
            }
        }
    }
    segment.version = SEGMENT_VERSION;
    Ok(true)
}

/// Reads the segments of one region file into the entity. `region` is the
/// three numbers in the file's name (`Name.x.y.z.smd3`). With a block list,
/// segments from older game versions are converted as the game converts them;
/// without one they are kept exactly as stored.
pub fn read_region(
    data: &[u8],
    region: (i32, i32, i32),
    entity: &mut Entity,
    blocks: Option<&dyn BlockInfo>,
) -> Result<ReadReport> {
    if data.len() < HEADER_SIZE {
        return Err(Error::Truncated("smd3 header"));
    }
    let mut report = ReadReport::default();
    for i in 0..INDEX_ENTRIES {
        let entry = &data[4 + i * 4..8 + i * 4];
        let slot = i16::from_be_bytes([entry[0], entry[1]]);
        let size = u16::from_be_bytes([entry[2], entry[3]]) as usize;
        // No slot: never written. A slot but no size: emptied; whatever is
        // still in the slot is stale.
        if slot <= 0 || size == 0 {
            continue;
        }
        let start = HEADER_SIZE + (slot as usize - 1) * SLOT_SIZE;
        if start + SEGMENT_HEADER.min(size) > data.len() {
            return Err(Error::Truncated("smd3 segment"));
        }
        let i = i as i32;
        let key = (
            region.0 * REGION + i % REGION - REGION / 2,
            region.1 * REGION + (i / REGION) % REGION - REGION / 2,
            region.2 * REGION + i / (REGION * REGION) - REGION / 2,
        );
        let mut r = Reader::new(&data[start..(start + SLOT_SIZE).min(data.len())], "smd3 segment");
        let version = r.u8()?;
        let timestamp = r.i64()?;
        // The stored position is ignored, as the game does: the index decides.
        let _ = (r.i32()?, r.i32()?, r.i32()?);
        let data_type = r.u8()?;

        if !(2..=SEGMENT_VERSION).contains(&version) {
            return Err(Error::Unsupported { what: "segment version", detail: version.to_string() });
        }
        let mut segment = Segment::empty();
        // Version 5 reads exactly as 6 and the game treats it as such.
        segment.version = if version == 5 { SEGMENT_VERSION } else { version };
        segment.timestamp = timestamp;
        // Version 2 data, kept aside for conversion.
        let mut raw_v2: Option<Vec<u8>> = None;
        match data_type {
            0 | TYPE_EMPTY => continue,
            TYPE_SINGLE | TYPE_BITMAP if version == 2 => {
                // The game itself throws on these.
                return Err(malformed(format!("data type {data_type} in a version 2 segment")));
            }
            TYPE_ARRAY => {
                let compressed = r.i32()?;
                if compressed < 0 || compressed as usize > r.remaining() {
                    return Err(malformed(format!("compressed size {compressed}")));
                }
                let mut raw = Vec::with_capacity(SEG_VOLUME * 3);
                ZlibDecoder::new(r.take(compressed as usize)?)
                    .read_to_end(&mut raw)
                    .map_err(|e| Error::Malformed { what: "smd3 segment data", detail: e.to_string() })?;
                if raw.len() != SEG_VOLUME * 3 {
                    // Other block sizes belong to formats this crate does not know.
                    return Err(Error::Unsupported {
                        what: "smd3 block encoding",
                        detail: format!("segment holds {} bytes, expected {}", raw.len(), SEG_VOLUME * 3),
                    });
                }
                for (i, chunk) in raw.chunks_exact(3).enumerate() {
                    segment.blocks[i] = Block::from_bytes([chunk[0], chunk[1], chunk[2]]);
                }
                if version == 2 {
                    raw_v2 = Some(raw);
                }
            }
            TYPE_SINGLE => {
                let value = Block(r.i32()? as u32 & 0xFF_FFFF);
                segment.blocks.fill(value);
            }
            TYPE_BITMAP => {
                let shift = r.i32()?;
                let entries = r.i32()?;
                if !(3..=5).contains(&shift) || !(0..=16).contains(&entries) {
                    return Err(malformed(format!("bitmap with shift {shift} and {entries} palette entries")));
                }
                let mut palette = [Block::AIR; 16];
                for slot in palette.iter_mut().take(entries as usize) {
                    *slot = Block(r.i32()? as u32 & 0xFF_FFFF);
                }
                let bits = 32u32 >> shift;
                let mask = (1u32 << bits) - 1;
                let per_word_mask = (1usize << shift) - 1;
                let mut words = Vec::with_capacity(SEG_VOLUME >> shift);
                for _ in 0..SEG_VOLUME >> shift {
                    words.push(r.i32()? as u32);
                }
                for (i, block) in segment.blocks.iter_mut().enumerate() {
                    let within = ((i & per_word_mask) as u32) << (5 - shift);
                    let index = (words[i >> shift] >> within) & mask;
                    *block = palette[index as usize & 15];
                }
            }
            TYPE_PLANET => {
                report.skipped_segments += 1;
                continue;
            }
            other => return Err(malformed(format!("unknown data type {other}"))),
        }
        if segment.version < SEGMENT_VERSION {
            if convert(&mut segment, raw_v2.as_deref(), blocks)? {
                report.converted_segments += 1;
            } else {
                report.old_segments += 1;
            }
        }
        entity.insert_segment(key, segment);
    }
    Ok(report)
}

/// Region coordinate and index slot of a segment. Regions are centred on the
/// core: region 0 holds segment coordinates -8..=7 on each axis.
fn region_of(key: (i32, i32, i32)) -> ((i32, i32, i32), usize) {
    let shifted = (key.0 + REGION / 2, key.1 + REGION / 2, key.2 + REGION / 2);
    let region = (shifted.0.div_euclid(REGION), shifted.1.div_euclid(REGION), shifted.2.div_euclid(REGION));
    let slot = shifted.0.rem_euclid(REGION) + shifted.1.rem_euclid(REGION) * REGION + shifted.2.rem_euclid(REGION) * REGION * REGION;
    (region, slot as usize)
}

/// Writes the entity's blocks as region files, keyed by region coordinate (the
/// three numbers in `Name.x.y.z.smd3`). Empty segments are left out.
pub fn write_regions(entity: &Entity) -> Result<BTreeMap<(i32, i32, i32), Vec<u8>>> {
    let mut by_region: BTreeMap<(i32, i32, i32), Vec<((i32, i32, i32), usize, &Segment)>> = BTreeMap::new();
    for (&key, segment) in entity.segments() {
        if segment.count == 0 {
            continue;
        }
        let (region, slot) = region_of(key);
        by_region.entry(region).or_default().push((key, slot, segment));
    }

    let mut out = BTreeMap::new();
    for (region, mut segments) in by_region {
        // Deterministic order so the same design always produces the same file.
        segments.sort_by_key(|&(_, slot, _)| slot);
        let mut index = vec![0u8; INDEX_ENTRIES * 4];
        let mut body = Vec::with_capacity(segments.len() * SLOT_SIZE);
        for (n, (key, slot, segment)) in segments.into_iter().enumerate() {
            let mut raw = Vec::with_capacity(SEG_VOLUME * 3);
            for b in segment.blocks.iter() {
                raw.extend_from_slice(&b.to_bytes());
            }
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&raw).map_err(|e| Error::Archive(e.to_string()))?;
            let compressed = encoder.finish().map_err(|e| Error::Archive(e.to_string()))?;
            if SEGMENT_HEADER + compressed.len() > SLOT_SIZE {
                return Err(Error::Unsupported {
                    what: "segment",
                    detail: format!("compresses to {} bytes, more than a slot holds", compressed.len()),
                });
            }

            let mut w = Writer::new();
            // The version says which block layout the data is in, so it goes back
            // out exactly as it came in.
            w.u8(segment.version);
            w.i64(segment.timestamp);
            w.i32(key.0 * SEG);
            w.i32(key.1 * SEG);
            w.i32(key.2 * SEG);
            w.u8(TYPE_ARRAY);
            w.i32(compressed.len() as i32);
            w.bytes(&compressed);
            let stored = w.out.len();
            w.out.resize(SLOT_SIZE, 0);
            body.extend_from_slice(&w.out);

            index[slot * 4..slot * 4 + 2].copy_from_slice(&((n + 1) as u16).to_be_bytes());
            index[slot * 4 + 2..slot * 4 + 4].copy_from_slice(&(stored as u16).to_be_bytes());
        }
        let mut file = Vec::with_capacity(HEADER_SIZE + body.len());
        file.extend_from_slice(&FILE_VERSION);
        file.extend_from_slice(&index);
        file.extend_from_slice(&body);
        out.insert(region, file);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;

    #[test]
    fn core_segment_sits_in_the_middle_slot_of_region_zero() {
        // Confirmed against a game-written file: segment (0,0,0) is index slot 2184.
        assert_eq!(region_of((0, 0, 0)), ((0, 0, 0), 2184));
        assert_eq!(region_of((0, 0, 1)), ((0, 0, 0), 2440));
        assert_eq!(region_of((-8, 0, 0)).0, (0, 0, 0));
        assert_eq!(region_of((-9, 0, 0)).0, (-1, 0, 0));
        assert_eq!(region_of((8, 0, 0)).0, (1, 0, 0));
    }

    #[test]
    fn written_regions_read_back_identically() {
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        e.set(Pos::new(-40, 3, 300), Block::new(598, 17, true));
        e.set(Pos::new(300, -300, -300), Block::new(5, 3, false));
        let files = write_regions(&e).unwrap();
        assert!(files.len() >= 2, "far-apart blocks land in different region files");

        let mut back = Entity::new();
        for (region, data) in &files {
            assert_eq!(read_region(data, *region, &mut back, None).unwrap(), ReadReport::default());
        }
        assert_eq!(back.block_count(), 3);
        let mut seen = Vec::new();
        back.for_each(|p, b| seen.push((p, b)));
        let mut want = Vec::new();
        e.for_each(|p, b| want.push((p, b)));
        seen.sort_by_key(|(p, _)| *p);
        want.sort_by_key(|(p, _)| *p);
        assert_eq!(seen, want);
    }

    /// One segment's record, padded to a slot.
    fn record(data_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(SEGMENT_VERSION);
        w.i64(0);
        // A deliberately wrong stored position: the index must win.
        w.i32(9999);
        w.i32(9999);
        w.i32(9999);
        w.u8(data_type);
        w.bytes(payload);
        w.out
    }

    /// A region file from (index entry, slot number, stored size, record) rows.
    fn region_file(rows: &[(usize, i16, Vec<u8>, bool)]) -> Vec<u8> {
        let mut file = FILE_VERSION.to_vec();
        let mut index = vec![0u8; INDEX_ENTRIES * 4];
        let slots = rows.iter().map(|r| r.1).max().unwrap_or(0) as usize;
        let mut body = vec![0u8; slots * SLOT_SIZE];
        for (entry, slot, record, listed) in rows {
            let at = (*slot as usize - 1) * SLOT_SIZE;
            body[at..at + record.len()].copy_from_slice(record);
            if *listed {
                index[entry * 4..entry * 4 + 2].copy_from_slice(&slot.to_be_bytes());
                index[entry * 4 + 2..entry * 4 + 4].copy_from_slice(&(record.len() as u16).to_be_bytes());
            }
        }
        file.extend_from_slice(&index);
        file.extend_from_slice(&body);
        file
    }

    #[test]
    fn the_index_decides_which_slots_are_read_and_where_they_go() {
        let hull = Block::new(5, 0, false);
        let single = record(TYPE_SINGLE, &(hull.0 as i32).to_be_bytes());
        let core_slot = region_of((0, 0, 0)).1;
        // Slot 1 is listed for the core segment; slot 2 holds data nothing points at.
        let file = region_file(&[(core_slot, 1, single.clone(), true), (core_slot + 1, 2, single, false)]);
        let mut e = Entity::new();
        read_region(&file, (0, 0, 0), &mut e, None).unwrap();
        assert_eq!(e.block_count(), SEG_VOLUME as u64, "the unlisted slot is stale and must be ignored");
        assert_eq!(e.get(Pos::new(0, 0, 0)), hull);
        assert_eq!(e.get(Pos::new(-16, -16, -16)), hull);
        assert_eq!(e.get(Pos::new(16, 0, 0)), Block::AIR);
        // The same file under another region name lands sixteen segments away.
        let mut far = Entity::new();
        read_region(&file, (1, 0, 0), &mut far, None).unwrap();
        assert_eq!(far.get(Pos::new(16 * 32, 0, 0)), hull);
    }

    #[test]
    fn palette_bitmaps_unpack_in_the_games_bit_order() {
        // Two bits per block (shift 4: sixteen blocks per word), palette of three.
        let palette = [Block::AIR, Block::new(5, 0, false), Block::new(77, 3, true)];
        let mut payload = Vec::new();
        payload.extend_from_slice(&4i32.to_be_bytes());
        payload.extend_from_slice(&3i32.to_be_bytes());
        for p in palette {
            payload.extend_from_slice(&(p.0 as i32).to_be_bytes());
        }
        // Word 0: block 0 -> palette 1, block 1 -> palette 2, block 15 -> palette 1.
        let word0: u32 = 1 | 2 << 2 | 1 << 30;
        payload.extend_from_slice(&word0.to_be_bytes());
        payload.extend(std::iter::repeat_n(0u8, ((SEG_VOLUME >> 4) - 1) * 4));
        let file = region_file(&[(region_of((0, 0, 0)).1, 1, record(TYPE_BITMAP, &payload), true)]);
        let mut e = Entity::new();
        read_region(&file, (0, 0, 0), &mut e, None).unwrap();
        assert_eq!(e.block_count(), 3);
        // Block index i is x + y*32 + z*1024 from the segment's corner at design -16.
        assert_eq!(e.get(Pos::new(-16, -16, -16)), palette[1]);
        assert_eq!(e.get(Pos::new(-15, -16, -16)), palette[2]);
        assert_eq!(e.get(Pos::new(-1, -16, -16)), palette[1]);
    }

    /// Styles for a handful of types: 5 a cube, 293 a wedge, 302 a corner.
    struct Styles;

    impl BlockInfo for Styles {
        fn style(&self, ty: u16) -> Option<u8> {
            match ty {
                5 => Some(0),
                293 => Some(1),
                302 => Some(2),
                _ => None,
            }
        }
    }

    /// A version 2 block as stored: big-endian, active flag inverted.
    fn v2(ty: u32, hp8: u32, active: bool, orientation: u32) -> [u8; 3] {
        let v = ty | hp8 << 11 | (!active as u32) << 19 | orientation << 20;
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    #[test]
    fn version_2_blocks_convert_as_the_game_converts_them() {
        // A cube keeps its orientation and comes out at full hit points.
        let cube = convert_v2(v2(5, 200, true, 4), &Styles);
        assert_eq!((cube.ty(), cube.hp(), cube.active_bit(), cube.orientation()), (5, 127, true, 4));
        // A wedge is not one of the 24-way styles: its orientation stays.
        assert_eq!(convert_v2(v2(293, 255, false, 7), &Styles).orientation(), 7);
        // A corner borrows the inverted active flag as a fifth orientation bit.
        assert_eq!(convert_v2(v2(302, 255, true, 9), &Styles).orientation(), 9);
        assert_eq!(convert_v2(v2(302, 255, false, 3), &Styles).orientation(), 19);
        assert_eq!(convert_v2(v2(302, 255, false, 12), &Styles).orientation(), 4, "wraps past 24");
        // A type the block list does not have keeps what it had, capped to 7 bits.
        let unknown = convert_v2(v2(1999, 90, false, 15), &Styles);
        assert_eq!((unknown.ty(), unknown.hp(), unknown.active_bit(), unknown.orientation()), (1999, 90, false, 15));
        assert_eq!(convert_v2(v2(1999, 200, true, 0), &Styles).hp(), 127);
        // An empty cell becomes air with the active bit set, still air.
        assert_eq!(convert_v2([0, 0, 0], &Styles), Block(0x04_0000));
        assert!(convert_v2([0, 0, 0], &Styles).is_air());
    }

    #[test]
    fn old_segments_are_converted_only_when_the_block_list_is_known() {
        // One version 4 segment holding a single damaged cube.
        let mut blocks = vec![0u8; SEG_VOLUME * 3];
        let mut damaged = Block::new(5, 2, false);
        damaged.set_hp(40);
        blocks[..3].copy_from_slice(&damaged.to_bytes());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&blocks).unwrap();
        let zipped = encoder.finish().unwrap();
        let mut payload = (zipped.len() as i32).to_be_bytes().to_vec();
        payload.extend_from_slice(&zipped);
        let mut rec = record(TYPE_ARRAY, &payload);
        rec[0] = 4;
        let file = region_file(&[(region_of((0, 0, 0)).1, 1, rec, true)]);
        let first = Pos::new(-16, -16, -16);

        let mut kept = Entity::new();
        let report = read_region(&file, (0, 0, 0), &mut kept, None).unwrap();
        assert_eq!((report.old_segments, report.converted_segments), (1, 0));
        assert_eq!(kept.get(first).hp(), 40, "left exactly as stored");
        assert_eq!(kept.segments().next().unwrap().1.version, 4, "and still marked as old");

        let mut converted = Entity::new();
        let report = read_region(&file, (0, 0, 0), &mut converted, Some(&Styles)).unwrap();
        assert_eq!((report.old_segments, report.converted_segments), (0, 1));
        assert_eq!(converted.get(first).hp(), 127);
        assert_eq!(converted.segments().next().unwrap().1.version, SEGMENT_VERSION);

        // Versions from the 16-block era and from the future are refused.
        for bad in [0u8, 1, 7] {
            let mut rec = record(TYPE_EMPTY, &[]);
            rec[0] = bad;
            let file = region_file(&[(region_of((0, 0, 0)).1, 1, rec, true)]);
            assert!(read_region(&file, (0, 0, 0), &mut Entity::new(), None).is_err(), "version {bad}");
        }
    }

    #[test]
    fn emptied_and_planet_segments_are_skipped() {
        let slot = region_of((0, 0, 0)).1;
        let file = region_file(&[(slot, 1, record(TYPE_EMPTY, &[]), true), (slot + 1, 2, record(TYPE_PLANET, &[0; 8]), true)]);
        let mut e = Entity::new();
        let report = read_region(&file, (0, 0, 0), &mut e, None).unwrap();
        assert_eq!(e.block_count(), 0);
        assert_eq!(report.skipped_segments, 1);
        let broken = region_file(&[(slot, 1, record(9, &[]), true)]);
        assert!(read_region(&broken, (0, 0, 0), &mut Entity::new(), None).is_err());
    }
}
