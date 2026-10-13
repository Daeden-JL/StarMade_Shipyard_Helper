//! PLY: a heading that lists what follows, then points and faces as text
//! or as bytes of either byte order.

use crate::colour::byte;
use crate::error::{cut_short, malformed, ImportError, Result};
use crate::files::Source;
use crate::model::{Corner, Model, Skin, Up};

const NAME: &str = "PLY";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Number {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Number {
    fn named(name: &str) -> Option<Number> {
        Some(match name {
            "char" | "int8" => Number::I8,
            "uchar" | "uint8" => Number::U8,
            "short" | "int16" => Number::I16,
            "ushort" | "uint16" => Number::U16,
            "int" | "int32" => Number::I32,
            "uint" | "uint32" => Number::U32,
            "float" | "float32" => Number::F32,
            "double" | "float64" => Number::F64,
            _ => return None,
        })
    }

    fn bytes(self) -> usize {
        match self {
            Number::I8 | Number::U8 => 1,
            Number::I16 | Number::U16 => 2,
            Number::I32 | Number::U32 | Number::F32 => 4,
            Number::F64 => 8,
        }
    }

    /// A colour part of this kind as a byte: whole numbers are scaled from
    /// their own range, others are parts of one.
    fn colour(self, value: f64) -> u8 {
        match self {
            Number::U8 | Number::I8 => value.clamp(0.0, 255.0) as u8,
            Number::U16 | Number::I16 => (value / 257.0).clamp(0.0, 255.0) as u8,
            Number::I32 | Number::U32 => value.clamp(0.0, 255.0) as u8,
            Number::F32 | Number::F64 => byte(value as f32),
        }
    }
}

#[derive(Debug)]
struct Property {
    name: String,
    /// For a list: the kind of its count.
    list: Option<Number>,
    kind: Number,
}

#[derive(Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// The values after the heading.
enum Body<'a> {
    Text(std::str::SplitAsciiWhitespace<'a>),
    Bytes { bytes: &'a [u8], at: usize, big: bool },
}

impl Body<'_> {
    fn next(&mut self, kind: Number) -> Result<f64> {
        match self {
            Body::Text(words) => words.next().and_then(|w| w.parse().ok()).ok_or_else(|| cut_short(NAME)),
            Body::Bytes { bytes, at, big } => {
                let raw = bytes.get(*at..*at + kind.bytes()).ok_or_else(|| cut_short(NAME))?;
                *at += kind.bytes();
                let mut b = [0u8; 8];
                b[..raw.len()].copy_from_slice(raw);
                if *big {
                    b[..raw.len()].reverse();
                }
                Ok(match kind {
                    Number::I8 => f64::from(b[0] as i8),
                    Number::U8 => f64::from(b[0]),
                    Number::I16 => f64::from(i16::from_le_bytes([b[0], b[1]])),
                    Number::U16 => f64::from(u16::from_le_bytes([b[0], b[1]])),
                    Number::I32 => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    Number::U32 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    Number::F32 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    Number::F64 => f64::from_le_bytes(b),
                })
            }
        }
    }

    /// Whether there can be `count` more of an element at all, to keep a
    /// lying count from reserving the earth: every value takes its bytes,
    /// or as text at least a digit and a blank.
    fn has_room(&self, element: &Element, count: usize, total: usize) -> bool {
        let (left, each) = match self {
            Body::Text(_) => (total, element.properties.len() * 2),
            Body::Bytes { bytes, at, .. } => (bytes.len().saturating_sub(*at), element.properties.iter().map(|p| p.list.unwrap_or(p.kind).bytes()).sum()),
        };
        count.checked_mul(each.max(1)).is_some_and(|needed| needed <= left)
    }
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let bytes = source.bytes;
    let end = bytes.windows(10).position(|w| w == b"end_header").ok_or_else(|| malformed("The PLY file has no end to its heading."))?;
    let body_start = bytes[end..].iter().position(|&b| b == b'\n').map(|i| end + i + 1).ok_or_else(|| cut_short(NAME))?;
    let heading = String::from_utf8_lossy(&bytes[..end]);
    let (mut elements, mut format, mut picture): (Vec<Element>, Option<String>, Option<String>) = (Vec::new(), None, None);
    for line in heading.lines() {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["format", kind, ..] => format = Some(kind.to_string()),
            ["comment", "TextureFile", ..] => picture = line.split_once("TextureFile").map(|(_, name)| name.trim().to_string()),
            ["element", name, count] => {
                let count = count.parse().map_err(|_| malformed("The PLY file's heading has a count that is not a number."))?;
                elements.push(Element { name: name.to_string(), count, properties: Vec::new() });
            }
            ["property", "list", count, kind, name] => {
                let (count, kind) = (Number::named(count), Number::named(kind));
                let (Some(count), Some(kind), Some(element)) = (count, kind, elements.last_mut()) else { return Err(bad_heading()) };
                element.properties.push(Property { name: name.to_string(), list: Some(count), kind });
            }
            ["property", kind, name] => {
                let (Some(kind), Some(element)) = (Number::named(kind), elements.last_mut()) else { return Err(bad_heading()) };
                element.properties.push(Property { name: name.to_string(), list: None, kind });
            }
            _ => {}
        }
    }
    let rest = &bytes[body_start..];
    let text;
    let mut body = match format.as_deref() {
        Some("ascii") => {
            text = String::from_utf8_lossy(rest);
            Body::Text(text.split_ascii_whitespace())
        }
        Some("binary_little_endian") => Body::Bytes { bytes: rest, at: 0, big: false },
        Some("binary_big_endian") => Body::Bytes { bytes: rest, at: 0, big: true },
        _ => return Err(malformed("The PLY file does not say whether it is text or bytes.")),
    };
    let mut model = Model::new(NAME, Up::Z);
    let texture = picture.and_then(|name| {
        let found = model.texture(&name, source.companion(&name));
        if found.is_none() {
            model.note("Textures were not found in the upload; other colours were used where the file has them.");
        }
        found
    });
    let skin = texture.map_or(Skin::None, |texture| Skin::Texture { texture, tint: [255; 3] });
    let mut points: Vec<Corner> = Vec::new();
    let mut faces = 0usize;
    for element in &elements {
        if !body.has_room(element, element.count, rest.len()) {
            return Err(cut_short(NAME));
        }
        match element.name.as_str() {
            "vertex" => {
                points.reserve(element.count);
                for _ in 0..element.count {
                    points.push(read_point(element, &mut body)?);
                }
            }
            "face" | "tristrips" => {
                faces += element.count;
                for _ in 0..element.count {
                    read_face(element, &mut body, &points, skin, &mut model)?;
                }
            }
            _ => {
                for _ in 0..element.count {
                    for property in &element.properties {
                        skip(property, &mut body)?;
                    }
                }
            }
        }
    }
    if faces == 0 && !points.is_empty() {
        return Err(ImportError::Empty("The PLY file holds only points and no faces. A cloud of points cannot be turned into blocks: build a surface from it first, for example in MeshLab.".into()));
    }
    model.finish()
}

