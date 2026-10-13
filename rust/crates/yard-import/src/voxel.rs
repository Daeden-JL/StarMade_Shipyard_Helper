//! Turning a model into blocks.
//!
//! The model is laid into a grid of unit cells so that its longest side is
//! as many cells as asked for. A cell becomes a surface block where the
//! model's surface crosses one of the three lines through the cell's centre
//! (from the middle of one side to the middle of the opposite side). That
//! gives the thinnest shell that still has no gap a six-way walk could slip
//! through, so a flat plate is one layer and not two, and what the shell
//! encloses can be found by flooding the empty space from outside.
//!
//! A second pass saves what the first cannot see: parts thinner than a cell
//! (an antenna, the rim of a thin wing) that pass between those lines. A
//! cell a triangle passes through is filled as well when there is no block
//! of the first pass in it or right in front of or behind it along the
//! triangle's steepest axis.
//!
//! The grid is kept sparse, in pages of 16 by 16 by 16 cells that exist only
//! where a bit is set, so a ship 4096 cells long costs what its surface
//! costs and not what its bounding box does. What is known per surface block
//! (its nearest triangle) lives in a hash map, which only grows with the
//! surface and is capped with the result at [`MAX_BLOCKS`]. Finding the
//! inside crosses empty pages in one step each.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};

use crate::colour::tinted;
use crate::error::{too_large, ImportError, Result};
use crate::math::{blend, cross, dot, nearest_on_triangle, sub, wide, V3};
use crate::model::{Blocks, Model, Paint, Triangle, Up, MAX_BLOCKS};
use crate::smooth::Patch;

/// The shortest and longest "longest side" that can be asked for.
pub const LONGEST_RANGE: std::ops::RangeInclusive<u32> = 4..=8192;

/// How many grid rows and columns all triangles together may be walked
/// over. A fine model needs a few million; this stops a file built to keep
/// the machine busy (millions of triangles each as large as the grid).
const MAX_WORK: u64 = 3_000_000_000;

/// Smoothing keeps about 150 bytes for every cell the surface passes
/// through; more than this many and it is refused.
const MAX_SMOOTHED: usize = 20_000_000;

/// How far a surface is pushed against its outward side before its cell is
/// chosen, in cells. A wall lying exactly between two cells then goes to
/// the cell behind it, so a model drawn on whole numbers keeps its size.
const INWARD: f64 = 1e-4;
/// How far inside the grid a point on its very border is taken to be.
const EDGE: f64 = 1e-6;

/// How to turn a model into blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The length of the longest side of the result in blocks, held to
    /// [`LONGEST_RANGE`]. A file that is blocks already is not resized;
    /// for it this is the most it may measure.
    pub longest: u32,
    /// Fill the inside (true) or leave a shell (false), `shell` blocks thick.
    pub solid: bool,
    /// How thick the shell of a hollow result is, in blocks, from 1 to
    /// [`MAX_SHELL`]: the inside cells within this many blocks of the
    /// surface (counting a step along an edge or a corner as one) are filled
    /// too. A shell of one block is the surface alone.
    pub shell: u8,
    /// Which axis of the file points up; `None` takes the format's
    /// convention, which is in [`Model::up`].
    ///
    /// The result is right-handed with +Y up and +Z forward (the nose of
    /// the ship), so +X is on the pilot's left.
    /// - `Up::Y`: the file's (x, y, z) is the ship's (x, y, z): the file's
    ///   +Z is the nose, as in glTF.
    /// - `Up::Z`: the file's (x, y, z) is the ship's (x, z, -y): the file's
    ///   -Y is the nose and +X stays on the left, as in Blender.
    pub up: Option<Up>,
    /// Quarter turns about the up axis, 0 to 3, applied after `up`. Each
    /// one swings the nose to the left: what pointed forward (+Z) points
    /// left (+X) afterwards.
    pub turn: u8,
    /// Swap left and right, applied last. For files made left-handed.
    pub mirror: bool,
    /// Smooth steps: where the surface of the model slants across a cell,
    /// the cell gets a [`Cell::partial`] mask saying which part of it the
    /// model fills, so that it can be a wedge or a corner and not a whole
    /// block. Only blocks of the plain result are reshaped, the outer layer:
    /// nothing is added in the hollows between steps. Cells where the surface bends sharply stay whole.
    /// Not applied to files that are blocks already.
    pub smooth: bool,
}

/// The thickest shell a hollow result may have.
pub const MAX_SHELL: u8 = 16;

impl Default for Options {
    fn default() -> Options {
        Options { longest: 64, solid: true, shell: 1, up: None, turn: 0, mirror: false, smooth: false }
    }
}

/// One block of the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// x, y, z counted from 0.
    pub pos: [u16; 3],
    /// True for a block of the filling, which the surface encloses; false
    /// for a block of the surface itself.
    pub inside: bool,
    /// The model's colour here as sRGB bytes. Nothing for inside blocks and
    /// where the file says nothing about colour.
    pub colour: Option<[u8; 3]>,
    /// Where the model fills only part of the cell (with `Options::smooth`):
    /// which of its 4 by 4 by 4 sample points, at the middles of the
    /// sub-cells, are filled, bit `x + 4 y + 16 z`. Nothing for a whole
    /// cell.
    pub partial: Option<u64>,
    /// With `Options::smooth`, for a block of the surface: which of its sides
    /// face open air (reached from outside the model), bit `1 << side` in
    /// the game's order: +Z, -Z, +Y, -Y, +X, -X. Zero otherwise.
    pub air: u8,
}

