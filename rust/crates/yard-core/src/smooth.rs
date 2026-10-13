//! Choosing a block shape for a cell that is only partly filled, and
//! bevelling the outer edges of steps.
//!
//! The importer says which of a cell's 64 sample points (4 by 4 by 4, at
//! the middles of the sub-cells) the model fills. Every shape the hull
//! blocks come in, at every distinct orientation, fills some of those
//! points as well; the shape whose points differ from the model's in the
//! fewest places wins. Wedges, corners, tetras and heptas in all their turns
//! are the choices, as is no block at all (a cell with only a sliver of the
//! model in it) and a whole block. Slabs are not: a slope made of stairs of
//! thin panels is what smoothing is there to get rid of.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

use crate::catalog::Shape;
use crate::shape::{ShapeSet, SIDE_DIRS};
use crate::{Block, Entity, Pos};

/// Sample points on a side of the cell: the model's own surface is measured
/// at 4 by 4 by 4 points, bit `x + 4 y + 16 z`.
pub const SAMPLES: usize = 4;

/// A shape a cell can be given: the block's shape and slab thickness, its
/// orientation, and the sample points it fills.
#[derive(Clone, Copy, Debug)]
pub struct Form {
    pub shape: Shape,
    pub slab: u8,
    pub orientation: u8,
    pub samples: u64,
    /// The sides of the cell (bit `1 << side`, in the game's order: front, back,
    /// top, bottom, right, left) the block does not touch at all. A block whose
    /// open side lies against a solid block leaves a pit there.
    pub open: u8,
}

/// How far past a face a sample may be and still count as inside the block.
const ON_THE_FACE: f32 = 1e-4;

/// All the shapes, each turning that looks different once, in the order
/// preferred when two fit equally well: whole, wedges, corners, heptas,
/// tetras.
pub fn forms() -> &'static [Form] {
    static FORMS: OnceLock<Vec<Form>> = OnceLock::new();
    FORMS.get_or_init(|| {
        let set = ShapeSet::new();
        let mut out: Vec<Form> = Vec::new();
        let mut add = |shape: Shape, slab: u8, count: u8| {
            for orientation in 0..count {
                let geom = set.geom(shape, slab, orientation);
                let mut samples = 0u64;
                for z in 0..SAMPLES {
                    for y in 0..SAMPLES {
                        for x in 0..SAMPLES {
                            let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32);
                            // Inside a convex block: behind every one of its faces.
                            let inside = geom.faces.iter().all(|f| {
                                let c = f.corners[0];
                                (0..3).map(|k| f.normal[k] * (at[k] - c[k])).sum::<f32>() <= ON_THE_FACE
                            });
                            if inside {
                                samples |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                            }
                        }
                    }
                }
                // Turnings that fill the same points are the same shape to us.
                let open = (0..6).filter(|&side| !geom.faces.iter().any(|f| f.side == Some(side))).fold(0u8, |m, side| m | 1 << side);
                if samples != 0 && !out.iter().any(|f| f.samples == samples) {
                    out.push(Form { shape, slab, orientation, samples, open });
                }
            }
        };
        add(Shape::Cube, 0, 1);
        for shape in [Shape::Wedge, Shape::Corner, Shape::Hepta, Shape::Tetra] {
            add(shape, 0, ShapeSet::choices(shape, false));
        }
        out
    })
}

/// How much worse it is for a shape to fill a point the model does not
/// than to miss one it does (3 to 2): a block sticking out past the model
/// is more noticeable than one a little short of it.
const OVERSHOOT: u32 = 3;
/// What missing a point the model fills costs, against `OVERSHOOT`.
const UNDERSHOOT: u32 = 2;

/// A shape is used only if its error is at most this share (2 in 5) of the
/// whole block's. Wedges and their kin are 45 degree slopes: where the
/// surface slants gently no one of them fits, and the stairs stay.
const CLEARLY_BETTER: (u32, u32) = (2, 5);

/// A shape must also match the surface on its own terms: differ from it
/// in no more than this many of the 64 sample points (about a sixth).
const MOST_MISSES: u32 = 10;

/// The shape that fills the points most like `wanted`, if one is clearly
/// nearer than the whole block the plain result has there. Otherwise
/// nothing, and the caller keeps the whole block. Slabs are never chosen: a
/// slope made of stairs of thin panels is what smoothing is there to get
/// rid of.
///
/// `open(side)` says whether the cell next to this one across a side of it
/// (in the game's numbering) is open, a cell with no block in it. A shape is
/// used only where each side it leaves empty faces an open cell: that is
/// the edge of the surface being trimmed, not a pit dug against a wall.
pub fn best_form(wanted: u64, open: impl Fn(usize) -> bool) -> Option<Form> {
    let cost = |filled: u64| OVERSHOOT * (filled & !wanted).count_ones() + UNDERSHOOT * (wanted & !filled).count_ones();
    let plain = cost(u64::MAX);
    let fits = |f: &&Form| (0..6).all(|side| f.open & (1 << side) == 0 || open(side));
    let (wrong, form) = forms().iter().filter(|f| f.shape != Shape::Cube).filter(fits).map(|f| (cost(f.samples), *f)).min_by_key(|(wrong, _)| *wrong)?;
    let misses = (form.samples ^ wanted).count_ones();
    // The misses must also be small next to what is being drawn: a tetra is no
    // stand-in for a speck.
    let size = form.samples.count_ones().max(wanted.count_ones());
    (misses <= MOST_MISSES && misses * 3 <= size && wrong * CLEARLY_BETTER.1 <= plain * CLEARLY_BETTER.0).then_some(form)
}

/// The sample points of a cell where `n . (p - middle) <= offset`: on the
/// inner side of a plane with outward normal `n`, `offset` out from the
/// middle of the cell (in the same units as `n`, the cell being 1 wide).
pub fn below_plane_at(n: [i32; 3], offset: f32) -> u64 {
    let mut bits = 0u64;
    for z in 0..SAMPLES {
        for y in 0..SAMPLES {
            for x in 0..SAMPLES {
                let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32 - 0.5);
                if (0..3).map(|k| n[k] as f32 * at[k]).sum::<f32>() <= offset + 1e-6 {
                    bits |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                }
            }
        }
    }
    bits
}

/// The sample points of a cell on the inner side of a plane through its
/// middle whose outward normal is `n`.
pub fn below_plane(n: [i32; 3]) -> u64 {
    below_plane_at(n, 0.0)
}

/// The most steps the bevelling looks down for.
pub const MOST_STEP: u8 = 4;

/// The part of a whole block to keep to bevel its outer edge where it is the
/// top of a step, or nothing where it is not an edge of one.
///
/// `air` has a bit (`1 << side`, the game's order: front, back, top, bottom,
/// right, left) for each side of the cell that faces open air, and `solid(d)`
/// says whether the cell at offset `d` from it has a block in it. An edge is
/// where two adjacent sides face air. It is the edge of a step when going out
/// across one side and down along the other, within `step` blocks, meets a
/// block: the next tread. Edges of taller drops, of plates (air on opposite
/// sides) and of the flat sides of a box that is taller than `step` are left
/// square. Each such edge is cut by its own plane through the middle of the
/// cell (a wedge); where two edges meet at a corner of the step they are both
/// cut, and what is left of the block is what is inside every cut: the
/// game's corner block, or where three edges meet, a tetra.
pub fn bevel(air: u8, solid: impl Fn([i32; 3]) -> bool, step: u8) -> Option<u64> {
    let open = |side: usize| air >> side & 1 == 1;
    if step == 0 {
        return None;
    }
    let step = step.min(MOST_STEP);
    // Across `a` then down `b`, within `step` blocks, to a block. This is
    // also what keeps a plate standing free (a wing) square: its edge has
    // nothing below it to step down to. A plate that is the skin of a hollow
    // hull (air above and the hull's open inside below) is beveled like any
    // step.
    let drops = |a: usize, b: usize| {
        (1..=i32::from(step)).any(|k| solid([0, 1, 2].map(|i| SIDE_DIRS[a][i] - k * SIDE_DIRS[b][i])))
    };
    let mut cuts: Vec<[i32; 3]> = Vec::new();
    for (a, da) in SIDE_DIRS.iter().enumerate() {
        for (b, db) in SIDE_DIRS.iter().enumerate().skip(a + 1) {
            if open(a) && open(b) && a / 2 != b / 2 && (drops(a, b) || drops(b, a)) {
                cuts.push([0, 1, 2].map(|i| da[i] + db[i]));
            }
        }
    }
    // A staircase one block thick, its blocks touching only at their edges,
    // steps down on both faces: cutting both edges would leave a sliver. The
    // edge kept is the one facing the open side, the way with more room in
    // front of it before the next block (the hull's inside is the other way).
    let room = |n: [i32; 3]| (1..=SKY).find(|&k| solid(n.map(|v| v * k))).unwrap_or(SKY + 1);
    let opposite = |n: &[i32; 3]| cuts.contains(&n.map(|v| -v));
    let mut mask = u64::MAX;
    let mut kept: Vec<[i32; 3]> = Vec::new();
    for n in &cuts {
        if opposite(n) && room(*n) <= room(n.map(|v| -v)) {
            continue;
        }
        mask &= below_plane(*n);
        kept.push(*n);
    }
    // Three edges round one corner of the block: three planes through the
    // middle match no block, and nearest to them in sample points is the
    // corner piece, whose square base would stick out past the row of wedges
    // below. The tetra is what is flush with all three rows.
    if let [a, b, c] = kept[..] {
        let vertex = [0, 1, 2].map(|i| (a[i] + b[i] + c[i]) / 2);
        if vertex.iter().all(|v| v.abs() == 1) && [a, b, c].iter().all(|n| (0..3).all(|i| n[i] == 0 || n[i] == vertex[i])) {
            return Some(below_plane_at(vertex, -0.5));
        }
    }
    (!kept.is_empty()).then_some(mask)
}

