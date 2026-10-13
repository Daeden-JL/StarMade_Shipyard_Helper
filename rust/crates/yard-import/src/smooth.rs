//! Finding the surface inside each cell, for smoothing.
//!
//! A model's surface is flat or nearly so inside one cell, even where the
//! blocks that approximate it step. So for every cell the surface passes
//! through, the pieces of triangles inside the cell are gathered into a
//! [`Patch`]: how much surface there is, where, and which way it faces. If
//! the patch is a single flat sheet, its plane says which of the cell's 64
//! sample points (4 by 4 by 4, at the middles of the sub-cells) lie in the
//! model, and whoever places the blocks can pick the block shape whose
//! solid covers the same sample points best.
//!
//! Where two flat sheets meet in a cell (an edge) the model is inside both
//! of them (a ridge) or inside either (a crease); the neighbouring cells
//! say which. A cell with three sheets (a corner), a thin wall (two sheets
//! close together), a bend too gradual to be called a sheet, or too small
//! a speck to tell, is left alone: it stays a whole block, or empty, as it
//! was.

use crate::math::{dot, V3};

/// Sample points on a side of the cell.
pub const SAMPLES: usize = 4;

/// Pieces of surface whose normals lie within this cosine of a sheet's
/// first piece belong to the sheet (about 26 degrees).
const ALIGNED: f64 = 0.9;
/// How flat a sheet must be: the average cosine between its pieces'
/// normals and its own (the mean normal's length over the area).
const FLAT: f64 = 0.96;
/// How far apart, along the normal, the pieces of a sheet may lie
/// (standard deviation, in cells). Two sheets a quarter cell apart have
/// 0.125.
const THICKNESS: f64 = 0.08;
/// Faces this far apart, in cells, are the two sides of a wall.
const WALL: f64 = 0.04;
/// A sheet with less surface than this, in squared cells, is noise at the
/// edge of a triangle and is ignored.
const NOISE: f64 = 0.02;
/// With less surface than this in all, the cell is a speck.
const SPECK: f64 = 1e-3;
/// How far past a plane a sample point may be and still count as on it.
const ON_THE_PLANE: f64 = 1e-6;
/// How much the neighbours' agreement with the best reading must beat
/// that with the next best for it to be taken (the neighbours' votes add
/// up to about 19).
const SURE: f64 = 3.0;

/// One sheet of surface in a cell: the pieces of triangles whose normals
/// point the same way. The numbers are sums over the pieces, each weighted
/// by its area; positions are in cells from the cell's lowest corner.
#[derive(Clone, Copy, Default)]
struct Sheet {
    area: f32,
    /// Sum of area times position.
    position: [f32; 3],
    /// Sum of area times position times position (xx, xy, xz, yy, yz, zz).
    spread: [f32; 6],
    /// Sum of area times normal, each normal turned to face the way of the
    /// first piece's, so that a mesh with some faces turned round is no
    /// trouble.
    towards: [f32; 3],
    /// The first piece's normal.
    reference: [f32; 3],
    /// The pieces that face the other way (round the back of a thin
    /// wall): their area and sum of area times position. They are in the
    /// sums above too.
    against: f32,
    against_position: [f32; 3],
}

impl Sheet {
    fn starts(normal: V3) -> Sheet {
        Sheet { reference: normal.map(|v| v as f32), ..Sheet::default() }
    }

    fn takes(&self, normal: V3) -> bool {
        dot(normal, self.reference.map(f64::from)).abs() >= ALIGNED
    }

    fn add(&mut self, area: f64, at: V3, normal: V3) {
        let face = if dot(normal, self.reference.map(f64::from)) < 0.0 { -1.0 } else { 1.0 };
        if face < 0.0 {
            self.against += area as f32;
            for (sum, v) in self.against_position.iter_mut().zip(at) {
                *sum += (area * v) as f32;
            }
        }
        self.area += area as f32;
        for (sum, v) in self.position.iter_mut().zip(at) {
            *sum += (area * v) as f32;
        }
        for (sum, v) in self.towards.iter_mut().zip(normal) {
            *sum += (area * face * v) as f32;
        }
        for (sum, v) in self.spread.iter_mut().zip(pairs(at, at)) {
            *sum += (area * v) as f32;
        }
    }

