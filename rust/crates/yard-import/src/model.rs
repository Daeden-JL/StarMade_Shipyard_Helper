//! What a file's contents turn out to be: triangles with their colours, or
//! a ready-made grid of blocks.

use crate::error::{too_large, ImportError, Result};
use crate::math::wide;

/// More triangles than this and the file is refused. Each one takes 68
/// bytes, so the largest model held is about 340 MB.
pub const MAX_TRIANGLES: usize = 5_000_000;
/// More blocks than this, in a ready-made grid or in a result, is refused.
pub const MAX_BLOCKS: usize = 40_000_000;
/// Textures wider or taller than this are thinned out on reading: a block
/// takes one texel, so more would only cost memory.
const MAX_TEXTURE_SIDE: u32 = 2048;
const MAX_TEXTURES: usize = 256;

/// Which axis of the file points up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Up {
    Y,
    Z,
}

/// Where a triangle gets its colour from. Colours are sRGB bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    /// The file says nothing about this triangle's colour.
    None,
    /// One colour all over.
    Flat([u8; 3]),
    /// A colour at each corner, blended in between.
    Corners([[u8; 3]; 3]),
    /// A picture: `texture` is an index into the model's textures, `uv` where
    /// each corner lies on it (0,0 is the top left, 1,1 the bottom right, and
    /// the picture repeats beyond), `tint` is multiplied in.
    Textured { texture: u32, tint: [u8; 3], uv: [[f32; 2]; 3] },
}

/// One triangle in the file's own units and axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub corners: [[f32; 3]; 3],
    pub paint: Paint,
}

/// A picture triangles are painted with: rows of red, green, blue bytes
/// from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Texture {
    /// The texel at a place on the picture; the picture repeats.
    pub fn sample(&self, uv: [f32; 2]) -> [u8; 3] {
        if self.width == 0 || self.height == 0 {
            return [255; 3];
        }
        let wrap = |t: f32, side: u32| {
            let t = if t.is_finite() { t - t.floor() } else { 0.0 };
            ((t * side as f32) as u32).min(side - 1) as usize
        };
        let at = (wrap(uv[1], self.height) * self.width as usize + wrap(uv[0], self.width)) * 3;
        match self.rgb.get(at..at + 3) {
            Some(texel) => [texel[0], texel[1], texel[2]],
            None => [255; 3],
        }
    }

    /// Reads a PNG or JPEG. A picture that cannot be read gives nothing; the
    /// reader then falls back to the material's plain colour.
    pub(crate) fn decode(bytes: &[u8]) -> Option<Texture> {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(512 * 1024 * 1024);
        reader.limits(limits);
        let picture = reader.decode().ok()?.into_rgb8();
        let (width, height) = picture.dimensions();
        let step = width.max(height).div_ceil(MAX_TEXTURE_SIDE).max(1);
        if step == 1 {
            return Some(Texture { width, height, rgb: picture.into_raw() });
        }
        let (small_w, small_h) = ((width / step).max(1), (height / step).max(1));
        let mut rgb = Vec::with_capacity((small_w * small_h * 3) as usize);
        for y in 0..small_h {
            for x in 0..small_w {
                rgb.extend_from_slice(&picture.get_pixel((x * step).min(width - 1), (y * step).min(height - 1)).0);
            }
        }
        Some(Texture { width: small_w, height: small_h, rgb })
    }
}

/// Blocks a file already holds, for formats that are grids of blocks
/// themselves. Positions are in the file's own axes and may start anywhere.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Blocks {
    pub cells: Vec<([i32; 3], [u8; 3])>,
}

/// A model as read from a file, before it is turned into blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The name of the format the file was read as, such as "Wavefront OBJ".
    pub format: &'static str,
    /// The surface, in the file's own units and axes.
    pub triangles: Vec<Triangle>,
    /// The pictures the triangles' paint refers to.
    pub textures: Vec<Texture>,
    /// Set for files that are blocks already (MagicaVoxel); `triangles` is
    /// then empty.
    pub blocks: Option<Blocks>,
    /// Which axis points up by the format's convention or the file's header.
    pub up: Up,
    /// Things the player should know about how the file was read.
    pub notes: Vec<String>,
    skipped: usize,
    texture_names: Vec<String>,
}

/// One corner of a face as a reader hands it over.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Corner {
    pub at: [f32; 3],
    pub colour: Option<[u8; 3]>,
    pub uv: Option<[f32; 2]>,
}

