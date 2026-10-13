//! The one part of `meta.smbpm` the editor has to understand so far: where a
//! parent names its docked entities.
//!
//! The game finds a docked entity through a path stored in its parent's meta
//! file, `Blueprint Name/ATTACHED_0`, looked up below the catalog folder. A
//! blueprint renamed without rewriting those paths loses its turrets: the game
//! looks for them under the old name. (The game's own answer to a name clash on
//! upload, adding `_0` to the folder, has exactly that flaw.)
//!
//! ```text
//! i32 version
//! then sections, each introduced by one byte, until 01 or 02:
//! 03  legacy docks   i32 n; n x (UTF child path, 27 bytes)
//! 06  rail dockers   u8 exists; if so: i32 n; n x 17 bytes
//! 07  cargo          u8 exists; if so: i32 n; n x 16 bytes
//! 04  rails          24 bytes; UTF rail id; i32 n; n x (UTF, 16 bytes);
//!                    i32 n; n x (UTF child path, i32 size, size bytes)
//! 05  AI             i32 size, size bytes
//! 02  manager tag    the rest of the file
//! 01  finish
//! ```

use crate::bytes::{Reader, Writer};
use crate::{Error, Result};

/// Copies a meta file, passing each docked child's path through `child`,
/// together with the record of how it is docked (empty for the old kind of
/// dock, which has none).
fn rewrite(meta: &[u8], mut child: impl FnMut(&str, &[u8]) -> String) -> Result<Vec<u8>> {
    let mut r = Reader::new(meta, "meta file");
    let mut w = Writer::new();
    let version = r.i32()?;
    if version < 2 {
        // Section 04 is laid out differently before version 2.
        return Err(Error::Unsupported { what: "meta file version", detail: version.to_string() });
    }
    w.i32(version);
    // A count followed by that many fixed-size records, copied as they are.
    fn records(r: &mut Reader, w: &mut Writer, size: usize) -> Result<()> {
        let n = count(r, w)?;
        w.bytes(r.take(n.checked_mul(size).ok_or(Error::Truncated("meta file"))?)?);
        Ok(())
    }
    fn count(r: &mut Reader, w: &mut Writer) -> Result<usize> {
        let n = r.i32()?;
        w.i32(n);
        usize::try_from(n).map_err(|_| Error::Malformed { what: "meta file", detail: format!("a count of {n}") })
    }
    loop {
        let section = r.u8()?;
        w.u8(section);
        match section {
            1 => break,
            2 => {
                w.bytes(r.rest());
                return Ok(w.out);
            }
            3 => {
                for _ in 0..count(&mut r, &mut w)? {
                    w.utf(&child(&r.utf()?, &[]));
                    w.bytes(r.take(27)?);
                }
            }
            6 | 7 => {
                let exists = r.u8()?;
                w.u8(exists);
                if exists != 0 {
                    records(&mut r, &mut w, if section == 6 { 17 } else { 16 })?;
                }
            }
            4 => {
                w.bytes(r.take(24)?);
                w.utf(&r.utf()?);
                for _ in 0..count(&mut r, &mut w)? {
                    w.utf(&r.utf()?);
                    w.bytes(r.take(16)?);
                }
                for _ in 0..count(&mut r, &mut w)? {
                    let path = r.utf()?;
                    let size = r.i32()?;
                    let record = r.take(usize::try_from(size).map_err(|_| Error::Malformed { what: "meta file", detail: format!("a size of {size}") })?)?;
                    w.utf(&child(&path, record));
                    w.i32(size);
                    w.bytes(record);
                }
            }
            5 => records(&mut r, &mut w, 1)?,
            other => return Err(Error::Malformed { what: "meta file", detail: format!("unknown section {other}") }),
        }
    }
    w.bytes(r.rest());
    Ok(w.out)
}

/// The paths of the docked entities a meta file names, in file order.
pub fn children(meta: &[u8]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    rewrite(meta, |path, _| {
        out.push(path.to_string());
        path.to_string()
    })?;
    Ok(out)
}

/// The meta file's layout version: before 4, positions inside it count from
/// a core at (8, 8, 8).
pub fn version(meta: &[u8]) -> Result<i32> {
    Reader::new(meta, "meta file").i32()
}

/// Each docked entity a meta file names, with the record of how it is
/// docked: a tag structure the game writes for its rail dock, empty for an
/// entity docked the old way. In file order.
pub fn dock_records(meta: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    rewrite(meta, |path, record| {
        out.push((path.to_string(), record.to_vec()));
        path.to_string()
    })?;
    Ok(out)
}

