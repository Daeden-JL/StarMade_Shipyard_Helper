//! Whether the blocks of one entity run into another's.
//!
//! A docked entity that turns or slides is stopped by whatever is in its
//! way. Both sides are grids of blocks, one grid turned and shifted against
//! the other, so the test is block against block: each block of the moving
//! entity against the few cells of the still one it could reach. Blocks are
//! taken in their real shapes. That matters: the game lets a wedge of a
//! turret share a cell with a wedge of the hull where the two fit together,
//! and builders use it all the time.

use crate::catalog::{Catalog, Shape};
use crate::dock::{Placement, Turn};
use crate::entity::{Bounds, Entity, Pos, Segment};
use crate::shape::{Geom, ShapeSet, SIDE_DIRS};
use crate::Block;
use std::collections::HashMap;

/// How much is shaved off every side of a full cube for the test. Blocks
/// that only touch (a turret's foot on the deck, a slider in its slot) must
/// not count as running into each other. The game shaves off a thousandth
/// (its boxes are 0.49899 to a side from the middle); a little more here
/// keeps the arithmetic of turned placements from finding a hit in a touch.
pub const SLACK: f32 = 0.004;
/// How much is shaved off where one of the two blocks is a wedge or another
/// cut shape. The game is as lenient there: it tests those with a margin of
/// three hundredths.
pub const SHAPE_SLACK: f32 = 0.03;

/// The solid shape of every block.
pub trait Shapes {
    /// None for a block other entities pass straight through: an open door,
    /// a pickup rail.
    fn geom(&self, block: Block) -> Option<&Geom>;
}

/// Block shapes straight from the game's block definitions.
pub struct CatalogShapes<'a> {
    catalog: &'a Catalog,
    set: ShapeSet,
}

impl<'a> CatalogShapes<'a> {
    pub fn new(catalog: &'a Catalog) -> Self {
        Self { catalog, set: ShapeSet::new() }
    }
}

impl Shapes for CatalogShapes<'_> {
    fn geom(&self, block: Block) -> Option<&Geom> {
        let def = self.catalog.get(block.ty());
        if passes_through(block, def.is_some_and(|d| d.door), def.is_some_and(|d| d.ghost)) {
            return None;
        }
        Some(match def {
            Some(def) => self.set.geom(def.shape, def.slab, block.orientation()),
            None => self.set.geom(Shape::Cube, 0, 0),
        })
    }
}

/// Whether other entities pass through a block, as the game decides it
/// (`RailCollisionChecker`): a door or force field that stands open (its
/// active bit is off), a block the game's block list marks as not physical
/// (`ghost`: pickup rails and points, trigger areas, plants), and a cargo
/// space turned to its fourth orientation.
pub fn passes_through(block: Block, door: bool, ghost: bool) -> bool {
    const TRIGGER_AREA: u16 = 411;
    const CARGO_SPACE: u16 = 689;
    if door {
        return !block.active_bit();
    }
    ghost || block.ty() == TRIGGER_AREA || (block.ty() == CARGO_SPACE && block.orientation() == 4)
}

/// Where an entity has blocks, coarsely: a bit for every cube of four
/// blocks a side that has a block in it or within two blocks of it. Most of
/// what moves is nowhere near anything, and one look here says so.
#[derive(Clone, Debug, PartialEq)]
pub struct Coarse {
    pub bounds: Bounds,
    /// The coarse cube the first bit stands for, and how many there are along each axis.
    origin: [i32; 3],
    size: [i32; 3],
    bits: Vec<u64>,
}

impl Coarse {
    const CUBE: i32 = 4;
    const MARGIN: i32 = 2;

    /// None for an entity with no blocks.
    pub fn of(entity: &Entity) -> Option<Coarse> {
        let bounds = entity.bounds()?;
        let (min, max) = (bounds.min.to_array(), bounds.max.to_array());
        let origin = min.map(|c| (c - Self::MARGIN).div_euclid(Self::CUBE));
        let size = [0, 1, 2].map(|i| (max[i] + Self::MARGIN).div_euclid(Self::CUBE) - origin[i] + 1);
        let mut coarse = Coarse { bounds, origin, size, bits: vec![0; (size[0] as usize * size[1] as usize * size[2] as usize).div_ceil(64)] };
        entity.for_each(|p, _| {
            let p = p.to_array();
            for corner in 0..8 {
                let c = [0, 1, 2].map(|i| p[i] + if corner >> i & 1 == 0 { -Self::MARGIN } else { Self::MARGIN });
                if let Some(bit) = coarse.bit(c) {
                    coarse.bits[bit / 64] |= 1 << (bit % 64);
                }
            }
        });
        Some(coarse)
    }

