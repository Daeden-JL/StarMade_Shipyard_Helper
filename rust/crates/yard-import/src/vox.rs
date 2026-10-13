//! MagicaVoxel: blocks already, up to 256 a side per model, and a scene
//! that places several models.

use std::collections::HashMap;

use crate::error::{cut_short, too_large, Result};
use crate::files::Source;
use crate::model::{Blocks, Model, Up, MAX_BLOCKS};

const NAME: &str = "MagicaVoxel";

/// A model: its size and its blocks as x, y, z, colour number.
struct Shape<'a> {
    size: [i32; 3],
    blocks: &'a [u8],
}

/// A node of the scene.
enum Node {
    /// Moves and turns its one child.
    Place { child: i32, shift: [i32; 3], turn: [[i32; 3]; 3] },
    Group(Vec<i32>),
    Shapes(Vec<i32>),
}

const NO_TURN: [[i32; 3]; 3] = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];

/// Reads through a chunk's bytes; running out is a file cut short.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| cut_short(NAME))?;
        let out = self.bytes.get(self.at..end).ok_or_else(|| cut_short(NAME))?;
        self.at = end;
        Ok(out)
    }

    fn int(&mut self) -> Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn length(&mut self) -> Result<usize> {
        usize::try_from(self.int()?).map_err(|_| cut_short(NAME))
    }

    fn string(&mut self) -> Result<String> {
        let n = self.length()?;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn dict(&mut self) -> Result<HashMap<String, String>> {
        let n = self.length()?;
        let mut out = HashMap::new();
        for _ in 0..n {
            let key = self.string()?;
            out.insert(key, self.string()?);
        }
        Ok(out)
    }
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let mut file = Cursor { bytes: source.bytes, at: 8 };
    // The MAIN chunk holds all others; its head says how many bytes of
    // them follow, which shows a file cut short between two chunks.
    file.take(8)?;
    let promised = file.length()?;
    if source.bytes.len() - file.at < promised {
        return Err(cut_short(NAME));
    }
    let mut shapes: Vec<Shape> = Vec::new();
    let mut size = None;
    let mut palette = default_palette();
    let mut nodes: HashMap<i32, Node> = HashMap::new();
    while file.at < source.bytes.len() {
        let id = file.take(4)?;
        let (content, children) = (file.length()?, file.length()?);
        let mut chunk = Cursor { bytes: file.take(content)?, at: 0 };
        file.take(children)?;
        match id {
            b"SIZE" => size = Some([chunk.int()?, chunk.int()?, chunk.int()?]),
            b"XYZI" => {
                let n = chunk.length()?;
                let blocks = chunk.take(n.checked_mul(4).ok_or_else(|| cut_short(NAME))?)?;
                shapes.push(Shape { size: size.take().unwrap_or([256; 3]), blocks });
            }
            b"RGBA" => {
                // The file's first colour is colour number one.
                for (slot, colour) in palette.iter_mut().skip(1).zip(chunk.take(1020)?.chunks_exact(4)) {
                    *slot = [colour[0], colour[1], colour[2]];
                }
            }
            b"nTRN" => {
                let node = chunk.int()?;
                chunk.dict()?;
                let child = chunk.int()?;
                chunk.take(8)?;
                let frames = chunk.length()?;
                let frame = if frames > 0 { chunk.dict()? } else { HashMap::new() };
                let mut shift = [0; 3];
                for (slot, word) in shift.iter_mut().zip(frame.get("_t").map(String::as_str).unwrap_or("").split_ascii_whitespace()) {
                    *slot = word.parse().unwrap_or(0);
                }
                let turn = frame.get("_r").and_then(|r| r.trim().parse::<u8>().ok()).map_or(NO_TURN, turn);
                nodes.insert(node, Node::Place { child, shift, turn });
            }
            b"nGRP" | b"nSHP" => {
                let node = chunk.int()?;
                chunk.dict()?;
                let n = chunk.length()?;
                let mut members = Vec::new();
                for _ in 0..n {
                    members.push(chunk.int()?);
                    if id == b"nSHP" {
                        chunk.dict()?;
                    }
                }
                nodes.insert(node, if id == b"nSHP" { Node::Shapes(members) } else { Node::Group(members) });
            }
            _ => {}
        }
    }
    let mut model = Model::new(NAME, Up::Z);
    let mut cells = Vec::new();
    if nodes.values().any(|node| matches!(node, Node::Shapes(_))) {
        // Follow the scene from its root, node 0.
        let mut waiting = vec![(0, [0; 3], NO_TURN, 0)];
        let (mut visits, mut placed) = (0, 0);
        while let Some((id, shift, turn, depth)) = waiting.pop() {
            visits += 1;
            if depth > 64 || visits > 100_000 {
                continue;
            }
            match nodes.get(&id) {
                Some(Node::Place { child, shift: by, turn: with }) => {
                    let moved = [0, 1, 2].map(|i| shift[i] + (0..3).map(|k| turn[i][k] * by[k]).sum::<i32>());
                    waiting.push((*child, moved, times(&turn, with), depth + 1));
                }
                Some(Node::Group(members)) => waiting.extend(members.iter().map(|&member| (member, shift, turn, depth + 1))),
                Some(Node::Shapes(members)) => {
                    for shape in members.iter().filter_map(|&m| usize::try_from(m).ok().and_then(|m| shapes.get(m))) {
                        placed += 1;
                        add(&mut cells, shape, &palette, shift, &turn)?;
                    }
                }
                None => {}
            }
        }
        if placed > 1 {
            model.note(format!("{placed} objects were merged."));
        }
    } else if let Some(first) = shapes.first() {
        // Without a scene the models are frames of an animation.
        add(&mut cells, first, &palette, [0; 3], &NO_TURN)?;
        if shapes.len() > 1 {
            model.note(format!("The file holds {} models; only the first was used.", shapes.len()));
        }
    }
    model.blocks = Some(Blocks { cells });
    model.finish()
}

