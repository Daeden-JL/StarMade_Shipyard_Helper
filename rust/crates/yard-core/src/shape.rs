//! Block geometry: the solid each block style occupies inside its cell, at
//! each orientation, as the game itself draws it.

use crate::catalog::Shape;
use crate::shape_data::{RawShape, CORNER, CUBE_UV, CUBE_UV_POINTING, HEPTA, RAIL_AXES, RAIL_MIRROR, RAIL_TEX, SLAB_BOX, SPRITE, TETRA, WEDGE};

/// The six sides of a cell in the game's own order and numbering.
pub const FRONT: usize = 0;
pub const BACK: usize = 1;
pub const TOP: usize = 2;
pub const BOTTOM: usize = 3;
pub const RIGHT: usize = 4;
pub const LEFT: usize = 5;

/// Step to the neighbouring cell on each side. Front is +Z, top is +Y, right is +X.
pub const SIDE_DIRS: [[i32; 3]; 6] = [[0, 0, 1], [0, 0, -1], [0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0]];

pub const fn opposite(side: usize) -> usize {
    side ^ 1
}

/// One flat face of a block.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Corners inside the unit cell (0..1 on each axis), counter-clockwise seen
    /// from outside the block.
    pub corners: Vec<[f32; 3]>,
    pub normal: [f32; 3],
    /// The cell side this face lies in, or None for a slanted or inset face.
    pub side: Option<usize>,
    /// Whether the face covers its whole cell side, hiding whatever touches it.
    pub full: bool,
    /// For a face of a block's 3D model: the texture sheet it is skinned
    /// from and where each corner falls in that sheet (0..1 from its top left).
    pub skin: Option<(u16, Vec<[f32; 2]>)>,
}

/// The faces of one block style at one orientation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geom {
    pub faces: Vec<Face>,
    /// Bit per side (1 << side): set where the block fills that whole side.
    pub full_sides: u8,
}

impl Geom {
    pub fn new(faces: Vec<Face>) -> Self {
        let full_sides = faces.iter().filter(|f| f.full).filter_map(|f| f.side).fold(0u8, |m, s| m | 1 << s);
        Self { faces, full_sides }
    }

    pub fn fills(&self, side: usize) -> bool {
        self.full_sides & (1 << side) != 0
    }

    /// A box from `lo` to `hi` inside the unit cell. The full cube is 0..1.
    pub fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> Self {
        let (l, h) = (lo, hi);
        let quads: [(usize, [[f32; 3]; 4]); 6] = [
            (FRONT, [[h[0], l[1], h[2]], [h[0], h[1], h[2]], [l[0], h[1], h[2]], [l[0], l[1], h[2]]]),
            (BACK, [[l[0], l[1], l[2]], [l[0], h[1], l[2]], [h[0], h[1], l[2]], [h[0], l[1], l[2]]]),
            (TOP, [[l[0], h[1], l[2]], [l[0], h[1], h[2]], [h[0], h[1], h[2]], [h[0], h[1], l[2]]]),
            (BOTTOM, [[l[0], l[1], h[2]], [l[0], l[1], l[2]], [h[0], l[1], l[2]], [h[0], l[1], h[2]]]),
            (RIGHT, [[h[0], l[1], l[2]], [h[0], h[1], l[2]], [h[0], h[1], h[2]], [h[0], l[1], h[2]]]),
            (LEFT, [[l[0], l[1], h[2]], [l[0], h[1], h[2]], [l[0], h[1], l[2]], [l[0], l[1], l[2]]]),
        ];
        let faces = quads
            .into_iter()
            .map(|(side, corners)| {
                let dir = SIDE_DIRS[side];
                let axis = axis_index(side);
                // The face lies in the cell side only if the box reaches it, and
                // covers it only if the box spans the other two axes entirely.
                let reaches = if dir[axis] > 0 { h[axis] >= 1.0 } else { l[axis] <= 0.0 };
                let spans = (0..3).filter(|&a| a != axis).all(|a| l[a] <= 0.0 && h[a] >= 1.0);
                Face {
                    corners: corners.to_vec(),
                    normal: [dir[0] as f32, dir[1] as f32, dir[2] as f32],
                    side: reaches.then_some(side),
                    full: reaches && spans,
                    skin: None,
                }
            })
            .collect();
        Self::new(faces)
    }

