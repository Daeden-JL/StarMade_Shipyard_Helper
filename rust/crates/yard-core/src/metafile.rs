//! A whole meta file (`meta.smbpm`) as editable parts, for the changes that
//! have to touch positions in it. [`crate::meta`] says what each section
//! holds; this keeps every section as it is stored, in file order, so a file
//! read and written without changes comes back byte for byte.
//!
//! Positions in the sections that hold them count from a core at (16, 16, 16)
//! like those in the block data, in files from version 4 on.

use crate::bytes::{Reader, Writer};
use crate::entity::CORE_OFFSET;
use crate::{Error, Pos, Result};

/// One wireless link: the name of the entity at the other end, the module
/// there, and the module on this entity, both as the game packs positions.
#[derive(Clone, Debug, PartialEq)]
pub struct WirelessPair {
    pub entity: String,
    pub to: i64,
    pub from: i64,
}

/// Section 04: how the entity is a rail and what is docked to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Rails {
    /// Twenty-four bytes whose meaning is not known; kept as they are.
    pub head: [u8; 24],
    /// The name this entity goes by among the entities docked together.
    pub name: String,
    pub wireless: Vec<WirelessPair>,
    /// Each docked entity's path and the record of how it is docked.
    pub children: Vec<(String, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Section {
    /// 03: docked the old way; each a child path and 27 bytes.
    Legacy(Vec<(String, [u8; 27])>),
    /// 06: the rail docker blocks of this entity, if the section holds a list.
    Dockers(Option<Vec<[u8; 17]>>),
    /// 07: storage and factory blocks with cargo, if the section holds a list.
    Cargo(Option<Vec<[u8; 16]>>),
    Rails(Rails),
    /// 05: AI settings.
    Ai(Vec<u8>),
}

/// How the file ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Tail {
    /// 01: nothing more.
    Finish,
    /// 02: the manager tag, per-block data of the entity (inventories, text,
    /// settings), keyed by position and not understood here.
    Manager(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetaFile {
    pub version: i32,
    pub sections: Vec<Section>,
    pub tail: Tail,
}

/// A position as the game packs it into a long: three 16-bit fields, x lowest.
pub fn pack(x: i32, y: i32, z: i32) -> i64 {
    ((z as i64 & 0xffff) << 32) + ((y as i64 & 0xffff) << 16) + (x as i64 & 0xffff)
}

/// The three signed 16-bit fields of a packed position.
pub fn unpack(index: i64) -> [i32; 3] {
    [0, 16, 32].map(|bits| ((index >> bits) & 0xffff) as u16 as i16 as i32)
}

/// Where positions in a meta file of this version start counting.
pub fn offset(version: i32) -> i32 {
    if version < 4 { 8 } else { CORE_OFFSET }
}

fn count(r: &mut Reader) -> Result<usize> {
    let n = r.i32()?;
    usize::try_from(n).map_err(|_| Error::Malformed { what: "meta file", detail: format!("a count of {n}") })
}

impl MetaFile {
    pub fn parse(meta: &[u8]) -> Result<Self> {
        let mut r = Reader::new(meta, "meta file");
        let version = r.i32()?;
        if version < 2 {
            return Err(Error::Unsupported { what: "meta file version", detail: version.to_string() });
        }
        let mut sections = Vec::new();
        let tail = loop {
            match r.u8()? {
                1 => {
                    if r.remaining() != 0 {
                        return Err(Error::Malformed { what: "meta file", detail: "bytes after the end".into() });
                    }
                    break Tail::Finish;
                }
                2 => break Tail::Manager(r.rest().to_vec()),
                3 => {
                    let mut docks = Vec::new();
                    for _ in 0..count(&mut r)? {
                        docks.push((r.utf()?, r.take(27)?.try_into().unwrap()));
                    }
                    sections.push(Section::Legacy(docks));
                }
                6 => sections.push(Section::Dockers(match r.u8()? {
                    0 => None,
                    _ => {
                        let mut list = Vec::new();
                        for _ in 0..count(&mut r)? {
                            list.push(r.take(17)?.try_into().unwrap());
                        }
                        Some(list)
                    }
                })),
                7 => sections.push(Section::Cargo(match r.u8()? {
                    0 => None,
                    _ => {
                        let mut list = Vec::new();
                        for _ in 0..count(&mut r)? {
                            list.push(r.take(16)?.try_into().unwrap());
                        }
                        Some(list)
                    }
                })),
                4 => {
                    let head = r.take(24)?.try_into().unwrap();
                    let name = r.utf()?;
                    let mut wireless = Vec::new();
                    for _ in 0..count(&mut r)? {
                        wireless.push(WirelessPair { entity: r.utf()?, to: r.i64()?, from: r.i64()? });
                    }
                    let mut children = Vec::new();
                    for _ in 0..count(&mut r)? {
                        let path = r.utf()?;
                        let size = r.i32()?;
                        let size = usize::try_from(size).map_err(|_| Error::Malformed { what: "meta file", detail: format!("a size of {size}") })?;
                        children.push((path, r.take(size)?.to_vec()));
                    }
                    sections.push(Section::Rails(Rails { head, name, wireless, children }));
                }
                5 => {
                    let n = count(&mut r)?;
                    sections.push(Section::Ai(r.take(n)?.to_vec()));
                }
                other => return Err(Error::Malformed { what: "meta file", detail: format!("unknown section {other}") }),
            }
        };
        Ok(Self { version, sections, tail })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.i32(self.version);
        for section in &self.sections {
            match section {
                Section::Legacy(docks) => {
                    w.u8(3);
                    w.i32(docks.len() as i32);
                    for (path, record) in docks {
                        w.utf(path);
                        w.bytes(record);
                    }
                }
                Section::Dockers(list) => {
                    w.u8(6);
                    w.u8(list.is_some() as u8);
                    if let Some(list) = list {
                        w.i32(list.len() as i32);
                        list.iter().for_each(|r| w.bytes(r));
                    }
                }
                Section::Cargo(list) => {
                    w.u8(7);
                    w.u8(list.is_some() as u8);
                    if let Some(list) = list {
                        w.i32(list.len() as i32);
                        list.iter().for_each(|r| w.bytes(r));
                    }
                }
                Section::Rails(rails) => {
                    w.u8(4);
                    w.bytes(&rails.head);
                    w.utf(&rails.name);
                    w.i32(rails.wireless.len() as i32);
                    for pair in &rails.wireless {
                        w.utf(&pair.entity);
                        w.i64(pair.to);
                        w.i64(pair.from);
                    }
                    w.i32(rails.children.len() as i32);
                    for (path, record) in &rails.children {
                        w.utf(path);
                        w.i32(record.len() as i32);
                        w.bytes(record);
                    }
                }
                Section::Ai(bytes) => {
                    w.u8(5);
                    w.i32(bytes.len() as i32);
                    w.bytes(bytes);
                }
            }
        }
        match &self.tail {
            Tail::Finish => w.u8(1),
            Tail::Manager(rest) => {
                w.u8(2);
                w.bytes(rest);
            }
        }
        w.out
    }

    /// Whether the entity has docks of the old kind.
    pub fn has_legacy_docks(&self) -> bool {
        self.sections.iter().any(|s| matches!(s, Section::Legacy(docks) if !docks.is_empty()))
    }

    pub fn rails(&self) -> Option<&Rails> {
        self.sections.iter().find_map(|s| if let Section::Rails(r) = s { Some(r) } else { None })
    }

    pub fn rails_mut(&mut self) -> Option<&mut Rails> {
        self.sections.iter_mut().find_map(|s| if let Section::Rails(r) = s { Some(r) } else { None })
    }

    /// Whether a manager tag follows.
    pub fn has_manager_tag(&self) -> bool {
        matches!(&self.tail, Tail::Manager(rest) if !rest.is_empty())
    }

    /// Applies `f`, a map of design positions, to the positions of this
    /// entity's own blocks that the file keeps: its rail dockers, its cargo
    /// blocks and the wireless modules on it. Positions at the other end of
    /// a wireless link go through `f` only where the link ends on this very
    /// entity. The records of docked children and the manager tag are not
    /// touched. Every position must be in the file's space already; others
    /// are left alone.
    /// Turns the orientation of the rail dockers this file lists, through
    /// `orient` (given the block type and value), as the entity turns.
    pub fn turn_own_dockers(&mut self, orient: &dyn Fn(u16, u8) -> u8) {
        for section in &mut self.sections {
            if let Section::Dockers(Some(list)) = section {
                for record in list {
                    let ty = u16::from_be_bytes([record[12], record[13]]);
                    record[14] = orient(ty, record[14] & 31);
                }
            }
        }
    }

    pub fn map_own_blocks(&mut self, f: &dyn Fn(Pos) -> Pos) {
        let shift = offset(self.version);
        let map = |p: [i32; 3]| {
            let q = f(Pos::new(p[0] - shift, p[1] - shift, p[2] - shift));
            [q.x + shift, q.y + shift, q.z + shift]
        };
        let own_name = self.rails().map(|r| r.name.clone());
        for section in &mut self.sections {
            match section {
                Section::Dockers(Some(list)) => {
                    for record in list {
                        let p = map([0, 4, 8].map(|i| i32::from_be_bytes(record[i..i + 4].try_into().unwrap())));
                        for (i, v) in p.iter().enumerate() {
                            record[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
                        }
                    }
                }
                Section::Cargo(Some(list)) => {
                    for record in list {
                        let p = unpack(i64::from_be_bytes(record[..8].try_into().unwrap()));
                        let q = map(p);
                        record[..8].copy_from_slice(&pack(q[0], q[1], q[2]).to_be_bytes());
                    }
                }
                Section::Rails(rails) => {
                    for pair in &mut rails.wireless {
                        let q = map(unpack(pair.from));
                        pair.from = pack(q[0], q[1], q[2]);
                        if Some(&pair.entity) == own_name.as_ref() {
                            let q = map(unpack(pair.to));
                            pair.to = pack(q[0], q[1], q[2]);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Applies `f` to the wireless modules this file names on the entity
    /// called `entity`. Returns how many it moved.
    pub fn map_wireless_to(&mut self, entity: &str, f: &dyn Fn(Pos) -> Pos) -> usize {
        let shift = offset(self.version);
        let mut moved = 0;
        for section in &mut self.sections {
            if let Section::Rails(rails) = section {
                for pair in rails.wireless.iter_mut().filter(|p| p.entity == entity) {
                    let p = unpack(pair.to);
                    let q = f(Pos::new(p[0] - shift, p[1] - shift, p[2] - shift));
                    pair.to = pack(q.x + shift, q.y + shift, q.z + shift);
                    moved += 1;
                }
            }
        }
        moved
    }

    /// Takes out the rail docker records (section 06) whose block is at a
    /// design position `take` says yes to, and returns them with the position.
    pub fn take_dockers(&mut self, take: &dyn Fn(Pos) -> bool) -> Vec<([u8; 17], Pos)> {
        let shift = offset(self.version);
        let mut out = Vec::new();
        for section in &mut self.sections {
            if let Section::Dockers(Some(list)) = section {
                let at = |r: &[u8; 17]| {
                    let c = [0, 4, 8].map(|i| i32::from_be_bytes(r[i..i + 4].try_into().unwrap()) - shift);
                    Pos::new(c[0], c[1], c[2])
                };
                let (gone, kept): (Vec<_>, Vec<_>) = list.drain(..).partition(|r| take(at(r)));
                *list = kept;
                out.extend(gone.into_iter().map(|r| (r, at(&r))));
            }
        }
        out
    }

    /// The same for the cargo records (section 07).
    pub fn take_cargo(&mut self, take: &dyn Fn(Pos) -> bool) -> Vec<([u8; 16], Pos)> {
        let shift = offset(self.version);
        let mut out = Vec::new();
        for section in &mut self.sections {
            if let Section::Cargo(Some(list)) = section {
                let at = |r: &[u8; 16]| {
                    let c = unpack(i64::from_be_bytes(r[..8].try_into().unwrap()));
                    Pos::new(c[0] - shift, c[1] - shift, c[2] - shift)
                };
                let (gone, kept): (Vec<_>, Vec<_>) = list.drain(..).partition(|r| take(at(r)));
                *list = kept;
                out.extend(gone.into_iter().map(|r| (r, at(&r))));
            }
        }
        out
    }

    /// Takes out the wireless links that end at a design position `gone`
    /// says yes to: on this entity's own modules, and (where the other end
    /// is the entity called `name`) on that entity's. Returns how many.
    pub fn drop_wireless(&mut self, own: bool, name: &str, gone: &dyn Fn(Pos) -> bool) -> usize {
        let shift = offset(self.version);
        let at = |index: i64| {
            let c = unpack(index);
            gone(Pos::new(c[0] - shift, c[1] - shift, c[2] - shift))
        };
        let own_name = self.rails().map(|r| r.name.clone());
        let mut dropped = 0;
        for section in &mut self.sections {
            if let Section::Rails(rails) = section {
                let before = rails.wireless.len();
                rails.wireless.retain(|p| {
                    let at_other_end = p.entity == name && at(p.to);
                    let here = own && (at(p.from) || (Some(&p.entity) == own_name.as_ref() && at(p.to)));
                    !(at_other_end || here)
                });
                dropped += before - rails.wireless.len();
            }
        }
        dropped
    }

    /// Puts a rails section (04) in the place the game writes it: after
    /// the docks, dockers and cargo, before the AI settings.
    pub fn add_rails(&mut self, rails: Rails) {
        let at = self.sections.iter().position(|s| matches!(s, Section::Ai(_))).unwrap_or(self.sections.len());
        self.sections.insert(at, Section::Rails(rails));
    }
}

/// A rail docker record (section 06): the block's position and what it is.
pub fn docker_record(version: i32, pos: Pos, ty: u16, orientation: u8) -> [u8; 17] {
    let shift = offset(version);
    let mut out = [0u8; 17];
    for (i, v) in [pos.x, pos.y, pos.z].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&(v + shift).to_be_bytes());
    }
    out[12..14].copy_from_slice(&ty.to_be_bytes());
    out[14] = orientation;
    out[16] = 127;
    out
}

/// Moves a docker record to another position.
pub fn move_docker(record: &mut [u8; 17], version: i32, pos: Pos) {
    let shift = offset(version);
    for (i, v) in [pos.x, pos.y, pos.z].iter().enumerate() {
        record[i * 4..i * 4 + 4].copy_from_slice(&(v + shift).to_be_bytes());
    }
}

/// Moves a cargo record to another position.
pub fn move_cargo(record: &mut [u8; 16], version: i32, pos: Pos) {
    let shift = offset(version);
    record[..8].copy_from_slice(&pack(pos.x + shift, pos.y + shift, pos.z + shift).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_pack_the_way_the_game_does() {
        for p in [[0, 0, 0], [16, 17, 18], [-3, 40, 1000], [-1, -1, -1], [32767, -32768, 5]] {
            assert_eq!(unpack(pack(p[0], p[1], p[2])), p);
        }
        // The game's getIndex(x, y, z), worked out by hand.
        assert_eq!(pack(1, 2, 3), (3 << 32) + (2 << 16) + 1);
        assert_eq!(pack(-1, 0, 0), 0xffff);
    }

    #[test]
    fn a_file_comes_back_as_it_went_in() {
        let mut w = Writer::new();
        w.i32(5);
        w.u8(3);
        w.i32(1);
        w.utf("X/ATTACHED_0");
        w.bytes(&[7; 27]);
        w.u8(6);
        w.u8(1);
        w.i32(2);
        w.bytes(&[6; 34]);
        w.u8(7);
        w.u8(0);
        w.u8(4);
        w.bytes(&[4; 24]);
        w.utf("rl0");
        w.i32(1);
        w.utf("marker");
        w.i64(5);
        w.i64(6);
        w.i32(1);
        w.utf("X/ATTACHED_1");
        w.i32(3);
        w.bytes(b"tag");
        w.u8(5);
        w.i32(2);
        w.bytes(b"ai");
        w.u8(2);
        w.bytes(b"whatever follows");
        let meta = MetaFile::parse(&w.out).unwrap();
        assert_eq!(meta.write(), w.out);
        assert!(meta.has_legacy_docks() && meta.has_manager_tag());
        assert_eq!(meta.rails().unwrap().children[0].0, "X/ATTACHED_1");
        assert_eq!(MetaFile::parse(&[0, 0, 0, 5, 1]).unwrap().write(), [0, 0, 0, 5, 1]);
        assert!(MetaFile::parse(&[0, 0, 0, 5, 1, 0]).is_err());
        assert!(MetaFile::parse(&[0, 0, 0, 5, 9]).is_err());
        assert!(MetaFile::parse(&w.out[..30]).is_err());
    }
}
