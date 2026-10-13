//! Which empty cells of an entity its blocks enclose: the inside of a hull,
//! where systems go, as against the space around the ship.
//!
//! Air is outside if it connects to the space beyond the entity's bounds
//! through faces of empty cells. A shell with openings (thruster ports, an
//! open hangar) would let the outside in everywhere, so gaps up to a given
//! width can be counted as closed: the blocks are grown by `seal` cells
//! before the outside is flooded, and the outside found so is grown back by
//! the same amount (a morphological closing).

use crate::entity::{Entity, Pos};

/// The widest `seal` [`Interior::of`] takes.
pub const MAX_SEAL: u32 = 6;
/// The widest seal [`Interior::find`] tries.
pub const AUTO_SEAL: u32 = 4;

/// One bit per cell of a box, in rows along X.
#[derive(Clone)]
struct Bits {
    n: [usize; 3],
    row_words: usize,
    words: Vec<u64>,
}

impl Bits {
    fn new(n: [usize; 3]) -> Self {
        let row_words = n[0].div_ceil(64);
        Self { n, row_words, words: vec![0; row_words * n[1] * n[2]] }
    }

    fn rows(&self) -> usize {
        self.n[1] * self.n[2]
    }

    fn row(&self, r: usize) -> &[u64] {
        &self.words[r * self.row_words..(r + 1) * self.row_words]
    }

    fn row_mut(&mut self, r: usize) -> &mut [u64] {
        let w = self.row_words;
        &mut self.words[r * w..(r + 1) * w]
    }

    fn get(&self, x: usize, y: usize, z: usize) -> bool {
        let w = (y + self.n[1] * z) * self.row_words + x / 64;
        self.words[w] >> (x % 64) & 1 == 1
    }

    fn set(&mut self, x: usize, y: usize, z: usize) {
        let w = (y + self.n[1] * z) * self.row_words + x / 64;
        self.words[w] |= 1 << (x % 64);
    }

    /// Bits past the end of each row, which must stay clear.
    fn tail_mask(&self) -> u64 {
        match self.n[0] % 64 {
            0 => u64::MAX,
            used => (1u64 << used) - 1,
        }
    }

    fn invert(&mut self) {
        let tail = self.tail_mask();
        let w = self.row_words;
        for (i, word) in self.words.iter_mut().enumerate() {
            *word = !*word;
            if i % w == w - 1 {
                *word &= tail;
            }
        }
    }

    /// Grows every set cell into the box of `r` cells around it.
    fn dilate(&self, r: usize) -> Self {
        if r == 0 {
            return self.clone();
        }
        let tail = self.tail_mask();
        let w = self.row_words;
        // Along X: shifted copies of each row, a cell at a time.
        let mut along_x = self.clone();
        for row in 0..self.rows() {
            let dst = along_x.row_mut(row);
            for _ in 0..r {
                let current: Vec<u64> = dst.to_vec();
                for i in 0..w {
                    let up = current[i] << 1 | if i > 0 { current[i - 1] >> 63 } else { 0 };
                    let down = current[i] >> 1 | if i + 1 < w { current[i + 1] << 63 } else { 0 };
                    dst[i] = current[i] | up | down;
                }
                dst[w - 1] &= tail;
            }
        }
        // Along Y and Z: whole rows.
        let mut out = along_x.clone();
        let (ny, nz) = (self.n[1], self.n[2]);
        for z in 0..nz {
            for y in 0..ny {
                let target = y + ny * z;
                for dz in -(r as i64)..=r as i64 {
                    for dy in -(r as i64)..=r as i64 {
                        let (sy, sz) = (y as i64 + dy, z as i64 + dz);
                        if (dy, dz) == (0, 0) || sy < 0 || sz < 0 || sy >= ny as i64 || sz >= nz as i64 {
                            continue;
                        }
                        let source = sy as usize + ny * sz as usize;
                        for (o, a) in out.row_mut(target).iter_mut().zip(along_x.row(source)) {
                            *o |= a;
                        }
                    }
                }
            }
        }
        out
    }