    pub fn cube() -> Self {
        Self::cuboid([0.0; 3], [1.0; 3])
    }
}

/// 0 for X, 1 for Y, 2 for Z.
pub const fn axis_index(side: usize) -> usize {
    match side {
        FRONT | BACK => 2,
        TOP | BOTTOM => 1,
        _ => 0,
    }
}

/// Where the corners of a cell side fall in a texture: the texture corner
/// (u, v; v = 0 is the top of the image) at three corners of the side. The
/// corners are named by the two axes that span the side, lowest axis first (x
/// and y for front and back, x and z for top and bottom, y and z for the other
/// two): lowest on both, highest on the first, highest on the second.
pub type SideUv = [[u8; 2]; 3];

/// The texture position of a point on a cell side.
pub fn side_uv(uv: &SideUv, side: usize, c: [f32; 3]) -> [f32; 2] {
    let (a, b) = match axis_index(side) {
        0 => (c[1], c[2]),
        1 => (c[0], c[2]),
        _ => (c[0], c[1]),
    };
    let along = |i: usize| {
        let at = |corner: usize| uv[corner][i] as f32;
        at(0) + a * (at(1) - at(0)) + b * (at(2) - at(0))
    };
    [along(0), along(1)]
}

/// How the game lays a cube's texture on one side at one orientation. Cubes
/// turn their texture with their orientation; `pointing` is for the blocks
/// whose side textures point where the block points.
pub fn cube_uv(orientation: u8, pointing: bool, side: usize) -> &'static SideUv {
    let table = if pointing { &CUBE_UV_POINTING } else { &CUBE_UV };
    &table[orientation as usize % 6][side]
}

/// For a block that turns 24 ways (a rail, a docker): which of its six
/// textures shows on a cell side, and how it lies there.
pub fn turned_side(orientation: u8, side: usize) -> (usize, &'static SideUv) {
    let cell = &RAIL_TEX[orientation as usize % 24][side];
    (cell.slot as usize, &cell.uv)
}

/// Where a 24-way block's own up and forward point: up is the side its first
/// texture faces (a docker's arrow side), forward is where that texture's top
/// edge points (the arrow's direction).
pub fn turned_axes(orientation: u8) -> ([i32; 3], [i32; 3]) {
    let [up, forward] = RAIL_AXES[orientation as usize % 24];
    (up.map(i32::from), forward.map(i32::from))
}

/// A turn of the grid by quarter turns, as three rows: a step `v` goes to
/// (row 0 · v, row 1 · v, row 2 · v).
pub type GridTurn = [[i32; 3]; 3];

pub const NO_GRID_TURN: GridTurn = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];

