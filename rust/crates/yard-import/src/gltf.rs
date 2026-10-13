//! glTF 2.0, as one binary file (.glb) or as text (.gltf) with its data
//! inside it or in companion files.

use std::borrow::Cow;

use base64::Engine;
use serde_json::Value;

use crate::colour::from_linear;
use crate::error::{cut_short, malformed, unsupported, ImportError, Result};
use crate::files::Source;
use crate::math::Mat4;
use crate::model::{Corner, Model, Skin, Up, MAX_TRIANGLES};

const NAME: &str = "glTF";
/// A scene may use a part many times over, and a broken one may contain
/// itself. This many steps down and this many parts in all are followed.
const MAX_DEPTH: usize = 64;
const MAX_VISITS: usize = 1_000_000;

struct Reader<'a> {
    doc: Value,
    source: Source<'a>,
    buffers: Vec<Option<Cow<'a, [u8]>>>,
    model: Model,
    /// What each material gives, worked out when first met.
    skins: Vec<Option<Look>>,
}

/// A material: the skin for its faces, the light its base colour lets
/// through (which corner colours are multiplied by), and which set of
/// picture places it uses, moved as the material says.
#[derive(Clone, Copy)]
struct Look {
    skin: Skin,
    factor: [f32; 3],
    places: u64,
    moved: Option<[[f32; 3]; 2]>,
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let (json, binary) = if source.bytes.starts_with(b"glTF") { split_binary(source.bytes)? } else { (source.bytes, None) };
    let doc: Value = serde_json::from_slice(json).map_err(|_| malformed("The glTF file's description is cut short or damaged."))?;
    if doc["asset"]["version"].as_str().is_some_and(|v| v.starts_with('1')) {
        return Err(unsupported("This is a glTF 1.0 file; only glTF 2.0 can be read. Export it again with a current exporter."));
    }
    let names = |key: &str| doc[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    let (required, used) = (names("extensionsRequired"), names("extensionsUsed"));
    if let Some(error) = required.iter().find_map(|name| packed(name)) {
        return Err(error);
    }
    let buffers = load_buffers(&doc, &source, binary)?;
    let material_count = doc["materials"].as_array().map_or(0, Vec::len);
    let mut reader = Reader { doc, source, buffers, model: Model::new(NAME, Up::Y), skins: vec![None; material_count] };
    reader.walk()?;
    if reader.model.triangles.is_empty() {
        if let Some(error) = used.iter().find_map(|name| packed(name)) {
            return Err(error);
        }
    }
    reader.model.finish()
}

/// The error for a file whose data is packed in a way that cannot be read.
fn packed(extension: &str) -> Option<ImportError> {
    match extension {
        "KHR_draco_mesh_compression" => Some(unsupported("The glTF file is compressed with Draco, which cannot be read. Export it again without compression.")),
        "EXT_meshopt_compression" | "KHR_meshopt_compression" => Some(unsupported("The glTF file is compressed with Meshopt, which cannot be read. Export it again without compression.")),
        _ => None,
    }
}

/// The description and the data of a binary glTF.
fn split_binary(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| cut_short(NAME));
    if word(4)? != 2 {
        return Err(unsupported("This is a glTF 1.0 file; only glTF 2.0 can be read. Export it again with a current exporter."));
    }
    if word(8)? > bytes.len() {
        return Err(cut_short(NAME));
    }
    let (mut json, mut binary) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let (length, kind) = (word(at)?, word(at + 4)?);
        let chunk = bytes.get(at + 8..at + 8 + length).ok_or_else(|| cut_short(NAME))?;
        match kind {
            0x4e4f_534a if json.is_none() => json = Some(chunk),
            0x004e_4942 if binary.is_none() => binary = Some(chunk),
            _ => {}
        }
        at += 8 + length + (4 - length % 4) % 4;
    }
    Ok((json.ok_or_else(|| cut_short(NAME))?, binary))
}