/// How far the bevelling looks out from an edge to tell the outside of a thin
/// staircase from its inside.
const SKY: i32 = 64;

/// The wedge that fills a notch on the inner face of a staircase one block
/// thick (its blocks touching only at their edges): an empty cell with
/// blocks on two sides at right angles and none in the cell between those
/// two, on the face with less room in front of it (the outer face has its
/// edges beveled instead; see `bevel`). `solid(d)` says whether the cell at
/// offset `d` has a block. Returns the side whose block's material to use
/// and the part of the cell to fill.
pub fn notch(solid: impl Fn([i32; 3]) -> bool) -> Option<(usize, u64)> {
    let mut found = None;
    for (a, da) in SIDE_DIRS.iter().enumerate() {
        for (b, db) in SIDE_DIRS.iter().enumerate().skip(a + 1) {
            let n = [0, 1, 2].map(|i| da[i] + db[i]);
            if a / 2 == b / 2 || !solid(*da) || !solid(*db) || solid(n) {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some((a, n));
        }
    }
    let (a, n) = found?;
    let room = |d: [i32; 3]| (1..=SKY).find(|&k| solid(d.map(|v| v * k))).unwrap_or(SKY + 1);
    // Beyond the staircase (past the cell between the two blocks) is the outer face.
    let beyond = (2..=SKY).find(|&k| solid(n.map(|v| v * k))).unwrap_or(SKY + 1) - 1;
    (room(n.map(|v| -v)) < beyond).then(|| (a, below_plane(n.map(|v| -v))))
}

/// The sample points a block of any shape fills at an orientation, as the
/// game draws it (slabs too).
pub fn samples_of(shape: Shape, slab: u8, orientation: u8) -> u64 {
    static SET: OnceLock<ShapeSet> = OnceLock::new();
    let geom = SET.get_or_init(ShapeSet::new).geom(shape, slab, orientation);
    let mut samples = 0u64;
    for z in 0..SAMPLES {
        for y in 0..SAMPLES {
            for x in 0..SAMPLES {
                let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32);
                let inside = geom.faces.iter().all(|f| {
                    let c = f.corners[0];
                    (0..3).map(|k| f.normal[k] * (at[k] - c[k])).sum::<f32>() <= ON_THE_FACE
                });
                if inside {
                    samples |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                }
            }
        }
    }
    samples
}

/// Whether a block has the whole top edge on a side (front, back, right or
/// left, the game's numbering) cut away: a wedge, or a corner piece, sloping
/// down that way.
pub fn cuts_top_edge(samples: u64, side: usize) -> bool {
    cuts_edge(samples, 2, side)
}

/// Whether a block has the whole edge between two sides at right angles
/// (the game's numbering) cut away.
pub fn cuts_edge(samples: u64, up: usize, side: usize) -> bool {
    let last = SAMPLES - 1;
    let (du, ds) = (SIDE_DIRS[up], SIDE_DIRS[side]);
    let end = |d: [i32; 3], a: usize| if d[a] > 0 { last } else { 0 };
    let (au, as_) = (up / 2, side / 2);
    (0..SAMPLES).all(|along| {
        // The game's sides pair up as z, y, x; sample index axes are x, y, z.
        let mut at = [along; 3];
        for (pair, d) in [(au, du), (as_, ds)] {
            let axis = 2 - pair;
            at[axis] = end(d, axis);
        }
        samples >> (at[0] + SAMPLES * at[1] + SAMPLES * SAMPLES * at[2]) & 1 == 0
    })
}

/// The sample points of a block on one side of its cell (16 bits, in the
/// order of the two axes across that side).
fn side_layer(samples: u64, side: usize) -> u16 {
    let d = SIDE_DIRS[side];
    let axis = (0..3).find(|&i| d[i] != 0).unwrap_or(0);
    let end = if d[axis] > 0 { SAMPLES - 1 } else { 0 };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut out = 0u16;
    for i in 0..SAMPLES {
        for j in 0..SAMPLES {
            let mut at = [0; 3];
            at[axis] = end;
            at[u] = i;
            at[v] = j;
            if samples >> (at[0] + SAMPLES * at[1] + SAMPLES * SAMPLES * at[2]) & 1 == 1 {
                out |= 1 << (i + SAMPLES * j);
            }
        }
    }
    out
}

/// Whether a block slopes along the edge between sides `a` and `b` as seen
/// from the block beyond its side `toward`: its face there is the triangle
/// a wedge cutting that edge leaves. A wedge or a corner piece sloping that
/// way has exactly that face; a tetra's slant is oblique, so its sample
/// points nearest the face lie just inside the triangle, short of its
/// diagonal (the triangle less at most the diagonal's four points). A hepta,
/// a slab, a whole block or a shape cut some other way does not pass.
pub fn face_cut(samples: u64, toward: usize, a: usize, b: usize) -> bool {
    let n = [0, 1, 2].map(|i| SIDE_DIRS[a][i] + SIDE_DIRS[b][i]);
    let (face, wedge) = (side_layer(samples, toward), side_layer(below_plane(n), toward));
    face != 0 && face & !wedge == 0 && (wedge & !face).count_ones() <= SAMPLES as u32
}

