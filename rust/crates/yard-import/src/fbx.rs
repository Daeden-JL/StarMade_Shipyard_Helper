//! FBX, as bytes or as text: a tree of named records. Geometry, the models
//! that place it, and materials are separate objects tied together by a
//! list of connections.

use std::cell::Cell;
use std::collections::HashMap;
use std::io::Read;

use crate::colour::byte;
use crate::error::{cut_short, malformed, too_large, Result};
use crate::files::Source;
use crate::math::Mat4;
use crate::model::{Corner, Model, Skin, Up};

const NAME: &str = "FBX";
const MAX_DEPTH: usize = 64;
const MAX_RECORDS: usize = 4_000_000;
/// The most bytes all packed arrays of a file may unpack to.
const MAX_UNPACKED: u64 = 512 * 1024 * 1024;

/// One value of a record.
#[derive(Debug)]
enum Value<'a> {
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(&'a [u8]),
    /// An array as the binary form stores it: maybe packed with zlib.
    Array { kind: u8, count: usize, packed: bool, bytes: &'a [u8] },
    /// An array of the text form.
    Numbers(Vec<f64>),
}

#[derive(Debug, Default)]
struct Record<'a> {
    name: String,
    values: Vec<Value<'a>>,
    children: Vec<Record<'a>>,
}