fn load_buffers<'a>(doc: &Value, source: &Source<'a>, binary: Option<&'a [u8]>) -> Result<Vec<Option<Cow<'a, [u8]>>>> {
    let mut out = Vec::new();
    for (i, buffer) in doc["buffers"].as_array().into_iter().flatten().enumerate() {
        out.push(match buffer["uri"].as_str() {
            None if i == 0 => binary.map(Cow::Borrowed),
            None => None,
            Some(uri) => Some(match data_uri(uri) {
                Some(data) => Cow::Owned(data?),
                None => Cow::Borrowed(source.companion(uri).ok_or_else(|| ImportError::Missing(format!("The glTF file keeps its data in a second file, {uri}, which was not in the upload. Put the .gltf, its .bin and its pictures in one zip and upload that, or export a single .glb.")))?),
            }),
        });
    }
    Ok(out)
}

/// The bytes written into a "data:" link, if the link is one.
fn data_uri(uri: &str) -> Option<Result<Vec<u8>>> {
    let rest = uri.strip_prefix("data:")?;
    let (_, data) = rest.split_once(";base64,")?;
    Some(base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|_| malformed("The data written into the glTF file is cut short or damaged.")))
}

/// One list of numbers a mesh is made of.
struct Numbers {
    /// How many numbers make one entry.
    each: usize,
    values: Vec<f32>,
}

impl Numbers {
    fn len(&self) -> usize {
        self.values.len() / self.each.max(1)
    }

    fn entry(&self, i: usize) -> Option<&[f32]> {
        self.values.get(i * self.each..(i + 1) * self.each)
    }
}

/// Reads the number of a kind at a place; `unit` scales whole numbers that
/// stand for parts of one.
fn number(bytes: &[u8], kind: u64, unit: bool) -> Option<f32> {
    Some(match kind {
        5120 => {
            let v = f32::from(*bytes.first()? as i8);
            if unit { (v / 127.0).max(-1.0) } else { v }
        }
        5121 => {
            let v = f32::from(*bytes.first()?);
            if unit { v / 255.0 } else { v }
        }
        5122 => {
            let v = f32::from(i16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]));
            if unit { (v / 32767.0).max(-1.0) } else { v }
        }
        5123 => {
            let v = f32::from(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]));
            if unit { v / 65535.0 } else { v }
        }
        5125 => u32::from_le_bytes([*bytes.first()?, *bytes.get(1)?, *bytes.get(2)?, *bytes.get(3)?]) as f32,
        5126 => f32::from_le_bytes([*bytes.first()?, *bytes.get(1)?, *bytes.get(2)?, *bytes.get(3)?]),
        _ => return None,
    })
}