impl Corner {
    pub fn bare(at: [f32; 3]) -> Corner {
        Corner { at, colour: None, uv: None }
    }
}

/// What a face's material gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Skin {
    None,
    Flat([u8; 3]),
    Texture { texture: u32, tint: [u8; 3] },
}

impl Model {
    /// A model made in code rather than read from a file.
    pub fn from_triangles(triangles: Vec<Triangle>, textures: Vec<Texture>, up: Up) -> Model {
        Model { triangles, textures, ..Model::new("triangles", up) }
    }

    /// Ready-made blocks made in code rather than read from a file.
    pub fn from_blocks(blocks: Blocks, up: Up) -> Model {
        Model { blocks: Some(blocks), ..Model::new("blocks", up) }
    }

    pub(crate) fn new(format: &'static str, up: Up) -> Model {
        Model { format, triangles: Vec::new(), textures: Vec::new(), blocks: None, up, notes: Vec::new(), skipped: 0, texture_names: Vec::new() }
    }

    pub(crate) fn note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    /// Adds a triangle. One with a corner that is not a number is left out
    /// and counted; one too many is an error.
    pub(crate) fn push(&mut self, corners: [[f32; 3]; 3], paint: Paint) -> Result<()> {
        if corners.iter().flatten().any(|c| !c.is_finite()) {
            self.skipped += 1;
            return Ok(());
        }
        if self.triangles.len() >= MAX_TRIANGLES {
            return Err(too_large(format!("The model has more than {MAX_TRIANGLES} triangles. Simplify it and try again.")));
        }
        self.triangles.push(Triangle { corners, paint });
        Ok(())
    }

    /// Counts a face that could not be used (an index pointing nowhere).
    pub(crate) fn skip(&mut self) {
        self.skipped += 1;
    }

    /// Adds a face of three or more corners, cut into triangles. The colour
    /// comes from the texture if the skin has one and the corners say where
    /// they lie on it, else from the corners' own colours, else from the
    /// skin's plain colour.
    pub(crate) fn face(&mut self, corners: &[Corner], skin: Skin) -> Result<()> {
        if corners.len() < 3 {
            self.skipped += 1;
            return Ok(());
        }
        if corners.len() == 3 {
            return self.push([corners[0].at, corners[1].at, corners[2].at], paint(&[corners[0], corners[1], corners[2]], skin));
        }
        let points: Vec<[f32; 3]> = corners.iter().map(|c| c.at).collect();
        for [a, b, c] in triangulate(&points) {
            let three = [corners[a], corners[b], corners[c]];
            self.push([three[0].at, three[1].at, three[2].at], paint(&three, skin))?;
        }
        Ok(())
    }

    /// The index of a picture, read once however often it is asked for by
    /// the same name. Nothing if it is missing or cannot be read.
    pub(crate) fn texture(&mut self, name: &str, bytes: Option<&[u8]>) -> Option<u32> {
        if let Some(known) = self.texture_names.iter().position(|n| n == name) {
            return Some(known as u32);
        }
        if self.textures.len() >= MAX_TEXTURES {
            return None;
        }
        let texture = Texture::decode(bytes?)?;
        self.textures.push(texture);
        self.texture_names.push(name.to_string());
        Some(self.textures.len() as u32 - 1)
    }

    /// Whether any triangle has a colour of its own.
    pub fn has_colours(&self) -> bool {
        self.triangles.iter().any(|t| t.paint != Paint::None) || self.blocks.is_some()
    }

    /// Checks the model holds something and says what was left out.
    pub(crate) fn finish(mut self) -> Result<Model> {
        let empty = match &self.blocks {
            Some(blocks) => blocks.cells.is_empty(),
            None => self.triangles.is_empty(),
        };
        if empty {
            return Err(ImportError::Empty(if self.skipped > 0 {
                "The file has no usable triangles: every one of them is broken.".into()
            } else {
                "The file has no triangles.".into()
            }));
        }
        if self.skipped > 0 {
            let note = if self.skipped == 1 { "1 broken face was skipped.".to_string() } else { format!("{} broken faces were skipped.", self.skipped) };
            self.note(note);
        }
        self.triangles.shrink_to_fit();
        Ok(self)
    }
}