    /// The plane of the sheet, or nothing if it is not flat enough: its
    /// unit normal (either way round) and the offset of the plane along it.
    fn plane(&self) -> Option<(V3, f64)> {
        let area = f64::from(self.area);
        let towards = self.towards.map(f64::from);
        let length = dot(towards, towards).sqrt();
        if length / area < FLAT {
            return None;
        }
        let n = towards.map(|v| v / length);
        let mean = self.position.map(|v| f64::from(v) / area);
        // Pieces facing both ways, a little apart, are the two faces of a
        // thin wall (pieces facing both ways in one place are a mesh with
        // some faces turned round).
        let (back, front) = (f64::from(self.against), area - f64::from(self.against));
        if back >= NOISE && front >= NOISE {
            let behind = self.against_position.map(|v| f64::from(v) / back);
            let ahead = [0, 1, 2].map(|k| (f64::from(self.position[k]) - f64::from(self.against_position[k])) / front);
            if dot(n, [ahead[0] - behind[0], ahead[1] - behind[1], ahead[2] - behind[2]]).abs() >= WALL {
                return None;
            }
        }
        let spread = self.spread.map(|v| f64::from(v) / area);
        if quadratic(spread, n) - dot(n, mean).powi(2) > THICKNESS * THICKNESS {
            return None;
        }
        Some((n, dot(n, mean)))
    }
}

fn pairs(a: [f64; 3], b: [f64; 3]) -> [f64; 6] {
    [a[0] * b[0], a[0] * b[1], a[0] * b[2], a[1] * b[1], a[1] * b[2], a[2] * b[2]]
}

/// vᵀ M v for a symmetric matrix kept as (xx, xy, xz, yy, yz, zz).
fn quadratic(m: [f64; 6], v: V3) -> f64 {
    m[0] * v[0] * v[0] + m[3] * v[1] * v[1] + m[5] * v[2] * v[2] + 2.0 * (m[1] * v[0] * v[1] + m[2] * v[0] * v[2] + m[4] * v[1] * v[2])
}

/// What is known about the surface in one cell: up to two sheets.
#[derive(Clone, Default)]
pub(crate) struct Patch {
    first: Sheet,
    second: Option<Box<Sheet>>,
    /// Surface that fits neither sheet.
    stray: f32,
    /// The triangle with the most surface in the cell, and how much.
    pub(crate) triangle: u32,
    biggest: f32,
}

impl Patch {
    /// Adds a piece of triangle `triangle`: its area, the middle of it and
    /// the triangle's unit normal.
    pub(crate) fn add(&mut self, triangle: u32, area: f64, at: V3, normal: V3) {
        if self.first.area == 0.0 {
            self.first = Sheet::starts(normal);
        }
        if self.first.takes(normal) {
            self.first.add(area, at, normal);
        } else {
            if self.second.is_none() {
                self.second = Some(Box::new(Sheet::starts(normal)));
            }
            match &mut self.second {
                Some(second) if second.takes(normal) => second.add(area, at, normal),
                _ => self.stray += area as f32,
            }
        }
        if area as f32 > self.biggest {
            self.biggest = area as f32;
            self.triangle = triangle;
        }
    }