    /// Every cell of `free` connected through faces to a set cell of `self`,
    /// added to `self`. Works a row at a time: runs of free cells along X are
    /// filled with carries, and rows hand new cells to their four
    /// neighbours.
    fn flood(&mut self, free: &Bits) {
        let w = self.row_words;
        let (ny, nz) = (self.n[1], self.n[2]);
        let mut queued = vec![false; self.rows()];
        let mut queue: Vec<usize> = Vec::new();
        for (row, is_queued) in queued.iter_mut().enumerate() {
            if self.row(row).iter().any(|&b| b != 0) {
                *is_queued = true;
                queue.push(row);
            }
        }
        while let Some(row) = queue.pop() {
            queued[row] = false;
            let filled = fill_runs(self.row(row), free.row(row));
            self.row_mut(row).copy_from_slice(&filled);
            let (y, z) = (row % ny, row / ny);
            let mut neighbours = [usize::MAX; 4];
            if y > 0 {
                neighbours[0] = row - 1;
            }
            if y + 1 < ny {
                neighbours[1] = row + 1;
            }
            if z > 0 {
                neighbours[2] = row - ny;
            }
            if z + 1 < nz {
                neighbours[3] = row + ny;
            }
            for next in neighbours.into_iter().filter(|&n| n != usize::MAX) {
                let mut grew = false;
                let reached = &mut self.words[next * w..(next + 1) * w];
                for ((r, f), &cells) in reached.iter_mut().zip(free.row(next)).zip(&filled) {
                    let new = cells & f & !*r;
                    if new != 0 {
                        *r |= new;
                        grew = true;
                    }
                }
                if grew && !queued[next] {
                    queued[next] = true;
                    queue.push(next);
                }
            }
        }
    }
}

/// The runs of `free` that hold a cell of `seeds`, as a whole row.
fn fill_runs(seeds: &[u64], free: &[u64]) -> Vec<u64> {
    let seeds: Vec<u64> = seeds.iter().zip(free).map(|(s, f)| s & f).collect();
    // Adding a seed to a run of ones carries through to the run's end.
    let upward = |seeds: &[u64], free: &[u64]| -> Vec<u64> {
        let mut carry = false;
        seeds
            .iter()
            .zip(free)
            .map(|(&s, &f)| {
                let (sum, c1) = f.overflowing_add(s);
                let (sum, c2) = sum.overflowing_add(carry as u64);
                carry = c1 || c2;
                ((sum ^ f) & f) | s
            })
            .collect()
    };
    let up = upward(&seeds, free);
    let reversed = |v: &[u64]| -> Vec<u64> { v.iter().rev().map(|w| w.reverse_bits()).collect() };
    let down = reversed(&upward(&reversed(&seeds), &reversed(free)));
    up.iter().zip(&down).map(|(a, b)| a | b).collect()
}

/// The enclosed empty cells of one entity.
pub struct Interior {
    /// The position of grid cell (0, 0, 0).
    origin: Pos,
    inside: Bits,
    /// Gaps in the shell up to `2 * seal` cells wide were counted as closed.
    pub seal: u32,
    /// How many empty cells are enclosed.
    pub volume: u64,
}

/// One connected enclosed space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Space {
    pub min: Pos,
    pub max: Pos,
    pub volume: u64,
}

impl Interior {
    /// Finds the enclosed cells of `entity`, choosing the seal: the
    /// narrowest that closes the openings of its shell, if it has any. A
    /// wider seal than needed also closes the hollows of the outer surface,
    /// so the narrowest after which the enclosed space stops growing by much
    /// is taken.
    pub fn find(entity: &Entity) -> Option<Self> {
        let bounds = entity.bounds()?;
        let (sx, sy, sz) = bounds.size();
        let boxed = sx as u64 * sy as u64 * sz as u64;
        let mut best = Self::of(entity, 0)?;
        let mut last = best.volume;
        for seal in 1..=AUTO_SEAL {
            let wider = Self::of(entity, seal)?;
            if wider.volume >= 2 * last + (boxed / 200).max(27) {
                last = wider.volume;
                best = wider;
            } else {
                last = last.max(wider.volume);
            }
        }
        Some(best)
    }

    /// Finds the enclosed cells of `entity`, counting gaps up to `2 * seal`
    /// cells wide as closed. None for an empty entity.
    pub fn of(entity: &Entity, seal: u32) -> Option<Self> {
        let bounds = entity.bounds()?;
        let seal = seal.min(MAX_SEAL);
        // A border of free cells all round, wider than the blocks grow.
        let pad = seal as i32 + 1;
        let origin = bounds.min.offset(-pad, -pad, -pad);
        let (sx, sy, sz) = bounds.size();
        let n = [(sx + 2 * pad) as usize, (sy + 2 * pad) as usize, (sz + 2 * pad) as usize];
        let mut solid = Bits::new(n);
        entity.for_each(|p, _| solid.set((p.x - origin.x) as usize, (p.y - origin.y) as usize, (p.z - origin.z) as usize));
        let mut free = solid.dilate(seal as usize);
        free.invert();
        let mut outside = Bits::new(n);
        outside.set(0, 0, 0);
        outside.flood(&free);
        let mut outside = outside.dilate(seal as usize);
        // Inside: neither outside nor a block.
        for (o, s) in outside.words.iter_mut().zip(&solid.words) {
            *o |= s;
        }
        outside.invert();
        let inside = outside;
        let volume = inside.words.iter().map(|w| w.count_ones() as u64).sum();
        Some(Self { origin, inside, seal, volume })
    }