/// A model as blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Voxels {
    /// How many cells the grid measures along x, y and z.
    pub size: [u32; 3],
    /// The filled cells, ordered by z, then y, then x.
    pub cells: Vec<Cell>,
    /// Things the player should know.
    pub notes: Vec<String>,
}

/// Turns a model into blocks. See [`Options`] for the choices and the axes
/// of the result.
pub fn voxelize(model: &Model, options: &Options) -> Result<Voxels> {
    let longest = options.longest.clamp(*LONGEST_RANGE.start(), *LONGEST_RANGE.end());
    let axes = Axes::new(options.up.unwrap_or(model.up), options.turn, options.mirror);
    let shell = options.shell.clamp(1, MAX_SHELL);
    let mut notes = model.notes.clone();
    if !model.has_colours() {
        notes.push("The file has no colours.".into());
    }
    let mut voxels = match &model.blocks {
        Some(blocks) => place_blocks(blocks, &axes, longest, options.solid, shell)?,
        None => voxelize_triangles(model, &axes, longest, options.solid, shell, options.smooth)?,
    };
    if options.smooth && model.blocks.is_some() {
        notes.push("A model made of blocks is not smoothed.".into());
    }
    voxels.notes = notes;
    Ok(voxels)
}

/// How the file's axes become the ship's: each ship axis takes one file
/// axis, maybe turned around.
#[derive(Clone, Copy, Debug)]
struct Axes {
    from: [usize; 3],
    sign: [i8; 3],
}

impl Axes {
    fn new(up: Up, turn: u8, mirror: bool) -> Axes {
        let mut axes = match up {
            Up::Y => Axes { from: [0, 1, 2], sign: [1, 1, 1] },
            Up::Z => Axes { from: [0, 2, 1], sign: [1, 1, -1] },
        };
        for _ in 0..turn % 4 {
            // (x, y, z) becomes (z, y, -x).
            axes = Axes { from: [axes.from[2], axes.from[1], axes.from[0]], sign: [axes.sign[2], axes.sign[1], -axes.sign[0]] };
        }
        if mirror {
            axes.sign[0] = -axes.sign[0];
        }
        axes
    }

    fn point(&self, p: V3) -> V3 {
        [0, 1, 2].map(|i| f64::from(self.sign[i]) * p[self.from[i]])
    }

    fn cell(&self, p: [i32; 3]) -> [i64; 3] {
        [0, 1, 2].map(|i| i64::from(self.sign[i]) * i64::from(p[self.from[i]]))
    }

    /// Whether this turns shapes into their mirror image, which also turns
    /// every triangle's front to the back.
    fn mirrors(&self) -> bool {
        let swaps = (0..3).filter(|&i| self.from[i] != i).count() == 2;
        let negatives = self.sign.iter().filter(|&&s| s < 0).count() % 2 == 1;
        swaps != negatives
    }
}

/// Bits per side of a page of the grid.
const PAGE_SHIFT: usize = 4;
const PAGE_SIDE: usize = 1 << PAGE_SHIFT;
const PAGE_MASK: usize = PAGE_SIDE - 1;
/// A page nobody has written to, and a page of which every cell counts.
const BLANK: u32 = u32::MAX;
const FULL: u32 = u32::MAX - 1;

/// One bit per cell, kept in pages of 16 by 16 by 16 that only exist once a
/// bit in them is set. A ship's surface touches a small share of its
/// bounding box, so a grid 4096 cells long costs what the surface costs, not
/// what the box does. A page can also be marked [`FULL`] as a whole.
#[derive(Clone, Default)]
struct Bits {
    size: [usize; 3],
    /// Pages along each axis.
    pages: [usize; 3],
    /// Which page of `arena` holds each page, or `BLANK` or `FULL`.
    table: Vec<u32>,
    arena: Vec<[u64; PAGE_SIDE * PAGE_SIDE * PAGE_SIDE / 64]>,
}

impl Bits {
    fn new(size: [usize; 3]) -> Bits {
        let pages = size.map(|n| n.div_ceil(PAGE_SIDE));
        Bits { size, pages, table: vec![BLANK; pages[0] * pages[1] * pages[2]], arena: Vec::new() }
    }

    fn page_of(&self, cell: [usize; 3]) -> usize {
        let p = cell.map(|v| v >> PAGE_SHIFT);
        p[0] + self.pages[0] * (p[1] + self.pages[1] * p[2])
    }

    fn page_at(&self, page: [usize; 3]) -> usize {
        page[0] + self.pages[0] * (page[1] + self.pages[1] * page[2])
    }

    /// The word and bit of a cell within its page.
    fn place(cell: [usize; 3]) -> (usize, u32) {
        let local = (cell[0] & PAGE_MASK) | (cell[1] & PAGE_MASK) << PAGE_SHIFT | (cell[2] & PAGE_MASK) << (2 * PAGE_SHIFT);
        (local / 64, (local % 64) as u32)
    }

    /// How many cells of a page are inside the grid, per axis.
    fn extent(&self, page: [usize; 3]) -> [usize; 3] {
        [0, 1, 2].map(|i| PAGE_SIDE.min(self.size[i] - page[i] * PAGE_SIDE))
    }

    fn get(&self, cell: [usize; 3]) -> bool {
        match self.table[self.page_of(cell)] {
            BLANK => false,
            FULL => true,
            at => {
                let (word, bit) = Bits::place(cell);
                self.arena[at as usize][word] >> bit & 1 == 1
            }
        }
    }

