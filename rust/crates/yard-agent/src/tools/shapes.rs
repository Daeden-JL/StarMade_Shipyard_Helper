//! Solid geometry on the block grid: ellipsoids, cylinders and cones, lofted
//! hulls, thick lines, and the shell of any of them.
//!
//! A shape is first worked out as a solid in a [`Grid`]; hollow shapes are
//! the solid minus the solid shrunk by the wall thickness. One way of
//! hollowing serves every shape.

use std::collections::BTreeSet;
use yard_core::Pos;

/// Most cells a shape's bounding box may have. A shape that fills it would
/// be refused as an edit anyway; this keeps the working memory small.
pub const MAX_GRID: u64 = 4 * yard_core::edit::MAX_CELLS;

/// A box of cells, each in or out of a shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    lo: Pos,
    size: [i32; 3],
    cells: Vec<bool>,
}

impl Grid {
    /// An empty grid over the box from `lo` to `hi`, both included.
    pub fn new(lo: Pos, hi: Pos) -> Result<Self, String> {
        let size = [hi.x - lo.x + 1, hi.y - lo.y + 1, hi.z - lo.z + 1];
        let volume = size.iter().map(|s| (*s).max(0) as u64).product::<u64>();
        if volume > MAX_GRID {
            return Err(format!("That shape spans {} x {} x {} blocks, which is too large to build in one step. Build it in parts.", size[0], size[1], size[2]));
        }
        Ok(Self { lo, size, cells: vec![false; volume as usize] })
    }

    fn index(&self, p: Pos) -> Option<usize> {
        let (x, y, z) = (p.x - self.lo.x, p.y - self.lo.y, p.z - self.lo.z);
        let inside = x >= 0 && y >= 0 && z >= 0 && x < self.size[0] && y < self.size[1] && z < self.size[2];
        inside.then(|| (x + self.size[0] * (y + self.size[1] * z)) as usize)
    }

    pub fn has(&self, p: Pos) -> bool {
        self.index(p).is_some_and(|i| self.cells[i])
    }

    pub fn set(&mut self, p: Pos) {
        if let Some(i) = self.index(p) {
            self.cells[i] = true;
        }
    }

    /// Marks every cell of the grid for which `inside` holds.
    fn fill(&mut self, inside: impl Fn(Pos) -> bool) {
        for z in 0..self.size[2] {
            for y in 0..self.size[1] {
                for x in 0..self.size[0] {
                    let p = self.lo.offset(x, y, z);
                    if inside(p) {
                        self.cells[(x + self.size[0] * (y + self.size[1] * z)) as usize] = true;
                    }
                }
            }
        }
    }

    pub fn count(&self) -> usize {
        self.cells.iter().filter(|c| **c).count()
    }

    /// The cells of the shape.
    pub fn cells(&self) -> BTreeSet<Pos> {
        let mut out = BTreeSet::new();
        for z in 0..self.size[2] {
            for y in 0..self.size[1] {
                for x in 0..self.size[0] {
                    if self.cells[(x + self.size[0] * (y + self.size[1] * z)) as usize] {
                        out.insert(self.lo.offset(x, y, z));
                    }
                }
            }
        }
        out
    }

    /// The outer `thickness` layers of the shape: every cell within that
    /// many steps of the outside. With `open_axis`, steps along that axis do
    /// not count, which leaves a tube open at both ends.
    pub fn shell(&self, thickness: u32, open_axis: Option<usize>) -> Grid {
        let steps: Vec<[i32; 3]> = yard_core::shape::SIDE_DIRS.iter().copied().filter(|d| open_axis.is_none_or(|axis| d[axis] == 0)).collect();
        let mut core = self.clone();
        for _ in 0..thickness {
            let before = core.clone();
            core.fill_from(|p| before.has(p) && steps.iter().all(|d| before.has(p.offset(d[0], d[1], d[2]))));
        }
        let mut shell = self.clone();
        shell.fill_from(|p| self.has(p) && !core.has(p));
        shell
    }

    /// Sets every cell to what `inside` says.
    fn fill_from(&mut self, inside: impl Fn(Pos) -> bool) {
        self.cells.iter_mut().for_each(|c| *c = false);
        self.fill(inside);
    }
}

fn axis_of(p: Pos, axis: usize) -> i32 {
    [p.x, p.y, p.z][axis]
}

fn with_axis(axis: usize, along: i32, u: i32, v: i32) -> Pos {
    match axis {
        0 => Pos::new(along, u, v),
        1 => Pos::new(u, along, v),
        _ => Pos::new(u, v, along),
    }
}

