//! Finding the block under the pointer.

use crate::entity::{Bounds, Entity, Pos};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub pos: Pos,
    /// The side of the block the ray came in through, as a step away from the
    /// block. All zero if the ray started inside it.
    pub normal: [i32; 3],
    pub distance: f32,
}

impl Hit {
    /// The empty cell in front of the face that was hit: where a new block goes.
    pub fn outside(&self) -> Pos {
        self.pos.offset(self.normal[0], self.normal[1], self.normal[2])
    }
}

/// Walks a ray through the block grid and returns the first block it meets.
/// `direction` need not be normalised. Blocks are centred on integer positions.
///
/// `within` limits the search to a box (normally the entity's bounds), which
/// keeps the walk short however far away the camera is.
pub fn raycast(entity: &Entity, origin: [f32; 3], direction: [f32; 3], within: Bounds) -> Option<Hit> {
    let len = (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    let dir = [direction[0] / len, direction[1] / len, direction[2] / len];
    // Cell coordinates: block p covers [p, p + 1) after shifting by half a block.
    let start = [origin[0] + 0.5, origin[1] + 0.5, origin[2] + 0.5];
    let lo = [within.min.x as f32, within.min.y as f32, within.min.z as f32];
    let hi = [within.max.x as f32 + 1.0, within.max.y as f32 + 1.0, within.max.z as f32 + 1.0];

    // Clip the ray to the box (slab method).
    let (mut t_in, mut t_out) = (0.0f32, f32::INFINITY);
    let mut entry_axis = None;
    for a in 0..3 {
        if dir[a].abs() < 1e-9 {
            if start[a] < lo[a] || start[a] >= hi[a] {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((lo[a] - start[a]) / dir[a], (hi[a] - start[a]) / dir[a]);
        let (near, far) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
        if near > t_in {
            t_in = near;
            entry_axis = Some(a);
        }
        t_out = t_out.min(far);
    }
    if t_in > t_out {
        return None;
    }

    let limits = [(within.min.x, within.max.x), (within.min.y, within.max.y), (within.min.z, within.max.z)];
    let mut cell = [0i32; 3];
    let mut step = [0i32; 3];
    let mut t_next = [f32::INFINITY; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for a in 0..3 {
        let at = start[a] + dir[a] * t_in;
        // Entering exactly on a face can land a hair outside; clamp into the box.
        cell[a] = (at.floor() as i32).clamp(limits[a].0, limits[a].1);
        if dir[a] > 1e-9 {
            step[a] = 1;
            t_delta[a] = 1.0 / dir[a];
            t_next[a] = (cell[a] as f32 + 1.0 - start[a]) / dir[a];
        } else if dir[a] < -1e-9 {
            step[a] = -1;
            t_delta[a] = -1.0 / dir[a];
            t_next[a] = (cell[a] as f32 - start[a]) / dir[a];
        }
    }
    let mut normal = [0i32; 3];
    if let Some(a) = entry_axis {
        normal[a] = -step[a];
    }
    let mut t = t_in;
    loop {
        let pos = Pos::new(cell[0], cell[1], cell[2]);
        if !entity.get(pos).is_air() {
            return Some(Hit { pos, normal, distance: t });
        }
        let a = if t_next[0] <= t_next[1] && t_next[0] <= t_next[2] {
            0
        } else if t_next[1] <= t_next[2] {
            1
        } else {
            2
        };
        t = t_next[a];
        cell[a] += step[a];
        if t > t_out || cell[a] < limits[a].0 || cell[a] > limits[a].1 {
            return None;
        }
        t_next[a] += t_delta[a];
        normal = [0; 3];
        normal[a] = -step[a];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    fn scene() -> (Entity, Bounds) {
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        e.set(Pos::new(0, 0, 5), Block::new(5, 0, false));
        e.set(Pos::new(-20, 3, 5), Block::new(5, 0, false));
        let b = e.bounds().unwrap();
        (e, b)
    }

    #[test]
    fn hits_the_nearest_block_and_reports_the_face() {
        let (e, b) = scene();
        // From far in front, looking back along -Z: the block at z = 5 is first.
        let hit = raycast(&e, [0.1, 0.2, 100.0], [0.0, 0.0, -1.0], b).unwrap();
        assert_eq!(hit.pos, Pos::new(0, 0, 5));
        assert_eq!(hit.normal, [0, 0, 1]);
        assert!((hit.distance - 94.5).abs() < 1e-3);
        assert_eq!(hit.outside(), Pos::new(0, 0, 6));
        // From below the core, looking up.
        let hit = raycast(&e, [0.0, -30.0, 0.0], [0.0, 2.0, 0.0], b).unwrap();
        assert_eq!((hit.pos, hit.normal), (Pos::new(0, 0, 0), [0, -1, 0]));
        // From the left, slightly diagonal, across a segment border.
        let hit = raycast(&e, [-60.0, 3.2, 5.1], [1.0, 0.001, 0.0], b).unwrap();
        assert_eq!((hit.pos, hit.normal), (Pos::new(-20, 3, 5), [-1, 0, 0]));
    }

    #[test]
    fn misses_return_none() {
        let (e, b) = scene();
        assert_eq!(raycast(&e, [3.0, 0.0, 100.0], [0.0, 0.0, -1.0], b), None);
        assert_eq!(raycast(&e, [0.0, 0.0, 100.0], [0.0, 0.0, 1.0], b), None, "pointing away");
        assert_eq!(raycast(&e, [0.0, 0.0, 100.0], [0.0, 0.0, 0.0], b), None);
        // Parallel to the box but outside it.
        assert_eq!(raycast(&e, [0.0, 50.0, 100.0], [0.0, 0.0, -1.0], b), None);
    }

    #[test]
    fn a_ray_starting_inside_a_block_hits_it_without_a_face() {
        let (e, b) = scene();
        let hit = raycast(&e, [0.2, 0.1, -0.3], [1.0, 0.0, 0.0], b).unwrap();
        assert_eq!((hit.pos, hit.normal), (Pos::new(0, 0, 0), [0, 0, 0]));
    }

    #[test]
    fn grazing_rays_and_edges_do_not_escape_the_box() {
        let (e, b) = scene();
        // Exactly along a cell boundary plane and exactly through a corner.
        for origin in [[0.5, 0.5, 100.0], [-0.5, -0.5, 100.0], [0.5, 0.0, 100.0]] {
            let _ = raycast(&e, origin, [0.0, 0.0, -1.0], b);
        }
        let hit = raycast(&e, [10.0, 10.0, 10.0], [-1.0, -1.0, -1.0], b).unwrap();
        assert_eq!(hit.pos, Pos::new(0, 0, 0));
    }
}