    fn set(&mut self, cell: [usize; 3]) {
        let index = self.page_of(cell);
        let at = match self.table[index] {
            FULL => return,
            BLANK => {
                self.arena.push([0; PAGE_SIDE * PAGE_SIDE * PAGE_SIDE / 64]);
                self.table[index] = (self.arena.len() - 1) as u32;
                self.arena.len() - 1
            }
            at => at as usize,
        };
        let (word, bit) = Bits::place(cell);
        self.arena[at][word] |= 1 << bit;
    }

    /// How many bits of a page are set; a full page counts its cells.
    fn count_page(&self, page: [usize; 3]) -> usize {
        match self.table[self.page_at(page)] {
            BLANK => 0,
            FULL => self.extent(page).iter().product(),
            at => self.arena[at as usize].iter().map(|w| w.count_ones() as usize).sum(),
        }
    }

    fn is_blank(&self, page: [usize; 3]) -> bool {
        self.table[self.page_at(page)] == BLANK
    }
}

/// Cell numbers are spread well already; mixing them is all the hashing
/// they need, and much faster than the standard hasher.
#[derive(Default)]
struct Mix(u64);

impl Hasher for Mix {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, n: u64) {
        let h = n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = h ^ (h >> 32);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type CellMap<T> = HashMap<u64, T, BuildHasherDefault<Mix>>;

/// The triangle nearest to a surface cell's centre so far.
#[derive(Clone, Copy)]
struct Hit {
    distance: f32,
    triangle: u32,
}

/// Where the model lies in the grid.
struct Fit {
    axes: Axes,
    scale: f64,
    shift: V3,
    size: [usize; 3],
}

impl Fit {
    /// Measures the model and fits its longest side to `longest` cells,
    /// the other sides to as many cells as they need, centred.
    fn new(triangles: &[Triangle], axes: Axes, longest: u32) -> Result<Fit> {
        let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for corner in triangles.iter().flat_map(|t| t.corners).filter(|c| c.iter().all(|v| v.is_finite())) {
            let p = axes.point(wide(corner));
            for i in 0..3 {
                low[i] = low[i].min(p[i]);
                high[i] = high[i].max(p[i]);
            }
        }
        if low[0] > high[0] {
            return Err(ImportError::Empty("The file has no triangles.".into()));
        }
        let extent = [0, 1, 2].map(|i| high[i] - low[i]);
        let widest = extent[0].max(extent[1]).max(extent[2]);
        if !(widest > 0.0 && widest.is_finite()) {
            return Err(ImportError::Empty("The model has no size: all its points are in one place.".into()));
        }
        let scale = f64::from(longest) / widest;
        if !scale.is_finite() {
            return Err(ImportError::Empty("The model is too small to measure.".into()));
        }
        let size = [0, 1, 2].map(|i| ((extent[i] * scale - 1e-6).ceil() as usize).clamp(1, longest as usize));
        let shift = [0, 1, 2].map(|i| (size[i] as f64 - extent[i] * scale) / 2.0 - low[i] * scale);
        Ok(Fit { axes, scale, shift, size })
    }

    /// A triangle in grid cells, or nothing for one that is not all numbers.
    fn place(&self, triangle: &Triangle) -> Option<[V3; 3]> {
        let mut out = [[0.0; 3]; 3];
        for (corner, placed) in triangle.corners.iter().zip(&mut out) {
            if corner.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let p = self.axes.point(wide(*corner));
            for i in 0..3 {
                placed[i] = (p[i] * self.scale + self.shift[i]).clamp(0.0, self.size[i] as f64);
            }
        }
        Some(out)
    }
}

/// The surface blocks found so far.
struct Shell {
    size: [usize; 3],
    /// Blocks of the first pass.
    thin: Bits,
    /// Those and the ones the second pass saved.
    surface: Bits,
    hits: CellMap<Hit>,
    /// The surface in each cell the model passes through, when smoothing.
    patches: Option<CellMap<Patch>>,
    work: u64,
}

impl Shell {
    fn index(&self, cell: [usize; 3]) -> u64 {
        (cell[0] + self.size[0] * (cell[1] + self.size[1] * cell[2])) as u64
    }

    /// Notes that a triangle passes through a surface cell, and keeps it as
    /// the cell's nearest if it comes closer to the centre than any before.
    fn touch(&mut self, cell: [usize; 3], index: u64, id: u32, triangle: &[V3; 3]) -> Result<()> {
        let centre = [cell[0] as f64 + 0.5, cell[1] as f64 + 0.5, cell[2] as f64 + 0.5];
        let away = sub(centre, blend(nearest_on_triangle(centre, *triangle), *triangle));
        let distance = dot(away, away) as f32;
        let hit = self.hits.entry(index).or_insert(Hit { distance: f32::INFINITY, triangle: id });
        if distance < hit.distance {
            *hit = Hit { distance, triangle: id };
        }
        if self.hits.len() > MAX_BLOCKS {
            return Err(too_many_blocks());
        }
        Ok(())
    }

    fn spend(&mut self, work: u64) -> Result<()> {
        self.work += work;
        if self.work > MAX_WORK {
            return Err(too_large("The model is too heavy to turn into blocks at this size. Choose a smaller size or simplify the model."));
        }
        Ok(())
    }