/// A solid ellipsoid. A radius counts blocks from the centre block outward,
/// so radius 3 spans 7 blocks.
pub fn ellipsoid(center: Pos, radii: [f64; 3]) -> Result<Grid, String> {
    let reach = radii.map(|r| r.max(0.0).floor() as i32);
    let mut grid = Grid::new(center.offset(-reach[0], -reach[1], -reach[2]), center.offset(reach[0], reach[1], reach[2]))?;
    grid.fill(|p| {
        let d = [(p.x - center.x) as f64, (p.y - center.y) as f64, (p.z - center.z) as f64];
        (0..3).map(|i| (d[i] / (radii[i].max(0.0) + 0.5)).powi(2)).sum::<f64>() <= 1.0
    });
    Ok(grid)
}

/// A solid cylinder, cone or tapered tube along an axis (0 X, 1 Y, 2 Z).
/// It starts with radius `start_radius` at `from` and runs `length` blocks
/// toward the positive end of the axis, or the negative end if `length` is
/// negative, reaching `end_radius` at its last layer.
pub fn cylinder(from: Pos, axis: usize, length: i32, start_radius: f64, end_radius: f64) -> Result<Grid, String> {
    let layers = length.abs().max(1);
    let last = axis_of(from, axis) + (layers - 1) * length.signum();
    let reach = start_radius.max(end_radius).max(0.0).floor() as i32;
    let (u0, v0) = cross(from, axis);
    let lo = with_axis(axis, axis_of(from, axis).min(last), u0 - reach, v0 - reach);
    let hi = with_axis(axis, axis_of(from, axis).max(last), u0 + reach, v0 + reach);
    let mut grid = Grid::new(lo, hi)?;
    grid.fill(|p| {
        let step = (axis_of(p, axis) - axis_of(from, axis)).abs();
        let t = if layers > 1 { step as f64 / (layers - 1) as f64 } else { 0.0 };
        let radius = start_radius + (end_radius - start_radius) * t;
        let (u, v) = cross(p, axis);
        let (du, dv) = ((u - u0) as f64, (v - v0) as f64);
        radius >= 0.0 && du * du + dv * dv <= (radius + 0.5).powi(2)
    });
    Ok(grid)
}

/// The two coordinates of a position across an axis.
fn cross(p: Pos, axis: usize) -> (i32, i32) {
    match axis {
        0 => (p.y, p.z),
        1 => (p.x, p.z),
        _ => (p.x, p.y),
    }
}

/// One cross-section of a hull.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// Where along the ship, relative to the hull's origin.
    pub z: i32,
    /// Blocks from the centre line to the side; 0 is a single column.
    pub half_width: f64,
    /// Blocks from the centre line to the top and to the bottom.
    pub half_height: f64,
    /// 0 is a box, 1 an ellipse, 2 a diamond; values between blend.
    pub roundness: f64,
    /// Raises the centre of this section.
    pub y_offset: f64,
}

/// A solid hull lofted through cross-sections along Z: between two
/// sections every measure changes evenly from one to the other.
pub fn hull(origin: Pos, sections: &[Section]) -> Result<Grid, String> {
    let mut sections = sections.to_vec();
    sections.sort_by_key(|s| s.z);
    let (Some(first), Some(last)) = (sections.first().copied(), sections.last().copied()) else {
        return Err("A hull needs at least one section.".to_string());
    };
    if sections.windows(2).any(|pair| pair[0].z == pair[1].z) {
        return Err("Two hull sections have the same z. Give each section its own z.".to_string());
    }
    let widest = sections.iter().map(|s| s.half_width.max(0.0)).fold(0.0, f64::max).floor() as i32;
    let top = sections.iter().map(|s| s.y_offset + s.half_height.max(0.0)).fold(f64::MIN, f64::max).ceil() as i32;
    let bottom = sections.iter().map(|s| s.y_offset - s.half_height.max(0.0)).fold(f64::MAX, f64::min).floor() as i32;
    let mut grid = Grid::new(origin.offset(-widest, bottom, first.z), origin.offset(widest, top, last.z))?;
    grid.fill(|p| {
        let z = p.z - origin.z;
        let at = section_at(&sections, z);
        let dx = (p.x - origin.x).abs() as f64;
        let dy = (p.y as f64 - origin.y as f64 - at.y_offset).abs();
        in_section(dx, dy, &at)
    });
    Ok(grid)
}

/// The cross-section at `z`, blended from the sections either side of it.
fn section_at(sections: &[Section], z: i32) -> Section {
    let after = sections.iter().position(|s| s.z >= z).unwrap_or(sections.len() - 1);
    let (a, b) = (sections[after.saturating_sub(1)], sections[after]);
    if a.z == b.z || z >= b.z {
        return b;
    }
    let t = (z - a.z) as f64 / (b.z - a.z) as f64;
    let blend = |from: f64, to: f64| from + (to - from) * t;
    Section { z, half_width: blend(a.half_width, b.half_width), half_height: blend(a.half_height, b.half_height), roundness: blend(a.roundness, b.roundness), y_offset: blend(a.y_offset, b.y_offset) }
}