fn paint(corners: &[Corner; 3], skin: Skin) -> Paint {
    if let (Skin::Texture { texture, tint }, [Some(a), Some(b), Some(c)]) = (skin, [corners[0].uv, corners[1].uv, corners[2].uv]) {
        return Paint::Textured { texture, tint, uv: [a, b, c] };
    }
    if let [Some(a), Some(b), Some(c)] = [corners[0].colour, corners[1].colour, corners[2].colour] {
        return if a == b && b == c { Paint::Flat(a) } else { Paint::Corners([a, b, c]) };
    }
    match skin {
        Skin::Flat(colour) | Skin::Texture { tint: colour, .. } => Paint::Flat(colour),
        Skin::None => Paint::None,
    }
}

/// Cuts a flat face into triangles, as corner numbers. A face that bulges
/// outward everywhere is cut as a fan; one with dents has its ears clipped
/// so that no triangle lies outside it.
pub(crate) fn triangulate(points: &[[f32; 3]]) -> Vec<[usize; 3]> {
    let n = points.len();
    let fan = |ring: &[usize]| (1..ring.len().saturating_sub(1)).map(|i| [ring[0], ring[i], ring[i + 1]]).collect::<Vec<_>>();
    let all: Vec<usize> = (0..n).collect();
    if n <= 3 || n > 512 {
        return fan(&all);
    }
    // Newell's method: the face's normal, whatever its shape.
    let mut normal = [0.0f64; 3];
    for i in 0..n {
        let (a, b) = (wide(points[i]), wide(points[(i + 1) % n]));
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let axis = (0..3).fold(0, |best, i| if normal[i].abs() > normal[best].abs() { i } else { best });
    if normal[axis] == 0.0 || !normal[axis].is_finite() {
        return fan(&all);
    }
    // Looking down the normal's main axis the face runs anticlockwise.
    let (u, v, sign) = ((axis + 1) % 3, (axis + 2) % 3, normal[axis].signum());
    let flat: Vec<[f64; 2]> = points.iter().map(|p| [f64::from(p[u]), f64::from(p[v]) * sign]).collect();
    let turn = |a: usize, b: usize, c: usize| (flat[b][0] - flat[a][0]) * (flat[c][1] - flat[a][1]) - (flat[b][1] - flat[a][1]) * (flat[c][0] - flat[a][0]);
    if (0..n).all(|i| turn(i, (i + 1) % n, (i + 2) % n) >= 0.0) {
        return fan(&all);
    }
    let mut ring = all;
    let mut out = Vec::with_capacity(n - 2);
    while ring.len() > 3 {
        let m = ring.len();
        let ear = (0..m).find(|&i| {
            let (a, b, c) = (ring[(i + m - 1) % m], ring[i], ring[(i + 1) % m]);
            turn(a, b, c) > 0.0 && ring.iter().all(|&p| p == a || p == b || p == c || !(turn(a, b, p) >= 0.0 && turn(b, c, p) >= 0.0 && turn(c, a, p) >= 0.0))
        });
        match ear {
            Some(i) => {
                out.push([ring[(i + m - 1) % m], ring[i], ring[(i + 1) % m]]);
                ring.remove(i);
            }
            // A face that crosses itself has no ear left; a fan is the best
            // that can be done with it.
            None => break,
        }
    }
    out.extend(fan(&ring));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dented_face_is_cut_without_covering_the_dent() {
        // An L shape: the corner at (2,2) is cut out.
        let l = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 2.0, 0.0]];
        let triangles = triangulate(&l);
        assert_eq!(triangles.len(), 4);
        let area: f32 = triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| l[i]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-6, "area {area}");
    }

    #[test]
    fn a_texture_repeats_beyond_its_edges() {
        let t = Texture { width: 2, height: 1, rgb: vec![255, 0, 0, 0, 0, 255] };
        assert_eq!(t.sample([0.25, 0.5]), [255, 0, 0]);
        assert_eq!(t.sample([1.75, -3.5]), [0, 0, 255]);
        assert_eq!(t.sample([f32::NAN, 0.0]), [255, 0, 0]);
    }

    #[test]
    fn a_triangle_with_a_corner_that_is_not_a_number_is_skipped_and_noted() {
        let mut model = Model::new("test", Up::Y);
        model.push([[0.0; 3], [1.0, 0.0, 0.0], [0.0, f32::NAN, 0.0]], Paint::None).unwrap();
        model.push([[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], Paint::None).unwrap();
        let model = model.finish().unwrap();
        assert_eq!(model.triangles.len(), 1);
        assert_eq!(model.notes, vec!["1 broken face was skipped."]);
    }
}
