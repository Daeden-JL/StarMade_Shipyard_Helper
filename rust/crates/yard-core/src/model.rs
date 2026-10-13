//! The small 3D models the game draws for some blocks in place of a textured
//! cube: light bars, consoles, pipes, ladders, buttons, capsules.
//!
//! The models belong to the game. The server reads them from the install and
//! hands them to the browser in this form.

use crate::catalog::Shape;
use crate::shape::{Face, Geom};
use crate::shape_data::RAIL_BASIS;
use serde::{Deserialize, Serialize};

/// Steps per block in [`Model::points`].
pub const POINT_SCALE: f32 = 1024.0;
/// The block type of the mushroom, the one model the game turns by its own rule.
const MUSHROOM: u16 = 104;
/// The mushroom's turn per orientation value 0..=5: it grows away from the
/// side it sits on, where every other sprite-style model is turned by the
/// table below (which stands them on their heads at the default value 2).
const MUSHROOM_BASIS: [[[i8; 3]; 3]; 6] = [
    [[-1, 0, 0], [0, 0, 1], [0, 1, 0]],
    [[1, 0, 0], [0, 0, 1], [0, -1, 0]],
    [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
    [[-1, 0, 0], [0, -1, 0], [0, 0, 1]],
    [[0, -1, 0], [1, 0, 0], [0, 0, 1]],
    [[0, 1, 0], [-1, 0, 0], [0, 0, 1]],
];

/// One model, already moved and turned the way its scene file says, so that
/// only the block's own orientation is left to apply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    /// The texture sheet the model is skinned from, in the numbering of
    /// `/api/textures/<page>`.
    pub page: u16,
    /// Corner positions, three numbers each, in 1/1024 of a block from the
    /// middle of the cell.
    pub points: Vec<i16>,
    /// Texture position per corner, two numbers each, 0..=65535 across the
    /// sheet from its top left.
    pub uvs: Vec<u16>,
    /// Corner numbers, three per triangle, counter-clockwise seen from outside.
    pub triangles: Vec<u16>,
}

/// Every model the game's block list uses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSet {
    pub models: Vec<Model>,
    /// Texture sheets the models need, beyond the block texture sheets.
    pub pages: Vec<u16>,
}

/// How many different turns a model can be drawn in.
pub const TURNS: usize = 24 + 6;

/// Which of the [`TURNS`] a block's model is drawn in. The game turns the
/// models of 24-way blocks by their full orientation, and those of
/// sprite-style blocks (light rods, capsules) by one of six directions.
pub fn turn(shape: Shape, ty: u16, orientation: u8) -> usize {
    let o = orientation as usize;
    match shape {
        _ if ty == MUSHROOM => 24 + o % 6,
        Shape::Sprite => (o % 6) * 4,
        _ => o % 24,
    }
}

fn basis(turn: usize) -> &'static [[i8; 3]; 3] {
    if turn < 24 { &RAIL_BASIS[turn] } else { &MUSHROOM_BASIS[(turn - 24) % 6] }
}