impl<'a> Record<'a> {
    fn child(&self, name: &str) -> Option<&Record<'a>> {
        self.children.iter().find(|c| c.name == name)
    }

    fn text(&self, i: usize) -> Option<&str> {
        match self.values.get(i)? {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// What the record is known by in connections: a number in newer
    /// files, its name in older ones.
    fn id(&self) -> Option<String> {
        match self.values.first()? {
            Value::Int(id) => Some(id.to_string()),
            Value::Text(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// The record's numbers, however it holds them: as an array, as plain
    /// values (older text files), or in an "a" record below it.
    fn numbers(&self, budget: &Cell<u64>) -> Result<Vec<f64>> {
        match self.values.first() {
            Some(Value::Array { kind, count, packed, bytes }) => unpack(*kind, *count, *packed, bytes, budget),
            Some(Value::Numbers(numbers)) => Ok(numbers.clone()),
            _ => match self.child("a") {
                Some(inner) => inner.numbers(budget),
                None => Ok(self.values.iter().filter_map(Value::number).collect()),
            },
        }
    }
}

impl Value<'_> {
    fn number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Unpacks an array of the binary form into numbers.
fn unpack(kind: u8, count: usize, packed: bool, bytes: &[u8], budget: &Cell<u64>) -> Result<Vec<f64>> {
    let each = match kind {
        b'f' | b'i' => 4,
        b'd' | b'l' => 8,
        _ => 1,
    };
    let length = count.checked_mul(each).ok_or_else(|| cut_short(NAME))?;
    if length as u64 > budget.get() {
        return Err(too_large(format!("The FBX file unpacks to more than {} MB.", MAX_UNPACKED / (1024 * 1024))));
    }
    let unpacked;
    let raw = if packed {
        let mut out = Vec::with_capacity(length);
        flate2::read::ZlibDecoder::new(bytes).take(length as u64).read_to_end(&mut out).map_err(|_| cut_short(NAME))?;
        unpacked = out;
        &unpacked[..]
    } else {
        bytes
    };
    if raw.len() < length {
        return Err(cut_short(NAME));
    }
    budget.set(budget.get() - length as u64);
    Ok(raw[..length]
        .chunks_exact(each)
        .map(|b| match kind {
            b'f' => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            b'i' => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            b'd' => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            b'l' => i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f64,
            _ => f64::from(b[0]),
        })
        .collect())
}

/// Whether a record is worth keeping: of the top sections only the three
/// that describe the model, and of the objects only what gives shape and
/// colour. Animation makes up most of many files and is passed over.
fn wanted(depth: usize, name: &str) -> bool {
    match depth {
        0 => matches!(name, "GlobalSettings" | "Objects" | "Connections"),
        1 => matches!(name, "Geometry" | "Model" | "Material" | "Texture" | "Video" | "C" | "Connect" | "Properties70" | "Properties60" | "P" | "Property"),
        // Shading hints: large, and of no use for blocks.
        _ => !matches!(name, "LayerElementNormal" | "LayerElementBinormal" | "LayerElementTangent" | "LayerElementSmoothing" | "Edges"),
    }
}

/// Reads the binary form.
struct Binary<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Newer files write their offsets in eight bytes.
    wide: bool,
    records: usize,
}

impl<'a> Binary<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| cut_short(NAME))?;
        let out = self.bytes.get(self.at..end).ok_or_else(|| cut_short(NAME))?;
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<usize> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn offset(&mut self) -> Result<usize> {
        if !self.wide {
            return self.u32();
        }
        let b = self.take(8)?;
        usize::try_from(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])).map_err(|_| cut_short(NAME))
    }

    /// The next record, or nothing at the empty record that ends a list.
    fn record(&mut self, depth: usize) -> Result<Option<Record<'a>>> {
        let (end, count, length) = (self.offset()?, self.offset()?, self.offset()?);
        let name_length = self.take(1)?[0] as usize;
        let name = String::from_utf8_lossy(self.take(name_length)?).into_owned();
        if end == 0 {
            return Ok(None);
        }
        self.records += 1;
        if end > self.bytes.len() || end < self.at || depth > MAX_DEPTH || self.records > MAX_RECORDS {
            return Err(cut_short(NAME));
        }
        let mut record = Record { name, values: Vec::new(), children: Vec::new() };
        if !wanted(depth, &record.name) {
            self.at = end;
            return Ok(Some(record));
        }
        let values_end = self.at.checked_add(length).filter(|&e| e <= end).ok_or_else(|| cut_short(NAME))?;
        for _ in 0..count {
            record.values.push(self.value()?);
        }
        self.at = values_end;
        while self.at < end {
            match self.record(depth + 1)? {
                Some(child) => record.children.push(child),
                None => break,
            }
        }
        self.at = end;
        Ok(Some(record))
    }

    fn value(&mut self) -> Result<Value<'a>> {
        let kind = self.take(1)?[0];
        Ok(match kind {
            b'Y' => {
                let b = self.take(2)?;
                Value::Int(i64::from(i16::from_le_bytes([b[0], b[1]])))
            }
            b'C' => Value::Int(i64::from(self.take(1)?[0])),
            b'I' => {
                let b = self.take(4)?;
                Value::Int(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            }
            b'F' => {
                let b = self.take(4)?;
                Value::Float(f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            }
            b'D' => {
                let b = self.take(8)?;
                Value::Float(f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            }
            b'L' => {
                let b = self.take(8)?;
                Value::Int(i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            }
            b'S' => {
                let n = self.u32()?;
                Value::Text(String::from_utf8_lossy(self.take(n)?).into_owned())
            }
            b'R' => {
                let n = self.u32()?;
                Value::Bytes(self.take(n)?)
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => {
                let (count, packing, length) = (self.u32()?, self.u32()?, self.u32()?);
                Value::Array { kind, count, packed: packing == 1, bytes: self.take(length)? }
            }
            _ => return Err(malformed("The FBX file holds a kind of value that is not known; it is damaged.")),
        })
    }
}

fn read_binary(bytes: &[u8]) -> Result<Vec<Record<'_>>> {
    let version = bytes.get(23..27).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| cut_short(NAME))?;
    let mut reader = Binary { bytes, at: 27, wide: version >= 7500, records: 0 };
    let mut out = Vec::new();
    // The records end with an empty one; a footer of other bytes follows.
    loop {
        match reader.record(0)? {
            Some(record) => out.push(record),
            None => return Ok(out),
        }
    }
}

/// One word of the text form.
#[derive(Debug, PartialEq)]
enum Token {
    /// A name with its colon.
    Key(String),
    Text(String),
    Number(f64),
    /// "*24": how many numbers the array below holds.
    Count,
    Open,
    Close,
    Word,
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    peeked: Option<Token>,
}

impl Lexer<'_> {
    fn peek(&mut self) -> Option<&Token> {
        if self.peeked.is_none() {
            self.peeked = self.read();
        }
        self.peeked.as_ref()
    }

    fn next(&mut self) -> Option<Token> {
        self.peeked.take().or_else(|| self.read())
    }

    fn read(&mut self) -> Option<Token> {
        loop {
            match self.bytes.get(self.at)? {
                b';' => self.at += self.bytes[self.at..].iter().position(|&b| b == b'\n').unwrap_or(self.bytes.len() - self.at),
                b if b.is_ascii_whitespace() || *b == b',' => self.at += 1,
                _ => break,
            }
        }
        let start = self.at;
        match self.bytes[start] {
            b'{' => {
                self.at += 1;
                Some(Token::Open)
            }
            b'}' => {
                self.at += 1;
                Some(Token::Close)
            }
            b'"' => {
                let length = self.bytes[start + 1..].iter().position(|&b| b == b'"').unwrap_or(self.bytes.len() - start - 1);
                self.at = (start + length + 2).min(self.bytes.len());
                Some(Token::Text(String::from_utf8_lossy(&self.bytes[start + 1..start + 1 + length]).into_owned()))
            }
            _ => {
                let length = self.bytes[start..].iter().position(|&b| b.is_ascii_whitespace() || matches!(b, b',' | b'{' | b'}' | b'"')).unwrap_or(self.bytes.len() - start);
                self.at = start + length.max(1);
                let word = String::from_utf8_lossy(&self.bytes[start..self.at]);
                Some(if let Some(name) = word.strip_suffix(':') {
                    Token::Key(name.to_string())
                } else if word.starts_with('*') {
                    Token::Count
                } else if let Ok(number) = word.parse::<f64>() {
                    Token::Number(number)
                } else {
                    Token::Word
                })
            }
        }
    }

    /// The records up to the closing brace or the end of the file.
    fn records(&mut self, depth: usize, count: &mut usize) -> Result<Vec<Record<'static>>> {
        if depth > MAX_DEPTH {
            return Err(malformed("The FBX file is nested too deeply; it is damaged."));
        }
        let mut out = Vec::new();
        loop {
            let name = match self.next() {
                None if depth == 0 => return Ok(out),
                None => return Err(cut_short(NAME)),
                Some(Token::Close) => return Ok(out),
                Some(Token::Key(name)) => name,
                Some(_) => continue,
            };
            *count += 1;
            if *count > MAX_RECORDS {
                return Err(too_large("The FBX file holds too many records."));
            }
            let mut record = Record { name, values: Vec::new(), children: Vec::new() };
            let mut numbers: Vec<f64> = Vec::new();
            let mut only_numbers = true;
            loop {
                match self.peek() {
                    Some(Token::Number(n)) => numbers.push(*n),
                    Some(Token::Text(_)) | Some(Token::Word) | Some(Token::Count) => only_numbers = false,
                    _ => break,
                }
                match self.next() {
                    Some(Token::Number(n)) if n.fract() == 0.0 && n.abs() < 9e15 => record.values.push(Value::Int(n as i64)),
                    Some(Token::Number(n)) => record.values.push(Value::Float(n)),
                    Some(Token::Text(text)) => record.values.push(Value::Text(text)),
                    _ => {}
                }
            }
            // A long run of numbers is an array; held as one it takes a
            // third of the room.
            if only_numbers && numbers.len() > 16 {
                record.values = vec![Value::Numbers(numbers)];
            }
            if self.peek() == Some(&Token::Open) {
                self.next();
                record.children = self.records(depth + 1, count)?;
            }
            if wanted(depth, &record.name) {
                out.push(record);
            }
        }
    }
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let records = if source.bytes.starts_with(b"Kaydara FBX Binary") {
        read_binary(source.bytes)?
    } else {
        let mut count = 0;
        Lexer { bytes: source.bytes, at: 0, peeked: None }.records(0, &mut count)?
    };
    let budget = Cell::new(MAX_UNPACKED);
    let mut scene = Scene { source, budget: &budget, model: Model::new(NAME, Up::Y), objects: HashMap::new(), links: Vec::new(), skins: HashMap::new() };
    let top = |name: &str| records.iter().find(|r| r.name == name);
    if let Some(up) = top("GlobalSettings").and_then(|settings| property(settings, "UpAxis")).and_then(|values| values.first().copied()) {
        match up as i64 {
            2 => scene.model.up = Up::Z,
            0 => scene.model.note("The file says its x axis points up, which is not supported; it was read as y up."),
            _ => {}
        }
    }
    for object in top("Objects").into_iter().flat_map(|objects| &objects.children) {
        if let Some(id) = object.id() {
            scene.objects.insert(id, object);
        }
    }
    for link in top("Connections").into_iter().flat_map(|c| &c.children) {
        let end = |i: usize| link.values.get(i).and_then(|v| match v {
            Value::Int(id) => Some(id.to_string()),
            Value::Text(name) => Some(name.clone()),
            _ => None,
        });
        if let (Some(child), Some(parent)) = (end(1), end(2)) {
            scene.links.push((child, parent, link.text(3).map(str::to_string)));
        }
    }
    scene.build()?;
    scene.model.finish()
}