/// Whether a cell `dx` from the centre line and `dy` from the section's
/// centre lies inside the section: a superellipse, from box to diamond.
fn in_section(dx: f64, dy: f64, s: &Section) -> bool {
    if s.half_width < 0.0 || s.half_height < 0.0 {
        return false;
    }
    let (a, b) = (s.half_width + 0.5, s.half_height + 0.5);
    if dx > a || dy > b {
        return false;
    }
    let roundness = s.roundness.clamp(0.0, 2.0);
    if roundness < 0.05 {
        return true;
    }
    let power = 2.0 / roundness;
    (dx / a).powf(power) + (dy / b).powf(power) <= 1.0
}

/// A straight line of blocks between two points, `thickness` blocks wide
/// and high.
pub fn line(from: Pos, to: Pos, thickness: i32) -> BTreeSet<Pos> {
    let delta = [to.x - from.x, to.y - from.y, to.z - from.z];
    let steps = delta.iter().map(|d| d.abs()).max().unwrap_or(0);
    let (before, after) = ((thickness - 1) / 2, thickness / 2);
    let mut cells = BTreeSet::new();
    for i in 0..=steps {
        let t = if steps == 0 { 0.0 } else { i as f64 / steps as f64 };
        let at = |axis: usize, start: i32| start + (delta[axis] as f64 * t).round() as i32;
        let centre = Pos::new(at(0, from.x), at(1, from.y), at(2, from.z));
        for dz in -before..=after {
            for dy in -before..=after {
                for dx in -before..=after {
                    cells.insert(centre.offset(dx, dy, dz));
                }
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: Pos = Pos::new(0, 0, 0);

    fn section(z: i32, half_width: f64, half_height: f64, roundness: f64) -> Section {
        Section { z, half_width, half_height, roundness, y_offset: 0.0 }
    }

    #[test]
    fn an_ellipsoid_spans_its_radii_and_is_symmetric() {
        let grid = ellipsoid(Pos::new(2, 0, -1), [3.0, 1.0, 5.0]).unwrap();
        let cells = grid.cells();
        let (lo, hi) = cells.iter().fold((Pos::new(99, 99, 99), Pos::new(-99, -99, -99)), |(lo, hi), p| (lo.lo(*p), hi.hi(*p)));
        assert_eq!((lo, hi), (Pos::new(-1, -1, -6), Pos::new(5, 1, 4)));
        assert!(cells.iter().all(|p| cells.contains(&Pos::new(4 - p.x, -p.y, -2 - p.z))));
        assert!(!grid.has(Pos::new(5, 1, 4)), "the corners of the bounding box are outside");
        assert_eq!(ellipsoid(ORIGIN, [0.0, 0.0, 0.0]).unwrap().cells().len(), 1);
    }

    #[test]
    fn a_shell_is_closed_and_as_thick_as_asked() {
        let solid = ellipsoid(ORIGIN, [6.0, 6.0, 6.0]).unwrap();
        let shell = solid.shell(2, None);
        assert!(!shell.has(ORIGIN) && !shell.has(Pos::new(4, 0, 0)));
        assert!(shell.has(Pos::new(5, 0, 0)) && shell.has(Pos::new(6, 0, 0)) && !shell.has(Pos::new(7, 0, 0)));
        // Closed: walking outward from the centre always meets the shell.
        for step in yard_core::shape::SIDE_DIRS {
            assert!((0..8).any(|i| shell.has(Pos::new(step[0] * i, step[1] * i, step[2] * i))));
        }
        assert!(shell.count() < solid.count());
        assert_eq!(solid.shell(20, None), solid, "walls thicker than the shape leave it solid");
    }

    #[test]
    fn a_cylinder_runs_along_its_axis_in_either_direction() {
        let up = cylinder(Pos::new(0, 2, 0), 1, 4, 1.0, 1.0).unwrap().cells();
        assert_eq!(up.len(), 4 * 9, "radius 1 is a 3 by 3 column");
        assert!(up.contains(&Pos::new(1, 5, -1)) && !up.contains(&Pos::new(0, 6, 0)) && !up.contains(&Pos::new(0, 1, 0)));
        let back = cylinder(Pos::new(0, 0, -3), 2, -5, 2.0, 2.0).unwrap().cells();
        assert!(back.contains(&Pos::new(0, 0, -7)) && !back.contains(&Pos::new(0, 0, -8)) && !back.contains(&Pos::new(0, 0, -2)));
        assert!(back.contains(&Pos::new(2, 1, -3)) && !back.contains(&Pos::new(2, 2, -3)), "the cross-section is round");
    }

    #[test]
    fn a_cone_tapers_to_its_end_radius_and_a_tube_can_stay_open() {
        let cone = cylinder(ORIGIN, 2, 7, 3.0, 0.0).unwrap();
        let width_at = |z: i32| (0..5).filter(|x| cone.has(Pos::new(*x, 0, z))).count();
        assert_eq!((width_at(0), width_at(3), width_at(6)), (4, 3, 1));
        let tube = cylinder(ORIGIN, 2, 6, 3.0, 3.0).unwrap();
        let open = tube.shell(1, Some(2));
        assert!(!open.has(ORIGIN) && !open.has(Pos::new(0, 0, 5)) && open.has(Pos::new(3, 0, 0)));
        let capped = tube.shell(1, None);
        assert!(capped.has(ORIGIN) && capped.has(Pos::new(0, 0, 5)) && !capped.has(Pos::new(0, 0, 2)));
    }

    #[test]
    fn a_hull_blends_evenly_between_its_sections() {
        // A stern 4 wide, widest amidships, a point at the nose.
        let sections = [section(-10, 2.0, 1.0, 0.0), section(0, 5.0, 3.0, 1.0), section(12, 0.0, 0.0, 1.0)];
        let grid = hull(ORIGIN, &sections).unwrap();
        let half_width_at = |z: i32| (0..9).filter(|x| grid.has(Pos::new(*x, 0, z))).count() as i32 - 1;
        assert_eq!((half_width_at(-10), half_width_at(0), half_width_at(12)), (2, 5, 0));
        assert_eq!(half_width_at(-5), 4, "halfway between 2 and 5, rounded outward");
        assert!(half_width_at(6) < half_width_at(0) && half_width_at(6) > half_width_at(12));
        assert_eq!(half_width_at(13), -1, "nothing beyond the last section");
        // The stern is a box, the middle an ellipse.
        assert!(grid.has(Pos::new(2, 1, -10)) && !grid.has(Pos::new(5, 3, 0)) && grid.has(Pos::new(0, 3, 0)));
        // Left and right halves match.
        let cells = grid.cells();
        assert!(cells.iter().all(|p| cells.contains(&Pos::new(-p.x, p.y, p.z))));
    }

    #[test]
    fn a_hull_follows_its_origin_and_section_offsets() {
        let mut raised = section(4, 2.0, 1.0, 0.0);
        raised.y_offset = 3.0;
        let grid = hull(Pos::new(10, 0, 100), &[section(0, 2.0, 1.0, 0.0), raised]).unwrap();
        assert!(grid.has(Pos::new(10, 0, 100)) && grid.has(Pos::new(12, -1, 100)) && !grid.has(Pos::new(10, 3, 100)));
        assert!(grid.has(Pos::new(10, 3, 104)) && grid.has(Pos::new(10, 4, 104)) && !grid.has(Pos::new(10, 0, 104)));
    }

    #[test]
    fn hull_sections_may_come_in_any_order_but_not_twice_at_one_z() {
        let (a, b) = (section(5, 1.0, 1.0, 0.0), section(0, 3.0, 1.0, 0.0));
        assert_eq!(hull(ORIGIN, &[a, b]).unwrap(), hull(ORIGIN, &[b, a]).unwrap());
        assert!(hull(ORIGIN, &[a, a]).unwrap_err().contains("same z"));
        assert!(hull(ORIGIN, &[]).is_err());
        assert_eq!(hull(ORIGIN, &[a]).unwrap().count(), 9, "one section is one slice");
    }

    #[test]
    fn a_line_joins_its_ends_without_gaps() {
        let cells = line(Pos::new(0, 0, 0), Pos::new(6, 3, -2), 1);
        assert_eq!(cells.len(), 7);
        assert!(cells.contains(&Pos::new(0, 0, 0)) && cells.contains(&Pos::new(6, 3, -2)));
        let thick = line(Pos::new(0, 0, 0), Pos::new(0, 0, 4), 3);
        assert_eq!(thick.len(), 9 * 7, "a 3 thick line is 3 by 3 around its path, ends included");
        assert_eq!(line(ORIGIN, ORIGIN, 1).len(), 1);
    }

    #[test]
    fn a_shape_too_large_to_build_is_refused_before_it_is_worked_out() {
        assert!(ellipsoid(ORIGIN, [200.0, 200.0, 200.0]).unwrap_err().contains("too large"));
    }
}
