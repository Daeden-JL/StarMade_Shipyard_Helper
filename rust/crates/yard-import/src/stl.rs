//! STL: bare triangles, as bytes or as text. No colours to speak of, though
//! two makers of the binary kind hide one in each triangle's spare bytes.

use crate::error::{cut_short, Result};
use crate::files::Source;
use crate::model::{Model, Paint, Up};

const NAME: &str = "STL";

/// Whether the bytes are a binary STL: its length must be the 84-byte
/// head plus 50 bytes for each triangle the head counts. This is asked
/// before looking for the word "solid", which binary files may start with
/// too.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    count(bytes).is_some_and(|n| n > 0 && 84 + n * 50 == bytes.len() as u64)
}

fn count(bytes: &[u8]) -> Option<u64> {
    let n = bytes.get(80..84)?;
    Some(u64::from(u32::from_le_bytes([n[0], n[1], n[2], n[3]])))
}

/// Whether the bytes read as a text STL.
pub(crate) fn is_text(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    text.trim_start().starts_with("solid") && (text.contains("facet") || text.contains("endsolid"))
}

pub(crate) fn read(source: Source) -> Result<Model> {
    if is_binary(source.bytes) || !is_text(source.bytes) {
        read_binary(source.bytes)
    } else {
        read_text(source.bytes)
    }
}

fn read_binary(bytes: &[u8]) -> Result<Model> {
    let mut model = Model::new(NAME, Up::Z);
    let n = count(bytes).ok_or_else(|| cut_short(NAME))?;
    let body = bytes.get(84..).unwrap_or_default();
    if n == 0 || (body.len() as u64) < n * 50 {
        return Err(cut_short(NAME));
    }
    // Materialise Magics writes "COLOR=" and a colour for the whole part in
    // the head, and then a set top bit means "use that one".
    let whole = bytes[..80].windows(6).position(|w| w == b"COLOR=").and_then(|at| bytes.get(at + 6..at + 9)).map(|c| [c[0], c[1], c[2]]);
    let five = |bits: u16| ((bits & 31) * 255 / 31) as u8;
    for record in body.chunks_exact(50).take(n as usize) {
        let number = |at: usize| f32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]]);
        let corners = [12, 24, 36].map(|at| [number(at), number(at + 4), number(at + 8)]);
        let spare = u16::from_le_bytes([record[48], record[49]]);
        let paint = match whole {
            Some(whole) if spare & 0x8000 != 0 => Paint::Flat(whole),
            Some(_) => Paint::Flat([five(spare), five(spare >> 5), five(spare >> 10)]),
            // VisCAM and SolidView: a set top bit means the colour is there.
            None if spare & 0x8000 != 0 => Paint::Flat([five(spare >> 10), five(spare >> 5), five(spare)]),
            None => Paint::None,
        };
        model.push(corners, paint)?;
    }
    model.finish()
}

fn read_text(bytes: &[u8]) -> Result<Model> {
    let mut model = Model::new(NAME, Up::Z);
    let text = String::from_utf8_lossy(bytes);
    let mut words = text.split_ascii_whitespace();
    let mut corners: Vec<[f32; 3]> = Vec::with_capacity(3);
    let mut solids = 0;
    let mut ended = false;
    while let Some(word) = words.next() {
        match word {
            "vertex" => {
                let point = crate::text::next_point(&mut words).ok_or_else(|| cut_short(NAME))?;
                corners.push(point);
            }
            "endloop" => {
                let points: Vec<_> = corners.drain(..).map(crate::model::Corner::bare).collect();
                model.face(&points, crate::model::Skin::None)?;
            }
            "solid" => {
                solids += 1;
                ended = false;
            }
            "endsolid" => ended = true,
            _ => {}
        }
    }
    if !ended || !corners.is_empty() {
        return Err(cut_short(NAME));
    }
    if solids > 1 {
        model.note(format!("{solids} objects were merged."));
    }
    model.finish()
}