    fn cell(&self, p: Pos) -> Option<(usize, usize, usize)> {
        let (x, y, z) = (p.x - self.origin.x, p.y - self.origin.y, p.z - self.origin.z);
        let n = self.inside.n;
        (x >= 0 && y >= 0 && z >= 0 && (x as usize) < n[0] && (y as usize) < n[1] && (z as usize) < n[2]).then_some((x as usize, y as usize, z as usize))
    }

    /// Whether `p` is an empty cell the blocks enclose.
    pub fn contains(&self, p: Pos) -> bool {
        self.cell(p).is_some_and(|(x, y, z)| self.inside.get(x, y, z))
    }

    /// Calls `f` for every enclosed cell inside the box from `lo` to `hi`,
    /// in rows along X, low Z first.
    pub fn for_each_in(&self, lo: Pos, hi: Pos, mut f: impl FnMut(Pos)) {
        let n = self.inside.n;
        let o = self.origin;
        if hi.x < o.x || hi.y < o.y || hi.z < o.z || lo.x >= o.x + n[0] as i32 || lo.y >= o.y + n[1] as i32 || lo.z >= o.z + n[2] as i32 {
            return;
        }
        let clamp = |v: i32, origin: i32, size: usize| (v - origin).clamp(0, size as i32 - 1) as usize;
        let (x0, x1) = (clamp(lo.x, o.x, n[0]), clamp(hi.x, o.x, n[0]));
        let (y0, y1) = (clamp(lo.y, o.y, n[1]), clamp(hi.y, o.y, n[1]));
        let (z0, z1) = (clamp(lo.z, o.z, n[2]), clamp(hi.z, o.z, n[2]));
        for z in z0..=z1 {
            for y in y0..=y1 {
                let row = self.inside.row(y + n[1] * z);
                for (i, &word) in row.iter().enumerate().take(x1 / 64 + 1).skip(x0 / 64) {
                    let mut bits = word;
                    while bits != 0 {
                        let x = i * 64 + bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        if x >= x0 && x <= x1 {
                            f(Pos::new(x as i32 + o.x, y as i32 + o.y, z as i32 + o.z));
                        }
                    }
                }
            }
        }
    }

    /// The enclosed cells inside the box from `lo` to `hi`.
    pub fn cells_in(&self, lo: Pos, hi: Pos) -> Vec<Pos> {
        let mut out = Vec::new();
        self.for_each_in(lo, hi, |p| out.push(p));
        out
    }

    /// The enclosed cells inside a box, layer by layer along `axis` (0 X,
    /// 1 Y, 2 Z), low end first; layers without any are left out.
    pub fn layers(&self, lo: Pos, hi: Pos, axis: usize) -> Vec<Space> {
        let mut by_layer: std::collections::BTreeMap<i32, Space> = std::collections::BTreeMap::new();
        self.for_each_in(lo, hi, |p| {
            let layer = by_layer.entry(p.to_array()[axis]).or_insert(Space { min: p, max: p, volume: 0 });
            layer.min = layer.min.lo(p);
            layer.max = layer.max.hi(p);
            layer.volume += 1;
        });
        by_layer.into_values().collect()
    }

    /// The separate enclosed spaces, largest first.
    pub fn spaces(&self) -> Vec<Space> {
        let n = self.inside.n;
        let mut seen = Bits::new(n);
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for z in 0..n[2] {
            for y in 0..n[1] {
                let r = y + n[1] * z;
                if self.inside.row(r).iter().all(|&w| w == 0) {
                    continue;
                }
                for x in 0..n[0] {
                    if !self.inside.get(x, y, z) || seen.get(x, y, z) {
                        continue;
                    }
                    let mut space = Space { min: Pos::new(i32::MAX, i32::MAX, i32::MAX), max: Pos::new(i32::MIN, i32::MIN, i32::MIN), volume: 0 };
                    seen.set(x, y, z);
                    stack.push((x, y, z));
                    while let Some((cx, cy, cz)) = stack.pop() {
                        let p = Pos::new(cx as i32 + self.origin.x, cy as i32 + self.origin.y, cz as i32 + self.origin.z);
                        space.min = space.min.lo(p);
                        space.max = space.max.hi(p);
                        space.volume += 1;
                        let steps = [(-1i64, 0i64, 0i64), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)];
                        for (dx, dy, dz) in steps {
                            let (nx, ny, nz) = (cx as i64 + dx, cy as i64 + dy, cz as i64 + dz);
                            if nx < 0 || ny < 0 || nz < 0 || nx >= n[0] as i64 || ny >= n[1] as i64 || nz >= n[2] as i64 {
                                continue;
                            }
                            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
                            if self.inside.get(nx, ny, nz) && !seen.get(nx, ny, nz) {
                                seen.set(nx, ny, nz);
                                stack.push((nx, ny, nz));
                            }
                        }
                    }
                    out.push(space);
                }
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.volume));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    fn hull() -> Block {
        Block::new(598, 0, false)
    }

