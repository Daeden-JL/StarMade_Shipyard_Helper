//! Stretching a part of an entity: a point of it is taken hold of and moved,
//! and the blocks round it follow, the nearer the further.
//!
//! The blocks within `reach` of the point are read once ([`Region::read`]).
//! Moving the point by `by` moves each place `p` of the region to
//! `p + weight(p) * by`, where the weight is 1 at the point and falls
//! smoothly to 0 at the edge of the region, so nothing outside it changes.
//! What a cell holds afterwards is what was at the place that moved onto it
//! ([`source`]): pulled out of a body, the cells beyond its surface fill with
//! the blocks that were at the surface; pushed into it, the cells at the
//! surface take the air that was outside, and the body caves in.
//!
//! Every result is worked out from the blocks as they were read, not from
//! the last result, so moving the point back gives back what was there.
//!
//! Links go with the blocks ([`carried_links`]): a block that moves keeps
//! its links where it ends up, and where a linked block is drawn out into
//! several, each of them is linked as it was.

use crate::entity::Pos;
use crate::Block;
use std::collections::{HashMap, HashSet};

/// How far the point may be moved from where the region was read, as a part
/// of the reach. Past about 0.65 two places would move onto the same cell
/// and the body would fold; under it a wall one block thick stays joined.
pub const SLACK: f32 = 0.6;

/// The blocks of a cube of cells, as they were when it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// The point taken hold of: the middle of the cube.
    pub centre: Pos,
    /// How many cells the cube goes from the middle, each way.
    pub reach: i32,
    /// The blocks, in the order of [`Region::cells`].
    pub blocks: Vec<Block>,
}

impl Region {
    pub fn read(centre: Pos, reach: i32, get: impl Fn(Pos) -> Block) -> Self {
        let mut region = Region { centre, reach: reach.max(1), blocks: Vec::new() };
        region.blocks = region.cells().map(get).collect();
        region
    }

    /// Every cell of the cube.
    pub fn cells(&self) -> impl Iterator<Item = Pos> {
        let (c, r) = (self.centre, self.reach);
        (-r..=r).flat_map(move |z| (-r..=r).flat_map(move |y| (-r..=r).map(move |x| c.offset(x, y, z))))
    }

    /// Where cell `p` is in [`Region::cells`], if it is in the cube.
    pub fn index(&self, p: Pos) -> Option<usize> {
        let (r, side) = (self.reach, 2 * self.reach + 1);
        let d = [p.x - self.centre.x + r, p.y - self.centre.y + r, p.z - self.centre.z + r];
        d.iter().all(|v| (0..side).contains(v)).then(|| ((d[2] * side + d[1]) * side + d[0]) as usize)
    }

    /// Whether cell `p` is in the cube.
    pub fn holds(&self, p: Pos) -> bool {
        self.index(p).is_some()
    }

    /// The block that was at `p`; air outside the cube.
    pub fn get(&self, p: Pos) -> Block {
        self.index(p).map_or(Block::AIR, |i| self.blocks[i])
    }
}

/// Whether the point can be moved `by` from where a region of this reach was
/// read. Further than that, the region is read again where the point is.
pub fn reaches(reach: i32, by: [i32; 3]) -> bool {
    let length = by.iter().map(|v| (v * v) as f32).sum::<f32>().sqrt();
    length <= SLACK * reach as f32
}

/// How much of the point's move a place `offset` from it makes.
fn weight(reach: i32, offset: [f32; 3]) -> f32 {
    let out = offset.iter().map(|v| v * v).sum::<f32>() / (reach * reach) as f32;
    if out >= 1.0 { 0.0 } else { (1.0 - out) * (1.0 - out) }
}

