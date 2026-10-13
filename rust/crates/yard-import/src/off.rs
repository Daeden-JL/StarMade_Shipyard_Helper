//! OFF: a list of points and a list of faces, as text. The COFF kind and
//! many plain files carry a colour after each point or face.

use crate::colour::byte;
use crate::error::{cut_short, malformed, unsupported, Result};
use crate::files::Source;
use crate::model::{Corner, Model, Skin, Up};

const NAME: &str = "OFF";

/// Whether a word is an OFF heading: OFF with any of the letters that say
/// what else each point carries in front.
pub(crate) fn is_heading(word: &str) -> bool {
    word.strip_suffix("OFF").is_some_and(|before| before.len() <= 5 && before.chars().all(|c| "STCN4n".contains(c)))
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let text = source.text();
    let mut lines = text.lines().map(|line| line.split('#').next().unwrap_or("").split_ascii_whitespace().collect::<Vec<_>>()).filter(|words| !words.is_empty());
    let mut first = lines.next().ok_or_else(|| cut_short(NAME))?;
    let mut normals = false;
    if is_heading(first[0]) {
        if first[0].contains('4') || first[0].contains('n') {
            return Err(unsupported("This OFF file has points in more than three dimensions, which cannot be read."));
        }
        normals = first[0].contains('N');
        first.remove(0);
        if first.is_empty() {
            first = lines.next().ok_or_else(|| cut_short(NAME))?;
        }
    }
    let count = |i: usize| first.get(i).and_then(|w| w.parse::<usize>().ok()).ok_or_else(|| malformed("The OFF file does not start with its counts of points and faces."));
    let (point_count, face_count) = (count(0)?, count(1)?);
    if point_count > text.len() || face_count > text.len() {
        return Err(cut_short(NAME));
    }
    let mut points = Vec::with_capacity(point_count);
    for _ in 0..point_count {
        let words = lines.next().ok_or_else(|| cut_short(NAME))?;
        let mut at = [0.0f32; 3];
        for (i, slot) in at.iter_mut().enumerate() {
            *slot = words.get(i).and_then(|w| w.parse().ok()).ok_or_else(|| malformed("A point in the OFF file is not three numbers."))?;
        }
        let rest = words.get(if normals { 6 } else { 3 }..).unwrap_or_default();
        points.push(Corner { at, colour: colour(rest), uv: None });
    }
    let mut model = Model::new(NAME, Up::Y);
    for _ in 0..face_count {
        let words = lines.next().ok_or_else(|| cut_short(NAME))?;
        let n = words[0].parse::<usize>().map_err(|_| malformed("A face in the OFF file does not start with its number of corners."))?;
        let indices = words.get(1..1 + n).ok_or_else(|| cut_short(NAME))?;
        let corners: Option<Vec<Corner>> = indices.iter().map(|w| w.parse::<usize>().ok().and_then(|i| points.get(i).copied())).collect();
        let skin = colour(&words[1 + n..]).map_or(Skin::None, Skin::Flat);
        match corners {
            // A face's own colour goes before its corners' colours.
            Some(mut corners) => {
                if skin != Skin::None {
                    corners.iter_mut().for_each(|c| c.colour = None);
                }
                model.face(&corners, skin)?
            }
            None => model.skip(),
        }
    }
    model.finish()
}

/// A colour as three or four numbers: whole numbers up to 255, or parts of
/// one when written with a point.
fn colour(words: &[&str]) -> Option<[u8; 3]> {
    if words.len() < 3 {
        return None;
    }
    let parts = words.iter().any(|w| w.contains('.'));
    let mut out = [0u8; 3];
    for (slot, word) in out.iter_mut().zip(words) {
        let value: f32 = word.parse().ok()?;
        *slot = if parts { byte(value) } else { value.clamp(0.0, 255.0) as u8 };
    }
    Some(out)
}
