//! The game's tagged data format (see [`crate::tag`]) read and written without
//! losing anything: member names, which members have none, the element type
//! of each list and whether the stream was gzipped all survive, so a record
//! can be changed in one place and written back as the game would write it.
//!
//! [`crate::tag`] stays the quick reader for looking at a record; this is for
//! changing one.

use crate::bytes::{Reader, Writer};
use crate::{Error, Result};
use std::io::{Read, Write};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
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
    /// The element type as the stream spells it (a negative one is a list
    /// the game wrote with unnamed elements), and the elements.
    List(i8, Vec<Value>),
    Struct(Vec<Member>),
    Vec4f([f32; 4]),
    /// Sixteen numbers, row by row.
    Matrix4([f32; 16]),
    Nothing,
    /// Nine numbers, row by row.
    Matrix3([f32; 9]),
}

/// One member of a structure. A negative type byte in the stream marks a
/// member without a name.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub name: Option<String>,
    pub value: Value,
}

/// A whole stream: `i16 version; i8 type; UTF name if type > 0; payload`.
#[derive(Clone, Debug, PartialEq)]
pub struct RawTag {
    pub gzipped: bool,
    pub version: i16,
    pub name: Option<String>,
    pub value: Value,
    /// Anything after the tag, kept so that writing gives back what was read.
    pub trailing: Vec<u8>,
}

const MAX_DEPTH: usize = 32;
const MAX_ITEMS: usize = 1 << 20;

fn malformed(detail: impl Into<String>) -> Error {
    Error::Malformed { what: "tag", detail: detail.into() }
}

/// The stream's type byte for a value.
fn type_of(v: &Value) -> u8 {
    match v {
        Value::Byte(_) => 1,
        Value::Short(_) => 2,
        Value::Int(_) => 3,
        Value::Long(_) => 4,
        Value::Float(_) => 5,
        Value::Double(_) => 6,
        Value::Bytes(_) => 7,
        Value::Text(_) => 8,
        Value::Vec3f(_) => 9,
        Value::Vec3i(_) => 10,
        Value::Vec3b(_) => 11,
        Value::List(..) => 12,
        Value::Struct(_) => 13,
        Value::Vec4f(_) => 15,
        Value::Matrix4(_) => 16,
        Value::Nothing => 17,
        Value::Matrix3(_) => 18,
    }
}

fn floats<const N: usize>(r: &mut Reader) -> Result<[f32; N]> {
    let mut out = [0.0; N];
    for v in &mut out {
        *v = r.f32()?;
    }
    Ok(out)
}

fn text(r: &mut Reader) -> Result<String> {
    let len = r.u16()? as usize;
    String::from_utf8(r.take(len)?.to_vec()).map_err(|_| malformed("text that is not UTF-8"))
}

fn payload(r: &mut Reader, ty: u8, depth: usize) -> Result<Value> {
    if depth > MAX_DEPTH {
        return Err(malformed("nested too deeply"));
    }
    Ok(match ty {
        1 => Value::Byte(r.u8()? as i8),
        2 => Value::Short(r.i16()?),
        3 => Value::Int(r.i32()?),
        4 => Value::Long(r.i64()?),
        5 => Value::Float(r.f32()?),
        6 => Value::Double(f64::from_bits(r.i64()? as u64)),
        7 => {
            let n = usize::try_from(r.i32()?).map_err(|_| malformed("a negative length"))?;
            Value::Bytes(r.take(n)?.to_vec())
        }
        8 => Value::Text(text(r)?),
        9 => Value::Vec3f(floats(r)?),
        10 => Value::Vec3i([r.i32()?, r.i32()?, r.i32()?]),
        11 => Value::Vec3b([r.u8()? as i8, r.u8()? as i8, r.u8()? as i8]),
        12 => {
            let element = r.u8()? as i8;
            let n = usize::try_from(r.i32()?).ok().filter(|n| *n <= MAX_ITEMS).ok_or_else(|| malformed("a list length out of range"))?;
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(payload(r, element.unsigned_abs(), depth + 1)?);
            }
            Value::List(element, items)
        }
        13 => {
            let mut members = Vec::new();
            loop {
                let ty = r.u8()? as i8;
                if ty == 0 {
                    break;
                }
                let name = if ty > 0 { Some(text(r)?) } else { None };
                members.push(Member { name, value: payload(r, ty.unsigned_abs(), depth + 1)? });
            }
            Value::Struct(members)
        }
        15 => Value::Vec4f(floats(r)?),
        16 => Value::Matrix4(floats(r)?),
        17 => Value::Nothing,
        18 => Value::Matrix3(floats(r)?),
        other => return Err(Error::Unsupported { what: "tag type", detail: other.to_string() }),
    })
}

