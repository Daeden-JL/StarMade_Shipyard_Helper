//! The game's tagged data format, which it uses wherever a structure is saved
//! as a blob: here, how a docked entity sits on its rail.
//!
//! ```text
//! [gzip]  i16 version;  i8 type;  UTF name if type > 0;  payload of |type|
//! struct  = repeated (i8 type; UTF name if type > 0; payload) until type 0
//! list    = i8 element type; i32 count; payloads
//! ```

use crate::bytes::Reader;
use crate::{Error, Result};
use std::io::Read;

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bytes(Vec<u8>),
    Text(String),
    Vec3f([f32; 3]),
    Vec3i([i32; 3]),
    Vec3b([i8; 3]),
    List(Vec<Tag>),
    /// The members in order; their names are not kept.
    Struct(Vec<Tag>),
    Vec4f([f32; 4]),
    /// Sixteen numbers, row by row.
    Matrix4([f32; 16]),
    Nothing,
    /// Nine numbers, row by row.
    Matrix3([f32; 9]),
}

/// Deepest nesting read; real data has four or five levels.
const MAX_DEPTH: usize = 32;
/// Most members a list may claim, as a guard against garbage counts.
const MAX_ITEMS: usize = 1 << 20;

fn malformed(detail: impl Into<String>) -> Error {
    Error::Malformed { what: "tag", detail: detail.into() }
}

fn payload(r: &mut Reader, ty: u8, depth: usize) -> Result<Tag> {
    if depth > MAX_DEPTH {
        return Err(malformed("nested too deeply"));
    }
    let floats = |r: &mut Reader, n: usize| -> Result<Vec<f32>> { (0..n).map(|_| r.f32()).collect() };
    Ok(match ty {
        1 => Tag::Byte(r.u8()? as i8),
        2 => Tag::Short(r.i16()?),
        3 => Tag::Int(r.i32()?),
        4 => Tag::Long(r.i64()?),
        5 => Tag::Float(r.f32()?),
        6 => Tag::Double(f64::from_bits(r.i64()? as u64)),
        7 => {
            let n = usize::try_from(r.i32()?).map_err(|_| malformed("a negative length"))?;
            Tag::Bytes(r.take(n)?.to_vec())
        }
        8 => Tag::Text(r.utf()?),
        9 => {
            let v = floats(r, 3)?;
            Tag::Vec3f([v[0], v[1], v[2]])
        }
        10 => Tag::Vec3i([r.i32()?, r.i32()?, r.i32()?]),
        11 => Tag::Vec3b([r.u8()? as i8, r.u8()? as i8, r.u8()? as i8]),
        12 => {
            let element = r.u8()? as i8;
            let n = usize::try_from(r.i32()?).ok().filter(|n| *n <= MAX_ITEMS).ok_or_else(|| malformed("a list length out of range"))?;
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(payload(r, element.unsigned_abs(), depth + 1)?);
            }
            Tag::List(items)
        }
        13 => {
            let mut members = Vec::new();
            loop {
                let ty = r.u8()? as i8;
                if ty == 0 {
                    break;
                }
                // A negative type marks a member without a name.
                if ty > 0 {
                    r.utf()?;
                }
                members.push(payload(r, ty.unsigned_abs(), depth + 1)?);
            }
            Tag::Struct(members)
        }
        15 => {
            let v = floats(r, 4)?;
            Tag::Vec4f([v[0], v[1], v[2], v[3]])
        }
        16 => {
            let v = floats(r, 16)?;
            let mut m = [0.0; 16];
            m.copy_from_slice(&v);
            Tag::Matrix4(m)
        }
        17 => Tag::Nothing,
        18 => {
            let v = floats(r, 9)?;
            let mut m = [0.0; 9];
            m.copy_from_slice(&v);
            Tag::Matrix3(m)
        }
        other => return Err(Error::Unsupported { what: "tag type", detail: other.to_string() }),
    })
}

/// Reads one top-level tag, unpacking it first if it is gzipped.
pub fn read(data: &[u8]) -> Result<Tag> {
    let unpacked;
    let data = if data.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data).read_to_end(&mut out).map_err(|e| malformed(format!("gzip: {e}")))?;
        unpacked = out;
        &unpacked[..]
    } else {
        data
    };
    let mut r = Reader::new(data, "tag");
    r.i16()?;
    let ty = r.u8()? as i8;
    if ty > 0 {
        r.utf()?;
    }
    payload(&mut r, ty.unsigned_abs(), 0)
}

impl Tag {
    /// The members of a struct or the items of a list.
    pub fn items(&self) -> Option<&[Tag]> {
        match self {
            Tag::Struct(items) | Tag::List(items) => Some(items),
            _ => None,
        }
    }

    /// Any whole number, widened.
    pub fn int(&self) -> Option<i64> {
        match self {
            Tag::Byte(v) => Some(*v as i64),
            Tag::Short(v) => Some(*v as i64),
            Tag::Int(v) => Some(*v as i64),
            Tag::Long(v) => Some(*v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Writer;

    #[test]
    fn structs_lists_and_matrices_are_read_in_order() {
        let mut w = Writer::new();
        w.i16(0);
        w.u8(13); // a named struct
        w.utf("top");
        w.u8(1);
        w.utf("kind");
        w.u8(3);
        w.u8((-10i8) as u8); // an unnamed position
        for v in [16, 15, 17] {
            w.i32(v);
        }
        w.u8(12);
        w.utf("list");
        w.u8(2); // of shorts
        w.i32(2);
        w.i16(7);
        w.i16(-7);
        w.u8((-13i8) as u8); // an unnamed struct inside
        w.u8(8);
        w.utf("s");
        w.utf("hello");
        w.u8(0);
        w.u8((-18i8) as u8);
        for i in 0..9 {
            w.f32(i as f32);
        }
        w.u8(0);
        let tag = read(&w.out).unwrap();
        let members = tag.items().unwrap();
        assert_eq!(members[0], Tag::Byte(3));
        assert_eq!(members[1], Tag::Vec3i([16, 15, 17]));
        assert_eq!(members[2], Tag::List(vec![Tag::Short(7), Tag::Short(-7)]));
        assert_eq!(members[3], Tag::Struct(vec![Tag::Text("hello".into())]));
        assert_eq!(members[4], Tag::Matrix3([0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]));
        assert_eq!((members[0].int(), members[1].int()), (Some(3), None));

        // The same, gzipped, reads the same.
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&w.out).unwrap();
        assert_eq!(read(&gz.finish().unwrap()).unwrap(), tag);
    }

    #[test]
    fn broken_tags_are_refused_not_trusted() {
        assert!(read(&[]).is_err());
        assert!(read(&[0, 0, 13, 0, 0, 3]).is_err(), "a struct that never ends");
        assert!(read(&[0, 0, 14u8.wrapping_neg(), 1]).is_err(), "a type this reader does not know");
        // A list claiming a billion items.
        assert!(read(&[0, 0, 12u8.wrapping_neg(), 1, 0x7f, 0xff, 0xff, 0xff]).is_err());
    }
}