    /// First pass: marks the cells whose centre lines the triangle crosses.
    /// `outward` is +1 or -1: the side of the triangle's own normal that
    /// faces out of the model.
    fn cross_lines(&mut self, id: u32, triangle: &[V3; 3], normal: V3, outward: f64) -> Result<()> {
        let length = dot(normal, normal).sqrt();
        for w in 0..3 {
            // A triangle seen edge-on along this axis is crossed by no line
            // worth finding; the other two axes see it.
            if normal[w].abs() <= 1e-9 * length {
                continue;
            }
            let (u, v) = ((w + 1) % 3, (w + 2) % 3);
            let along = |axis: usize| (triangle[0][axis].min(triangle[1][axis]).min(triangle[2][axis]), triangle[0][axis].max(triangle[1][axis]).max(triangle[2][axis]));
            let ((v_low, v_high), (w_low, w_high)) = (along(v), along(w));
            let Some((first_row, last_row)) = centres_between(v_low, v_high, self.size[v]) else { continue };
            self.spend((last_row - first_row + 1) as u64)?;
            for row in first_row..=last_row {
                let y = row as f64 + 0.5;
                let Some((left, right)) = span(triangle, u, v, y) else { continue };
                let Some((first, last)) = centres_between(left, right, self.size[u]) else { continue };
                self.spend((last - first + 1) as u64)?;
                for column in first..=last {
                    let x = column as f64 + 0.5;
                    let depth = triangle[0][w] - (normal[u] * (x - triangle[0][u]) + normal[v] * (y - triangle[0][v])) / normal[w];
                    let depth = depth.clamp(w_low, w_high) - INWARD * outward * normal[w].signum();
                    let layer = depth.clamp(EDGE, self.size[w] as f64 - EDGE) as usize;
                    let mut cell = [0; 3];
                    (cell[u], cell[v], cell[w]) = (column, row, layer.min(self.size[w] - 1));
                    let index = self.index(cell);
                    self.thin.set(cell);
                    self.touch(cell, index, id, triangle)?;
                }
            }
        }
        Ok(())
    }

    /// Notes the part of a triangle that lies in each cell of one column
    /// (`piece` is the triangle cut to the column; `axes` are u, v and the
    /// depth along which the column runs).
    fn collect(&mut self, id: u32, piece: &Outline, axes: [usize; 3], column: [usize; 2], normal: V3) -> Result<()> {
        let [u, v, w] = axes;
        let length = dot(normal, normal).sqrt();
        let unit = normal.map(|c| c / length);
        let (low, high) = piece.range(2);
        let first = (low.max(0.0) as usize).min(self.size[w] - 1);
        let last = (high.ceil().max(1.0) as usize - 1).clamp(first, self.size[w] - 1);
        self.spend((last - first + 1) as u64)?;
        for layer in first..=last {
            let part = piece.cut(2, layer as f64, true).cut(2, layer as f64 + 1.0, false);
            if part.len < 3 {
                continue;
            }
            let mut cell = [0; 3];
            (cell[u], cell[v], cell[w]) = (column[0], column[1], layer);
            // The part's area and middle, as the sum of a fan of triangles.
            let points = &part.points;
            let (mut area, mut middle) = (0.0, [0.0; 3]);
            for i in 1..part.len - 1 {
                let (a, b) = (sub(points[i], points[0]), sub(points[i + 1], points[0]));
                let size = dot(cross(a, b), cross(a, b)).sqrt() / 2.0;
                area += size;
                for k in 0..3 {
                    middle[k] += size * (points[0][k] + points[i][k] + points[i + 1][k]) / 3.0;
                }
            }
            if area <= 0.0 {
                continue;
            }
            // From (u, v, depth) back to x, y, z, and to cell coordinates.
            let mut at = [0.0; 3];
            (at[u], at[v], at[w]) = (middle[0] / area - cell[u] as f64, middle[1] / area - cell[v] as f64, middle[2] / area - layer as f64);
            let index = self.index(cell);
            if let Some(patches) = &mut self.patches {
                patches.entry(index).or_default().add(id, area, at, unit);
                if patches.len() > MAX_SMOOTHED {
                    return Err(too_large(format!("Smoothing a result of more than {MAX_SMOOTHED} blocks takes too much memory. Choose a smaller size or turn smoothing off.")));
                }
            }
        }
        Ok(())
    }