/// What a meta file says about wireless logic: the name the entity goes by
/// among the entities docked together ("" for the main one, "rl0", "rl01"...
/// for docked ones), and for each of its wireless logic modules that speaks
/// to a module in the same design: (the module here, the name of the entity
/// the other one is on, the other module). Positions count from the core.
pub fn wireless(meta: &[u8]) -> Result<(String, Vec<(crate::Pos, String, crate::Pos)>)> {
    let mut r = Reader::new(meta, "meta file");
    let version = r.i32()?;
    if version < 2 {
        return Err(Error::Unsupported { what: "meta file version", detail: version.to_string() });
    }
    let count = |r: &mut Reader| -> Result<usize> {
        let n = r.i32()?;
        usize::try_from(n).map_err(|_| Error::Malformed { what: "meta file", detail: format!("a count of {n}") })
    };
    // A block position as the game packs one into a long: three signed
    // 16-bit numbers, x lowest, counted with the core at (16, 16, 16), or at
    // (8, 8, 8) in files older than version 4.
    let shift = if version < 4 { 8 } else { 0 } - crate::entity::CORE_OFFSET;
    let pos = |index: i64| {
        let part = |bits: u32| ((index >> bits) & 0xffff) as u16 as i16 as i32 + shift;
        crate::Pos::new(part(0), part(16), part(32))
    };
    loop {
        match r.u8()? {
            3 => {
                for _ in 0..count(&mut r)? {
                    r.utf()?;
                    r.take(27)?;
                }
            }
            section @ (6 | 7) => {
                if r.u8()? != 0 {
                    let n = count(&mut r)?;
                    r.take(n.checked_mul(if section == 6 { 17 } else { 16 }).ok_or(Error::Truncated("meta file"))?)?;
                }
            }
            4 => {
                r.take(24)?;
                let name = r.utf()?;
                let mut pairs = Vec::new();
                for _ in 0..count(&mut r)? {
                    let other_entity = r.utf()?;
                    let (to, from) = (r.i64()?, r.i64()?);
                    pairs.push((pos(from), other_entity, pos(to)));
                }
                return Ok((name, pairs));
            }
            5 => {
                let n = count(&mut r)?;
                r.take(n)?;
            }
            _ => return Ok((String::new(), Vec::new())),
        }
    }
}

/// The meta file with every docked child's path moved under the blueprint
/// name `root`: `Old Name/ATTACHED_0/ATTACHED_1` becomes `root/ATTACHED_0/ATTACHED_1`.
pub fn with_root(meta: &[u8], root: &str) -> Result<Vec<u8>> {
    rewrite(meta, |path, _| match path.find('/') {
        Some(slash) => format!("{root}{}", &path[slash..]),
        None => path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A meta file as the game writes one for a ship with two docked entities,
    /// one on an old-style dock and one on a rail.
    fn sample(root: &str) -> Vec<u8> {
        let mut w = Writer::new();
        w.i32(5);
        w.u8(3);
        w.i32(1);
        w.utf(&format!("{root}/ATTACHED_0"));
        w.bytes(&[7; 27]);
        w.u8(6);
        w.u8(1);
        w.i32(2);
        w.bytes(&[6; 34]);
        w.u8(7);
        w.u8(0);
        w.u8(4);
        w.bytes(&[4; 24]);
        w.utf("ENTITY_SHIP_x");
        w.i32(1);
        w.utf("marker");
        w.bytes(&[9; 16]);
        w.i32(1);
        w.utf(&format!("{root}/ATTACHED_1"));
        w.i32(3);
        w.bytes(b"tag");
        w.u8(5);
        w.i32(2);
        w.bytes(b"ai");
        w.u8(2);
        // The manager tag may hold anything, the old name included.
        w.bytes(format!("manager {root}/ATTACHED_9").as_bytes());
        w.out
    }

    #[test]
    fn docked_children_are_found_and_follow_a_rename() {
        let meta = sample("Old Name");
        assert_eq!(children(&meta).unwrap(), vec!["Old Name/ATTACHED_0", "Old Name/ATTACHED_1"]);
        // Same root: byte for byte what went in.
        assert_eq!(with_root(&meta, "Old Name").unwrap(), meta);
        // A new root changes the two paths and nothing else, whatever their length.
        let renamed = with_root(&meta, "A Much Longer New Name").unwrap();
        assert_eq!(children(&renamed).unwrap(), vec!["A Much Longer New Name/ATTACHED_0", "A Much Longer New Name/ATTACHED_1"]);
        assert_eq!(with_root(&renamed, "Old Name").unwrap(), meta);
        assert!(renamed.ends_with(b"manager Old Name/ATTACHED_9"));
        // Deeper paths keep everything after the blueprint's name.
        let mut deep = Writer::new();
        deep.i32(5);
        deep.u8(3);
        deep.i32(1);
        deep.utf("Old/ATTACHED_0/ATTACHED_2");
        deep.bytes(&[0; 27]);
        deep.u8(1);
        assert_eq!(children(&with_root(&deep.out, "New").unwrap()).unwrap(), vec!["New/ATTACHED_0/ATTACHED_2"]);
    }

    #[test]
    fn the_smallest_meta_and_broken_ones_are_handled() {
        assert_eq!(children(&[0, 0, 0, 5, 1]).unwrap(), Vec::<String>::new());
        assert_eq!(with_root(&[0, 0, 0, 5, 1], "X").unwrap(), vec![0, 0, 0, 5, 1]);
        // Cut short, an impossible count, a section nobody knows, a version too old.
        let meta = sample("Old Name");
        assert!(children(&meta[..40]).is_err());
        assert!(children(&[0, 0, 0, 5, 3, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(children(&[0, 0, 0, 5, 3, 0x7f, 0xff, 0xff, 0xff]).is_err());
        assert!(children(&[0, 0, 0, 5, 9]).is_err());
        assert!(children(&[0, 0, 0, 1, 1]).is_err());
    }
}