    /// Which of the cell's sample points are in the model, bit
    /// `x + 4 y + 16 z`, or nothing where the surface in the cell is not
    /// one flat sheet or two meeting at an edge (or the neighbours cannot
    /// say which side is the model). `solid` tells whether the neighbouring
    /// cell at a step from this one is part of the model.
    pub(crate) fn samples(&self, solid: impl Fn([i32; 3]) -> bool) -> Option<u64> {
        let total = f64::from(self.first.area) + self.second.as_ref().map_or(0.0, |s| f64::from(s.area)) + f64::from(self.stray);
        if total < SPECK || f64::from(self.stray) >= NOISE {
            return None;
        }
        let mut planes = Vec::new();
        for sheet in std::iter::once(&self.first).chain(self.second.as_deref()) {
            if f64::from(sheet.area) >= NOISE {
                planes.push(sheet.plane()?);
            }
        }
        if planes.is_empty() {
            return None;
        }
        // The readings: each plane facing out or in, and with two planes the
        // model inside both or inside either.
        let mut readings = Vec::new();
        for flips in 0..1u32 << planes.len() {
            let turned: Vec<(V3, f64)> = planes.iter().enumerate().map(|(i, &(n, offset))| if flips >> i & 1 == 1 { (n.map(|v| -v), -offset) } else { (n, offset) }).collect();
            readings.push((turned.clone(), true));
            if turned.len() == 2 {
                readings.push((turned, false));
            }
        }
        let is_in = |reading: &(Vec<(V3, f64)>, bool), at: V3| {
            let mut inside = reading.0.iter().map(|&(n, offset)| dot(n, at) <= offset + ON_THE_PLANE);
            if reading.1 {
                inside.all(|b| b)
            } else {
                inside.any(|b| b)
            }
        };
        // Each neighbour votes for the readings that put it where it is.
        let mut votes = vec![0.0; readings.len()];
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    let middle = [0.5 + f64::from(dx), 0.5 + f64::from(dy), 0.5 + f64::from(dz)];
                    // A neighbour on a plane says nothing about it.
                    if planes.iter().any(|&(n, offset)| (dot(n, middle) - offset).abs() < 0.05) {
                        continue;
                    }
                    let weight = 1.0 / f64::from(dx * dx + dy * dy + dz * dz).sqrt();
                    let actual = solid([dx, dy, dz]);
                    for (vote, reading) in votes.iter_mut().zip(&readings) {
                        *vote += if is_in(reading, middle) == actual { weight } else { -weight };
                    }
                }
            }
        }
        let best = (0..readings.len()).max_by(|&a, &b| votes[a].total_cmp(&votes[b]))?;
        let next = (0..readings.len()).filter(|&i| i != best).map(|i| votes[i]).fold(f64::NEG_INFINITY, f64::max);
        if votes[best] - next < SURE {
            return None;
        }
        let mut bits = 0u64;
        for z in 0..SAMPLES {
            for y in 0..SAMPLES {
                for x in 0..SAMPLES {
                    let at = [x, y, z].map(|i| (i as f64 + 0.5) / SAMPLES as f64);
                    // A sample on a plane counts as in, as it does for the
                    // shapes it is matched with.
                    if is_in(&readings[best], at) {
                        bits |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                    }
                }
            }
        }
        Some(bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pieces of a flat sheet through a point: nine of them around it.
    fn pieces(point: V3, normal: V3) -> Vec<(f64, V3, V3)> {
        let length = dot(normal, normal).sqrt();
        let n = normal.map(|v| v / length);
        let cross = |a: V3, b: V3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let helper = if n[1].abs() > 0.5 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let across = cross(n, helper);
        let across = across.map(|v| v / dot(across, across).sqrt());
        let other = cross(n, across);
        let mut out = Vec::new();
        for i in -1..=1 {
            for j in -1..=1 {
                let (a, b) = (f64::from(i) * 0.15, f64::from(j) * 0.15);
                out.push((0.1, [0, 1, 2].map(|k| point[k] + a * across[k] + b * other[k]), n));
            }
        }
        out
    }

    fn patch(sheets: &[(V3, V3)]) -> Patch {
        let mut patch = Patch::default();
        for &(point, normal) in sheets {
            for (area, at, n) in pieces(point, normal) {
                patch.add(7, area, at, n);
            }
        }
        patch
    }

    /// Neighbours that are solid below a horizontal plane at `height`.
    fn below(height: f64) -> impl Fn([i32; 3]) -> bool {
        move |d| 0.5 + f64::from(d[1]) < height
    }

    #[test]
    fn a_level_sheet_takes_the_samples_below_it() {
        let bits = patch(&[([0.5, 0.5, 0.5], [0.0, 1.0, 0.0])]).samples(below(0.5)).unwrap();
        // The plane runs between two layers of samples, so half are in.
        assert_eq!(bits.count_ones(), 32);
        // The sample at the bottom corner is in, the top one is out.
        assert_eq!(bits & 1, 1);
        assert_eq!(bits >> (3 * SAMPLES) & 1, 0);
    }

    #[test]
    fn the_sheet_is_found_whichever_way_its_normal_was_written() {
        let up = patch(&[([0.5, 0.75, 0.5], [0.0, 1.0, 0.0])]).samples(below(0.75)).unwrap();
        let down = patch(&[([0.5, 0.75, 0.5], [0.0, -1.0, 0.0])]).samples(below(0.75)).unwrap();
        assert_eq!(up, down);
        assert_eq!(up.count_ones(), 48);
    }

    #[test]
    fn a_slanted_sheet_takes_the_corner_side_of_it() {
        // The plane x + y = 1 through the middle; the model is below it.
        let bits = patch(&[([0.5, 0.5, 0.5], [1.0, 1.0, 0.0])]).samples(|d| f64::from(d[0]) + f64::from(d[1]) < 0.0).unwrap();
        // Ten of the sixteen in each layer: six below the plane and four on it.
        assert_eq!(bits.count_ones(), 40);
        // The corner at x = y = 0 is in, the opposite one is out.
        assert_eq!(bits & 1, 1);
        assert_eq!(bits >> (SAMPLES - 1 + SAMPLES * (SAMPLES - 1)) & 1, 0);
    }

    #[test]
    fn two_sheets_at_a_ridge_take_what_is_inside_both() {
        // The model is the part with x and y below a half: the top face at
        // y = 0.5 over the left half, the right face at x = 0.5 below it.
        let p = patch(&[([0.3, 0.5, 0.5], [0.0, 1.0, 0.0]), ([0.5, 0.3, 0.5], [1.0, 0.0, 0.0])]);
        let bits = p.samples(|d| d[0] < 0 && d[1] < 0).unwrap();
        assert_eq!(bits.count_ones(), 16);
        for z in 0..SAMPLES {
            for y in 0..SAMPLES {
                for x in 0..SAMPLES {
                    assert_eq!(bits >> (x + SAMPLES * y + SAMPLES * SAMPLES * z) & 1 == 1, x < 2 && y < 2, "({x}, {y}, {z})");
                }
            }
        }
    }

    #[test]
    fn two_sheets_at_a_crease_take_what_is_inside_either() {
        // The model is everything but the corner where x and y are above a
        // half: the floor at y = 0.5 on the right, the wall at x = 0.5 above.
        let p = patch(&[([0.7, 0.5, 0.5], [0.0, 1.0, 0.0]), ([0.5, 0.7, 0.5], [1.0, 0.0, 0.0])]);
        let bits = p.samples(|d| d[0] < 0 || d[1] < 0).unwrap();
        assert_eq!(bits.count_ones(), 48);
    }

    #[test]
    fn three_sheets_are_a_corner_and_left_alone() {
        let p = patch(&[([0.3, 0.5, 0.5], [0.0, 1.0, 0.0]), ([0.5, 0.3, 0.5], [1.0, 0.0, 0.0]), ([0.5, 0.5, 0.3], [0.0, 0.0, 1.0])]);
        assert_eq!(p.samples(|d| d[0] < 0 && d[1] < 0 && d[2] < 0), None);
    }

    #[test]
    fn two_sheets_apart_are_a_thin_wall_and_left_alone() {
        let p = patch(&[([0.5, 0.2, 0.5], [0.0, 1.0, 0.0]), ([0.5, 0.8, 0.5], [0.0, -1.0, 0.0])]);
        assert_eq!(p.samples(below(0.5)), None);
    }

    #[test]
    fn a_plate_thinner_than_a_cell_is_a_wall_whose_faces_are_close() {
        let p = patch(&[([0.5, 0.45, 0.5], [0.0, 1.0, 0.0]), ([0.5, 0.55, 0.5], [0.0, -1.0, 0.0])]);
        assert_eq!(p.samples(below(0.5)), None);
    }

    #[test]
    fn faces_turned_round_in_one_place_are_still_one_sheet() {
        let mut p = patch(&[([0.5, 0.5, 0.5], [0.0, 1.0, 0.0])]);
        for (area, at, n) in pieces([0.5, 0.5, 0.5], [0.0, -1.0, 0.0]).into_iter().take(3) {
            p.add(2, area, at, n);
        }
        assert_eq!(p.samples(below(0.5)).map(u64::count_ones), Some(32));
    }

    #[test]
    fn a_speck_and_neighbours_that_cannot_decide_are_left_alone() {
        let mut speck = Patch::default();
        speck.add(1, 1e-5, [0.5; 3], [0.0, 1.0, 0.0]);
        assert_eq!(speck.samples(below(0.5)), None);
        // Every neighbour on the level of the cell solid: nothing says which
        // side of the plane is the model.
        let flat = patch(&[([0.5, 0.5, 0.5], [0.0, 1.0, 0.0])]);
        assert_eq!(flat.samples(|d| d[1] == 0), None);
    }

    #[test]
    fn a_bent_surface_is_not_a_sheet() {
        // Normals spread over 45 degrees (all within the sheet's alignment
        // but not flat enough).
        let mut p = Patch::default();
        for i in 0..9 {
            let angle = (f64::from(i) - 4.0) * 0.1;
            p.add(1, 0.1, [0.5, 0.5, 0.5], [angle.sin(), angle.cos(), 0.0]);
        }
        assert!(p.samples(below(0.5)).is_some(), "a gentle bend is a sheet");
        let mut bent = Patch::default();
        for i in 0..9 {
            let angle = (f64::from(i) - 4.0) * 0.12;
            bent.add(1, 0.1, [0.5, 0.5, 0.5], [angle.sin(), angle.cos(), 0.0]);
        }
        assert!(bent.samples(below(0.5)).is_none(), "a sharp bend is not");
    }
}