    /// Second pass: walks every cell the triangle passes through, column by
    /// column along its steepest axis. A cell that is surface already hears
    /// of the triangle for its colour; one that is not becomes surface if
    /// the first pass left nothing in it or next to it in its column.
    fn pass_through(&mut self, id: u32, triangle: &[V3; 3], normal: V3, outward: f64) -> Result<()> {
        let w = (0..3).fold(0, |best, i| if normal[i].abs() > normal[best].abs() { i } else { best });
        let (u, v) = ((w + 1) % 3, (w + 2) % 3);
        let whole = Outline::of(triangle.map(|p| [p[u], p[v], p[w]]));
        let least = 1e-9 * whole.area().min(1.0);
        let (v_low, v_high) = whole.range(1);
        let (first_row, last_row) = cells_between(v_low, v_high, self.size[v]);
        let push = INWARD * outward * normal[w].signum();
        self.spend((last_row - first_row + 1) as u64)?;
        for row in first_row..=last_row {
            let strip = whole.cut(1, row as f64, true).cut(1, row as f64 + 1.0, false);
            if strip.len < 3 {
                continue;
            }
            let (u_low, u_high) = strip.range(0);
            let (first, last) = cells_between(u_low, u_high, self.size[u]);
            self.spend((last - first + 1) as u64)?;
            for column in first..=last {
                let piece = strip.cut(0, column as f64, true).cut(0, column as f64 + 1.0, false);
                // A piece with no area only touches the column's border.
                if piece.len < 3 || piece.area() <= least {
                    continue;
                }
                if self.patches.is_some() {
                    self.collect(id, &piece, [u, v, w], [column, row], normal)?;
                }
                let (w_low, w_high) = piece.range(2);
                let top = self.size[w] as f64 - EDGE;
                let (w_low, w_high) = ((w_low - push).clamp(EDGE, top), (w_high - push).clamp(EDGE, top));
                let first_layer = (w_low as usize).min(self.size[w] - 1);
                let last_layer = ((w_high.ceil() as usize).saturating_sub(1)).clamp(first_layer, self.size[w] - 1);
                for layer in first_layer..=last_layer {
                    let mut cell = [0; 3];
                    (cell[u], cell[v], cell[w]) = (column, row, layer);
                    let index = self.index(cell);
                    if !self.surface.get(cell) {
                        let next = |layer: usize| {
                            let mut c = cell;
                            c[w] = layer;
                            c
                        };
                        let before = layer > 0 && self.thin.get(next(layer - 1));
                        let after = layer + 1 < self.size[w] && self.thin.get(next(layer + 1));
                        if before || after {
                            continue;
                        }
                        self.surface.set(cell);
                    }
                    self.touch(cell, index, id, triangle)?;
                }
            }
        }
        Ok(())
    }
}

/// The cells whose centres lie between two coordinates, both ends counted.
fn centres_between(low: f64, high: f64, size: usize) -> Option<(usize, usize)> {
    let first = (low - 0.5).ceil().max(0.0);
    let last = (high - 0.5).floor().min(size as f64 - 1.0);
    (first <= last).then_some((first as usize, last as usize))
}

/// The cells a stretch between two coordinates reaches into. Ending
/// exactly on a cell's border does not reach into the next cell.
fn cells_between(low: f64, high: f64, size: usize) -> (usize, usize) {
    let first = (low.max(0.0) as usize).min(size - 1);
    let last = (high.ceil().max(1.0) as usize - 1).clamp(first, size - 1);
    (first, last)
}

/// Where the line v = y crosses a triangle seen along an axis: the least
/// and greatest u. Each edge's crossing is worked out from its lower end,
/// so two triangles sharing an edge get the very same number and no centre
/// line can slip between them.
fn span(triangle: &[V3; 3], u: usize, v: usize, y: f64) -> Option<(f64, f64)> {
    let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..3 {
        let (mut p, mut q) = (triangle[i], triangle[(i + 1) % 3]);
        if (p[v], p[u]) > (q[v], q[u]) {
            (p, q) = (q, p);
        }
        if p[v] > y || q[v] < y {
            continue;
        }
        if p[v] == q[v] {
            left = left.min(p[u]);
            right = right.max(q[u]);
            continue;
        }
        let x = p[u] + (y - p[v]) * (q[u] - p[u]) / (q[v] - p[v]);
        left = left.min(x);
        right = right.max(x);
    }
    (left <= right).then_some((left, right))
}

/// A triangle or what is left of it after cutting it to a row or a
/// column: up to nine points, each as (u, v, depth).
#[derive(Clone, Copy)]
struct Outline {
    points: [[f64; 3]; 9],
    len: usize,
}

impl Outline {
    fn of(triangle: [[f64; 3]; 3]) -> Outline {
        let mut points = [[0.0; 3]; 9];
        points[..3].copy_from_slice(&triangle);
        Outline { points, len: 3 }
    }

    fn push(&mut self, p: [f64; 3]) {
        if self.len < self.points.len() {
            self.points[self.len] = p;
            self.len += 1;
        }
    }

    /// The part on one side of the line where `axis` is `bound`: above it
    /// if `keep_above`, else below (Sutherland and Hodgman's clipping).
    fn cut(&self, axis: usize, bound: f64, keep_above: bool) -> Outline {
        let mut out = Outline { points: [[0.0; 3]; 9], len: 0 };
        let side = |p: &[f64; 3]| if keep_above { p[axis] - bound } else { bound - p[axis] };
        for i in 0..self.len {
            let (p, q) = (self.points[i], self.points[(i + 1) % self.len]);
            let (dp, dq) = (side(&p), side(&q));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp > 0.0 && dq < 0.0) || (dp < 0.0 && dq > 0.0) {
                let t = dp / (dp - dq);
                let mut cut = [0, 1, 2].map(|k| p[k] + t * (q[k] - p[k]));
                cut[axis] = bound;
                out.push(cut);
            }
        }
        out
    }

    fn range(&self, axis: usize) -> (f64, f64) {
        self.points[..self.len].iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| (low.min(p[axis]), high.max(p[axis])))
    }

    /// The area as seen along the depth.
    fn area(&self) -> f64 {
        let p = &self.points;
        let twice: f64 = (1..self.len.saturating_sub(1)).map(|i| (p[i][0] - p[0][0]) * (p[i + 1][1] - p[0][1]) - (p[i][1] - p[0][1]) * (p[i + 1][0] - p[0][0])).sum();
        twice.abs() / 2.0
    }
}

fn too_many_blocks() -> ImportError {
    too_large(format!("The result would have more than {MAX_BLOCKS} blocks. Choose a smaller size."))
}