/// A step turned by `turn`.
pub fn turn_step(turn: &GridTurn, v: [i32; 3]) -> [i32; 3] {
    turn.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

/// The way each value of [`ShapeSet::facing`] points (and each sprite faces).
const FACING_DIRS: [[i32; 3]; 6] = [[0, 0, 1], [0, 0, -1], [0, 1, 0], [0, -1, 0], [-1, 0, 0], [1, 0, 0]];

/// The corner points of a shape turned about the middle of its cell, in
/// quarter cells from the middle, sorted.
fn corners(shape: &RawShape, turn: &GridTurn) -> Vec<[i32; 3]> {
    let mut out: Vec<[i32; 3]> = shape
        .faces
        .iter()
        .flat_map(|f| f.verts.iter())
        .map(|c| turn_step(turn, c.map(|v| (v * 4.0).round() as i32 - 2)))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The geometry of every block style at every orientation.
pub struct ShapeSet {
    cube: Geom,
    /// Three thicknesses times six orientations.
    slabs: Vec<Geom>,
    wedge: Vec<Geom>,
    corner: Vec<Geom>,
    tetra: Vec<Geom>,
    hepta: Vec<Geom>,
    sprite: Vec<Geom>,
}

impl Default for ShapeSet {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeSet {
    pub fn new() -> Self {
        let table = |raw: &[RawShape]| -> Vec<Geom> {
            raw.iter()
                .map(|shape| {
                    Geom::new(
                        shape
                            .faces
                            .iter()
                            .map(|f| Face {
                                corners: f.verts.to_vec(),
                                normal: f.normal,
                                side: (f.side >= 0).then_some(f.side as usize),
                                full: f.full,
                                skin: None,
                            })
                            .collect(),
                    )
                })
                .collect()
        };
        let slabs = SLAB_BOX
            .iter()
            .flat_map(|by_orientation| by_orientation.iter().map(|b| Geom::cuboid([b[0], b[1], b[2]], [b[3], b[4], b[5]])))
            .collect();
        Self {
            cube: Geom::cube(),
            slabs,
            wedge: table(&WEDGE),
            corner: table(&CORNER),
            tetra: table(&TETRA),
            hepta: table(&HEPTA),
            sprite: table(&SPRITE),
        }
    }

    /// What a block of this style looks like. `slab` is 0 for a full block and
    /// 1..=3 for slabs; `orientation` is the value stored in the block.
    pub fn geom(&self, shape: Shape, slab: u8, orientation: u8) -> &Geom {
        let o = orientation as usize;
        match shape {
            Shape::Wedge => self.wedge.get(o).unwrap_or(&self.cube),
            Shape::Corner => self.corner.get(o).unwrap_or(&self.cube),
            Shape::Tetra => self.tetra.get(o).unwrap_or(&self.cube),
            Shape::Hepta => self.hepta.get(o).unwrap_or(&self.cube),
            Shape::Sprite => &self.sprite[o % self.sprite.len()],
            // The game draws a slab with an orientation above 5 as a full cube.
            Shape::Cube if (1..=3).contains(&slab) && o < 6 => &self.slabs[(slab as usize - 1) * 6 + o],
            Shape::Cube | Shape::Rail => &self.cube,
        }
    }

    /// How many orientation values give different results for a block, counting
    /// from 0. `faces_matter` says the block is a slab or has a distinct front.
    pub fn choices(shape: Shape, faces_matter: bool) -> u8 {
        match shape {
            Shape::Wedge => 12,
            Shape::Corner | Shape::Rail => 24,
            Shape::Tetra | Shape::Hepta => 8,
            Shape::Sprite => 6,
            Shape::Cube if faces_matter => 6,
            Shape::Cube => 1,
        }
    }

    /// The orientation that turns a slab's open side, or a block's front, toward
    /// `normal` (a step along one axis). The game numbers these front, back,
    /// top, bottom, then -X as 4 and +X as 5.
    pub fn facing(normal: [i32; 3]) -> u8 {
        match normal {
            [0, 0, z] if z < 0 => 1,
            [0, y, 0] if y > 0 => 2,
            [0, y, 0] if y < 0 => 3,
            [x, 0, 0] if x < 0 => 4,
            [x, 0, 0] if x > 0 => 5,
            _ => 0,
        }
    }

    /// The orientation of a block turned with the grid by `turn` (one of the
    /// 24 turns that keep a cell a cell). A turn made of half turns is a pair
    /// of mirror images, done with the game's own mirror tables; quarter turns
    /// find the orientation whose solid, axes or facing land where the turned
    /// block's do. Of two values with the same solid, the one of the same kind
    /// (inside or past [`ShapeSet::choices`]) is taken, so that a turn and
    /// its reverse come back to a value below the choices they started from.
    pub fn turned(shape: Shape, orientation: u8, faces_matter: bool, turn: &GridTurn) -> u8 {
        let o = orientation;
        if (0..3).all(|r| (0..3).all(|c| r == c || turn[r][c] == 0)) {
            return Self::mirrored(shape, o, [turn[0][0] < 0, turn[1][1] < 0, turn[2][2] < 0], faces_matter);
        }
        let solid = |raw: &[RawShape]| -> u8 {
            let Some(want) = raw.get(o as usize).map(|s| corners(s, turn)) else { return o };
            let same: Vec<u8> = (0..raw.len() as u8).filter(|&c| corners(&raw[c as usize], &NO_GRID_TURN) == want).collect();
            let inside = |v: u8| v < Self::choices(shape, faces_matter);
            same.iter().copied().find(|&v| inside(v) == inside(o)).or(same.first().copied()).unwrap_or(o)
        };
        match shape {
            Shape::Wedge => solid(&WEDGE),
            Shape::Corner => solid(&CORNER),
            Shape::Tetra => solid(&TETRA),
            Shape::Hepta => solid(&HEPTA),
            // Crossed sheets look the same from both ends: they turn by the way they face.
            Shape::Sprite if o < 6 => Self::facing(turn_step(turn, FACING_DIRS[o as usize])),
            Shape::Sprite => o,
            Shape::Rail => {
                let (up, forward) = turned_axes(o);
                let want = (turn_step(turn, up), turn_step(turn, forward));
                (0..24u8).find(|&c| turned_axes(c) == want).unwrap_or(o)
            }
            Shape::Cube if faces_matter && o < 6 => Self::facing(turn_step(turn, FACING_DIRS[o as usize])),
            Shape::Cube => o,
        }
    }

    /// The orientation of a block's mirror image, flipped along the given axes
    /// (X, Y, Z). `faces_matter` as in [`ShapeSet::choices`].
    pub fn mirrored(shape: Shape, orientation: u8, flip: [bool; 3], faces_matter: bool) -> u8 {
        let mut o = orientation;
        for axis in 0..3 {
            if !flip[axis] {
                continue;
            }
            let table = |raw: &[RawShape]| raw.get(o as usize).map_or(o, |s| s.mirror[axis]);
            o = match shape {
                Shape::Wedge => table(&WEDGE),
                Shape::Corner => table(&CORNER),
                Shape::Tetra => table(&TETRA),
                Shape::Hepta => table(&HEPTA),
                Shape::Sprite => table(&SPRITE),
                Shape::Rail => RAIL_MIRROR.get(o as usize).map_or(o, |m| m[axis]),
                // Values come in pairs along each axis: 4/5 on X, 2/3 on Y, 0/1 on Z.
                Shape::Cube if faces_matter && o < 6 && o as usize / 2 == [2, 1, 0][axis] => o ^ 1,
                Shape::Cube => o,
            };
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }

    /// Every face must wind counter-clockwise seen from outside, or it is culled.
    pub(crate) fn assert_winding(g: &Geom) {
        for f in &g.faces {
            let c = &f.corners;
            let a = [c[1][0] - c[0][0], c[1][1] - c[0][1], c[1][2] - c[0][2]];
            let b = [c[2][0] - c[0][0], c[2][1] - c[0][1], c[2][2] - c[0][2]];
            let n = cross(a, b);
            let dot = n[0] * f.normal[0] + n[1] * f.normal[1] + n[2] * f.normal[2];
            assert!(dot > 0.0, "face {:?} winds the wrong way", f.normal);
        }
    }

    #[test]
    fn game_shapes_are_well_formed() {
        let set = ShapeSet::new();
        for (shape, count) in [(Shape::Wedge, 24), (Shape::Corner, 24), (Shape::Tetra, 24), (Shape::Hepta, 24), (Shape::Sprite, 6)] {
            for o in 0..count {
                let g = set.geom(shape, 0, o);
                assert!(!g.faces.is_empty(), "{shape:?} {o} has no faces");
                assert_winding(g);
                for f in &g.faces {
                    assert!(f.corners.len() >= 3);
                    assert!(f.corners.iter().flatten().all(|c| (0.0..=1.0).contains(c)));
                    // A face that fills a side is a face lying in that side.
                    assert!(!f.full || f.side.is_some());
                }
            }
        }
        // Wedge 0, checked against the game: full bottom and full front wall,
        // the slope faces up and back.
        let wedge = set.geom(Shape::Wedge, 0, 0);
        assert!(wedge.fills(BOTTOM) && wedge.fills(FRONT));
        assert!(!wedge.fills(TOP) && !wedge.fills(BACK) && !wedge.fills(LEFT) && !wedge.fills(RIGHT));
        let slope = wedge.faces.iter().find(|f| f.side.is_none()).unwrap();
        assert!(slope.normal[1] > 0.0 && slope.normal[2] < 0.0);
        // Values past the table fall back to a cube rather than panicking.
        assert_eq!(set.geom(Shape::Corner, 0, 31).faces.len(), 6);
    }

    /// All corner points of a shape, each reflected along one axis, sorted.
    fn points(g: &Geom, flip: Option<usize>) -> Vec<[i32; 3]> {
        let mut out: Vec<[i32; 3]> = g
            .faces
            .iter()
            .flat_map(|f| f.corners.iter())
            .map(|c| {
                let mut c = *c;
                if let Some(axis) = flip {
                    c[axis] = 1.0 - c[axis];
                }
                [(c[0] * 4.0).round() as i32, (c[1] * 4.0).round() as i32, (c[2] * 4.0).round() as i32]
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn mirrored_orientations_are_the_reflected_shapes() {
        let set = ShapeSet::new();
        for shape in [Shape::Wedge, Shape::Corner, Shape::Tetra, Shape::Hepta] {
            for o in 0..24u8 {
                for axis in 0..3 {
                    let mut flip = [false; 3];
                    flip[axis] = true;
                    let m = ShapeSet::mirrored(shape, o, flip, false);
                    assert_eq!(
                        points(set.geom(shape, 0, m), None),
                        points(set.geom(shape, 0, o), Some(axis)),
                        "{shape:?} {o} mirrored along axis {axis} gave {m}"
                    );
                }
            }
        }
        // Slabs: the value pairs flip along their own axis only.
        for slab in 1..=3u8 {
            for o in 0..6u8 {
                for axis in 0..3 {
                    let mut flip = [false; 3];
                    flip[axis] = true;
                    let m = ShapeSet::mirrored(Shape::Cube, o, flip, true);
                    assert_eq!(points(set.geom(Shape::Cube, slab, m), None), points(set.geom(Shape::Cube, slab, o), Some(axis)));
                }
            }
        }
        assert_eq!(ShapeSet::mirrored(Shape::Cube, 4, [true, false, false], false), 4, "plain cubes keep their value");
    }

    /// The 24 turns of the grid: signed permutations that do not mirror.
    pub(crate) fn grid_turns() -> Vec<GridTurn> {
        let mut out = Vec::new();
        for perm in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
            for signs in 0..8 {
                let mut m = [[0; 3]; 3];
                for r in 0..3 {
                    m[r][perm[r]] = if signs >> r & 1 == 1 { -1 } else { 1 };
                }
                let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
                if det == 1 {
                    out.push(m);
                }
            }
        }
        out
    }

    fn reversed(m: &GridTurn) -> GridTurn {
        [0, 1, 2].map(|r| [0, 1, 2].map(|c| m[c][r]))
    }

    #[test]
    fn turned_orientations_are_the_turned_shapes_and_turn_back() {
        let turns = grid_turns();
        assert_eq!(turns.len(), 24);
        let set = ShapeSet::new();
        let turned_points = |g: &Geom, m: &GridTurn| {
            let mut out: Vec<[i32; 3]> = g.faces.iter().flat_map(|f| f.corners.iter()).map(|c| turn_step(m, c.map(|v| (v * 4.0).round() as i32 - 2))).collect();
            out.sort();
            out.dedup();
            out
        };
        for m in &turns {
            for (shape, count) in [(Shape::Wedge, 24u8), (Shape::Corner, 24), (Shape::Tetra, 24), (Shape::Hepta, 24), (Shape::Sprite, 6)] {
                for o in 0..count {
                    let t = ShapeSet::turned(shape, o, false, m);
                    assert_eq!(turned_points(set.geom(shape, 0, t), &NO_GRID_TURN), turned_points(set.geom(shape, 0, o), m), "{shape:?} {o} turned by {m:?} gave {t}");
                    // Values past the choices are the same solids again, which the
                    // game's own mirror tables fold back below them.
                    if o < ShapeSet::choices(shape, false) {
                        assert_eq!(ShapeSet::turned(shape, t, false, &reversed(m)), o, "{shape:?} {o} turned by {m:?} and back");
                    }
                }
            }
            for o in 0..24u8 {
                let t = ShapeSet::turned(Shape::Rail, o, false, m);
                let (up, forward) = turned_axes(o);
                assert_eq!(turned_axes(t), (turn_step(m, up), turn_step(m, forward)));
                assert_eq!(ShapeSet::turned(Shape::Rail, t, false, &reversed(m)), o);
            }
            for slab in 1..=3u8 {
                for o in 0..6u8 {
                    let t = ShapeSet::turned(Shape::Cube, o, true, m);
                    assert_eq!(turned_points(set.geom(Shape::Cube, slab, t), &NO_GRID_TURN), turned_points(set.geom(Shape::Cube, slab, o), m), "slab {slab} {o} by {m:?}");
                    assert_eq!(ShapeSet::turned(Shape::Cube, t, true, &reversed(m)), o);
                }
            }
            assert_eq!(ShapeSet::turned(Shape::Cube, 3, false, m), 3, "plain cubes keep their value");
        }
    }

    #[test]
    fn slabs_sit_against_the_side_opposite_their_orientation() {
        let set = ShapeSet::new();
        // Placed on a floor (clicked face points up): the slab rests on the bottom.
        let on_floor = set.geom(Shape::Cube, 2, ShapeSet::facing([0, 1, 0]));
        assert!(on_floor.fills(BOTTOM) && !on_floor.fills(TOP));
        // Placed against a wall whose face points toward +X: it rests on its -X side.
        let on_wall = set.geom(Shape::Cube, 3, ShapeSet::facing([1, 0, 0]));
        assert!(on_wall.fills(LEFT) && !on_wall.fills(RIGHT));
        let on_ceiling = set.geom(Shape::Cube, 1, ShapeSet::facing([0, -1, 0]));
        assert!(on_ceiling.fills(TOP));
        let behind = set.geom(Shape::Cube, 1, ShapeSet::facing([0, 0, -1]));
        assert!(behind.fills(FRONT));
        assert_eq!(set.geom(Shape::Cube, 2, 9).full_sides, 0b11_1111, "odd values draw as a cube");
    }

    #[test]
    fn a_24_way_block_shows_its_first_texture_up_with_the_top_edge_forward() {
        for o in 0..24u8 {
            let (up, forward) = turned_axes(o);
            assert_eq!(up.iter().zip(forward).map(|(a, b)| a * b).sum::<i32>(), 0, "{o}: up and forward are at right angles");
            let main = SIDE_DIRS.iter().position(|d| *d == up).unwrap();
            let (slot, uv) = turned_side(o, main);
            assert_eq!(slot, 0, "{o}: the first texture faces up");
            let (back_slot, _) = turned_side(o, opposite(main));
            assert_eq!(back_slot, 1, "{o}: the second texture faces down");
            // Walk from the middle of that side along the block's forward: the
            // texture position moves straight toward the top of the image.
            let mut middle = [0.5f32; 3];
            middle[axis_index(main)] = if up[axis_index(main)] > 0 { 1.0 } else { 0.0 };
            let ahead = [middle[0] + forward[0] as f32 * 0.5, middle[1] + forward[1] as f32 * 0.5, middle[2] + forward[2] as f32 * 0.5];
            let (from, to) = (side_uv(uv, main, middle), side_uv(uv, main, ahead));
            assert_eq!((from, to), ([0.5, 0.5], [0.5, 0.0]), "{o}");
        }
        // The value the game places these blocks with: arrow side up, pointing forward.
        assert_eq!(turned_axes(14), ([0, 1, 0], [0, 0, 1]));
        // Values 4 and 5 of the game's sides are swapped: "right" is -X.
        assert_eq!(turned_axes(16).0, [-1, 0, 0]);
    }

    #[test]
    fn cube_textures_turn_with_the_block_without_being_mirrored() {
        for pointing in [false, true] {
            for o in 0..6u8 {
                for side in 0..6 {
                    let uv = cube_uv(o, pointing, side);
                    // The three corners span the whole texture: a quarter turn
                    // or none, never a squeeze.
                    let (du, dv) = ([uv[1][0] as i32 - uv[0][0] as i32, uv[1][1] as i32 - uv[0][1] as i32], [uv[2][0] as i32 - uv[0][0] as i32, uv[2][1] as i32 - uv[0][1] as i32]);
                    assert_eq!(du[0] * dv[0] + du[1] * dv[1], 0);
                    assert_eq!((du[0].abs() + du[1].abs(), dv[0].abs() + dv[1].abs()), (1, 1));
                    // Seen from outside, the image is never flipped. The axes
                    // spanning a side run counter-clockwise seen from outside on
                    // front, bottom and +X, clockwise on the others; the image's
                    // own u and v run clockwise (v points down).
                    let turn = du[0] * dv[1] - du[1] * dv[0];
                    let outside = if [FRONT, BOTTOM, RIGHT].contains(&side) { -1 } else { 1 };
                    assert_eq!(turn, outside, "orientation {o} side {side} pointing {pointing}");
                }
            }
        }
        // Unturned, walls are upright: the top of the cell is the top of the image.
        for side in [FRONT, BACK, LEFT, RIGHT] {
            assert_eq!(side_uv(cube_uv(0, false, side), side, [0.5, 1.0, 0.5])[1], 0.0);
        }
    }

    #[test]
    fn a_cube_fills_all_six_sides() {
        let cube = Geom::cube();
        assert_eq!(cube.faces.len(), 6);
        assert_eq!(cube.full_sides, 0b11_1111);
        assert_winding(&cube);
        for side in 0..6 {
            assert_eq!(opposite(opposite(side)), side);
            let (a, b) = (SIDE_DIRS[side], SIDE_DIRS[opposite(side)]);
            assert_eq!([a[0] + b[0], a[1] + b[1], a[2] + b[2]], [0, 0, 0]);
        }
    }

    #[test]
    fn a_half_slab_fills_only_the_side_it_sits_on() {
        let slab = Geom::cuboid([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]);
        assert_winding(&slab);
        assert!(slab.fills(BOTTOM));
        assert!(!slab.fills(TOP) && !slab.fills(FRONT) && !slab.fills(RIGHT));
        // Its top face floats inside the cell; the side faces touch their cell sides.
        let top = slab.faces.iter().find(|f| f.normal == [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(top.side, None);
        let front = slab.faces.iter().find(|f| f.normal == [0.0, 0.0, 1.0]).unwrap();
        assert_eq!((front.side, front.full), (Some(FRONT), false));
    }
}
