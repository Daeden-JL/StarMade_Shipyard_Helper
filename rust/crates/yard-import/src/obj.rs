//! Wavefront OBJ, with the colours of its MTL companion when that came
//! along in the same zip.

use std::collections::HashMap;

use crate::colour::byte;
use crate::error::Result;
use crate::files::Source;
use crate::model::{Corner, Model, Skin, Up};
use crate::text::{next_f32, resolve};

const NAME: &str = "Wavefront OBJ";

/// Whether a text looks like an OBJ: it has no heading to know it by, only
/// lines of points and faces.
pub(crate) fn looks_like(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(65_536)]);
    let starts = |start: &str| head.lines().any(|line| line.trim_start().starts_with(start));
    starts("v ") && starts("f ")
}

/// What an MTL file says of one material.
#[derive(Clone, Debug, Default)]
struct Material {
    colour: Option<[u8; 3]>,
    picture: Option<String>,
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let mut model = Model::new(NAME, Up::Y);
    let text = source.text();
    let mut points: Vec<Corner> = Vec::new();
    let mut places: Vec<[f32; 2]> = Vec::new();
    let mut materials: HashMap<String, Material> = HashMap::new();
    let mut skins: HashMap<String, Skin> = HashMap::new();
    let mut skin = Skin::None;
    let (mut objects, mut groups) = (0, 0);
    let mut corners: Vec<Corner> = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        // A line ending in a backslash goes on in the next.
        let line = if let Some(more) = raw.trim_end().strip_suffix('\\') {
            pending.push_str(more);
            pending.push(' ');
            continue;
        } else if pending.is_empty() {
            std::borrow::Cow::Borrowed(raw)
        } else {
            pending.push_str(raw);
            std::borrow::Cow::Owned(std::mem::take(&mut pending))
        };
        let line = line.trim();
        let (keyword, rest) = line.split_once(|c: char| c.is_ascii_whitespace()).unwrap_or((line, ""));
        let mut words = rest.split_ascii_whitespace();
        match keyword {
            "v" => {
                let numbers: Vec<f32> = words.map(|w| w.parse().unwrap_or(f32::NAN)).collect();
                let at = [0, 1, 2].map(|i| numbers.get(i).copied().unwrap_or(f32::NAN));
                // Six or seven numbers: a colour follows the place (and its
                // rarely written fourth part).
                let colour = (numbers.len() >= 6).then(|| [0, 1, 2].map(|i| byte(numbers[numbers.len() - 3 + i])));
                points.push(Corner { at, colour, uv: None });
            }
            "vt" => {
                let u = next_f32(&mut words).unwrap_or(0.0);
                let v = next_f32(&mut words).unwrap_or(0.0);
                // OBJ counts a picture's rows from the bottom.
                places.push([u, 1.0 - v]);
            }
            "f" => {
                corners.clear();
                let mut whole = true;
                for word in words {
                    let mut parts = word.split('/');
                    let point = parts.next().and_then(|p| p.parse::<i64>().ok()).and_then(|i| resolve(i, 1, points.len()));
                    let place = parts.next().and_then(|p| p.parse::<i64>().ok()).and_then(|i| resolve(i, 1, places.len()));
                    match point {
                        Some(point) => corners.push(Corner { uv: place.map(|p| places[p]), ..points[point] }),
                        None => whole = false,
                    }
                }
                if whole {
                    model.face(&corners, skin)?;
                } else {
                    model.skip();
                }
            }
            "usemtl" => {
                let name = rest.trim();
                skin = match skins.get(name) {
                    Some(known) => *known,
                    None => {
                        let found = skin_of(&mut model, &source, materials.get(name));
                        skins.insert(name.to_string(), found);
                        found
                    }
                };
            }
            "mtllib" => read_materials(&mut model, &source, rest.trim(), &mut materials),
            "o" => objects += 1,
            "g" => groups += 1,
            _ => {}
        }
    }
    // Writers name a part with `o`, with `g`, or with both.
    let parts = objects.max(groups);
    if parts > 1 {
        model.note(format!("{parts} objects were merged."));
    }
    model.finish()
}

/// Reads the MTL files a `mtllib` line names. The names may hold blanks,
/// so the whole line is tried as one name before its words are.
fn read_materials(model: &mut Model, source: &Source, names: &str, materials: &mut HashMap<String, Material>) {
    let found: Vec<&[u8]> = match source.companion(names) {
        Some(bytes) => vec![bytes],
        None => names.split_ascii_whitespace().filter_map(|name| source.companion(name)).collect(),
    };
    if found.is_empty() {
        model.note(if source.files.is_empty() {
            format!("The model's colours are in a second file, {names}. Put both files in one zip and upload that to get them.")
        } else {
            format!("The material file {names} was not found in the upload, so its colours are missing.")
        });
    }
    for bytes in found {
        parse_materials(&String::from_utf8_lossy(bytes), materials);
    }
}

fn parse_materials(text: &str, materials: &mut HashMap<String, Material>) {
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        let (keyword, rest) = line.split_once(|c: char| c.is_ascii_whitespace()).unwrap_or((line, ""));
        if keyword == "newmtl" {
            current = Some(rest.trim().to_string());
            materials.entry(rest.trim().to_string()).or_default();
            continue;
        }
        let Some(material) = current.as_ref().and_then(|name| materials.get_mut(name)) else { continue };
        match keyword {
            "Kd" => {
                let mut words = rest.split_ascii_whitespace();
                if let Some(red) = next_f32(&mut words) {
                    // One number alone stands for a grey.
                    let green = next_f32(&mut words).unwrap_or(red);
                    let blue = next_f32(&mut words).unwrap_or(red);
                    material.colour = Some([byte(red), byte(green), byte(blue)]);
                }
            }
            "map_Kd" => material.picture = Some(picture_name(rest)),
            _ => {}
        }
    }
}

/// The file name of a `map_Kd` line, after the options that may precede it.
fn picture_name(rest: &str) -> String {
    let words: Vec<&str> = rest.split_ascii_whitespace().collect();
    let mut at = 0;
    while at < words.len() && words[at].starts_with('-') && words.len() > at + 1 {
        let takes = match words[at] {
            "-mm" => 2,
            "-o" | "-s" | "-t" => words[at + 1..].iter().take(3).take_while(|w| w.parse::<f32>().is_ok()).count(),
            _ => 1,
        };
        at += 1 + takes;
    }
    words.get(at..).unwrap_or_default().join(" ")
}

fn skin_of(model: &mut Model, source: &Source, material: Option<&Material>) -> Skin {
    let Some(material) = material else { return Skin::None };
    if let Some(picture) = &material.picture {
        match model.texture(picture, source.companion(picture)) {
            // The picture carries the colour; Kd beside a picture is
            // written as anything from black to white and meant as neither.
            Some(texture) => return Skin::Texture { texture, tint: [255; 3] },
            None => model.note("Textures were not found in the upload; material colours were used."),
        }
    }
    material.colour.map_or(Skin::None, Skin::Flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_name_follows_the_options() {
        assert_eq!(picture_name("hull.png"), "hull.png");
        assert_eq!(picture_name("-s 1 1 1 -o 0 0 -clamp on my hull.png"), "my hull.png");
        assert_eq!(picture_name("-bm 0.5 -mm 0 1 a.jpg"), "a.jpg");
    }
}