fn write_payload(w: &mut Writer, v: &Value) {
    match v {
        Value::Byte(x) => w.u8(*x as u8),
        Value::Short(x) => w.i16(*x),
        Value::Int(x) => w.i32(*x),
        Value::Long(x) => w.i64(*x),
        Value::Float(x) => w.f32(*x),
        Value::Double(x) => w.i64(x.to_bits() as i64),
        Value::Bytes(b) => {
            w.i32(b.len() as i32);
            w.bytes(b);
        }
        Value::Text(s) => w.utf(s),
        Value::Vec3f(a) => a.iter().for_each(|x| w.f32(*x)),
        Value::Vec3i(a) => a.iter().for_each(|x| w.i32(*x)),
        Value::Vec3b(a) => a.iter().for_each(|x| w.u8(*x as u8)),
        Value::List(element, items) => {
            w.u8(*element as u8);
            w.i32(items.len() as i32);
            items.iter().for_each(|i| write_payload(w, i));
        }
        Value::Struct(members) => {
            for m in members {
                let ty = type_of(&m.value) as i8;
                w.u8((if m.name.is_some() { ty } else { -ty }) as u8);
                if let Some(name) = &m.name {
                    w.utf(name);
                }
                write_payload(w, &m.value);
            }
            w.u8(0);
        }
        Value::Vec4f(a) => a.iter().for_each(|x| w.f32(*x)),
        Value::Matrix4(a) => a.iter().for_each(|x| w.f32(*x)),
        Value::Nothing => {}
        Value::Matrix3(a) => a.iter().for_each(|x| w.f32(*x)),
    }
}

impl RawTag {
    /// Reads one top-level tag, unpacking it first if it is gzipped.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let gzipped = data.starts_with(&[0x1f, 0x8b]);
        let unpacked;
        let data = if gzipped {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(data).read_to_end(&mut out).map_err(|e| malformed(format!("gzip: {e}")))?;
            unpacked = out;
            &unpacked[..]
        } else {
            data
        };
        let mut r = Reader::new(data, "tag");
        let version = r.i16()?;
        let ty = r.u8()? as i8;
        if ty == 0 {
            return Err(malformed("a tag with no type"));
        }
        let name = if ty > 0 { Some(text(&mut r)?) } else { None };
        let value = payload(&mut r, ty.unsigned_abs(), 0)?;
        Ok(Self { gzipped, version, name, value, trailing: r.rest().to_vec() })
    }

    /// The stream without compression.
    pub fn write_plain(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.i16(self.version);
        let ty = type_of(&self.value) as i8;
        w.u8((if self.name.is_some() { ty } else { -ty }) as u8);
        if let Some(name) = &self.name {
            w.utf(name);
        }
        write_payload(&mut w, &self.value);
        w.bytes(&self.trailing);
        w.out
    }

    /// The stream, gzipped if it was when read.
    pub fn write(&self) -> Vec<u8> {
        let plain = self.write_plain();
        if !self.gzipped {
            return plain;
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&plain).expect("writing to memory");
        gz.finish().expect("writing to memory")
    }
}