/// The numbers of a named property of an object, from the property list
/// of either generation of the format.
fn property(record: &Record, name: &str) -> Option<Vec<f64>> {
    let list = record.child("Properties70").or_else(|| record.child("Properties60"))?;
    let found = list.children.iter().find(|p| p.text(0) == Some(name))?;
    // After the name come the type and its notes as text, then the values.
    Some(found.values.iter().skip(1).filter_map(Value::number).collect())
}

fn triple(record: &Record, name: &str, default: f64) -> [f64; 3] {
    match property(record, name) {
        Some(v) if v.len() >= 3 && v.iter().all(|c| c.is_finite()) => [v[0], v[1], v[2]],
        _ => [default; 3],
    }
}

/// A turn from three angles in degrees, about x, then y, then z unless
/// `order` says otherwise.
fn turn(degrees: [f64; 3], order: i64) -> Mat4 {
    let about = |axis: usize| {
        let mut along = [0.0; 3];
        along[axis] = 1.0;
        Mat4::about(along, degrees[axis].to_radians())
    };
    // Each order names the axes from the first turned about to the last.
    let [first, second, third] = match order {
        1 => [0, 2, 1],
        2 => [1, 2, 0],
        3 => [1, 0, 2],
        4 => [2, 0, 1],
        5 => [2, 1, 0],
        _ => [0, 1, 2],
    };
    about(third).times(&about(second)).times(&about(first))
}

