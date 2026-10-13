//! Shapes the tests build their files and models from.
#![allow(dead_code)]

pub mod samples;

use yard_import::{Paint, Triangle};

/// The corners of the unit cube: corner i is at (i & 1, i >> 1 & 1, i >> 2).
pub const CUBE_POINTS: [[f32; 3]; 8] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0]];

/// The cube's sides as corner numbers, anticlockwise seen from outside, in
/// the order -x, +x, -y, +y, -z, +z.
pub const CUBE_SIDES: [[usize; 4]; 6] = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];

/// The cube's twelve triangles as corner numbers.
pub fn cube_triangles() -> Vec<[usize; 3]> {
    CUBE_SIDES.iter().flat_map(|s| [[s[0], s[1], s[2]], [s[0], s[2], s[3]]]).collect()
}

/// A box from `low` to `high`, each side painted by `paint` (sides in the
/// order of [`CUBE_SIDES`]).
pub fn painted_box(low: [f32; 3], high: [f32; 3], paint: impl Fn(usize) -> Paint) -> Vec<Triangle> {
    let at = |i: usize| [0, 1, 2].map(|k| low[k] + CUBE_POINTS[i][k] * (high[k] - low[k]));
    CUBE_SIDES.iter().enumerate().flat_map(|(side, s)| [[s[0], s[1], s[2]], [s[0], s[2], s[3]]].map(|t| Triangle { corners: t.map(at), paint: paint(side) })).collect()
}

pub fn plain_box(low: [f32; 3], high: [f32; 3]) -> Vec<Triangle> {
    painted_box(low, high, |_| Paint::None)
}

/// A ball about the origin, cut along circles of latitude and longitude.
pub fn ball(radius: f32, rings: usize, segments: usize) -> Vec<Triangle> {
    let at = |ring: usize, segment: usize| {
        let (polar, around) = (std::f32::consts::PI * ring as f32 / rings as f32, std::f32::consts::TAU * (segment % segments) as f32 / segments as f32);
        [radius * polar.sin() * around.cos(), radius * polar.cos(), radius * polar.sin() * around.sin()]
    };
    let mut out = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let (a, b, c, d) = (at(ring, segment), at(ring, segment + 1), at(ring + 1, segment + 1), at(ring + 1, segment));
            if ring > 0 {
                out.push(Triangle { corners: [a, b, c], paint: Paint::None });
            }
            if ring + 1 < rings {
                out.push(Triangle { corners: [a, c, d], paint: Paint::None });
            }
        }
    }
    out
}

/// The lowest and highest corner of a model's triangles.
pub fn bounds(triangles: &[Triangle]) -> ([f32; 3], [f32; 3]) {
    let (mut low, mut high) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for corner in triangles.iter().flat_map(|t| t.corners) {
        for i in 0..3 {
            low[i] = low[i].min(corner[i]);
            high[i] = high[i].max(corner[i]);
        }
    }
    (low, high)
}

pub fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
}