fn voxelize_triangles(model: &Model, axes: &Axes, longest: u32, solid: bool, thickness: u8, smooth: bool) -> Result<Voxels> {
    let fit = Fit::new(&model.triangles, *axes, longest)?;
    let mut shell = Shell { size: fit.size, thin: Bits::new(fit.size), surface: Bits::default(), hits: CellMap::default(), patches: smooth.then(CellMap::default), work: 0 };
    // A mirrored model has every triangle's front and back swapped.
    let outward = if axes.mirrors() { -1.0 } else { 1.0 };
    let placed = |triangle: &Triangle| {
        let corners = fit.place(triangle)?;
        let normal = cross(sub(corners[1], corners[0]), sub(corners[2], corners[0]));
        // A triangle squashed to a line or a point has no surface.
        (dot(normal, normal) > 0.0).then_some((corners, normal))
    };
    for (id, triangle) in model.triangles.iter().enumerate() {
        if let Some((corners, normal)) = placed(triangle) {
            shell.cross_lines(id as u32, &corners, normal, outward)?;
        }
    }
    shell.surface = shell.thin.clone();
    for (id, triangle) in model.triangles.iter().enumerate() {
        if let Some((corners, normal)) = placed(triangle) {
            shell.pass_through(id as u32, &corners, normal, outward)?;
        }
    }
    let Shell { surface, hits, patches, .. } = shell;
    if hits.is_empty() {
        return Err(ImportError::Empty("The file has no usable triangles: every one of them is squashed flat.".into()));
    }
    let colour = |index: u64| {
        let from = hits.get(&index).map(|hit| hit.triangle).or_else(|| patches.as_ref()?.get(&index).map(|patch| patch.triangle))?;
        let triangle = model.triangles.get(from as usize)?;
        let size = fit.size;
        let i = index as usize;
        let centre = [(i % size[0]) as f64 + 0.5, (i / size[0] % size[1]) as f64 + 0.5, (i / (size[0] * size[1])) as f64 + 0.5];
        let weights = nearest_on_triangle(centre, fit.place(triangle)?);
        colour_at(model, &triangle.paint, weights)
    };
    fill(fit.size, &surface, solid, thickness, colour, patches.as_ref())
}

/// A triangle's colour at a point given as the weights of its corners.
fn colour_at(model: &Model, paint: &Paint, weights: [f64; 3]) -> Option<[u8; 3]> {
    match paint {
        Paint::None => None,
        Paint::Flat(colour) => Some(*colour),
        Paint::Corners(corners) => Some([0, 1, 2].map(|k| (0..3).map(|i| weights[i] * f64::from(corners[i][k])).sum::<f64>().round().clamp(0.0, 255.0) as u8)),
        Paint::Textured { texture, tint, uv } => {
            let at = [0, 1].map(|k| (0..3).map(|i| weights[i] * f64::from(uv[i][k])).sum::<f64>() as f32);
            Some(match model.textures.get(*texture as usize) {
                Some(texture) => tinted(texture.sample(at), *tint),
                None => *tint,
            })
        }
    }
}

/// Lays ready-made blocks into the grid: turned as asked, never resized.
fn place_blocks(blocks: &Blocks, axes: &Axes, longest: u32, solid: bool, shell: u8) -> Result<Voxels> {
    if blocks.cells.is_empty() {
        return Err(ImportError::Empty("The file has no blocks.".into()));
    }
    let (mut low, mut high) = ([i64::MAX; 3], [i64::MIN; 3]);
    for (at, _) in &blocks.cells {
        let p = axes.cell(*at);
        for i in 0..3 {
            low[i] = low[i].min(p[i]);
            high[i] = high[i].max(p[i]);
        }
    }
    let measures = [0, 1, 2].map(|i| high[i] - low[i] + 1);
    if measures.iter().any(|&m| m > i64::from(longest)) {
        let most = *LONGEST_RANGE.end();
        let [a, b, c] = measures;
        return Err(too_large(if measures.iter().any(|&m| m > i64::from(most)) {
            format!("The model is {a} x {b} x {c} blocks, and a ship can be at most {most} blocks long. Models made of blocks are not resized.")
        } else {
            format!("The model is {a} x {b} x {c} blocks, longer than the {longest} blocks asked for. Models made of blocks are not resized: choose a size of at least {}.", a.max(b).max(c))
        }));
    }
    let size = measures.map(|m| m as usize);
    let mut surface = Bits::new(size);
    let mut colours: CellMap<[u8; 3]> = CellMap::default();
    for (at, colour) in &blocks.cells {
        let p = axes.cell(*at);
        let cell = [0, 1, 2].map(|i| (p[i] - low[i]) as usize);
        surface.set(cell);
        colours.insert((cell[0] + size[0] * (cell[1] + size[1] * cell[2])) as u64, *colour);
        if colours.len() > MAX_BLOCKS {
            return Err(too_many_blocks());
        }
    }
    fill(size, &surface, solid, shell, |index| colours.get(&index).copied(), None)
}