/// Where a model stands in its parent, by the long recipe of the format:
/// shift, offsets and pivots around a turn and around a stretch.
fn local(model: &Record) -> Mat4 {
    let shift = |name: &str| Mat4::shift(triple(model, name, 0.0));
    let order = property(model, "RotationOrder").and_then(|v| v.first().copied()).map_or(0, |o| o as i64);
    let (turn_pivot, stretch_pivot) = (shift("RotationPivot"), shift("ScalingPivot"));
    let back = |pivot: &Mat4| pivot.inverse().unwrap_or(Mat4::SAME);
    let post = turn(triple(model, "PostRotation", 0.0), 0).inverse().unwrap_or(Mat4::SAME);
    [
        shift("Lcl Translation"),
        shift("RotationOffset"),
        turn_pivot,
        turn(triple(model, "PreRotation", 0.0), 0),
        turn(triple(model, "Lcl Rotation", 0.0), order),
        post,
        back(&turn_pivot),
        shift("ScalingOffset"),
        stretch_pivot,
        Mat4::stretch(triple(model, "Lcl Scaling", 1.0)),
        back(&stretch_pivot),
    ]
    .iter()
    .fold(Mat4::SAME, |all, step| all.times(step))
}

/// How a model's geometry alone is moved on top of the model's own place.
fn geometric(model: &Record) -> Mat4 {
    Mat4::shift(triple(model, "GeometricTranslation", 0.0)).times(&turn(triple(model, "GeometricRotation", 0.0), 0)).times(&Mat4::stretch(triple(model, "GeometricScaling", 1.0)))
}

struct Scene<'a, 'r> {
    source: Source<'a>,
    budget: &'r Cell<u64>,
    model: Model,
    objects: HashMap<String, &'r Record<'r>>,
    /// Child, parent, and the parent's property the child is tied to.
    links: Vec<(String, String, Option<String>)>,
    skins: HashMap<String, Skin>,
}