fn bad_heading() -> ImportError {
    malformed("The PLY file's heading names a kind of number that is not known.")
}

fn skip(property: &Property, body: &mut Body) -> Result<()> {
    let n = match property.list {
        Some(count) => list_length(body.next(count)?, body)?,
        None => 1,
    };
    for _ in 0..n {
        body.next(property.kind)?;
    }
    Ok(())
}

/// A list's length as written, refused if the file cannot hold that many.
fn list_length(written: f64, body: &Body) -> Result<usize> {
    let room = match body {
        Body::Text(_) => usize::MAX,
        Body::Bytes { bytes, at, .. } => bytes.len().saturating_sub(*at),
    };
    if !(0.0..=1e9).contains(&written) || written as usize > room {
        return Err(cut_short(NAME));
    }
    Ok(written as usize)
}

fn read_point(element: &Element, body: &mut Body) -> Result<Corner> {
    let mut at = [f32::NAN; 3];
    let mut colour = [None; 3];
    let mut uv = [None; 2];
    for property in &element.properties {
        if property.list.is_some() {
            skip(property, body)?;
            continue;
        }
        let value = body.next(property.kind)?;
        match property.name.as_str() {
            "x" => at[0] = value as f32,
            "y" => at[1] = value as f32,
            "z" => at[2] = value as f32,
            "red" | "r" | "diffuse_red" => colour[0] = Some(property.kind.colour(value)),
            "green" | "g" | "diffuse_green" => colour[1] = Some(property.kind.colour(value)),
            "blue" | "b" | "diffuse_blue" => colour[2] = Some(property.kind.colour(value)),
            "s" | "u" | "texture_u" => uv[0] = Some(value as f32),
            // PLY counts a picture's rows from the bottom.
            "t" | "v" | "texture_v" => uv[1] = Some(1.0 - value as f32),
            _ => {}
        }
    }
    let colour = match colour {
        [Some(r), Some(g), Some(b)] => Some([r, g, b]),
        _ => None,
    };
    let uv = match uv {
        [Some(u), Some(v)] => Some([u, v]),
        _ => None,
    };
    Ok(Corner { at, colour, uv })
}