/// The cell whose block ends up at `q` when the point at `centre` is moved `by`.
pub fn source(centre: Pos, reach: i32, by: [i32; 3], q: Pos) -> Pos {
    let offset = [(q.x - centre.x) as f32, (q.y - centre.y) as f32, (q.z - centre.z) as f32];
    if by == [0; 3] || weight(reach, offset) == 0.0 {
        return q;
    }
    // The place came `t * by` of the way, with `t` its own weight: one such
    // `t` between 0 and 1, since no two places move onto the same one.
    let back = |t: f32| [0, 1, 2].map(|a| offset[a] - t * by[a] as f32);
    let (mut least, mut most) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let t = (least + most) / 2.0;
        if t < weight(reach, back(t)) {
            least = t;
        } else {
            most = t;
        }
    }
    let from = back((least + most) / 2.0);
    centre.offset(from[0].round() as i32, from[1].round() as i32, from[2].round() as i32)
}

/// The cells of a region after its point is moved `by`, in the order of
/// [`Region::cells`]: the block each holds and the cell that block was in.
/// Only blocks that are `movable` (asked with the cell they were in) move;
/// the others stay where they are, and what would move onto them does not.
pub fn moved(base: &Region, by: [i32; 3], movable: impl Fn(Pos, Block) -> bool) -> Vec<(Block, Pos)> {
    base.cells()
        .zip(&base.blocks)
        .map(|(q, &old)| {
            if !old.is_air() && !movable(q, old) {
                return (old, q);
            }
            let from = source(base.centre, base.reach, by, q);
            let block = base.get(from);
            if !block.is_air() && movable(from, block) { (block, from) } else { (Block::AIR, q) }
        })
        .collect()
}

/// The blocks alone of [`moved`].
pub fn stretched(base: &Region, by: [i32; 3], movable: impl Fn(Pos, Block) -> bool) -> Vec<Block> {
    moved(base, by, movable).into_iter().map(|(block, _)| block).collect()
}

