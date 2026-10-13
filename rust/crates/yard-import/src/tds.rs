//! 3D Studio (.3ds): nested chunks, each a two-byte name and a length.

use std::collections::HashMap;

use crate::error::{cut_short, Result};
use crate::files::Source;
use crate::model::{Corner, Model, Skin, Up};

const NAME: &str = "3D Studio";

pub(crate) fn looks_like(bytes: &[u8]) -> bool {
    let word = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let length = bytes.get(2..6).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    word(0) == Some(0x4d4d) && (length == Some(bytes.len()) || matches!(word(6), Some(0x0002 | 0x3d3d)))
}

/// The chunks lying one after another in some bytes.
fn chunks(mut bytes: &[u8]) -> impl Iterator<Item = Result<(u16, &[u8])>> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let head = bytes.get(..6);
        let length = head.map(|h| u32::from_le_bytes([h[2], h[3], h[4], h[5]]) as usize);
        match (head, length.and_then(|length| bytes.get(6..length))) {
            (Some(head), Some(body)) => {
                bytes = &bytes[6 + body.len()..];
                Some(Ok((u16::from_le_bytes([head[0], head[1]]), body)))
            }
            _ => {
                bytes = &[];
                Some(Err(cut_short(NAME)))
            }
        }
    })
}

/// A name that ends with a zero byte, and what follows it.
fn name(bytes: &[u8]) -> Result<(String, &[u8])> {
    let end = bytes.iter().position(|&b| b == 0).ok_or_else(|| cut_short(NAME))?;
    Ok((String::from_utf8_lossy(&bytes[..end]).into_owned(), &bytes[end + 1..]))
}

fn count(bytes: &[u8]) -> Result<(usize, &[u8])> {
    let n = bytes.get(..2).ok_or_else(|| cut_short(NAME))?;
    Ok((u16::from_le_bytes([n[0], n[1]]) as usize, &bytes[2..]))
}

fn float(bytes: &[u8]) -> f32 {
    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[derive(Default)]
struct Material {
    colour: Option<[u8; 3]>,
    picture: Option<String>,
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let mut model = Model::new(NAME, Up::Z);
    let mut materials: HashMap<String, Material> = HashMap::new();
    let mut objects: Vec<&[u8]> = Vec::new();
    // Only the first chunk is the file; some writers leave odds and ends
    // after it.
    for main in chunks(source.bytes).take(1) {
        let (id, body) = main?;
        if id != 0x4d4d {
            continue;
        }
        for top in chunks(body) {
            let (id, body) = top?;
            if id != 0x3d3d {
                continue;
            }
            for part in chunks(body) {
                match part? {
                    (0xafff, body) => {
                        let (name, material) = read_material(body)?;
                        materials.insert(name, material);
                    }
                    (0x4000, body) => objects.push(name(body)?.1),
                    _ => {}
                }
            }
        }
    }
    let mut skins: HashMap<String, Skin> = HashMap::new();
    let mut meshes = 0;
    for object in objects {
        for part in chunks(object) {
            if let (0x4100, body) = part? {
                meshes += 1;
                read_mesh(body, &source, &materials, &mut skins, &mut model)?;
            }
        }
    }
    if meshes > 1 {
        model.note(format!("{meshes} objects were merged."));
    }
    model.finish()
}

fn read_material(bytes: &[u8]) -> Result<(String, Material)> {
    let (mut called, mut material) = (String::new(), Material::default());
    for part in chunks(bytes) {
        match part? {
            (0xa000, body) => called = name(body)?.0,
            (0xa020, body) => {
                for colour in chunks(body) {
                    match colour? {
                        (0x0011 | 0x0012, rgb) if rgb.len() >= 3 => material.colour = Some([rgb[0], rgb[1], rgb[2]]),
                        (0x0010 | 0x0013, rgb) if rgb.len() >= 12 => material.colour = Some([0, 4, 8].map(|at| crate::colour::byte(float(&rgb[at..])))),
                        _ => {}
                    }
                }
            }
            (0xa200, body) => {
                for map in chunks(body) {
                    if let (0xa300, file) = map? {
                        material.picture = Some(name(file)?.0);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((called, material))
}

fn read_mesh(bytes: &[u8], source: &Source, materials: &HashMap<String, Material>, skins: &mut HashMap<String, Skin>, model: &mut Model) -> Result<()> {
    let mut corners: Vec<Corner> = Vec::new();
    let mut places: Vec<[f32; 2]> = Vec::new();
    let mut faces: Vec<[usize; 3]> = Vec::new();
    let mut face_skins: Vec<Skin> = Vec::new();
    for part in chunks(bytes) {
        match part? {
            (0x4110, body) => {
                let (n, rest) = count(body)?;
                let points = rest.get(..n * 12).ok_or_else(|| cut_short(NAME))?;
                corners = points.chunks_exact(12).map(|p| Corner::bare([float(p), float(&p[4..]), float(&p[8..])])).collect();
            }
            (0x4140, body) => {
                let (n, rest) = count(body)?;
                let pairs = rest.get(..n * 8).ok_or_else(|| cut_short(NAME))?;
                // 3D Studio counts a picture's rows from the bottom.
                places = pairs.chunks_exact(8).map(|p| [float(p), 1.0 - float(&p[4..])]).collect();
            }
            (0x4120, body) => {
                let (n, rest) = count(body)?;
                let list = rest.get(..n * 8).ok_or_else(|| cut_short(NAME))?;
                faces = list.chunks_exact(8).map(|f| [0, 2, 4].map(|at| u16::from_le_bytes([f[at], f[at + 1]]) as usize)).collect();
                face_skins = vec![Skin::None; faces.len()];
                // After the faces: which of them wear which material.
                for group in chunks(&rest[n * 8..]) {
                    let (0x4130, body) = group? else { continue };
                    let (called, rest) = name(body)?;
                    let (n, rest) = count(rest)?;
                    let members = rest.get(..n * 2).ok_or_else(|| cut_short(NAME))?;
                    let skin = match skins.get(&called) {
                        Some(known) => *known,
                        None => {
                            let found = skin_of(model, source, materials.get(&called));
                            skins.insert(called, found);
                            found
                        }
                    };
                    for member in members.chunks_exact(2) {
                        if let Some(slot) = face_skins.get_mut(u16::from_le_bytes([member[0], member[1]]) as usize) {
                            *slot = skin;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (corner, place) in corners.iter_mut().zip(&places) {
        corner.uv = Some(*place);
    }
    for (face, skin) in faces.iter().zip(face_skins) {
        match (corners.get(face[0]), corners.get(face[1]), corners.get(face[2])) {
            (Some(a), Some(b), Some(c)) => model.face(&[*a, *b, *c], skin)?,
            _ => model.skip(),
        }
    }
    Ok(())
}

fn skin_of(model: &mut Model, source: &Source, material: Option<&Material>) -> Skin {
    let Some(material) = material else { return Skin::None };
    if let Some(picture) = &material.picture {
        match model.texture(picture, source.companion(picture)) {
            Some(texture) => return Skin::Texture { texture, tint: [255; 3] },
            None => model.note("Textures were not found in the upload; material colours were used."),
        }
    }
    material.colour.map_or(Skin::None, Skin::Flat)
}