    /// A hollow box from `lo` to `hi` with walls one block thick.
    fn shell(e: &mut Entity, lo: Pos, hi: Pos) {
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    if x == lo.x || x == hi.x || y == lo.y || y == hi.y || z == lo.z || z == hi.z {
                        e.set(Pos::new(x, y, z), hull());
                    }
                }
            }
        }
    }

    #[test]
    fn a_closed_shell_encloses_its_inside_and_nothing_else() {
        let mut e = Entity::new();
        shell(&mut e, Pos::new(-3, -2, -10), Pos::new(3, 2, 70));
        e.set(Pos::new(0, 0, 0), hull());
        let inside = Interior::of(&e, 0).unwrap();
        assert_eq!(inside.volume, 5 * 3 * 79 - 1);
        assert!(inside.contains(Pos::new(2, 1, 69)) && inside.contains(Pos::new(-2, -1, -9)));
        assert!(!inside.contains(Pos::new(0, 0, 0)), "a block is not empty");
        assert!(!inside.contains(Pos::new(3, 0, 5)) && !inside.contains(Pos::new(4, 0, 5)) && !inside.contains(Pos::new(0, 0, 900)));
        assert_eq!(inside.cells_in(Pos::new(-100, -100, -100), Pos::new(100, 100, 100)).len() as u64, inside.volume);
        assert_eq!(inside.cells_in(Pos::new(0, 0, 60), Pos::new(9, 9, 99)).len(), 3 * 2 * 10);
        assert_eq!(inside.spaces(), vec![Space { min: Pos::new(-2, -1, -9), max: Pos::new(2, 1, 69), volume: inside.volume }]);
    }

    #[test]
    fn a_hole_lets_the_outside_in_unless_it_is_sealed() {
        let mut e = Entity::new();
        shell(&mut e, Pos::new(0, 0, 0), Pos::new(80, 10, 10));
        // A port two blocks wide in the back wall.
        e.set(Pos::new(40, 5, 0), Block::AIR);
        e.set(Pos::new(41, 5, 0), Block::AIR);
        assert_eq!(Interior::of(&e, 0).unwrap().volume, 0);
        let sealed = Interior::of(&e, 1).unwrap();
        assert!(sealed.volume > 79 * 9 * 9 - 20, "{}", sealed.volume);
        assert!(sealed.contains(Pos::new(70, 5, 5)) && !sealed.contains(Pos::new(40, 5, -1)) && !sealed.contains(Pos::new(90, 5, 5)));
        assert_eq!(Interior::find(&e).unwrap().seal, 1, "the narrowest seal that closes the port");
        e.set(Pos::new(40, 5, 0), hull());
        e.set(Pos::new(41, 5, 0), hull());
        assert_eq!(Interior::find(&e).unwrap().seal, 0, "a closed shell needs none");
    }

    #[test]
    fn layers_count_the_enclosed_cells_of_a_box_along_an_axis() {
        let mut e = Entity::new();
        shell(&mut e, Pos::new(0, 0, 0), Pos::new(6, 4, 9));
        let inside = Interior::of(&e, 0).unwrap();
        let layers = inside.layers(Pos::new(0, 0, 7), Pos::new(3, 9, 99), 2);
        assert_eq!(layers.iter().map(|l| (l.min.z, l.volume)).collect::<Vec<_>>(), vec![(7, 9), (8, 9)]);
        assert_eq!((layers[0].min, layers[0].max), (Pos::new(1, 1, 7), Pos::new(3, 3, 7)));
    }

    #[test]
    fn separate_rooms_are_separate_spaces() {
        let mut e = Entity::new();
        shell(&mut e, Pos::new(0, 0, 0), Pos::new(10, 4, 20));
        for x in 0..=10 {
            for y in 0..=4 {
                e.set(Pos::new(x, y, 8), hull());
            }
        }
        let spaces = Interior::of(&e, 0).unwrap().spaces();
        assert_eq!(spaces.iter().map(|s| s.volume).collect::<Vec<_>>(), vec![9 * 3 * 11, 9 * 3 * 7]);
        assert_eq!((spaces[1].min, spaces[1].max), (Pos::new(1, 1, 1), Pos::new(9, 3, 7)));
    }

    #[test]
    fn runs_fill_across_words_both_ways() {
        let free = vec![u64::MAX << 60, u64::MAX, 0b111];
        let seeds = vec![0, 1 << 30, 0];
        assert_eq!(fill_runs(&seeds, &free), free);
        assert_eq!(fill_runs(&[0, 0, 0], &free), vec![0, 0, 0]);
    }
}