/// A beveled block's mask with the edges cut that its neighbours along them
/// cut: where a row of wedges ends at a block whose step below recedes, the
/// block's own bevel leaves that edge square and it would stick out past the
/// row's slope (a corner piece where the row wants a tetra). `air` is as for
/// [`bevel`]; an edge is only cut where both its sides are open.
/// `neighbour(d)` gives the sample points the block at offset `d` fills once
/// smoothed, as for [`inside_corner`]; a row cuts an edge where the
/// neighbour's face toward the block is the triangle a wedge along that
/// edge leaves (see [`face_cut`]).
///
/// A block whose own cuts are on the face opposite one a row cuts (its
/// bottom edges beveled as the lip of an overhang, say, where the blocks
/// along it slope at the top) was shaped the wrong way up: its cuts are
/// mirrored to the row's face before the row's edge is added.
pub fn row_end(air: u8, mask: u64, neighbour: impl Fn([i32; 3]) -> Option<u64>) -> u64 {
    let open = |s: usize| air >> s & 1 == 1;
    let normal = |(a, b): (usize, usize)| [0, 1, 2].map(|i| SIDE_DIRS[a][i] + SIDE_DIRS[b][i]);
    let pairs: Vec<(usize, usize)> = (0..6).flat_map(|a| (a + 1..6).map(move |b| (a, b))).filter(|(a, b)| a / 2 != b / 2).collect();
    // The edges the block's own shape cuts.
    let mut cuts: Vec<(usize, usize)> = pairs.iter().copied().filter(|&e| mask & !below_plane(normal(e)) == 0).collect();
    // The open edges a row along them cuts that the block does not.
    let along = |(a, b): (usize, usize)| (0..6).filter(move |c| c / 2 != a / 2 && c / 2 != b / 2);
    let wanted: Vec<(usize, usize)> = pairs
        .iter()
        .copied()
        .filter(|&(a, b)| open(a) && open(b) && !cuts.contains(&(a, b)))
        .filter(|&(a, b)| along((a, b)).any(|c| neighbour(SIDE_DIRS[c]).is_some_and(|s| s != u64::MAX && face_cut(s, c ^ 1, a, b))))
        .collect();
    if wanted.is_empty() {
        return mask;
    }
    for &(a, b) in &wanted {
        for s in [a, b] {
            for cut in cuts.iter_mut() {
                if cut.0 == s ^ 1 {
                    cut.0 = s;
                }
                if cut.1 == s ^ 1 {
                    cut.1 = s;
                }
            }
        }
    }
    cuts.extend(wanted);
    for cut in cuts.iter_mut() {
        if cut.0 > cut.1 {
            *cut = (cut.1, cut.0);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = u64::MAX;
    for &e in &cuts {
        out &= below_plane(normal(e));
    }
    // All three edges round an open corner cut: three planes through the
    // middle match no block, and the tetra is what is flush with them all.
    for (a, b, c) in [(0, 2, 4), (0, 2, 5), (0, 3, 4), (0, 3, 5), (1, 2, 4), (1, 2, 5), (1, 3, 4), (1, 3, 5)] {
        if [a, b, c].iter().all(|&s| open(s)) && [(a, b), (a, c), (b, c)].iter().all(|e| cuts.contains(e)) {
            return below_plane_at([0, 1, 2].map(|i| SIDE_DIRS[a][i] + SIDE_DIRS[b][i] + SIDE_DIRS[c][i]), -0.5);
        }
    }
    out
}

/// What a pass of smoothing does, besides which blocks it may touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Smoothing {
    /// The tallest step whose edges are beveled, in blocks (0: none).
    pub step: u8,
    /// Rooms sealed inside the hull count as open air too, and the notches on
    /// the inner face of a thin staircase are filled.
    pub inside: bool,
    /// The edge of a slope that ends against a whole block is carried on
    /// through it (see [`continued`]).
    pub sharpen: bool,
}

/// Whether a block's sample points are those of a slope: a wedge, corner,
/// hepta or tetra at some turning. A slab or a whole block is not one.
pub fn sloped(samples: u64) -> bool {
    samples != u64::MAX && forms().iter().any(|f| f.samples == samples)
}

/// The part of a whole block to keep where a slope ends against it: each of
/// its edges with open air on both sides (`air`, as for [`bevel`]) that the
/// sloped block next to it along that edge cuts is cut the same way, so the
/// slope carries on through the block instead of ending at a square edge.
/// Three such edges round one corner make a tetra. `neighbour(d)` gives the
/// sample points of the block at offset `d`, as for [`inside_corner`]. None
/// where no slope ends here.
pub fn continued(air: u8, neighbour: impl Fn([i32; 3]) -> Option<u64>) -> Option<u64> {
    let flush = row_end(air, u64::MAX, |d| neighbour(d).filter(|s| sloped(*s)));
    (flush != u64::MAX).then_some(flush)
}

/// The part of a whole block to keep at an inside corner of a step, where
/// the block itself is flat on top but the beveled rows beside it meet:
/// the block with its corner between them taken off (a hepta). Decided by
/// what the neighbours became: `neighbour(d)` gives the sample points the
/// block at offset `d` fills once smoothed (`u64::MAX` for a whole block,
/// nothing for an empty cell). The corner goes where the neighbour on one
/// side slopes down toward the other side's neighbour and that one slopes
/// down toward it. A block with two such corners keeps both: one hepta cannot
/// take off two corners. `air` is as for [`bevel`]: the face the step looks
/// out of must be open and no side round it may be (the face opposite may,
/// under the skin of a hollow hull). That face is any of the six: the top of
/// a step, the underside of an overhang, a wall.
pub fn inside_corner(air: u8, neighbour: impl Fn([i32; 3]) -> Option<u64>) -> Option<u64> {
    let mut found = None;
    for (up, du) in SIDE_DIRS.iter().enumerate() {
        // Open this way, and on no side round it; the other way too for the skin of a hollow hull.
        if air & 1 << up == 0 || air & !(1 << up | 1 << (up ^ 1)) != 0 {
            continue;
        }
        let round: Vec<usize> = (0..6).filter(|s| s / 2 != up / 2).collect();
        for &a in &round {
            for &b in round.iter().filter(|&&b| b > a && b / 2 != a / 2) {
                let (da, db) = (SIDE_DIRS[a], SIDE_DIRS[b]);
                let (Some(along_a), Some(along_b)) = (neighbour(da), neighbour(db)) else { continue };
                if !(cuts_edge(along_a, up, b) && cuts_edge(along_b, up, a)) {
                    continue;
                }
                if found.is_some() {
                    return None;
                }
                // Take off the corner where this face and the two sides meet.
                found = Some(below_plane_at([0, 1, 2].map(|i| da[i] + db[i] + du[i]), 0.5));
            }
        }
    }
    found
}

/// The empty cells of an entity that open air reaches from beyond it: a
/// cavity inside a hull is not open air. Worked out once for a whole entity,
/// a segment (32 by 32 by 32) at a time where a segment has no blocks.
pub struct Outside {
    /// Segments with no blocks that the open air fills.
    full: HashSet<(i32, i32, i32)>,
    /// For segments with blocks: one bit per cell the open air reaches.
    cells: HashMap<(i32, i32, i32), Box<[u64; 512]>>,
    /// The segments looked at: the entity's, and one more all round.
    low: (i32, i32, i32),
    high: (i32, i32, i32),
}

const SEG: i32 = crate::entity::SEG;

fn segment_of(p: Pos) -> ((i32, i32, i32), usize) {
    let a = [p.x + crate::entity::CORE_OFFSET, p.y + crate::entity::CORE_OFFSET, p.z + crate::entity::CORE_OFFSET];
    let key = (a[0].div_euclid(SEG), a[1].div_euclid(SEG), a[2].div_euclid(SEG));
    let l = a.map(|v| v.rem_euclid(SEG) as usize);
    (key, l[0] + SEG as usize * (l[1] + SEG as usize * l[2]))
}

impl Outside {
    pub fn of(entity: &Entity) -> Outside {
        Self::of_with(entity, &HashSet::new())
    }

    /// The open air, with the empty cells `sealed` counted as blocks.
    pub fn of_with(entity: &Entity, sealed: &HashSet<Pos>) -> Outside {
        let mut present: HashSet<(i32, i32, i32)> = entity.segments().filter(|(_, s)| s.count > 0).map(|(k, _)| *k).collect();
        present.extend(sealed.iter().map(|p| segment_of(*p).0));
        let mut out = Outside { full: HashSet::new(), cells: HashMap::new(), low: (0, 0, 0), high: (0, 0, 0) };
        let Some(first) = present.iter().next().copied() else { return out };
        let (mut low, mut high) = (first, first);
        for k in &present {
            low = (low.0.min(k.0), low.1.min(k.1), low.2.min(k.2));
            high = (high.0.max(k.0), high.1.max(k.1), high.2.max(k.2));
        }
        (out.low, out.high) = ((low.0 - 1, low.1 - 1, low.2 - 1), (high.0 + 1, high.1 + 1, high.2 + 1));
        let (dl, dh) = (out.low, out.high);
        let inside = move |k: (i32, i32, i32)| (dl.0..=dh.0).contains(&k.0) && (dl.1..=dh.1).contains(&k.1) && (dl.2..=dh.2).contains(&k.2);
        let (mut segments, mut waiting): (Vec<(i32, i32, i32)>, VecDeque<Pos>) = (vec![out.low], VecDeque::new());
        out.full.insert(out.low);
        let origin = |k: (i32, i32, i32)| Pos::new(k.0 * SEG - crate::entity::CORE_OFFSET, k.1 * SEG - crate::entity::CORE_OFFSET, k.2 * SEG - crate::entity::CORE_OFFSET);
        let reach = |out: &mut Outside, segments: &mut Vec<(i32, i32, i32)>, waiting: &mut VecDeque<Pos>, p: Pos| {
            let (key, index) = segment_of(p);
            if !inside(key) {
                return;
            }
            if !present.contains(&key) {
                if out.full.insert(key) {
                    segments.push(key);
                }
                return;
            }
            let bits = out.cells.entry(key).or_insert_with(|| Box::new([0; 512]));
            if bits[index / 64] >> (index % 64) & 1 == 1 || !entity.get(p).is_air() || sealed.contains(&p) {
                return;
            }
            bits[index / 64] |= 1 << (index % 64);
            waiting.push_back(p);
        };
        loop {
            if let Some(key) = segments.pop() {
                // An empty segment open to the air: the cells just beyond each face.
                let o = origin(key);
                for d in SIDE_DIRS {
                    let axis = (0..3).find(|&i| d[i] != 0).unwrap_or(0);
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    for i in 0..SEG {
                        for j in 0..SEG {
                            let mut c = [o.x, o.y, o.z];
                            c[axis] += if d[axis] > 0 { SEG } else { -1 };
                            c[u] += i;
                            c[v] += j;
                            reach(&mut out, &mut segments, &mut waiting, Pos::new(c[0], c[1], c[2]));
                        }
                    }
                }
            } else if let Some(p) = waiting.pop_front() {
                for d in SIDE_DIRS {
                    reach(&mut out, &mut segments, &mut waiting, Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]));
                }
            } else {
                break;
            }
        }
        out
    }

    /// Whether open air reaches this cell.
    pub fn contains(&self, p: Pos) -> bool {
        let (key, index) = segment_of(p);
        let beyond = !(self.low.0..=self.high.0).contains(&key.0) || !(self.low.1..=self.high.1).contains(&key.1) || !(self.low.2..=self.high.2).contains(&key.2);
        beyond || self.full.contains(&key) || self.cells.get(&key).is_some_and(|bits| bits[index / 64] >> (index % 64) & 1 == 1)
    }
}