    fn bit(&self, cell: [i32; 3]) -> Option<usize> {
        let c = [0, 1, 2].map(|i| cell[i].div_euclid(Self::CUBE) - self.origin[i]);
        (0..3).all(|i| c[i] >= 0 && c[i] < self.size[i]).then(|| (c[0] + self.size[0] * (c[1] + self.size[1] * c[2])) as usize)
    }

    /// Whether there may be a block in or right around a cell.
    fn near(&self, cell: [i32; 3]) -> bool {
        self.bit(cell).is_some_and(|bit| self.bits[bit / 64] >> (bit % 64) & 1 == 1)
    }
}

/// The part of an entity that can run into something: its blocks with at
/// least one open side. A block buried among others is reached only through
/// those.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub shell: Vec<Pos>,
    pub bounds: Bounds,
}

impl Body {
    /// None for an entity with no blocks.
    pub fn of(entity: &Entity) -> Option<Body> {
        let bounds = entity.bounds()?;
        let mut shell = Vec::new();
        let mut near = Near::new(entity);
        entity.for_each(|p, _| {
            if SIDE_DIRS.iter().any(|d| near.get(p.offset(d[0], d[1], d[2])).is_air()) {
                shell.push(p);
            }
        });
        Some(Body { shell, bounds })
    }
}

/// Reads blocks that lie close together: the segment last read from is kept
/// at hand, which saves looking it up again for nearly every block.
struct Near<'a> {
    entity: &'a Entity,
    key: (i32, i32, i32),
    segment: Option<&'a Segment>,
}

impl<'a> Near<'a> {
    fn new(entity: &'a Entity) -> Self {
        Self { entity, key: (i32::MAX, 0, 0), segment: None }
    }

    fn get(&mut self, p: Pos) -> Block {
        let (key, index) = Entity::locate(p);
        if key != self.key {
            self.key = key;
            self.segment = self.entity.segment(key);
        }
        self.segment.map_or(Block::AIR, |s| s.blocks[index])
    }
}

/// A block's shape for the test, about the centre of its cell.
enum Solid {
    /// The full cube: the commonest by far, and the quickest to test.
    Cube,
    /// The corners of any other shape, shrunk toward its middle.
    Corners(Vec<[f32; 3]>),
}

/// Most corners a shape is tested with. A block's 3D model has far more;
/// the box around it stands in for it.
const MOST_CORNERS: usize = 32;

fn solid_of(geom: &Geom) -> Solid {
    if geom.full_sides == 0b11_1111 {
        return Solid::Cube;
    }
    let mut corners: Vec<[f32; 3]> = Vec::new();
    for c in geom.faces.iter().flat_map(|f| &f.corners) {
        let c = c.map(|v| v - 0.5);
        if !corners.contains(&c) {
            corners.push(c);
        }
    }
    if corners.len() > MOST_CORNERS {
        let lo = corners.iter().fold([f32::MAX; 3], |lo, c| [0, 1, 2].map(|i| lo[i].min(c[i])));
        let hi = corners.iter().fold([f32::MIN; 3], |hi, c| [0, 1, 2].map(|i| hi[i].max(c[i])));
        corners = (0..8).map(|k| [0, 1, 2].map(|i| if k >> i & 1 == 0 { lo[i] } else { hi[i] })).collect();
    }
    if corners.is_empty() {
        return Solid::Cube;
    }
    let n = corners.len() as f32;
    let middle = corners.iter().fold([0.0; 3], |m, c| [0, 1, 2].map(|i| m[i] + c[i] / n));
    let keep = 1.0 - 2.0 * SHAPE_SLACK;
    Solid::Corners(corners.iter().map(|c| [0, 1, 2].map(|i| middle[i] + (c[i] - middle[i]) * keep)).collect())
}