fn read_face(element: &Element, body: &mut Body, points: &[Corner], skin: Skin, model: &mut Model) -> Result<()> {
    let mut indices: Vec<i64> = Vec::new();
    let mut places: Vec<f32> = Vec::new();
    let mut colour = [None; 3];
    for property in &element.properties {
        let Some(count) = property.list else {
            let value = body.next(property.kind)?;
            match property.name.as_str() {
                "red" => colour[0] = Some(property.kind.colour(value)),
                "green" => colour[1] = Some(property.kind.colour(value)),
                "blue" => colour[2] = Some(property.kind.colour(value)),
                _ => {}
            }
            continue;
        };
        let n = list_length(body.next(count)?, body)?;
        let is_indices = matches!(property.name.as_str(), "vertex_indices" | "vertex_index");
        for _ in 0..n {
            let value = body.next(property.kind)?;
            if is_indices {
                indices.push(value as i64);
            } else if property.name == "texcoord" {
                places.push(value as f32);
            }
        }
    }
    let corner = |i: i64| usize::try_from(i).ok().and_then(|i| points.get(i).copied());
    if element.name == "tristrips" {
        // One long strip; -1 starts it anew.
        for run in indices.split(|&i| i < 0) {
            for (k, three) in run.windows(3).enumerate() {
                let three = if k % 2 == 0 { [three[0], three[1], three[2]] } else { [three[1], three[0], three[2]] };
                match (corner(three[0]), corner(three[1]), corner(three[2])) {
                    (Some(a), Some(b), Some(c)) => model.face(&[a, b, c], skin)?,
                    _ => model.skip(),
                }
            }
        }
        return Ok(());
    }
    let corners: Option<Vec<Corner>> = indices.iter().map(|&i| corner(i)).collect();
    let Some(mut corners) = corners else {
        model.skip();
        return Ok(());
    };
    // MeshLab writes where each corner of a face lies on the picture.
    if places.len() == corners.len() * 2 {
        for (corner, place) in corners.iter_mut().zip(places.chunks_exact(2)) {
            corner.uv = Some([place[0], 1.0 - place[1]]);
        }
    }
    let skin = match (skin, colour) {
        (Skin::None, [Some(r), Some(g), Some(b)]) => {
            corners.iter_mut().for_each(|c| c.colour = None);
            Skin::Flat([r, g, b])
        }
        _ => skin,
    };
    model.face(&corners, skin)
}