/// The sides of a block smoothing treats as open: those facing the open air
/// outside the hull (`open`) wherever there are any, so a block of a thin
/// hull is shaped for its outer face; with `inside`, every empty side
/// (`any`) of a block the outside does not reach, for rooms and the inner
/// face of the hull.
pub fn outer_first(open: u8, any: u8, inside: bool) -> u8 {
    if open != 0 || !inside {
        open
    } else {
        any
    }
}

/// The block to put in the empty cell `q` where it is a notch on the inner
/// face of a thin staircase (see `notch`), in the material of the block
/// beside it; None where it is not one.
pub fn notch_block(entity: &Entity, q: Pos, whole: impl Fn(Block) -> bool, reshape: impl Fn(Block, &Form) -> Option<Block>) -> Option<Block> {
    let at = |d: [i32; 3]| Pos::new(q.x + d[0], q.y + d[1], q.z + d[2]);
    let (side, mask) = notch(|d| !entity.get(at(d)).is_air())?;
    let beside = entity.get(at(SIDE_DIRS[side]));
    if !whole(beside) {
        return None;
    }
    // The slope faces the notch's own open side; a side it leaves bare may
    // rest on a deck, as the cell was empty anyway.
    let form = best_form(mask, |_| true)?;
    reshape(beside, &form)
}

/// One pass of smoothing over a whole entity: the edges of steps up to `step`
/// high beveled (wedges, and corner pieces where two meet), then the inside
/// corners where beveled rows meet (heptas). Only blocks `whole` accepts
/// are reshaped; `reshape` gives the block of a shape in the material of a
/// block (none where the material lacks it); `samples` gives the sample
/// points a block fills (for blocks already shaped). Returns the blocks to
/// write. Running it again picks up what the first pass made possible.
/// With `inside`, every empty cell counts as air, rooms sealed inside the
/// hull too, and the notches on the inner face of thin staircases are filled.
pub fn smooth_entity(
    entity: &Entity,
    step: u8,
    whole: impl Fn(Block) -> bool,
    reshape: impl Fn(Block, &Form) -> Option<Block>,
    samples: impl Fn(Block) -> Option<u64>,
    inside: bool,
) -> Vec<(Pos, Block)> {
    smooth_entity_opts(entity, &Smoothing { step, inside, sharpen: false }, whole, reshape, samples)
}

/// [`smooth_entity`] with every option: with `sharpen`, after the edges of
/// steps are beveled, a slope (one the pass made, or one placed by hand)
/// that ends against a whole block is carried on through it, and on along
/// the edge as far as the block after that is one too, with both sides of
/// the edge open to the air.
pub fn smooth_entity_opts(
    entity: &Entity,
    opts: &Smoothing,
    whole: impl Fn(Block) -> bool,
    reshape: impl Fn(Block, &Form) -> Option<Block>,
    samples: impl Fn(Block) -> Option<u64>,
) -> Vec<(Pos, Block)> {
    if opts.step == 0 {
        return Vec::new();
    }
    smooth_sealed(entity, &HashSet::new(), opts, whole, reshape, samples)
}

/// [`smooth_entity`], with the empty cells `sealed` counted as blocks in
/// working out the open air (and only there).
fn smooth_sealed(
    entity: &Entity,
    sealed: &HashSet<Pos>,
    opts: &Smoothing,
    whole: impl Fn(Block) -> bool,
    reshape: impl Fn(Block, &Form) -> Option<Block>,
    samples: impl Fn(Block) -> Option<u64>,
) -> Vec<(Pos, Block)> {
    let (step, inside) = (opts.step, opts.inside);
    let at = |p: Pos, d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
    // The inner face of a staircase one block thick: its notches filled. The
    // open air is what it is once they are, so a second pass sees the same.
    let mut filled: HashMap<Pos, Block> = HashMap::new();
    if inside {
        entity.for_each(|p, b| {
            if !whole(b) {
                return;
            }
            for d in SIDE_DIRS {
                let q = at(p, d);
                if !entity.get(q).is_air() || filled.contains_key(&q) {
                    continue;
                }
                if let Some(new) = notch_block(entity, q, &whole, &reshape) {
                    filled.insert(q, new);
                }
            }
        });
    }
    let mut closed = sealed.clone();
    closed.extend(filled.keys().copied());
    let outside = Outside::of_with(entity, &closed);
    let air_of = |p: Pos| -> u8 {
        let (mut open, mut any) = (0u8, 0u8);
        for (side, d) in SIDE_DIRS.iter().enumerate() {
            let q = at(p, *d);
            if entity.get(q).is_air() {
                any |= 1 << side;
                open |= u8::from(outside.contains(q)) << side;
            }
        }
        outer_first(open, any, inside)
    };
    let mut planned: HashMap<Pos, Block> = HashMap::new();
    entity.for_each(|p, b| {
        if !whole(b) {
            return;
        }
        let air = air_of(p);
        if air == 0 {
            return;
        }
        let open = |side: usize| entity.get(at(p, SIDE_DIRS[side])).is_air();
        let form = bevel(air, |d| !entity.get(at(p, d)).is_air(), step).and_then(|mask| best_form(mask, open));
        if let Some(new) = form.and_then(|f| reshape(b, &f)).filter(|n| *n != b) {
            planned.insert(p, new);
        }
    });
    // The ends of rows of wedges, cut flush with the row: blocks beveled
    // just now, and blocks an earlier pass beveled (a block whose shape is
    // just what the bevel makes), which a pass otherwise leaves alone. A
    // round at a time until nothing changes, since a block cut flush can be
    // what the next one along is to be flush with.
    let whole_form = forms()[0];
    loop {
        let mut ends = Vec::new();
        entity.for_each(|p, b| {
            let fresh = planned.contains_key(&p);
            if !fresh && (whole(b) || !reshape(b, &whole_form).is_some_and(&whole)) {
                return;
            }
            let air = air_of(p);
            let Some(mask) = bevel(air, |d| !entity.get(at(p, d)).is_air(), step) else { return };
            let open = |side: usize| entity.get(at(p, SIDE_DIRS[side])).is_air();
            if !fresh && best_form(mask, open).map(|f| f.samples) != samples(b) {
                return;
            }
            let neighbour = |d: [i32; 3]| samples(planned.get(&at(p, d)).copied().unwrap_or_else(|| entity.get(at(p, d))));
            let flush = row_end(air, mask, neighbour);
            if flush == mask {
                return;
            }
            let now = planned.get(&p).copied().unwrap_or(b);
            if let Some(new) = best_form(flush, open).and_then(|f| reshape(b, &f)).filter(|n| *n != now) {
                ends.push((p, new));
            }
        });
        if ends.is_empty() {
            break;
        }
        planned.extend(ends);
    }
    // Slopes carried on through the whole blocks they end against, and on
    // along the edge: a round at a time, each seeing what the last made.
    if opts.sharpen {
        loop {
            let mut more = Vec::new();
            entity.for_each(|p, b| {
                if !whole(b) || planned.contains_key(&p) {
                    return;
                }
                let air = air_of(p);
                if air == 0 {
                    return;
                }
                let neighbour = |d: [i32; 3]| samples(planned.get(&at(p, d)).copied().unwrap_or_else(|| entity.get(at(p, d))));
                let Some(mask) = continued(air, neighbour) else { return };
                let open = |side: usize| entity.get(at(p, SIDE_DIRS[side])).is_air();
                if let Some(new) = best_form(mask, open).and_then(|f| reshape(b, &f)).filter(|n| *n != b) {
                    more.push((p, new));
                }
            });
            if more.is_empty() {
                break;
            }
            planned.extend(more);
        }
    }
    let mut corners = Vec::new();
    entity.for_each(|p, b| {
        if !whole(b) || planned.contains_key(&p) {
            return;
        }
        let air = air_of(p);
        // Open on one face (and maybe the one opposite) and no other.
        if air.count_ones() == 0 || air.count_ones() > 2 || (air.count_ones() == 2 && air & (air >> 1) & 0b010101 == 0) {
            return;
        }
        let neighbour = |d: [i32; 3]| samples(planned.get(&at(p, d)).copied().unwrap_or_else(|| entity.get(at(p, d))));
        let open = |side: usize| entity.get(at(p, SIDE_DIRS[side])).is_air();
        let form = inside_corner(air, neighbour).and_then(|mask| best_form(mask, open));
        if let Some(new) = form.and_then(|f| reshape(b, &f)).filter(|n| *n != b) {
            corners.push((p, new));
        }
    });
    let mut out: Vec<(Pos, Block)> = planned.into_iter().collect();
    out.extend(corners);
    out.extend(filled);
    out.sort_by_key(|(p, _)| (p.z, p.y, p.x));
    out
}