/// Adds a model's blocks where the scene puts it. A model is placed by
/// its middle; working in half blocks keeps a turned model on the grid.
fn add(cells: &mut Vec<([i32; 3], [u8; 3])>, shape: &Shape, palette: &[[u8; 3]; 256], shift: [i32; 3], turn: &[[i32; 3]; 3]) -> Result<()> {
    if cells.len() + shape.blocks.len() / 4 > MAX_BLOCKS {
        return Err(too_large(format!("The model has more than {MAX_BLOCKS} blocks.")));
    }
    for block in shape.blocks.chunks_exact(4) {
        let halves = [0, 1, 2].map(|i| 2 * i32::from(block[i]) + 1 - 2 * (shape.size[i].clamp(0, 4096) / 2));
        let at = [0, 1, 2].map(|i| shift[i].saturating_add((0..3).map(|k| turn[i][k] * halves[k]).sum::<i32>().div_euclid(2)));
        cells.push((at, palette[block[3] as usize]));
    }
    Ok(())
}

/// The turn a scene node's `_r` number stands for: which column of each of
/// the first two rows holds its one entry, and the three rows' signs.
fn turn(packed: u8) -> [[i32; 3]; 3] {
    let (first, second) = ((packed & 3) as usize, (packed >> 2 & 3) as usize);
    if first > 2 || second > 2 || first == second {
        return NO_TURN;
    }
    let mut out = [[0; 3]; 3];
    for (row, column) in [first, second, 3 - first - second].into_iter().enumerate() {
        out[row][column] = if packed >> (4 + row) & 1 == 1 { -1 } else { 1 };
    }
    out
}

fn times(a: &[[i32; 3]; 3], b: &[[i32; 3]; 3]) -> [[i32; 3]; 3] {
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// The colours a file without a palette of its own uses: a cube of six
/// steps per part, then ramps of red, green, blue and grey.
fn default_palette() -> [[u8; 3]; 256] {
    let mut out = [[0u8; 3]; 256];
    let steps = [0xff, 0xcc, 0x99, 0x66, 0x33, 0x00];
    for i in 0..215 {
        out[i + 1] = [steps[i / 36], steps[i / 6 % 6], steps[i % 6]];
    }
    let ramp = [0xee, 0xdd, 0xbb, 0xaa, 0x88, 0x77, 0x55, 0x44, 0x22, 0x11];
    for (i, &v) in ramp.iter().enumerate() {
        out[216 + i] = [v, 0, 0];
        out[226 + i] = [0, v, 0];
        out[236 + i] = [0, 0, v];
        out[246 + i] = [v, v, v];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_palette_has_its_known_corners() {
        let p = default_palette();
        assert_eq!(p[1], [255, 255, 255]);
        assert_eq!(p[2], [255, 255, 0xcc]);
        assert_eq!(p[7], [255, 0xcc, 255]);
        assert_eq!(p[215], [0, 0, 0x33]);
        assert_eq!(p[216], [0xee, 0, 0]);
        assert_eq!(p[255], [0x11, 0x11, 0x11]);
    }

    #[test]
    fn the_packed_turn_of_four_is_no_turn() {
        assert_eq!(turn(4), NO_TURN);
        // Rows: (0 1 0), (-1 0 0), (0 0 1): a quarter turn about z.
        assert_eq!(turn(1 | (1 << 5)), [[0, 1, 0], [-1, 0, 0], [0, 0, 1]]);
    }
}