fn cube_corners() -> Vec<[f32; 3]> {
    let h = 0.5 - SHAPE_SLACK;
    (0..8).map(|k| [0, 1, 2].map(|i| if k >> i & 1 == 0 { -h } else { h })).collect()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn neg(a: [f32; 3]) -> [f32; 3] {
    [-a[0], -a[1], -a[2]]
}

/// The corner furthest along a direction.
fn furthest(corners: &[[f32; 3]], along: [f32; 3]) -> [f32; 3] {
    let mut best = (f32::MIN, [0.0; 3]);
    for &c in corners {
        let d = dot(c, along);
        if d > best.0 {
            best = (d, c);
        }
    }
    best.1
}

/// Whether two convex shapes, each given by its corners, overlap. The
/// Gilbert-Johnson-Keerthi method: the shapes overlap exactly when the set
/// of all differences between a point of one and a point of the other holds
/// the origin, and that is decided by closing in on the origin with a
/// point, a line, a triangle and at last a tetrahedron of such differences.
fn convex_overlap(a: &[[f32; 3]], b: &[[f32; 3]], apart: [f32; 3]) -> bool {
    let support = |d: [f32; 3]| sub(furthest(a, d), furthest(b, neg(d)));
    let nothing = |d: [f32; 3]| dot(d, d) < 1e-12;
    let mut d = if nothing(apart) { [1.0, 0.0, 0.0] } else { apart };
    // The newest point is first.
    let mut simplex: Vec<[f32; 3]> = Vec::with_capacity(4);
    let first = support(d);
    simplex.push(first);
    d = neg(first);
    for _ in 0..32 {
        if nothing(d) {
            // The origin lies on what has been found so far.
            return true;
        }
        let p = support(d);
        if dot(p, d) < 0.0 {
            return false;
        }
        simplex.insert(0, p);
        loop {
            let ao = neg(simplex[0]);
            match simplex.len() {
                2 => {
                    let ab = sub(simplex[1], simplex[0]);
                    if dot(ab, ao) > 0.0 {
                        d = cross(cross(ab, ao), ab);
                    } else {
                        simplex.truncate(1);
                        d = ao;
                    }
                }
                3 => {
                    let (pa, pb, pc) = (simplex[0], simplex[1], simplex[2]);
                    let (ab, ac) = (sub(pb, pa), sub(pc, pa));
                    let abc = cross(ab, ac);
                    if dot(cross(abc, ac), ao) > 0.0 {
                        if dot(ac, ao) > 0.0 {
                            simplex = vec![pa, pc];
                            d = cross(cross(ac, ao), ac);
                        } else {
                            simplex = vec![pa, pb];
                            continue;
                        }
                    } else if dot(cross(ab, abc), ao) > 0.0 {
                        simplex = vec![pa, pb];
                        continue;
                    } else if dot(abc, ao) > 0.0 {
                        d = abc;
                    } else {
                        simplex = vec![pa, pc, pb];
                        d = neg(abc);
                    }
                }
                _ => {
                    let (pa, pb, pc, pd) = (simplex[0], simplex[1], simplex[2], simplex[3]);
                    let (ab, ac, ad) = (sub(pb, pa), sub(pc, pa), sub(pd, pa));
                    if dot(cross(ab, ac), ao) > 0.0 {
                        simplex = vec![pa, pb, pc];
                    } else if dot(cross(ac, ad), ao) > 0.0 {
                        simplex = vec![pa, pc, pd];
                    } else if dot(cross(ad, ab), ao) > 0.0 {
                        simplex = vec![pa, pd, pb];
                    } else {
                        return true;
                    }
                    continue;
                }
            }
            break;
        }
    }
    false
}

/// The box, along the still entity's axes, that a placed box of blocks fits in.
fn spanned(bounds: Bounds, placement: &Placement) -> ([f32; 3], [f32; 3]) {
    let (min, max) = (bounds.min.to_array(), bounds.max.to_array());
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for corner in 0..8 {
        let c = [0, 1, 2].map(|i| if corner >> i & 1 == 0 { min[i] as f32 - 0.5 } else { max[i] as f32 + 0.5 });
        let at = placement.at(c);
        for i in 0..3 {
            lo[i] = lo[i].min(at[i]);
            hi[i] = hi[i].max(at[i]);
        }
    }
    (lo, hi)
}

/// Whether two cubes of half-width `h` overlap: one square to the axes at
/// the origin, the other turned by `turn` with its centre at `t`. `abs` is
/// `turn` with every entry made positive (and a hair more, so that edges
/// that run parallel do not divide the test by nothing). The separating axis
/// test over the fifteen directions two boxes can be told apart along.
fn cubes_overlap(t: [f32; 3], turn: &Turn, abs: &Turn, h: f32) -> bool {
    for i in 0..3 {
        // Along the still cube's sides, then the turned cube's.
        if t[i].abs() > h + h * (abs[i][0] + abs[i][1] + abs[i][2]) {
            return false;
        }
        let along = t[0] * turn[0][i] + t[1] * turn[1][i] + t[2] * turn[2][i];
        if along.abs() > h + h * (abs[0][i] + abs[1][i] + abs[2][i]) {
            return false;
        }
    }
    // Along each pairing of an edge of one with an edge of the other.
    for i in 0..3 {
        let (i1, i2) = ((i + 1) % 3, (i + 2) % 3);
        for j in 0..3 {
            let (j1, j2) = ((j + 1) % 3, (j + 2) % 3);
            let reach = h * (abs[i1][j] + abs[i2][j]) + h * (abs[i][j1] + abs[i][j2]);
            if (t[i2] * turn[i1][j] - t[i1] * turn[i2][j]).abs() > reach {
                return false;
            }
        }
    }
    true
}

/// The first place a moving entity runs into a still one, if it does: the
/// mover's block and the still entity's. `body` is the mover's own;
/// `placement` puts the mover's blocks into the still entity's coordinates;
/// `coarse` is the still entity's own.
pub fn hit(body: &Body, mover: &Entity, placement: &Placement, still: &Entity, coarse: &Coarse, shapes: &dyn Shapes) -> Option<(Pos, Pos)> {
    let within = coarse.bounds;
    let (min, max) = (within.min.to_array().map(|c| c as f32), within.max.to_array().map(|c| c as f32));
    let (lo, hi) = spanned(body.bounds, placement);
    if (0..3).any(|i| lo[i] > max[i] + 0.5 || hi[i] < min[i] - 0.5) {
        return None;
    }
    let h = 0.5 - SLACK;
    let turn = &placement.turn;
    let abs = turn.map(|row| row.map(|c| c.abs() + 1e-6));
    // How far apart along each axis the middles of two cells can be, one
    // turned, and the blocks in them still touch.
    let reach = [0, 1, 2].map(|i| 0.5 + 0.5 * (abs[i][0] + abs[i][1] + abs[i][2]) - SLACK);
    let mut near = Near::new(still);
    let mut own = Near::new(mover);
    // Shapes other than the cube, by where their geometry is kept, and
    // those of the body's blocks turned into the still entity's directions.
    let mut solids: HashMap<*const Geom, Solid> = HashMap::new();
    let mut turned: HashMap<*const Geom, Vec<[f32; 3]>> = HashMap::new();
    let cube = cube_corners();
    let turned_cube: Vec<[f32; 3]> = cube.iter().map(|c| crate::dock::turned(turn, *c)).collect();
    for &p in &body.shell {
        let c = placement.at([p.x as f32, p.y as f32, p.z as f32]);
        if !coarse.near(c.map(|v| v.round() as i32)) {
            continue;
        }
        // The whole cells whose middle is nearer than that on every axis.
        let from = [0, 1, 2].map(|i| (c[i] - reach[i]).floor() as i32 + 1);
        let to = [0, 1, 2].map(|i| (c[i] + reach[i]).ceil() as i32 - 1);
        // Read now, not when the body was made: a door may have opened since.
        let Some(mine) = shapes.geom(own.get(p)) else { continue };
        for x in from[0]..=to[0] {
            for y in from[1]..=to[1] {
                for z in from[2]..=to[2] {
                    let q = Pos::new(x, y, z);
                    let other = near.get(q);
                    if other.is_air() {
                        continue;
                    }
                    let t = [c[0] - x as f32, c[1] - y as f32, c[2] - z as f32];
                    let Some(theirs) = shapes.geom(other) else { continue };
                    let mine_solid = solids.entry(mine as *const Geom).or_insert_with(|| solid_of(mine));
                    let mine_is_cube = matches!(mine_solid, Solid::Cube);
                    if let Solid::Corners(corners) = mine_solid {
                        turned.entry(mine as *const Geom).or_insert_with(|| corners.iter().map(|c| crate::dock::turned(turn, *c)).collect());
                    }
                    let their_solid = solids.entry(theirs as *const Geom).or_insert_with(|| solid_of(theirs));
                    let overlap = match (mine_is_cube, their_solid) {
                        (true, Solid::Cube) => cubes_overlap(t, turn, &abs, h),
                        (_, their_solid) => {
                            let a = match their_solid {
                                Solid::Cube => &cube,
                                Solid::Corners(corners) => &*corners,
                            };
                            let b = if mine_is_cube { &turned_cube } else { &turned[&(mine as *const Geom)] };
                            let b: Vec<[f32; 3]> = b.iter().map(|c| [c[0] + t[0], c[1] + t[1], c[2] + t[2]]).collect();
                            convex_overlap(a, &b, neg(t))
                        }
                    };
                    if overlap {
                        return Some((p, q));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{about, NO_TURN};
    use crate::Block;

    /// Every block a full cube, except type 2: a wedge, and types 11 to 13:
    /// slabs three quarters, a half and a quarter thick.
    struct TestShapes(ShapeSet);

    impl Shapes for TestShapes {
        fn geom(&self, block: Block) -> Option<&Geom> {
            Some(match block.ty() {
                2 => self.0.geom(Shape::Wedge, 0, block.orientation()),
                ty @ 11..=13 => self.0.geom(Shape::Cube, (ty - 10) as u8, block.orientation()),
                _ => self.0.geom(Shape::Cube, 0, 0),
            })
        }
    }

    thread_local! {
        static SHAPES: &'static TestShapes = Box::leak(Box::new(TestShapes(ShapeSet::new())));
    }

    struct Cubes;
    impl Shapes for Cubes {
        fn geom(&self, block: Block) -> Option<&Geom> {
            SHAPES.with(|s| s.geom(Block::new(5, block.orientation(), false)))
        }
    }
    const CUBES: Cubes = Cubes;

    fn blocks(cells: &[(i32, i32, i32)]) -> Entity {
        let mut e = Entity::new();
        for &(x, y, z) in cells {
            e.set(Pos::new(x, y, z), Block::new(5, 0, false));
        }
        e
    }

    fn placed(turn: Turn, shift: [f32; 3]) -> Placement {
        Placement { turn, shift }
    }

    #[test]
    fn blocks_that_share_a_cell_hit_and_blocks_that_only_touch_do_not() {
        let still = blocks(&[(0, 0, 0), (1, 0, 0)]);
        let bounds = Coarse::of(&still).unwrap();
        let mover = blocks(&[(0, 0, 0)]);
        let body = Body::of(&mover).unwrap();
        assert_eq!(hit(&body, &mover, &placed(NO_TURN, [1.0, 0.0, 0.0]), &still, &bounds, &CUBES), Some((Pos::new(0, 0, 0), Pos::new(1, 0, 0))));
        // Resting on top, face to face.
        assert_eq!(hit(&body, &mover, &placed(NO_TURN, [0.0, 1.0, 0.0]), &still, &bounds, &CUBES), None);
        // Halfway into the cell beside the row.
        assert_eq!(hit(&body, &mover, &placed(NO_TURN, [2.0, 0.0, 0.0]), &still, &bounds, &CUBES), None);
        assert!(hit(&body, &mover, &placed(NO_TURN, [1.6, 0.0, 0.0]), &still, &bounds, &CUBES).is_some());
        // Far away: not even looked at.
        assert_eq!(hit(&body, &mover, &placed(NO_TURN, [40.0, 0.0, 0.0]), &still, &bounds, &CUBES), None);
    }

    #[test]
    fn a_turned_block_reaches_further_with_its_corners() {
        // Two blocks side by side, face to face. Square on, they only
        // touch; turned an eighth of a turn, a corner pokes into the neighbour.
        let still = blocks(&[(0, 0, 0)]);
        let bounds = Coarse::of(&still).unwrap();
        let mover = blocks(&[(0, 0, 0)]);
        let body = Body::of(&mover).unwrap();
        let beside = [1.0, 0.0, 0.0];
        assert_eq!(hit(&body, &mover, &placed(NO_TURN, beside), &still, &bounds, &CUBES), None);
        let eighth = about([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_4);
        assert!(hit(&body, &mover, &placed(eighth, beside), &still, &bounds, &CUBES).is_some());
        // Corner to corner on the diagonal there is room again.
        assert_eq!(hit(&body, &mover, &placed(eighth, [1.0, 0.0, 1.0]), &still, &bounds, &CUBES), None);
        // A quarter turn puts it square again.
        let quarter = about([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
        assert_eq!(hit(&body, &mover, &placed(quarter, beside), &still, &bounds, &CUBES), None);
    }

    #[test]
    fn a_bar_swinging_round_is_stopped_by_a_post() {
        // A bar five blocks long along +Z from the pivot, and a post at
        // (3, 0, 3): the bar swings freely until it comes round to the post.
        let mover = blocks(&[(0, 0, 1), (0, 0, 2), (0, 0, 3), (0, 0, 4), (0, 0, 5)]);
        let bar = Body::of(&mover).unwrap();
        let post = blocks(&[(3, 0, 3)]);
        let bounds = Coarse::of(&post).unwrap();
        let swung = |degrees: f32| hit(&bar, &mover, &placed(about([0.0, 1.0, 0.0], degrees.to_radians()), [0.0; 3]), &post, &bounds, &CUBES);
        assert_eq!(swung(0.0), None);
        assert_eq!(swung(20.0), None);
        assert!(swung(45.0).is_some(), "pointing straight at the post");
        assert_eq!(swung(90.0), None, "past it, along +X");
        assert_eq!(swung(-45.0), None, "the other way there is nothing");
    }

    #[test]
    fn only_blocks_with_an_open_side_are_in_the_shell() {
        let mut cube = Entity::new();
        for x in 0..3 {
            for y in 0..3 {
                for z in 0..3 {
                    cube.set(Pos::new(x, y, z), Block::new(5, 0, false));
                }
            }
        }
        let body = Body::of(&cube).unwrap();
        assert_eq!(body.shell.len(), 26);
        assert!(!body.shell.contains(&Pos::new(1, 1, 1)));
        assert!(Body::of(&Entity::new()).is_none());
    }

    #[test]
    fn shapes_that_fit_together_share_a_cell_and_shapes_that_do_not_collide() {
        let shapes = SHAPES.with(|s| *s);
        let one = |ty: u16, turn: u8| {
            let mut e = Entity::new();
            e.set(Pos::new(0, 0, 0), Block::new(ty, turn, false));
            e
        };
        let same_cell = |a: &Entity, b: &Entity| hit(&Body::of(a).unwrap(), a, &Placement::SAME, b, &Coarse::of(b).unwrap(), shapes).is_some();
        // A quarter slab and a three-quarter slab facing each other fill one
        // cell between them; a half slab more is too much.
        assert!(!same_cell(&one(13, 2), &one(11, 3)));
        assert!(!same_cell(&one(12, 2), &one(12, 3)));
        assert!(same_cell(&one(12, 2), &one(11, 3)));
        assert!(same_cell(&one(13, 2), &one(13, 2)), "the same quarter twice");
        assert!(same_cell(&one(13, 2), &one(5, 0)), "a slab in a full block");
        // Every wedge fits with exactly one other wedge: its other half.
        for turn in 0..12u8 {
            let fits: Vec<u8> = (0..12).filter(|other| !same_cell(&one(2, turn), &one(2, *other))).collect();
            assert_eq!(fits.len(), 1, "wedge {turn} fits with {fits:?}");
            assert!(same_cell(&one(2, turn), &one(2, turn)));
            assert!(same_cell(&one(2, turn), &one(5, 0)));
        }
        // Beside each other, nothing touches, whatever the shapes.
        let beside = Placement { turn: NO_TURN, shift: [1.0, 0.0, 0.0] };
        for turn in 0..12u8 {
            let cube = one(5, 0);
            assert_eq!(hit(&Body::of(&one(2, turn)).unwrap(), &one(2, turn), &beside, &cube, &Coarse::of(&cube).unwrap(), shapes), None);
        }
        // A wedge turned an eighth of a turn pokes into the cube beside it
        // with a corner, as a cube would.
        let eighth = Placement { turn: about([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_4), shift: [1.0, 0.0, 0.0] };
        let cube = one(5, 0);
        let poking = (0..12u8).filter(|turn| hit(&Body::of(&one(2, *turn)).unwrap(), &one(2, *turn), &eighth, &cube, &Coarse::of(&cube).unwrap(), shapes).is_some()).count();
        assert!(poking >= 6, "{poking} of 12 wedges reach the cube");
    }
}