/// Finds what the surface encloses, if asked, and lists the filled cells.
/// With `patches` (smoothing), cells where the surface slants get the part
/// of them the model fills, and cells the surface only clips get that part
/// as blocks of their own.
fn fill(size: [usize; 3], surface: &Bits, solid: bool, shell: u8, colour: impl Fn(u64) -> Option<[u8; 3]>, patches: Option<&CellMap<Patch>>) -> Result<Voxels> {
    let thick = !solid && shell > 1;
    // Smoothing needs to know what is inside even for a hollow result.
    let outside = if solid || thick || patches.is_some() { Some(flood(size, surface)) } else { None };
    // A thick shell: the inside cells within `shell - 1` steps of the surface.
    let lining = match (&outside, thick) {
        (Some(outside), true) => Some(lining(size, surface, outside, shell - 1)?),
        _ => None,
    };
    let (mut surface_count, mut inside_count) = (0usize, 0usize);
    for_pages(surface, |page, extent| {
        let cells: usize = extent.iter().product();
        let on = surface.count_page(page);
        surface_count += on;
        if let Some(outside) = &outside {
            inside_count += cells - on - outside.count_page(page);
        }
    });
    if !solid {
        inside_count = 0;
        if let Some(lining) = &lining {
            for_pages(lining, |page, _| inside_count += lining.count_page(page));
        }
    }
    let too_many = |count: usize| {
        too_large(format!(
            "The result would have {count} blocks, more than the {MAX_BLOCKS} that can be handled. Choose a smaller size{}.",
            if inside_count > 0 { " or leave the model hollow" } else { "" }
        ))
    };
    if surface_count + inside_count > MAX_BLOCKS {
        return Err(too_many(surface_count + inside_count));
    }
    let index_of = |cell: [usize; 3]| (cell[0] + size[0] * (cell[1] + size[1] * cell[2])) as u64;
    let cell_of = |index: u64| {
        let i = index as usize;
        [i % size[0], i / size[0] % size[1], i / (size[0] * size[1])]
    };
    let is_solid = |cell: [usize; 3]| surface.get(cell) || outside.as_ref().is_some_and(|outside| !outside.get(cell));
    // What the plane through each cell says, where it says anything.
    let mut partial: CellMap<u64> = CellMap::default();
    if let Some(patches) = patches {
        for (&index, patch) in patches {
            let cell = cell_of(index);
            // Whole cells of the inside stay whole.
            if !surface.get(cell) && is_solid(cell) {
                continue;
            }
            let near = |d: [i32; 3]| {
                let at = [0, 1, 2].map(|i| cell[i] as i64 + i64::from(d[i]));
                // Past the edge of the grid is outside.
                (0..3).all(|i| at[i] >= 0 && at[i] < size[i] as i64) && is_solid([at[0] as usize, at[1] as usize, at[2] as usize])
            };
            if let Some(mask) = patch.samples(near) {
                partial.insert(index, mask);
            }
        }
    }
    let whole = u64::MAX;
    // Which sides of a cell face the open air: a neighbour that is not part
    // of the model and can be reached from outside it (or is past the edge
    // of the grid). The inside of a hollow ship is not open air.
    let air_of = |cell: [usize; 3]| -> u8 {
        let (Some(outside), true) = (&outside, patches.is_some()) else { return 0 };
        const SIDES: [[i64; 3]; 6] = [[0, 0, 1], [0, 0, -1], [0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0]];
        let mut bits = 0u8;
        for (side, d) in SIDES.iter().enumerate() {
            let at = [0, 1, 2].map(|i| cell[i] as i64 + d[i]);
            let inside_grid = (0..3).all(|i| at[i] >= 0 && at[i] < size[i] as i64);
            let open = !inside_grid || {
                let c = [at[0] as usize, at[1] as usize, at[2] as usize];
                !surface.get(c) && outside.get(c)
            };
            bits |= (open as u8) << side;
        }
        bits
    };
    let mut out = Vec::with_capacity(surface_count + inside_count);
    for_pages(surface, |page, extent| {
        let all_outside = outside.as_ref().is_some_and(|o| o.table[o.page_at(page)] == FULL);
        // A page with no surface and either nothing inside or no filling asked for.
        if surface.is_blank(page) && (!solid || all_outside) && lining.as_ref().is_none_or(|l| l.is_blank(page)) {
            return;
        }
        let origin = page.map(|p| p * PAGE_SIDE);
        for z in 0..extent[2] {
            for y in 0..extent[1] {
                for x in 0..extent[0] {
                    let cell = [origin[0] + x, origin[1] + y, origin[2] + z];
                    let pos = cell.map(|v| v as u16);
                    let index = index_of(cell);
                    if surface.get(cell) {
                        let mut mask = partial.get(&index).copied();
                        // A surface cell the plane leaves empty is only a speck: left
                        // out, where there is filling behind it, and otherwise kept,
                        // since a hollow ship has nothing behind its skin.
                        if mask == Some(0) {
                            if solid {
                                continue;
                            }
                            mask = None;
                        }
                        out.push(Cell { pos, inside: false, colour: colour(index), partial: mask.filter(|&m| m != whole), air: air_of(cell) });
                    } else if (solid && outside.as_ref().is_some_and(|outside| !outside.get(cell))) || lining.as_ref().is_some_and(|l| l.get(cell)) {
                        out.push(Cell { pos, inside: true, colour: None, partial: None, air: 0 });
                    }
                }
            }
        }
    });
    if out.len() > MAX_BLOCKS {
        return Err(too_many(out.len()));
    }
    out.sort_unstable_by_key(|c| (c.pos[2], c.pos[1], c.pos[0]));
    Ok(Voxels { size: size.map(|s| s as u32), cells: out, notes: Vec::new() })
}