fn width(kind: u64) -> Option<usize> {
    match kind {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

impl<'a> Reader<'a> {
    /// The bytes of a buffer view from an offset on, and the step from one
    /// entry to the next if the view sets one.
    fn view(&self, index: &Value, offset: u64) -> Result<(&[u8], Option<usize>)> {
        let view = &self.doc["bufferViews"][index.as_u64().ok_or_else(broken)? as usize];
        let buffer = self.buffers.get(view["buffer"].as_u64().ok_or_else(broken)? as usize).and_then(Option::as_ref).ok_or_else(broken)?;
        let start = view["byteOffset"].as_u64().unwrap_or(0);
        let length = view["byteLength"].as_u64().ok_or_else(broken)?;
        let end = start.checked_add(length).ok_or_else(broken)?;
        let bytes = buffer.get(start.checked_add(offset).ok_or_else(broken)? as usize..end as usize).ok_or_else(|| cut_short(NAME))?;
        Ok((bytes, view["byteStride"].as_u64().map(|s| s as usize)))
    }

    /// Reads an accessor: the numbers of one attribute or index list.
    fn numbers(&self, index: &Value) -> Result<Numbers> {
        let accessor = &self.doc["accessors"][index.as_u64().ok_or_else(broken)? as usize];
        let count = accessor["count"].as_u64().ok_or_else(broken)? as usize;
        let kind = accessor["componentType"].as_u64().ok_or_else(broken)?;
        let size = width(kind).ok_or_else(broken)?;
        let each = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") | Some("MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            _ => return Err(broken()),
        };
        let unit = accessor["normalized"].as_bool().unwrap_or(false);
        let mut values;
        if accessor["bufferView"].is_null() {
            // No data of its own: all zeros, for a sparse list to fill in.
            if count > MAX_TRIANGLES * 3 {
                return Err(broken());
            }
            values = vec![0.0; count * each];
        } else {
            let (bytes, stride) = self.view(&accessor["bufferView"], accessor["byteOffset"].as_u64().unwrap_or(0))?;
            // A step shorter than an entry would let a small file claim a
            // huge list.
            let stride = stride.unwrap_or(0).max(size * each);
            let needed = count.checked_sub(1).map_or(Some(0), |n| n.checked_mul(stride).and_then(|n| n.checked_add(size * each)));
            if needed.is_none_or(|needed| needed > bytes.len()) {
                return Err(cut_short(NAME));
            }
            values = Vec::with_capacity(count * each);
            for i in 0..count {
                for part in 0..each {
                    values.push(number(&bytes[i * stride + part * size..], kind, unit).ok_or_else(broken)?);
                }
            }
        }
        let sparse = &accessor["sparse"];
        if sparse.is_object() {
            let n = sparse["count"].as_u64().ok_or_else(broken)? as usize;
            let index_kind = sparse["indices"]["componentType"].as_u64().ok_or_else(broken)?;
            let index_size = width(index_kind).ok_or_else(broken)?;
            let (indices, _) = self.view(&sparse["indices"]["bufferView"], sparse["indices"]["byteOffset"].as_u64().unwrap_or(0))?;
            let (changed, _) = self.view(&sparse["values"]["bufferView"], sparse["values"]["byteOffset"].as_u64().unwrap_or(0))?;
            for i in 0..n {
                let at = number(indices.get(i * index_size..).ok_or_else(|| cut_short(NAME))?, index_kind, false).ok_or_else(|| cut_short(NAME))? as usize;
                for part in 0..each {
                    let value = number(changed.get((i * each + part) * size..).ok_or_else(|| cut_short(NAME))?, kind, unit).ok_or_else(|| cut_short(NAME))?;
                    if let Some(slot) = values.get_mut(at * each + part) {
                        *slot = value;
                    }
                }
            }
        }
        Ok(Numbers { each, values })
    }

    /// Follows the scene down from its roots, placing every mesh.
    fn walk(&mut self) -> Result<()> {
        let nodes = self.doc["nodes"].as_array().map_or(0, Vec::len);
        let scene = self.doc["scene"].as_u64().unwrap_or(0) as usize;
        let mut roots: Vec<usize> = self.doc["scenes"][scene]["nodes"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
        if roots.is_empty() {
            // No scene: every node that is nobody's child is a root.
            let mut is_child = vec![false; nodes];
            for node in self.doc["nodes"].as_array().into_iter().flatten() {
                for child in node["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
                    if let Some(slot) = is_child.get_mut(child as usize) {
                        *slot = true;
                    }
                }
            }
            roots = (0..nodes).filter(|&n| !is_child[n]).collect();
        }
        let mut waiting: Vec<(usize, Mat4, usize)> = roots.into_iter().rev().map(|n| (n, Mat4::SAME, 0)).collect();
        let (mut visits, mut meshes, mut left_out) = (0, 0, false);
        while let Some((index, above, depth)) = waiting.pop() {
            visits += 1;
            if index >= nodes || depth > MAX_DEPTH || visits > MAX_VISITS {
                continue;
            }
            let node = &self.doc["nodes"][index];
            let placed = above.times(&node_transform(node));
            let children: Vec<usize> = node["children"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
            let mesh = node["mesh"].as_u64();
            waiting.extend(children.into_iter().rev().map(|n| (n, placed, depth + 1)));
            if let Some(mesh) = mesh {
                meshes += 1;
                let primitives = self.doc["meshes"][mesh as usize]["primitives"].as_array().map_or(0, Vec::len);
                for primitive in 0..primitives {
                    left_out |= !self.primitive(mesh as usize, primitive, &placed)?;
                }
            }
        }
        if meshes > 1 {
            self.model.note(format!("{meshes} objects were merged."));
        }
        if left_out {
            self.model.note("Lines and points in the file were left out.");
        }
        Ok(())
    }

    /// Adds one part of a mesh. False if it is made of lines or points.
    fn primitive(&mut self, mesh: usize, primitive: usize, placed: &Mat4) -> Result<bool> {
        let part = self.doc["meshes"][mesh]["primitives"][primitive].clone();
        let mode = part["mode"].as_u64().unwrap_or(4);
        if !(4..=6).contains(&mode) {
            return Ok(false);
        }
        let attributes = &part["attributes"];
        if attributes["POSITION"].is_null() {
            return Ok(true);
        }
        let positions = self.numbers(&attributes["POSITION"])?;
        if positions.each < 3 {
            return Err(broken());
        }
        let look = match part["material"].as_u64() {
            Some(material) => self.look(material as usize),
            None => Look { skin: Skin::None, factor: [1.0; 3], places: 0, moved: None },
        };
        let colours = if attributes["COLOR_0"].is_null() { None } else { Some(self.numbers(&attributes["COLOR_0"])?) };
        let places = &attributes[format!("TEXCOORD_{}", look.places)];
        let places = if places.is_null() || !matches!(look.skin, Skin::Texture { .. }) { None } else { Some(self.numbers(places)?) };
        let corners: Vec<Corner> = (0..positions.len())
            .map(|i| {
                let p = &positions.values[i * positions.each..];
                let colour = colours.as_ref().and_then(|c| c.entry(i)).filter(|c| c.len() >= 3).map(|c| [0, 1, 2].map(|k| from_linear(c[k] * look.factor[k])));
                let uv = places.as_ref().and_then(|p| p.entry(i)).filter(|p| p.len() >= 2).map(|p| match look.moved {
                    Some(m) => [m[0][0] * p[0] + m[0][1] * p[1] + m[0][2], m[1][0] * p[0] + m[1][1] * p[1] + m[1][2]],
                    None => [p[0], p[1]],
                });
                Corner { at: placed.point([p[0], p[1], p[2]]), colour, uv }
            })
            .collect();
        let order: Vec<usize> = if part["indices"].is_null() { (0..corners.len()).collect() } else { self.numbers(&part["indices"])?.values.iter().map(|&i| i as usize).collect() };
        let mut add = |a: usize, b: usize, c: usize| match (corners.get(a), corners.get(b), corners.get(c)) {
            (Some(a), Some(b), Some(c)) => self.model.face(&[*a, *b, *c], look.skin),
            _ => {
                self.model.skip();
                Ok(())
            }
        };
        match mode {
            4 => order.chunks_exact(3).try_for_each(|t| add(t[0], t[1], t[2]))?,
            5 => order.windows(3).enumerate().try_for_each(|(i, t)| if i % 2 == 0 { add(t[0], t[1], t[2]) } else { add(t[1], t[0], t[2]) })?,
            _ => order.windows(2).skip(1).try_for_each(|t| add(order[0], t[0], t[1]))?,
        }
        Ok(true)
    }

    /// What a material gives its faces.
    fn look(&mut self, index: usize) -> Look {
        if let Some(Some(known)) = self.skins.get(index) {
            return *known;
        }
        let material = self.doc["materials"][index].clone();
        let rough = &material["pbrMetallicRoughness"];
        let glossy = &material["extensions"]["KHR_materials_pbrSpecularGlossiness"];
        let (factor, picture) = if !rough["baseColorFactor"].is_null() || !rough["baseColorTexture"].is_null() || glossy.is_null() { (&rough["baseColorFactor"], &rough["baseColorTexture"]) } else { (&glossy["diffuseFactor"], &glossy["diffuseTexture"]) };
        let said = !factor.is_null() || !picture.is_null();
        let factor = [0, 1, 2].map(|i| factor[i].as_f64().unwrap_or(1.0) as f32);
        let tint = factor.map(from_linear);
        let texture = if picture.is_null() { None } else { self.picture(picture["index"].as_u64().unwrap_or(u64::MAX) as usize) };
        let skin = match texture {
            Some(texture) => Skin::Texture { texture, tint },
            None if said => Skin::Flat(tint),
            // A material that says nothing of its colour is glTF's plain
            // white; that is no more than "no colour".
            None => Skin::None,
        };
        let look = Look { skin, factor, places: picture["texCoord"].as_u64().unwrap_or(0), moved: moved(&picture["extensions"]["KHR_texture_transform"]) };
        if let Some(slot) = self.skins.get_mut(index) {
            *slot = Some(look);
        }
        look
    }

    /// Loads the picture a texture stands for.
    fn picture(&mut self, texture: usize) -> Option<u32> {
        let entry = &self.doc["textures"][texture];
        let Some(image) = entry["source"].as_u64() else {
            self.model.note("Some textures are in a format that cannot be read (KTX2 or WebP); material colours were used.");
            return None;
        };
        let image = &self.doc["images"][image as usize];
        let name = format!("image {}", entry["source"]);
        let embedded;
        let bytes: Option<&[u8]> = if let Some(uri) = image["uri"].as_str() {
            match data_uri(uri) {
                Some(data) => {
                    embedded = data.ok();
                    embedded.as_deref()
                }
                None => self.source.companion(uri),
            }
        } else {
            self.view(&image["bufferView"], 0).ok().map(|(bytes, _)| bytes)
        };
        let missing = bytes.is_none();
        // The picture is copied out so the model can be changed while the
        // buffers are still borrowed.
        let bytes = bytes.map(<[u8]>::to_vec);
        let found = self.model.texture(&name, bytes.as_deref());
        if found.is_none() {
            self.model.note(if missing { "Textures were not found in the upload; material colours were used." } else { "Some textures could not be read; material colours were used." });
        }
        found
    }
}

/// How a material shifts, turns and scales its picture places
/// (KHR_texture_transform), as two rows for u and v.
fn moved(transform: &Value) -> Option<[[f32; 3]; 2]> {
    if !transform.is_object() {
        return None;
    }
    let pair = |key: &str, default: f32| [0, 1].map(|i| transform[key][i].as_f64().map_or(default, |v| v as f32));
    let (offset, scale) = (pair("offset", 0.0), pair("scale", 1.0));
    let (sin, cos) = (transform["rotation"].as_f64().unwrap_or(0.0) as f32).sin_cos();
    Some([[cos * scale[0], sin * scale[1], offset[0]], [-sin * scale[0], cos * scale[1], offset[1]]])
}

fn node_transform(node: &Value) -> Mat4 {
    let list = |key: &str| node[key].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(f64::NAN)).collect::<Vec<f64>>());
    if let Some(matrix) = list("matrix").and_then(|m| Mat4::from_columns(&m)) {
        return matrix;
    }
    let mut out = Mat4::SAME;
    if let Some(t) = list("translation").filter(|t| t.len() == 3 && t.iter().all(|v| v.is_finite())) {
        out = out.times(&Mat4::shift([t[0], t[1], t[2]]));
    }
    if let Some(r) = list("rotation").filter(|r| r.len() == 4 && r.iter().all(|v| v.is_finite())) {
        out = out.times(&Mat4::quaternion([r[0], r[1], r[2], r[3]]));
    }
    if let Some(s) = list("scale").filter(|s| s.len() == 3 && s.iter().all(|v| v.is_finite())) {
        out = out.times(&Mat4::stretch([s[0], s[1], s[2]]));
    }
    out
}

fn broken() -> ImportError {
    malformed("The glTF file's description does not fit its data; the file is damaged.")
}