/// Smoothing done over from the start, for a design smoothed before (an
/// ordinary pass leaves blocks that already have a shape alone): every
/// shaped block `unshape` gives a whole block for is taken back to it, and
/// one pass of [`smooth_entity`] is made. A wedge in a notch of a thin hull,
/// such as smoothing the inside puts there, stays as it is: there is no
/// telling it from one placed by hand, and it may be what seals the hull.
/// The pass is planned as if the notch were empty, the way the first pass
/// saw it, but with the open air worked out with the wedge in place (without
/// it the open air could reach the inside, and the inner face would be
/// shaped as if it were the outer). Returns the cells that change.
pub fn resmooth_entity(
    entity: &Entity,
    step: u8,
    whole: impl Fn(Block) -> bool,
    reshape: impl Fn(Block, &Form) -> Option<Block>,
    samples: impl Fn(Block) -> Option<u64>,
    inside: bool,
    unshape: impl Fn(Block) -> Option<Block>,
) -> Vec<(Pos, Block)> {
    resmooth_entity_opts(entity, &Smoothing { step, inside, sharpen: false }, whole, reshape, samples, unshape)
}

/// [`resmooth_entity`] with every option (see [`smooth_entity_opts`]). With
/// `sharpen`, only the slopes the pass itself makes are carried on: the
/// shaped blocks are all whole again when it runs.
pub fn resmooth_entity_opts(
    entity: &Entity,
    opts: &Smoothing,
    whole: impl Fn(Block) -> bool,
    reshape: impl Fn(Block, &Form) -> Option<Block>,
    samples: impl Fn(Block) -> Option<u64>,
    unshape: impl Fn(Block) -> Option<Block>,
) -> Vec<(Pos, Block)> {
    let mut flat = Entity::new();
    for (key, segment) in entity.segments() {
        let copy = crate::entity::Segment { blocks: segment.blocks.clone(), count: segment.count, version: segment.version, timestamp: segment.timestamp };
        flat.insert_segment(*key, copy);
    }
    let mut shaped: Vec<(Pos, Block, Block)> = Vec::new();
    entity.for_each(|p, b| {
        if let Some(cube) = unshape(b).filter(|c| *c != b) {
            shaped.push((p, b, cube));
        }
    });
    for (p, _, cube) in &shaped {
        flat.set(*p, *cube);
    }
    // A notch's wedge is the one smoothing would put in the cell were it
    // empty, judged with the other notches empty as they were when it was put
    // there: found a round at a time until no more are.
    let mut notches = Vec::new();
    loop {
        let found: Vec<_>;
        (found, shaped) = shaped.into_iter().partition(|(p, b, _)| notch_block(&flat, *p, &whole, &reshape) == Some(*b));
        if found.is_empty() {
            break;
        }
        for (p, _, _) in &found {
            flat.set(*p, Block::AIR);
        }
        notches.extend(found);
    }
    let sealed: HashSet<Pos> = notches.iter().map(|(p, _, _)| *p).collect();
    let mut done: HashMap<Pos, Block> = if opts.step == 0 { HashMap::new() } else { smooth_sealed(&flat, &sealed, opts, &whole, &reshape, &samples).into_iter().collect() };
    for (p, b, _) in notches {
        flat.set(p, b);
        done.remove(&p);
    }
    let mut touched: HashSet<Pos> = shaped.iter().map(|(p, _, _)| *p).collect();
    touched.extend(done.keys().copied());
    let mut out: Vec<(Pos, Block)> = touched
        .into_iter()
        .filter_map(|p| {
            let now = done.get(&p).copied().unwrap_or_else(|| flat.get(p));
            (now != entity.get(p)).then_some((p, now))
        })
        .collect();
    out.sort_by_key(|(p, _)| (p.z, p.y, p.x));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(shape: Shape) -> usize {
        forms().iter().filter(|f| f.shape == shape).count()
    }

    #[test]
    fn every_shape_is_there_in_every_look_it_has_and_no_slab_is() {
        assert_eq!(forms().iter().filter(|f| f.shape == Shape::Cube).count(), 1);
        assert!(forms().iter().all(|f| f.slab == 0), "slabs are not among the choices");
        // The game turns a wedge 12 ways, a corner 24, a tetra and a hepta 8.
        assert_eq!(count(Shape::Wedge), 12);
        assert_eq!(count(Shape::Corner), 24);
        assert_eq!(count(Shape::Tetra), 8);
        assert_eq!(count(Shape::Hepta), 8);
    }

    #[test]
    fn the_shapes_fill_the_share_of_the_cell_they_should() {
        let share = |shape: Shape| forms().iter().find(|f| f.shape == shape).map(|f| f.samples.count_ones());
        assert_eq!(share(Shape::Cube), Some(64));
        // A wedge is half a cube; the samples on the slope count for both
        // halves, so a little over half.
        let wedge = share(Shape::Wedge).unwrap();
        assert!((32..=48).contains(&wedge), "{wedge}");
    }

    #[test]
    fn a_shape_is_found_again_from_its_own_samples() {
        for form in forms().iter().filter(|f| f.shape != Shape::Cube) {
            let found = best_form(form.samples, |_| true).unwrap();
            assert_eq!(found.samples, form.samples, "{form:?}");
        }
    }

    #[test]
    fn a_sliver_or_a_nearly_whole_cell_keeps_the_whole_block() {
        assert!(best_form(0b101, |_| true).is_none());
        assert!(best_form(!0b11u64, |_| true).is_none());
        assert!(best_form(u64::MAX, |_| true).is_none());
    }

    #[test]
    fn a_gentle_slope_is_left_as_it_is_and_not_made_of_pyramids() {
        // A layer of samples a quarter high, tilted: no wedge fits it.
        let mut mask = 0u64;
        for z in 0..SAMPLES {
            for y in 0..SAMPLES {
                for x in 0..SAMPLES {
                    let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32);
                    if at[1] <= 0.25 + 0.15 * at[0] {
                        mask |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                    }
                }
            }
        }
        assert!(best_form(mask, |_| true).is_none(), "{:?}", best_form(mask, |_| true));
    }

    #[test]
    fn a_slope_is_a_wedge_whichever_way_it_faces() {
        // The part of the cell below the plane through its middle, tilted
        // each of four ways: always a wedge, never a slab or a cube.
        for (sx, sz) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let mut mask = 0u64;
            for z in 0..SAMPLES {
                for y in 0..SAMPLES {
                    for x in 0..SAMPLES {
                        let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32 - 0.5);
                        if at[1] <= sx * at[0] + sz * at[2] + 1e-6 {
                            mask |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                        }
                    }
                }
            }
            let form = best_form(mask, |_| true).unwrap();
            assert_eq!((form.shape, form.slab), (Shape::Wedge, 0), "tilt {sx} {sz}");
        }
    }

    #[test]
    fn a_wedge_is_not_dug_in_against_a_wall() {
        // The slope that makes a wedge: below the plane through the middle.
        let mut mask = 0u64;
        for z in 0..SAMPLES {
            for y in 0..SAMPLES {
                for x in 0..SAMPLES {
                    let at = [x, y, z].map(|i| (i as f32 + 0.5) / SAMPLES as f32 - 0.5);
                    if at[1] <= at[2] + 1e-6 {
                        mask |= 1 << (x + SAMPLES * y + SAMPLES * SAMPLES * z);
                    }
                }
            }
        }
        let form = best_form(mask, |_| true).unwrap();
        assert_eq!(form.shape, Shape::Wedge);
        // The sides it leaves empty are the ones it needs open.
        let needed: Vec<usize> = (0..6).filter(|&side| form.open & (1 << side) != 0).collect();
        assert!(!needed.is_empty());
        for &wall in &needed {
            assert!(best_form(mask, |side| side != wall).is_none_or(|f| f.open & (1 << wall) == 0), "a wall at side {wall} is ignored");
        }
    }

    const TOP: usize = 2;
    const BOTTOM: usize = 3;
    const RIGHT: usize = 4;
    const FRONT: usize = 0;

    /// An upper level (everything at or below the cell, to its left) and a
    /// lower tread `drop` blocks down, to its right.
    fn step_of(drop: i32) -> impl Fn([i32; 3]) -> bool {
        move |d| (d[0] <= 0 && d[1] <= 0) || (d[0] >= 1 && d[1] <= -drop)
    }

    #[test]
    fn the_top_edge_of_a_step_becomes_a_wedge() {
        let air = 1 << TOP | 1 << RIGHT;
        let mask = bevel(air, step_of(1), 1).unwrap();
        assert_eq!(mask, below_plane([1, 1, 0]));
        let form = best_form(mask, |_| true).unwrap();
        assert_eq!(form.shape, Shape::Wedge);
    }

    #[test]
    fn a_taller_drop_is_beveled_only_when_the_step_height_allows() {
        let air = 1 << TOP | 1 << RIGHT;
        assert_eq!(bevel(air, step_of(3), 1), None);
        assert_eq!(bevel(air, step_of(3), 2), None);
        assert!(bevel(air, step_of(3), 3).is_some());
        assert_eq!(bevel(air, step_of(1), 0), None);
    }

    #[test]
    fn a_free_plate_the_corner_of_a_box_and_a_flat_side_are_left_square() {
        // Air above and below, standing free: a plate whose edge has nothing to step down to.
        assert_eq!(bevel(1 << TOP | 1 << BOTTOM | 1 << RIGHT, |d| d[1] == 0 && d[0] <= 0, 4), None);
        // The same plate as the skin of a hollow hull, with the next tread below: beveled.
        assert_eq!(bevel(1 << TOP | 1 << BOTTOM | 1 << RIGHT, |d| (d[1] == 0 && d[0] <= 0) || (d[1] == -1 && d[0] >= 1), 1), Some(below_plane([1, 1, 0])));
        // The corner of a box: nothing lower beside it.
        assert_eq!(bevel(1 << TOP | 1 << RIGHT, |d| d[0] <= 0 && d[1] <= 0, 4), None);
        // One side open: flat.
        assert_eq!(bevel(1 << TOP, step_of(1), 4), None);
        // Two open sides that are opposite make no edge.
        assert_eq!(bevel(1 << FRONT | 1 << (FRONT + 1), step_of(1), 4), None);
    }

    #[test]
    fn where_two_edges_of_a_step_meet_the_block_is_the_corner_piece() {
        // The corner of an upper level that is a quadrant, with lower ground all around it.
        let air = 1 << TOP | 1 << RIGHT | 1 << FRONT;
        let quadrant = |d: [i32; 3]| d[1] <= -1 || (d[0] <= 0 && d[2] <= 0 && d[1] <= 0);
        let mask = bevel(air, quadrant, 1).unwrap();
        // Inside both cuts: a wedge's cut across the top and right, another across top and front.
        assert_eq!(mask, below_plane([1, 1, 0]) & below_plane([0, 1, 1]));
        let form = best_form(mask, |_| true).unwrap();
        assert_eq!(form.shape, Shape::Corner, "{form:?}");
        // And the same corner turned each of the other ways is a corner block too.
        for (a, b) in [(FRONT + 1, RIGHT), (FRONT, RIGHT + 1), (FRONT + 1, RIGHT + 1)] {
            let (da, db) = (SIDE_DIRS[a], SIDE_DIRS[b]);
            let ground = |d: [i32; 3]| d[1] <= -1 || (d[0] * db[0] <= 0 && d[2] * da[2] <= 0 && d[1] <= 0);
            let mask = bevel(1 << TOP | 1 << a | 1 << b, ground, 1).unwrap();
            assert_eq!(best_form(mask, |_| true).map(|f| f.shape), Some(Shape::Corner), "sides {a} {b}");
        }
    }

    #[test]
    fn sharpening_carries_a_slope_on_through_the_blocks_its_edge_ends_against() {
        // A box three high on a wide deck: too tall a drop for the step height, so
        // its edges stay square. One wedge put by hand at the end of its top-front
        // edge is a slope that ends against the next block.
        let mut e = Entity::new();
        let cube = Block::new(5, 0, false);
        for z in -8i32..=8 {
            for x in -8i32..=8 {
                e.set(Pos::new(x, -1, z), cube);
                if (0..=8).contains(&x) && (0..=2).contains(&z) {
                    for y in 0..=2 {
                        e.set(Pos::new(x, y, z), cube);
                    }
                }
            }
        }
        let wedge = best_form(below_plane([0, 1, 1]), |_| true).unwrap();
        e.set(Pos::new(0, 2, 2), reshape_test(cube, &wedge).unwrap());
        let plain = smooth_entity_opts(&e, &Smoothing { step: 1, inside: false, sharpen: false }, |b| b.ty() == 5, reshape_test, samples_test);
        assert!(plain.is_empty(), "{plain:?}");
        let sharp = smooth_entity_opts(&e, &Smoothing { step: 1, inside: false, sharpen: true }, |b| b.ty() == 5, reshape_test, samples_test);
        // The whole edge, to the box's far end, in one pass, and nothing else.
        let mut expected: Vec<(Pos, Block)> = (1..=8).map(|x| (Pos::new(x, 2, 2), reshape_test(cube, &wedge).unwrap())).collect();
        expected.sort_by_key(|(p, _)| (p.z, p.y, p.x));
        assert_eq!(sharp, expected);
        // Smoothing again from scratch takes the hand-placed wedge back to a whole
        // block first, so there is no slope to carry on.
        let unshape = |b: Block| (b.ty() != 5 && b.ty() != 0).then(|| cube);
        let again = resmooth_entity_opts(&e, &Smoothing { step: 1, inside: false, sharpen: true }, |b| b.ty() == 5, reshape_test, samples_test, unshape);
        assert_eq!(again, vec![(Pos::new(0, 2, 2), cube)]);
    }

    #[test]
    fn sharpening_closes_the_tip_where_three_slopes_meet() {
        // A nose whose three edges are staircases: the blocks beside the tip are
        // steps and become wedges, but the tip itself has nothing to step down to
        // and stays a whole block, sticking out past the three slopes.
        let mut e = Entity::new();
        let cube = Block::new(5, 0, false);
        for z in -6i32..=6 {
            for y in -6i32..=6 {
                for x in -6i32..=6 {
                    if x + y <= 0 && y + z <= 0 && x + z <= 0 {
                        e.set(Pos::new(x, y, z), cube);
                    }
                }
            }
        }
        let tip = Pos::new(0, 0, 0);
        let plain = smooth_entity_opts(&e, &Smoothing { step: 1, inside: false, sharpen: false }, |b| b.ty() == 5, reshape_test, samples_test);
        assert!(plain.iter().all(|(p, _)| *p != tip));
        assert!(plain.iter().any(|(p, b)| *p == Pos::new(-1, 0, 0) && b.ty() == 6));
        let sharp = smooth_entity_opts(&e, &Smoothing { step: 1, inside: false, sharpen: true }, |b| b.ty() == 5, reshape_test, samples_test);
        let at_tip = sharp.iter().find(|(p, _)| *p == tip).map(|(_, b)| *b).expect("the tip is shaped");
        assert_eq!(at_tip.ty(), 9, "a tetra");
        for (a, b) in [(TOP, RIGHT), (TOP, FRONT), (FRONT, RIGHT)] {
            assert!(cuts_edge(samples_test(at_tip).unwrap(), a, b), "edge {a} {b}");
        }
    }

    #[test]
    fn where_three_edges_of_a_step_meet_the_block_is_a_tetra() {
        // The tip of a nose that steps in every way: the deck below reaches a block
        // further front and right, the wall to the left a block further front.
        let air = 1 << TOP | 1 << RIGHT | 1 << FRONT;
        let nose = |d: [i32; 3]| (d[0] <= 0 && d[1] <= 0 && d[2] <= 0) || (d[1] <= -1 && d[0] <= 1 && d[2] <= 1) || (d[0] <= -1 && d[1] <= 0 && d[2] <= 1);
        let mask = bevel(air, nose, 1).unwrap();
        assert_eq!(mask, below_plane_at([1, 1, 1], -0.5));
        let tetra = best_form(mask, |_| true).unwrap();
        assert_eq!(tetra.shape, Shape::Tetra, "{tetra:?}");
        for (a, b) in [(TOP, RIGHT), (TOP, FRONT), (FRONT, RIGHT)] {
            assert!(cuts_edge(tetra.samples, a, b), "edge {a} {b}");
        }
        // The whole nose: the tip a tetra, and what is round it shaped to meet it.
        let mut e = Entity::new();
        let cube = Block::new(5, 0, false);
        for z in -6i32..=6 {
            for y in -6i32..=6 {
                for x in -6i32..=6 {
                    if nose([x, y, z]) {
                        e.set(Pos::new(x, y, z), cube);
                    }
                }
            }
        }
        let got = smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false);
        let at = |p: Pos| got.iter().find(|(q, _)| *q == p).map(|(_, b)| b.ty());
        assert_eq!(at(Pos::new(0, 0, 0)), Some(9), "the tip is a tetra: {got:?}");
        // Flush with it: a hepta to its left with its corner toward the tip taken
        // off, the wedge of the row behind it, and in front of the hepta the corner
        // piece that cuts the same upright edge.
        assert_eq!(at(Pos::new(-1, 0, 0)), Some(8), "beside the tip");
        assert_eq!(at(Pos::new(0, 0, -1)), Some(6), "the row behind");
        assert_eq!(at(Pos::new(-1, 0, 1)), Some(7), "the wall's corner");
    }

    #[test]
    fn a_staircase_one_block_thick_is_beveled_on_its_outer_face_only() {
        // Blocks touching only at their edges, stepping down toward the front,
        // over a deck three blocks down (the hull's inside is that way).
        let stairs = |d: [i32; 3]| d[1] == -d[2] || d[1] <= -3;
        let air = 1 << TOP | 1 << BOTTOM | 1 << FRONT | 1 << (FRONT + 1);
        let mask = bevel(air, stairs, 1).unwrap();
        assert_eq!(mask, below_plane([0, 1, 1]));
        assert_eq!(best_form(mask, |_| true).map(|f| f.shape), Some(Shape::Wedge));
        // With as much room on both faces there is no telling; left square.
        assert_eq!(bevel(air, |d| d[1] == -d[2], 1), None);
    }

    #[test]
    fn the_notches_on_the_inner_face_of_a_thin_staircase_are_filled_and_not_on_the_outer() {
        // Under the staircase (blocks above and in front), over a deck: filled,
        // the wedge solid toward the two blocks.
        let under = |d: [i32; 3]| d[1] == 1 - d[2] || d[1] <= -2;
        let (side, mask) = notch(under).unwrap();
        assert!(side == FRONT || side == TOP, "{side}");
        assert_eq!(mask, below_plane([0, -1, -1]));
        assert_eq!(best_form(mask, |s| !under(SIDE_DIRS[s])).map(|f| f.shape), Some(Shape::Wedge));
        // On top of it, open to the sky: left for the bevel.
        assert_eq!(notch(|d| d[1] == -1 - d[2] || d[1] <= -4), None);
        // The corner of a room in a thick hull is not a notch.
        assert_eq!(notch(|d| d[1] <= -1 || d[2] >= 1), None);
    }

    /// The neighbours of a block at an inside corner: the one in front slopes
    /// down toward +X, the one at +X slopes down toward the front.
    fn beveled_rows(d: [i32; 3]) -> Option<u64> {
        match d {
            [0, 0, 1] => Some(below_plane([1, 1, 0])),
            [1, 0, 0] => Some(below_plane([0, 1, 1])),
            [_, 1, _] => None,
            _ => Some(u64::MAX),
        }
    }

    #[test]
    fn an_inside_corner_of_a_step_loses_the_corner_that_stands_up_between_the_slopes() {
        let mask = inside_corner(1 << TOP, beveled_rows).unwrap();
        let form = best_form(mask, |_| true).unwrap();
        assert_eq!(form.shape, Shape::Hepta, "{form:?}");
        // The corner taken off is the one toward +X and the front, at the top.
        assert_eq!(mask, below_plane_at([1, 1, 1], 0.5));
        // Not where a neighbour is whole, or slopes the other way, or the block has other sides open.
        assert_eq!(inside_corner(1 << TOP, |d| if d == [1, 0, 0] { Some(u64::MAX) } else { beveled_rows(d) }), None);
        assert_eq!(inside_corner(1 << TOP, |d| if d == [1, 0, 0] { Some(below_plane([0, 1, -1])) } else { beveled_rows(d) }), None);
        assert_eq!(inside_corner(1 << TOP | 1 << RIGHT, beveled_rows), None);
        // The same step upside down, the underside of an overhang: the corner at the bottom.
        let flipped = |d: [i32; 3]| match d {
            [0, 0, 1] => Some(below_plane([1, -1, 0])),
            [1, 0, 0] => Some(below_plane([0, -1, 1])),
            [_, -1, _] => None,
            _ => Some(u64::MAX),
        };
        let mask = inside_corner(1 << BOTTOM, flipped).unwrap();
        assert_eq!(mask, below_plane_at([1, -1, 1], 0.5));
        assert_eq!(best_form(mask, |_| true).map(|f| f.shape), Some(Shape::Hepta));
        // The skin of a hollow hull, open below to its inside: the same.
        assert_eq!(inside_corner(1 << TOP | 1 << BOTTOM, beveled_rows), Some(below_plane_at([1, 1, 1], 0.5)));
    }

    #[test]
    fn a_block_between_two_inside_corners_stays_whole() {
        // Rows sloping toward it from +X/front and from +X/back at once.
        let both = |d: [i32; 3]| match d {
            [0, 0, 1] => Some(below_plane([1, 1, 0])),
            [0, 0, -1] => Some(below_plane([1, 1, 0])),
            [1, 0, 0] => Some(below_plane([0, 1, 1]) & below_plane([0, 1, -1])),
            [_, 1, _] => None,
            _ => Some(u64::MAX),
        };
        assert_eq!(inside_corner(1 << TOP, both), None);
    }

    #[test]
    fn the_top_edges_a_shape_cuts_are_told_apart() {
        let wedge = below_plane([1, 1, 0]);
        assert!(cuts_top_edge(wedge, RIGHT) && !cuts_top_edge(wedge, FRONT) && !cuts_top_edge(wedge, RIGHT + 1));
        let corner = below_plane([1, 1, 0]) & below_plane([0, 1, 1]);
        assert!(cuts_top_edge(corner, RIGHT) && cuts_top_edge(corner, FRONT));
        let hepta = below_plane_at([1, 1, 1], 0.5);
        assert!(!cuts_top_edge(hepta, RIGHT) && !cuts_top_edge(hepta, FRONT), "a hepta takes off a corner, not an edge");
        assert!(!cuts_top_edge(u64::MAX, RIGHT));
        // The samples of a placed block match the forms they come from.
        for form in forms() {
            assert_eq!(samples_of(form.shape, form.slab, form.orientation), form.samples, "{form:?}");
        }
    }

    /// A test material: 5 is the whole block; a shaped block is 6, turned as the form says,
    /// with its shape kept in the form table's order (the test only needs to tell them apart).
    fn reshape_test(_: Block, form: &Form) -> Option<Block> {
        if form.shape == Shape::Cube {
            return Some(Block::new(5, 0, false));
        }
        let shape = [Shape::Wedge, Shape::Corner, Shape::Hepta, Shape::Tetra].iter().position(|s| *s == form.shape)? as u16;
        Some(Block::new(6 + shape, form.orientation, false))
    }

    fn samples_test(b: Block) -> Option<u64> {
        let shape = match b.ty() {
            0 => return None,
            6 => Shape::Wedge,
            7 => Shape::Corner,
            8 => Shape::Hepta,
            9 => Shape::Tetra,
            _ => return Some(u64::MAX),
        };
        Some(samples_of(shape, 0, b.orientation()))
    }

    #[test]
    fn an_overhang_is_smoothed_as_its_mirror_image_on_top_is() {
        // A base with an L-shaped level on it (so there is an inside corner),
        // and the same thing upside down.
        let build = |up: i32| {
            let mut e = Entity::new();
            for x in -4i32..=4 {
                for z in -4i32..=4 {
                    for y in 0..=3 {
                        e.set(Pos::new(x, -y * up, z), Block::new(5, 0, false));
                    }
                    if x.abs() <= 2 && z.abs() <= 2 && !(x > 0 && z > 0) {
                        e.set(Pos::new(x, up, z), Block::new(5, 0, false));
                    }
                }
            }
            e
        };
        let count = |up: i32| {
            let done = smooth_entity(&build(up), 1, |b| b.ty() == 5, reshape_test, samples_test, true);
            let mut by = [0; 4];
            for (_, b) in &done {
                by[usize::from(b.ty() - 6)] += 1;
            }
            by
        };
        let (top, under) = (count(1), count(-1));
        assert!(top[2] > 0, "the inside corner on top is a hepta: {top:?}");
        assert_eq!(under, top, "wedges, corners, heptas, tetras");
    }

    #[test]
    fn open_air_is_what_reaches_the_entity_from_beyond_it() {
        // A hollow box 9 wide with a cavity inside.
        let mut e = Entity::new();
        for x in -4i32..=4 {
            for y in -4i32..=4 {
                for z in -4i32..=4 {
                    if x.abs() == 4 || y.abs() == 4 || z.abs() == 4 {
                        e.set(Pos::new(x, y, z), Block::new(5, 0, false));
                    }
                }
            }
        }
        let outside = Outside::of(&e);
        assert!(outside.contains(Pos::new(5, 0, 0)) && outside.contains(Pos::new(100, -40, 3)));
        assert!(!outside.contains(Pos::new(0, 0, 0)) && !outside.contains(Pos::new(3, 3, 3)), "the cavity is not open air");
        assert!(!outside.contains(Pos::new(4, 0, 0)), "a block is not air");
        // A hole in the wall lets the air in.
        e.set(Pos::new(4, 0, 0), Block::AIR);
        let outside = Outside::of(&e);
        assert!(outside.contains(Pos::new(0, 0, 0)));
    }

    #[test]
    fn a_pass_over_a_stepped_hill_bevels_its_edges_and_their_corners_and_not_its_cavity() {
        // Two levels: a 9 by 9 base and a 5 by 5 top, and a sealed cavity in the base.
        let mut e = Entity::new();
        for x in -4i32..=4 {
            for z in -4i32..=4 {
                for y in -3..=0 {
                    e.set(Pos::new(x, y, z), Block::new(5, 0, false));
                }
                if x.abs() <= 2 && z.abs() <= 2 {
                    e.set(Pos::new(x, 1, z), Block::new(5, 0, false));
                }
            }
        }
        e.set(Pos::new(0, -2, 0), Block::AIR);
        let done = smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false);
        let at = |p: Pos| done.iter().find(|(q, _)| *q == p).map(|(_, b)| b.ty());
        // The top level's edges: wedges along the sides, corner pieces at the corners.
        assert_eq!(at(Pos::new(0, 1, 2)), Some(6));
        assert_eq!(at(Pos::new(2, 1, 2)), Some(7));
        // The base's outer edge drops more than one block (to nothing): left square.
        assert_eq!(at(Pos::new(4, 0, 0)), None);
        // Nothing round the cavity.
        assert!(done.iter().all(|(p, _)| p.y >= 0), "{done:?}");
        // Step height 0 does nothing.
        assert!(smooth_entity(&e, 0, |b| b.ty() == 5, reshape_test, samples_test, false).is_empty());
    }
    #[test]
    fn smoothing_again_from_scratch_redoes_shapes_a_pass_left_alone() {
        // A hull one block thick stepping down toward the front over a deck,
        // walled in at the ends and the back: the room under it is sealed.
        let mut e = Entity::new();
        let cube = Block::new(5, 0, false);
        for x in -4i32..=4 {
            for y in -5..=4 {
                e.set(Pos::new(x, y, -5), cube);
            }
            for z in -4i32..=4 {
                e.set(Pos::new(x, -5, z), cube);
                if x.abs() == 4 {
                    for y in -4..=-z {
                        e.set(Pos::new(x, y, z), cube);
                    }
                } else {
                    e.set(Pos::new(x, -z, z), cube);
                }
            }
        }
        let apply = |e: &mut Entity, cells: Vec<(Pos, Block)>| {
            for (p, b) in cells {
                e.set(p, b);
            }
        };
        let unshape = |b: Block| (6..=9).contains(&b.ty()).then_some(cube);
        let fresh = smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, true);
        let notches: Vec<Pos> = fresh.iter().filter(|(p, _)| e.get(*p).is_air()).map(|(p, _)| *p).collect();
        assert!(!notches.is_empty(), "the notches under the hull are filled");
        apply(&mut e, fresh);
        // Done over, the same shapes come out: nothing changes.
        let again = resmooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, true, unshape);
        assert!(again.is_empty(), "{:?}", again.iter().map(|(p, b)| (p, e.get(*p), b)).collect::<Vec<_>>());
        // A shape from some earlier pass that an ordinary pass would leave be.
        let stale = Pos::new(0, 0, 0);
        let good = e.get(stale);
        assert_eq!(good.ty(), 6, "the stair is a wedge");
        e.set(stale, Block::new(6, (good.orientation() + 1) % 12, false));
        assert!(smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, true).iter().all(|(p, _)| *p != stale));
        assert_eq!(resmooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, true, unshape), vec![(stale, good)]);
        // The notch wedges stay, the inside smoothed or not.
        let plain = resmooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false, unshape);
        assert!(plain.iter().all(|(p, _)| !notches.contains(p)), "{plain:?}");
    }
    #[test]
    fn the_end_of_a_row_of_wedges_is_cut_flush_with_the_row() {
        // Open below, to the left and to the back; the block behind it along the row is a
        // wedge cutting the bottom-left edge, and its own bevel cut the other two.
        let air = 1 << (FRONT + 1) | 1 << BOTTOM | 1 << (RIGHT + 1);
        let corner = below_plane([0, -1, -1]) & below_plane([-1, 0, -1]);
        assert_eq!(best_form(corner, |_| true).map(|f| f.shape), Some(Shape::Corner));
        let row = |d: [i32; 3]| match d {
            [0, 0, 1] => Some(below_plane([-1, -1, 0])),
            [0, 0, -1] | [0, -1, 0] | [-1, 0, 0] => None,
            _ => Some(u64::MAX),
        };
        let flush = row_end(air, corner, row);
        let tetra = best_form(flush, |_| true).unwrap();
        assert_eq!(tetra.shape, Shape::Tetra);
        // Its slanted face looks out of the corner: none of the three open sides keeps a square edge.
        for (a, b) in [(1, 3), (1, 5), (3, 5)] {
            assert!(cuts_edge(tetra.samples, a, b), "edge {a} {b}");
        }
        // A platform's corner: nothing beside its upright edge cuts it; the corner piece stays.
        assert_eq!(row_end(air, corner, |d| if d[2] == 0 && d != [0, -1, 0] && d != [-1, 0, 0] { Some(u64::MAX) } else { None }), corner);
    }
    #[test]
    fn a_block_cut_for_the_face_opposite_its_row_is_turned_right_way_up() {
        // The front-left corner of a deck, open above and below (the layer under it
        // is cut back at the corner) and in front and to the left. Its own bevel took
        // it for the lip of an overhang and cut its bottom edges; the block beside it
        // along the front edge slopes at the top.
        let air = 1 << TOP | 1 << BOTTOM | 1 << FRONT | 1 << (RIGHT + 1);
        // Its bottom-front and bottom-left edges and the upright one between them
        // are cut: the tetra at its bottom-front-left corner.
        let lip = below_plane_at([-1, -1, 1], -0.5);
        let top_right = below_plane_at([1, 1, 1], -0.5);
        assert!(face_cut(top_right, RIGHT + 1, TOP, FRONT));
        assert!(!face_cut(top_right, RIGHT + 1, BOTTOM, FRONT), "a tetra's face is one wedge's triangle, not every edge it lacks");
        let row = |d: [i32; 3]| match d {
            [1, 0, 0] => Some(top_right),
            [0, 0, -1] => Some(u64::MAX),
            _ => None,
        };
        let flush = row_end(air, lip, row);
        assert_eq!(flush, below_plane_at([-1, 1, 1], -0.5), "the tetra at the top-front-left corner");
        let form = best_form(flush, |_| true).unwrap();
        assert_eq!(form.shape, Shape::Tetra);
        for (a, b) in [(TOP, FRONT), (TOP, RIGHT + 1), (FRONT, RIGHT + 1)] {
            assert!(cuts_edge(form.samples, a, b), "edge {a} {b}");
        }
        // The same block with the row sloping at the bottom beside it keeps its lip.
        let bottom_right = below_plane_at([1, -1, 1], -0.5);
        assert_eq!(row_end(air, lip, |d| if d == [1, 0, 0] { Some(bottom_right) } else { None }), lip);
    }

    #[test]
    fn a_pass_cuts_a_row_end_an_earlier_pass_left_square() {
        // A raised row along z on top of a deck, its edge toward +X stepping down onto a
        // lower tread for z 1 to 4 but not at z 0, where the tread falls back a block.
        let mut e = Entity::new();
        let cube = Block::new(5, 0, false);
        for z in -3i32..=6 {
            for x in -4i32..=3 {
                if x <= 0 || (x == 1 && z >= 1) {
                    e.set(Pos::new(x, -1, z), cube);
                }
                e.set(Pos::new(x, -2, z), cube);
            }
        }
        for z in 0..=4 {
            for x in -3..=0 {
                e.set(Pos::new(x, 0, z), cube);
            }
        }
        let end = Pos::new(0, 0, 0);
        let fresh = smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false);
        let shaped = |cells: &[(Pos, Block)]| cells.iter().find(|(p, _)| *p == end).map(|(_, b)| samples_test(*b).unwrap());
        let flush = shaped(&fresh).unwrap();
        assert!(cuts_edge(flush, TOP, RIGHT), "the row's edge is cut at its end too");
        // As an earlier pass left it: only the edge toward the back cut.
        let air = 1 << TOP | 1 << RIGHT | 1 << (FRONT + 1);
        let old = best_form(bevel(air, |d| !e.get(Pos::new(d[0], d[1], d[2])).is_air(), 1).unwrap(), |_| true).unwrap();
        assert!(!cuts_edge(old.samples, TOP, RIGHT));
        e.set(end, reshape_test(cube, &old).unwrap());
        let again = smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false);
        assert_eq!(shaped(&again), Some(flush), "an ordinary pass re-cuts it");
        // And once cut, it stays.
        for (p, b) in again {
            e.set(p, b);
        }
        assert!(smooth_entity(&e, 1, |b| b.ty() == 5, reshape_test, samples_test, false).iter().all(|(p, _)| *p != end));
    }
}