/// The inside cells within `depth` steps of the surface (a step to any of
/// the 26 cells round a cell), a layer at a time.
fn lining(size: [usize; 3], surface: &Bits, outside: &Bits, depth: u8) -> Result<Bits> {
    let mut lined = Bits::new(size);
    let inside = |cell: [usize; 3]| !surface.get(cell) && !outside.get(cell);
    let mut front: Vec<u64> = Vec::new();
    for_pages(surface, |page, extent| {
        if surface.is_blank(page) {
            return;
        }
        let origin = page.map(|p| p * PAGE_SIDE);
        for c in 0..extent[0] * extent[1] * extent[2] {
            let cell = [origin[0] + c % extent[0], origin[1] + c / extent[0] % extent[1], origin[2] + c / (extent[0] * extent[1])];
            if surface.get(cell) {
                front.push(pack(cell));
            }
        }
    });
    let mut count = 0usize;
    for _ in 0..depth {
        let mut next = Vec::new();
        for &n in &front {
            let cell = unpack(n);
            for d in 0..27 {
                let step = [d % 3, d / 3 % 3, d / 9].map(|v| v as i64 - 1);
                let at = [0, 1, 2].map(|i| cell[i] as i64 + step[i]);
                if (0..3).any(|i| at[i] < 0 || at[i] >= size[i] as i64) {
                    continue;
                }
                let at = at.map(|v| v as usize);
                if inside(at) && !lined.get(at) {
                    lined.set(at);
                    next.push(pack(at));
                    count += 1;
                    if count > MAX_BLOCKS {
                        return Err(too_many_blocks());
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        front = next;
    }
    Ok(lined)
}

/// Calls `each` with every page of the grid and how many of its cells lie
/// inside the grid.
fn for_pages(grid: &Bits, mut each: impl FnMut([usize; 3], [usize; 3])) {
    for z in 0..grid.pages[2] {
        for y in 0..grid.pages[1] {
            for x in 0..grid.pages[0] {
                each([x, y, z], grid.extent([x, y, z]));
            }
        }
    }
}

/// The walk from outside the grid into the empty space.
struct Flood<'a> {
    surface: &'a Bits,
    outside: Bits,
    /// Cells reached whose neighbours are not looked at yet, as x, y and z
    /// packed into one number.
    cells: VecDeque<u64>,
    /// Pages with no surface in them that were reached: the whole page is
    /// outside, and only the cells next to its faces need a look.
    pages: Vec<[usize; 3]>,
}

fn pack(cell: [usize; 3]) -> u64 {
    cell[0] as u64 | (cell[1] as u64) << 21 | (cell[2] as u64) << 42
}

fn unpack(n: u64) -> [usize; 3] {
    [(n & 0x1f_ffff) as usize, (n >> 21 & 0x1f_ffff) as usize, (n >> 42) as usize]
}

impl Flood<'_> {
    /// Notes that a cell is reachable from outside, unless it is surface or
    /// already noted. A page of empty space goes in as a whole.
    fn reach(&mut self, cell: [usize; 3]) {
        let page = cell.map(|v| v >> PAGE_SHIFT);
        let at = self.surface.page_at(page);
        if self.surface.table[at] == BLANK {
            if self.outside.table[at] != FULL {
                self.outside.table[at] = FULL;
                self.pages.push(page);
            }
        } else if !self.surface.get(cell) && !self.outside.get(cell) {
            self.outside.set(cell);
            self.cells.push_back(pack(cell));
        }
    }

    fn step(&mut self, cell: [usize; 3]) {
        let size = self.surface.size;
        for axis in 0..3 {
            if cell[axis] > 0 {
                let mut next = cell;
                next[axis] -= 1;
                self.reach(next);
            }
            if cell[axis] + 1 < size[axis] {
                let mut next = cell;
                next[axis] += 1;
                self.reach(next);
            }
        }
    }

    /// Looks over the border of a page that is all outside.
    fn open(&mut self, page: [usize; 3]) {
        let size = self.surface.size;
        let extent = self.surface.extent(page);
        let origin = page.map(|p| p * PAGE_SIDE);
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for (edge, towards) in [(origin[axis], -1i64), (origin[axis] + extent[axis] - 1, 1)] {
                let outer = edge as i64 + towards;
                if outer < 0 || outer >= size[axis] as i64 {
                    continue;
                }
                for a in 0..extent[u] {
                    for b in 0..extent[v] {
                        let mut cell = [0; 3];
                        (cell[axis], cell[u], cell[v]) = (outer as usize, origin[u] + a, origin[v] + b);
                        self.reach(cell);
                    }
                }
            }
        }
    }
}

/// The empty cells that can be reached from outside the grid by steps
/// through the six sides of a cell. Space is crossed a page at a time where
/// a page has no surface in it, and cell by cell (breadth first, so the
/// waiting line is only as long as the flood's front) only near the surface.
fn flood(size: [usize; 3], surface: &Bits) -> Bits {
    let mut walk = Flood { surface, outside: Bits::new(size), cells: VecDeque::new(), pages: Vec::new() };
    let last = surface.pages.map(|n| n - 1);
    for z in 0..surface.pages[2] {
        for y in 0..surface.pages[1] {
            for x in 0..surface.pages[0] {
                let page = [x, y, z];
                if ![0, 1, 2].iter().any(|&i| page[i] == 0 || page[i] == last[i]) {
                    continue;
                }
                let origin = page.map(|p| p * PAGE_SIDE);
                if surface.is_blank(page) {
                    walk.reach(origin);
                    continue;
                }
                let extent = surface.extent(page);
                for c in 0..extent[0] * extent[1] * extent[2] {
                    let cell = [origin[0] + c % extent[0], origin[1] + c / extent[0] % extent[1], origin[2] + c / (extent[0] * extent[1])];
                    if (0..3).any(|i| cell[i] == 0 || cell[i] == size[i] - 1) {
                        walk.reach(cell);
                    }
                }
            }
        }
    }
    loop {
        if let Some(page) = walk.pages.pop() {
            walk.open(page);
        } else if let Some(n) = walk.cells.pop_front() {
            walk.step(unpack(n));
        } else {
            break;
        }
    }
    walk.outside
}