impl<'r> Scene<'_, 'r> {
    fn kind(&self, id: &str) -> Option<&'r str> {
        self.objects.get(id).map(|r| r.name.as_str())
    }

    /// The objects of a kind tied to a parent, in the order they were tied.
    fn tied(&self, parent: &str, kind: &str) -> Vec<&String> {
        self.links.iter().filter(|(child, to, _)| to == parent && self.kind(child) == Some(kind)).map(|(child, _, _)| child).collect()
    }

    fn build(&mut self) -> Result<()> {
        let models: Vec<(String, &'r Record<'r>)> = self.objects.iter().filter(|(_, r)| r.name == "Model").map(|(id, r)| (id.clone(), *r)).collect();
        let parents: HashMap<String, String> = self.links.iter().filter(|(child, parent, _)| self.kind(child) == Some("Model") && self.kind(parent) == Some("Model")).map(|(child, parent, _)| (child.clone(), parent.clone())).collect();
        let mut meshes = 0;
        let mut ordered = models.clone();
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, model) in &ordered {
            // Older files keep a model's geometry in the model itself.
            let mut shapes: Vec<&'r Record<'r>> = self.tied(id, "Geometry").into_iter().filter_map(|g| self.objects.get(g).copied()).collect();
            if model.child("Vertices").is_some() {
                shapes.push(model);
            }
            if shapes.is_empty() {
                continue;
            }
            let mut placed = local(model);
            let mut at = id;
            for _ in 0..MAX_DEPTH {
                let Some(parent) = parents.get(at) else { break };
                if let Some(record) = self.objects.get(parent) {
                    placed = local(record).times(&placed);
                }
                at = parent;
            }
            let placed = placed.times(&geometric(model));
            let materials: Vec<String> = self.tied(id, "Material").into_iter().cloned().collect();
            let skins: Vec<Skin> = materials.iter().map(|m| self.skin(m)).collect();
            for shape in shapes {
                meshes += 1;
                self.geometry(shape, &placed, &skins)?;
            }
        }
        if meshes == 0 {
            // Geometry no model lays claim to: used as it lies.
            let loose: Vec<&'r Record<'r>> = self.objects.values().filter(|r| r.name == "Geometry").copied().collect();
            for shape in &loose {
                self.geometry(shape, &Mat4::SAME, &[])?;
            }
            if !loose.is_empty() {
                self.model.note("The file does not say where its parts go; they were used as they lie.");
            }
        }
        if meshes > 1 {
            self.model.note(format!("{meshes} objects were merged."));
        }
        Ok(())
    }

    fn geometry(&mut self, shape: &Record, placed: &Mat4, skins: &[Skin]) -> Result<()> {
        let (Some(points), Some(order)) = (shape.child("Vertices"), shape.child("PolygonVertexIndex")) else { return Ok(()) };
        let points = points.numbers(self.budget)?;
        let order = order.numbers(self.budget)?;
        let colours = Layer::read(shape, "LayerElementColor", "Colors", "ColorIndex", self.budget)?;
        let places = Layer::read(shape, "LayerElementUV", "UV", "UVIndex", self.budget)?;
        let wearing = Layer::read(shape, "LayerElementMaterial", "Materials", "", self.budget)?;
        let mut corners: Vec<Corner> = Vec::new();
        let mut polygon = 0;
        for (nth, &written) in order.iter().enumerate() {
            // The last corner of a face is written as minus itself less one.
            let last = written < 0.0;
            let point = if last { -written - 1.0 } else { written } as usize;
            let at = points.get(point.saturating_mul(3)..point.saturating_mul(3).saturating_add(3)).map(|p| placed.point([p[0] as f32, p[1] as f32, p[2] as f32]));
            let colour = colours.as_ref().and_then(|c| c.at(nth, point, polygon, 4)).map(|c| [byte(c[0] as f32), byte(c[1] as f32), byte(c[2] as f32)]);
            // FBX counts a picture's rows from the bottom.
            let uv = places.as_ref().and_then(|p| p.at(nth, point, polygon, 2)).map(|p| [p[0] as f32, 1.0 - p[1] as f32]);
            corners.push(Corner { at: at.unwrap_or([f32::NAN; 3]), colour, uv });
            if last {
                let slot = wearing.as_ref().and_then(|w| w.at(nth, point, polygon, 1)).map_or(0, |m| m[0] as usize);
                let skin = skins.get(slot).or(skins.first()).copied().unwrap_or(Skin::None);
                self.model.face(&corners, skin)?;
                corners.clear();
                polygon += 1;
            }
        }
        Ok(())
    }

    /// What a material gives: the picture tied to its diffuse colour, or
    /// that colour.
    fn skin(&mut self, id: &str) -> Skin {
        if let Some(known) = self.skins.get(id) {
            return *known;
        }
        let skin = self.find_skin(id);
        self.skins.insert(id.to_string(), skin);
        skin
    }

    fn find_skin(&mut self, id: &str) -> Skin {
        let Some(material) = self.objects.get(id).copied() else { return Skin::None };
        let flat = property(material, "DiffuseColor").or_else(|| property(material, "Diffuse")).filter(|c| c.len() >= 3).map(|c| [byte(c[0] as f32), byte(c[1] as f32), byte(c[2] as f32)]);
        let textures: Vec<&(String, String, Option<String>)> = self.links.iter().filter(|(child, to, _)| to == id && self.kind(child) == Some("Texture")).collect();
        let diffuse = textures.iter().find(|(_, _, tied)| tied.as_deref().is_some_and(|t| t.contains("Diffuse"))).or(textures.iter().find(|(_, _, tied)| tied.is_none()));
        let Some((texture, _, _)) = diffuse else { return flat.map_or(Skin::None, Skin::Flat) };
        let Some(record) = self.objects.get(texture).copied() else { return flat.map_or(Skin::None, Skin::Flat) };
        // The picture may be packed into the file, in a Video object tied
        // to the texture, or lie beside it.
        let packed = self.tied(texture, "Video").into_iter().filter_map(|v| self.objects.get(v)).find_map(|video| match video.child("Content")?.values.first()? {
            Value::Bytes(bytes) if !bytes.is_empty() => Some(*bytes),
            _ => None,
        });
        let names: Vec<&str> = ["RelativeFilename", "FileName"].iter().filter_map(|key| record.child(key)?.text(0)).collect();
        let beside = names.iter().find_map(|name| self.source.companion(name));
        match self.model.texture(texture, packed.or(beside)) {
            Some(texture) => Skin::Texture { texture, tint: [255; 3] },
            None => {
                self.model.note("Textures were not found in the upload; material colours were used.");
                flat.map_or(Skin::None, Skin::Flat)
            }
        }
    }
}