impl Model {
    /// The model's faces inside the unit cell, in one of its turns.
    pub fn geom(&self, turn: usize) -> Geom {
        let rows = basis(turn);
        let corner = |index: u16| -> Option<([f32; 3], [f32; 2])> {
            let i = index as usize;
            let p = self.points.get(i * 3..i * 3 + 3)?;
            let uv = self.uvs.get(i * 2..i * 2 + 2)?;
            let p = [p[0] as f32 / POINT_SCALE, p[1] as f32 / POINT_SCALE, p[2] as f32 / POINT_SCALE];
            let turned = [0, 1, 2].map(|a| 0.5 + (0..3).map(|k| rows[a][k] as f32 * p[k]).sum::<f32>());
            Some((turned, [uv[0] as f32 / 65535.0, uv[1] as f32 / 65535.0]))
        };
        let mut faces = Vec::with_capacity(self.triangles.len() / 3);
        for triangle in self.triangles.chunks_exact(3) {
            let (Some(a), Some(b), Some(c)) = (corner(triangle[0]), corner(triangle[1]), corner(triangle[2])) else { continue };
            let (u, v) = ([0, 1, 2].map(|i| b.0[i] - a.0[i]), [0, 1, 2].map(|i| c.0[i] - a.0[i]));
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if length < 1e-9 {
                continue; // A triangle squeezed to a line: the game's files have a few.
            }
            faces.push(Face {
                corners: vec![a.0, b.0, c.0],
                normal: n.map(|c| c / length),
                side: None,
                full: false,
                skin: Some((self.page, vec![a.1, b.1, c.1])),
            });
        }
        Geom::new(faces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A light bar as the game's file has it: a quarter block wide, an eighth
    /// high, lying on the floor of the cell along Z.
    fn bar() -> Model {
        let (x, y) = (128, 128);
        let mut points = Vec::new();
        for (px, py, pz) in [(-x, -512, -512), (x, -512, -512), (x, -512 + y, -512), (-x, -512 + y, -512), (-x, -512, 512), (x, -512, 512), (x, -512 + y, 512), (-x, -512 + y, 512)] {
            points.extend([px as i16, py as i16, pz as i16]);
        }
        // Top face only, two triangles, counter-clockwise seen from above.
        Model { page: 9, points, uvs: vec![0; 16], triangles: vec![3, 7, 6, 3, 6, 2] }
    }

    fn extent(g: &Geom) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for c in g.faces.iter().flat_map(|f| f.corners.iter()) {
            for a in 0..3 {
                lo[a] = lo[a].min(c[a]);
                hi[a] = hi[a].max(c[a]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn a_light_bar_lies_where_its_orientation_puts_it() {
        // As placed (14): on the floor of the cell, along Z, its top facing up.
        let placed = bar().geom(turn(Shape::Rail, 977, 14));
        assert_eq!(placed.faces.len(), 2);
        assert_eq!(extent(&placed), ([0.375, 0.125, 0.0], [0.625, 0.125, 1.0]));
        assert!(placed.faces.iter().all(|f| f.normal == [0.0, 1.0, 0.0] && f.side.is_none() && !f.full));
        assert_eq!(placed.full_sides, 0, "a model never hides its neighbours");
        assert_eq!(placed.faces[0].skin.as_ref().unwrap().0, 9);
        // 8: on the ceiling, still along Z. 0: on the back wall, upright.
        let (lo, hi) = extent(&bar().geom(turn(Shape::Rail, 977, 8)));
        assert_eq!((lo[1], hi[1], lo[2], hi[2]), (0.875, 0.875, 0.0, 1.0));
        let (lo, hi) = extent(&bar().geom(turn(Shape::Rail, 977, 0)));
        assert_eq!((lo[1], hi[1], lo[2], hi[2]), (0.0, 1.0, 0.125, 0.125));
        // 16: on the +X wall. 20: on the -X wall.
        assert_eq!(extent(&bar().geom(turn(Shape::Rail, 977, 16))).0[0], 0.875);
        assert_eq!(extent(&bar().geom(turn(Shape::Rail, 977, 20))).1[0], 0.125);
    }

    #[test]
    fn every_turn_keeps_faces_pointing_outward() {
        let up = |t: usize| [0, 1, 2].map(|a| basis(t)[a][1] as f32);
        for t in 0..TURNS {
            let g = bar().geom(t);
            assert_eq!(g.faces.len(), 2);
            for f in &g.faces {
                // The bar's top faces wherever the turn points the model's
                // up: a turn never flips a face inside out.
                assert_eq!(f.normal, up(t), "turn {t}");
            }
        }
        let up = |t: usize| [0, 1, 2].map(|a| basis(t)[a][1]);
        // Sprite-style blocks use six of the 24 turns; the mushroom its own.
        assert_eq!(turn(Shape::Sprite, 499, 2), 8);
        assert_eq!(turn(Shape::Sprite, 499, 7), 4);
        assert_eq!(turn(Shape::Sprite, MUSHROOM, 2), 26);
        // At its default value the mushroom stands upright, a light rod's
        // table entry points the model's up at the floor.
        assert_eq!(up(turn(Shape::Sprite, MUSHROOM, 2)), [0, 1, 0]);
        assert_eq!(up(turn(Shape::Sprite, 499, 2)), [0, -1, 0]);
    }
}