impl Value {
    /// The members of a structure.
    pub fn members(&self) -> Option<&[Member]> {
        match self {
            Value::Struct(m) => Some(m),
            _ => None,
        }
    }

    pub fn members_mut(&mut self) -> Option<&mut Vec<Member>> {
        match self {
            Value::Struct(m) => Some(m),
            _ => None,
        }
    }

    /// The elements of a list.
    pub fn items_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::List(_, items) => Some(items),
            _ => None,
        }
    }

    /// The members of a structure, or the elements of a list, in order, as
    /// the plain reader sees them.
    pub fn children_mut(&mut self) -> Vec<&mut Value> {
        match self {
            Value::Struct(m) => m.iter_mut().map(|m| &mut m.value).collect(),
            Value::List(_, items) => items.iter_mut().collect(),
            _ => Vec::new(),
        }
    }

    /// Any whole number, widened.
    pub fn int(&self) -> Option<i64> {
        match self {
            Value::Byte(v) => Some(*v as i64),
            Value::Short(v) => Some(*v as i64),
            Value::Int(v) => Some(*v as i64),
            Value::Long(v) => Some(*v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = Writer::new();
        w.i16(0);
        w.u8(13);
        w.utf("top");
        w.u8(1);
        w.utf("kind");
        w.u8(3);
        w.u8((-10i8) as u8); // an unnamed position
        for v in [16, 15, 17] {
            w.i32(v);
        }
        w.u8((-12i8) as u8); // an unnamed list of shorts
        w.u8(2);
        w.i32(2);
        w.i16(7);
        w.i16(-7);
        w.u8(12);
        w.utf("list of unnamed structs");
        w.u8((-13i8) as u8);
        w.i32(1);
        w.u8(8);
        w.utf("s");
        w.utf("hello");
        w.u8(0);
        w.u8((-18i8) as u8);
        for i in 0..9 {
            w.f32(i as f32);
        }
        w.u8(0);
        w.out
    }

    #[test]
    fn names_unnamed_members_and_list_types_survive_a_round_trip() {
        let bytes = sample();
        let tag = RawTag::parse(&bytes).unwrap();
        assert!(!tag.gzipped);
        assert_eq!(tag.name.as_deref(), Some("top"));
        let members = tag.value.members().unwrap();
        assert_eq!(members[0].name.as_deref(), Some("kind"));
        assert_eq!(members[1].name, None);
        assert_eq!(members[1].value, Value::Vec3i([16, 15, 17]));
        assert_eq!(members[2].value, Value::List(2, vec![Value::Short(7), Value::Short(-7)]));
        assert_eq!(members[3].value, Value::List(-13, vec![Value::Struct(vec![Member { name: Some("s".into()), value: Value::Text("hello".into()) }])]));
        assert_eq!(tag.write(), bytes);
        // Gzipped, it reads the same and writes the same stream inside.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bytes).unwrap();
        let packed = RawTag::parse(&gz.finish().unwrap()).unwrap();
        assert!(packed.gzipped);
        assert_eq!(packed.write_plain(), bytes);
        assert_eq!(RawTag::parse(&packed.write()).unwrap(), packed);
        // Bytes after the tag are kept.
        let mut more = bytes.clone();
        more.extend_from_slice(&[9, 9]);
        assert_eq!(RawTag::parse(&more).unwrap().write(), more);
    }

    #[test]
    fn broken_tags_are_refused() {
        assert!(RawTag::parse(&[]).is_err());
        assert!(RawTag::parse(&[0, 0, 0]).is_err());
        assert!(RawTag::parse(&[0, 0, 13, 0, 0, 3]).is_err(), "a struct that never ends");
        assert!(RawTag::parse(&[0, 0, 14u8.wrapping_neg(), 1]).is_err());
        assert!(RawTag::parse(&[0, 0, 8, 0, 1, 0xff, 0, 1, 0xff]).is_err(), "text that is not UTF-8");
    }
}