/// Something a geometry gives per corner, per point or per face: its
/// values, and how a corner finds its own among them.
struct Layer {
    values: Vec<f64>,
    indices: Option<Vec<f64>>,
    by: By,
}

enum By {
    Corner,
    Point,
    Polygon,
    All,
}

impl Layer {
    fn read(shape: &Record, layer: &str, values: &str, indices: &str, budget: &Cell<u64>) -> Result<Option<Layer>> {
        let Some(layer) = shape.child(layer) else { return Ok(None) };
        let Some(values) = layer.child(values) else { return Ok(None) };
        let by = match layer.child("MappingInformationType").and_then(|m| m.text(0)) {
            Some("ByPolygonVertex") => By::Corner,
            Some("ByVertice") | Some("ByVertex") | Some("ByControlPoint") => By::Point,
            Some("ByPolygon") => By::Polygon,
            Some("AllSame") => By::All,
            _ => return Ok(None),
        };
        let direct = layer.child("ReferenceInformationType").and_then(|r| r.text(0)) == Some("Direct");
        let indices = match layer.child(indices) {
            Some(indices) if !direct => Some(indices.numbers(budget)?),
            _ => None,
        };
        Ok(Some(Layer { values: values.numbers(budget)?, indices, by }))
    }

    /// The `each` values that belong to a corner: the `nth` corner of the
    /// whole geometry, at point `point`, of face `polygon`.
    fn at(&self, nth: usize, point: usize, polygon: usize, each: usize) -> Option<&[f64]> {
        let which = match self.by {
            By::Corner => nth,
            By::Point => point,
            By::Polygon => polygon,
            By::All => 0,
        };
        let entry = match &self.indices {
            Some(indices) => usize::try_from(*indices.get(which)? as i64).ok()?,
            None => which,
        };
        self.values.get(entry.checked_mul(each)?..entry.checked_mul(each)?.checked_add(each)?)
    }
}