/// The links of the blocks of a region after a stretch, as (controller,
/// controlled) cells. `links` are the links that had an end in the region
/// when it was read, and `after` is what [`moved`] gave. A controlled block
/// is linked wherever it ended up, in every cell it was drawn out into; a
/// controller that was drawn out into several controls from one of them (the
/// cell it was in, or else the nearest to it); a link whose controller or
/// controlled block went altogether is left out.
pub fn carried_links(base: &Region, after: &[(Block, Pos)], links: &[(Pos, Pos)]) -> Vec<(Pos, Pos)> {
    let ends: HashSet<Pos> = links.iter().flat_map(|(c, t)| [*c, *t]).filter(|p| base.holds(*p)).collect();
    // Where the block of each end is now.
    let mut now: HashMap<Pos, Vec<Pos>> = HashMap::new();
    for (q, (block, from)) in base.cells().zip(after) {
        if !block.is_air() && ends.contains(from) {
            now.entry(*from).or_default().push(q);
        }
    }
    let places = |p: Pos| -> Vec<Pos> { if base.holds(p) { now.get(&p).cloned().unwrap_or_default() } else { vec![p] } };
    let far = |a: Pos, b: Pos| (a.x - b.x).pow(2) + (a.y - b.y).pow(2) + (a.z - b.z).pow(2);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for &(controller, target) in links {
        let Some(c) = places(controller).into_iter().min_by_key(|p| far(*p, controller)) else { continue };
        for t in places(target) {
            if t != c && seen.insert((c, t)) {
                out.push((c, t));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const HULL: u16 = 5;
    const SYSTEM: u16 = 9;

    /// A body filling everything at or below y = 0, with hull on top.
    fn ground(p: Pos) -> Block {
        if p.y > 0 { Block::AIR } else { Block::new(HULL, 0, false) }
    }

    fn after(base: &Region, by: [i32; 3], movable: impl Fn(Pos, Block) -> bool) -> HashMap<Pos, Block> {
        base.cells().zip(stretched(base, by, movable)).collect()
    }

    /// The highest block of the column at (x, z).
    fn top(cells: &HashMap<Pos, Block>, x: i32, z: i32) -> i32 {
        (-8..=8).rev().find(|y| !cells[&Pos::new(x, *y, z)].is_air()).unwrap()
    }

    #[test]
    fn a_region_gives_back_what_it_read() {
        let region = Region::read(Pos::new(3, -2, 7), 4, |p| Block::new((p.x + 10 * p.y + 100 * p.z + 500) as u16, 0, false));
        assert_eq!(region.blocks.len(), 9 * 9 * 9);
        for p in region.cells() {
            assert_eq!(region.get(p).ty() as i32, p.x + 10 * p.y + 100 * p.z + 500);
        }
        assert!(region.get(Pos::new(8, -2, 7)).is_air(), "past the cube");
        // Not moved, nothing changes.
        assert_eq!(stretched(&region, [0; 3], |_, _| true), region.blocks);
    }

    #[test]
    fn pulling_a_point_out_raises_a_hill_under_it() {
        let base = Region::read(Pos::new(0, 0, 0), 8, ground);
        let cells = after(&base, [0, 3, 0], |_, _| true);
        // The point itself goes all the way; the blocks round it less, down to nothing at the edge.
        assert_eq!(top(&cells, 0, 0), 3);
        let heights: Vec<i32> = (0..=8).map(|x| top(&cells, x, 0)).collect();
        assert!(heights.windows(2).all(|w| w[0] >= w[1] && w[0] - w[1] <= 1), "{heights:?}");
        assert_eq!(heights[8], 0);
        assert_eq!(top(&cells, -4, 0), top(&cells, 4, 0));
        assert_eq!(top(&cells, 0, 4), top(&cells, 4, 0));
        // Blocks were added and none taken away.
        assert!(base.cells().all(|p| base.get(p).is_air() || !cells[&p].is_air()));
        assert!(cells.values().filter(|b| !b.is_air()).count() > base.blocks.iter().filter(|b| !b.is_air()).count());
    }

    #[test]
    fn pushing_a_point_in_caves_the_body_in() {
        let base = Region::read(Pos::new(0, 0, 0), 8, ground);
        let cells = after(&base, [0, -3, 0], |_, _| true);
        assert_eq!(top(&cells, 0, 0), -3);
        let heights: Vec<i32> = (0..=8).map(|x| top(&cells, x, 0)).collect();
        assert!(heights.windows(2).all(|w| w[0] <= w[1] && w[1] - w[0] <= 1), "{heights:?}");
        assert_eq!(heights[8], 0);
        // Blocks were taken away and none added.
        assert!(base.cells().all(|p| !base.get(p).is_air() || cells[&p].is_air()));
        // And moving the point back gives back what was there.
        assert_eq!(stretched(&base, [0; 3], |_, _| true), base.blocks);
    }

    #[test]
    fn a_wall_one_block_thick_moves_as_a_wall() {
        let wall = |p: Pos| if p.y == 0 { Block::new(HULL, 0, false) } else { Block::AIR };
        let base = Region::read(Pos::new(0, 0, 0), 10, wall);
        for by in [[0, 6, 0], [0, -6, 0]] {
            assert!(reaches(10, by));
            let cells = after(&base, by, |_, _| true);
            for x in -10..=10 {
                for z in -10..=10 {
                    // One block in every column still, and next to its neighbours'.
                    let column: Vec<i32> = (-10..=10).filter(|y| !cells[&Pos::new(x, *y, z)].is_air()).collect();
                    assert_eq!(column.len(), 1, "column {x}, {z}");
                    if x < 10 {
                        assert!((column[0] - top_of(&cells, x + 1, z)).abs() <= 1);
                    }
                }
            }
        }
        assert!(!reaches(10, [0, 7, 0]) && reaches(2, [1, 0, 0]) && !reaches(2, [1, 1, 0]));

        fn top_of(cells: &HashMap<Pos, Block>, x: i32, z: i32) -> i32 {
            (-10..=10).find(|y| !cells[&Pos::new(x, *y, z)].is_air()).unwrap()
        }
    }

    #[test]
    fn blocks_that_are_not_to_move_stay_and_are_not_overwritten() {
        // Hull over a layer of systems.
        let body = |p: Pos| match p.y {
            1.. => Block::AIR,
            0 => Block::new(HULL, 0, false),
            -1 => Block::new(SYSTEM, 0, false),
            _ => Block::new(HULL, 0, false),
        };
        let base = Region::read(Pos::new(0, 0, 0), 6, body);
        let hull_only = |_: Pos, b: Block| b.ty() == HULL;
        for by in [[0, 3, 0], [0, -3, 0], [2, 0, 1]] {
            let cells = after(&base, by, hull_only);
            for p in base.cells() {
                assert_eq!(cells[&p].ty() == SYSTEM, p.y == -1, "{p:?} moved {by:?}");
            }
        }
        // Pulled out, the hull goes up from the systems, which stay behind.
        let cells = after(&base, [0, 3, 0], hull_only);
        assert_eq!(cells[&Pos::new(0, 3, 0)].ty(), HULL);
        // With everything moving, the systems go along.
        let cells = after(&base, [0, 3, 0], |_, _| true);
        assert_eq!(cells[&Pos::new(0, 2, 0)].ty(), SYSTEM);
        // But for the cells that are held in place.
        let held = Pos::new(0, -1, 0);
        let cells = after(&base, [0, 3, 0], |p, _| p != held);
        assert_eq!(cells[&held].ty(), SYSTEM);
        assert!(cells[&Pos::new(0, 2, 0)].is_air(), "and left a gap where it would have gone");
    }

    #[test]
    fn links_go_with_the_blocks_and_new_blocks_are_linked_too() {
        // A column of systems up to the surface at y = 0, run by a computer
        // far below (outside the region) and one beside it (inside).
        let body = |p: Pos| match (p.x, p.y, p.z) {
            (_, 1.., _) => Block::AIR,
            (0, _, 0) => Block::new(SYSTEM, 0, false),
            _ => Block::new(HULL, 0, false),
        };
        let base = Region::read(Pos::new(0, 0, 0), 6, body);
        let (outside, top, lower, beside) = (Pos::new(0, -40, 0), Pos::new(0, 0, 0), Pos::new(0, -6, 0), Pos::new(1, 0, 0));
        let column: Vec<Pos> = (-6..=0).map(|y| Pos::new(0, y, 0)).collect();
        let mut links: Vec<(Pos, Pos)> = column.iter().map(|t| (outside, *t)).collect();
        links.push((beside, top));
        // Not moved: the links as they were.
        let still = moved(&base, [0; 3], |_, _| true);
        assert_eq!(carried_links(&base, &still, &links), links);
        // Pulled up 3: the column is drawn out, and every block of it is linked, the new ones too.
        let after = moved(&base, [0, 3, 0], |_, _| true);
        let carried = carried_links(&base, &after, &links);
        let longer: Vec<Pos> = base.cells().zip(&after).filter(|(_, (b, _))| b.ty() == SYSTEM).map(|(q, _)| q).collect();
        assert_eq!(longer, (-6..=3).map(|y| Pos::new(0, y, 0)).collect::<Vec<_>>());
        for t in &longer {
            assert!(carried.contains(&(outside, *t)), "{t:?}");
        }
        // The computer beside the top rose with it and controls it from where it is now.
        assert!(carried.contains(&(Pos::new(1, 3, 0), Pos::new(0, 3, 0))));
        assert_eq!(carried.iter().filter(|(c, _)| *c != outside).count(), 1);
        assert_eq!(carried.len(), longer.len() + 1);
        // Pushed in 3, the column is shorter: the top block and its computer
        // are 3 lower, linked still, and the blocks squeezed out are gone with their links.
        let after = moved(&base, [0, -3, 0], |_, _| true);
        let carried = carried_links(&base, &after, &links);
        let mut expected: Vec<(Pos, Pos)> = (-6..=-3).map(|y| (outside, Pos::new(0, y, 0))).collect();
        expected.push((Pos::new(1, -3, 0), Pos::new(0, -3, 0)));
        assert_eq!(carried, expected);
        assert!(carried.contains(&(outside, lower)));
    }
}
