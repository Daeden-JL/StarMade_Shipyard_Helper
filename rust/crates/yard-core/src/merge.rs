//! A piece (blocks made from another 3D model, or any other entity) fitted
//! into an entity: moved, turned in quarter turns and mirrored, then its
//! blocks written into the entity's cells.
//!
//! The browser draws the piece where a [`Fit`] puts it while the player
//! moves it about, and the server writes it when it is locked in; both work
//! out the cells here, so what is locked in is what was shown.

use serde::{Deserialize, Serialize};

use crate::catalog::Shape;
use crate::shape::{turn_step, GridTurn, ShapeSet, NO_GRID_TURN};
use crate::{Block, Entity, Pos};

/// Where a piece goes: mirrored left to right (in its own coordinates)
/// if asked, turned by quarter turns about Y (`yaw`), then X (`pitch`),
/// then Z (`roll`), each applied last first, and its origin put at `at`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fit {
    pub at: [i32; 3],
    #[serde(default)]
    pub yaw: u8,
    #[serde(default)]
    pub pitch: u8,
    #[serde(default)]
    pub roll: u8,
    #[serde(default)]
    pub mirror: bool,
}

/// A quarter turn about Y: +Z (front) goes to +X (right).
const YAW: GridTurn = [[0, 0, 1], [0, 1, 0], [-1, 0, 0]];
/// A quarter turn about X: +Z (front) goes to -Y (down), nose down.
const PITCH: GridTurn = [[1, 0, 0], [0, 0, -1], [0, 1, 0]];
/// A quarter turn about Z: +X (right) goes to +Y (up).
const ROLL: GridTurn = [[0, -1, 0], [1, 0, 0], [0, 0, 1]];

fn times(a: &GridTurn, b: &GridTurn) -> GridTurn {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn power(t: &GridTurn, n: u8) -> GridTurn {
    (0..n % 4).fold(NO_GRID_TURN, |all, _| times(t, &all))
}

impl Fit {
    /// The turn, without the mirror.
    pub fn turn(&self) -> GridTurn {
        times(&power(&YAW, self.yaw), &times(&power(&PITCH, self.pitch), &power(&ROLL, self.roll)))
    }

    /// The cell a cell of the piece lands in.
    pub fn cell(&self, p: Pos) -> Pos {
        let mirrored = if self.mirror { [-p.x, p.y, p.z] } else { [p.x, p.y, p.z] };
        let t = turn_step(&self.turn(), mirrored);
        Pos::new(t[0] + self.at[0], t[1] + self.at[1], t[2] + self.at[2])
    }

    /// Where a piece already mirrored (see [`mirrored`]) is drawn: the turn
    /// and the shift, without the mirror, which would turn its faces inside out.
    pub fn placement(&self) -> crate::dock::Placement {
        let t = self.turn();
        crate::dock::Placement { turn: t.map(|row| row.map(|v| v as f32)), shift: self.at.map(|v| v as f32) }
    }
}

/// What turning and mirroring need to know of a block type: its shape, and
/// whether it has a distinct front (a slab, or a cube with six textures).
pub type ShapeOf<'a> = &'a dyn Fn(u16) -> (Shape, bool);

/// A block mirrored left to right.
fn mirror_block(b: Block, shape_of: ShapeOf) -> Block {
    let (shape, faces) = shape_of(b.ty());
    let mut out = b;
    out.set_orientation(ShapeSet::mirrored(shape, b.orientation(), [true, false, false], faces));
    out
}

/// The piece mirrored left to right, block shapes and all: what the browser
/// draws while the mirror is on.
pub fn mirrored(piece: &Entity, shape_of: ShapeOf) -> Entity {
    let mut out = Entity::new();
    piece.for_each(|p, b| out.set(Pos::new(-p.x, p.y, p.z), mirror_block(b, shape_of)));
    out
}

/// The blocks of the piece where `fit` puts them, each turned with it.
pub fn placed(piece: &Entity, fit: &Fit, shape_of: ShapeOf) -> Vec<(Pos, Block)> {
    let turn = fit.turn();
    let mut out = Vec::with_capacity(piece.block_count() as usize);
    piece.for_each(|p, b| {
        let b = if fit.mirror { mirror_block(b, shape_of) } else { b };
        let (shape, faces) = shape_of(b.ty());
        let mut turned = b;
        turned.set_orientation(ShapeSet::turned(shape, b.orientation(), faces, &turn));
        out.push((fit.cell(p), turned));
    });
    out
}

/// The smallest and largest cell the piece covers where `fit` puts it.
pub fn placed_bounds(piece: &Entity, fit: &Fit) -> Option<(Pos, Pos)> {
    let b = piece.bounds()?;
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for x in [b.min.x, b.max.x] {
        for y in [b.min.y, b.max.y] {
            for z in [b.min.z, b.max.z] {
                let c = fit.cell(Pos::new(x, y, z));
                for (i, v) in [c.x, c.y, c.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(v);
                    hi[i] = hi[i].max(v);
                }
            }
        }
    }
    Some((Pos::new(lo[0], lo[1], lo[2]), Pos::new(hi[0], hi[1], hi[2])))
}

/// What a blueprint gives when one of its entities is taken as a piece.
pub struct Taken {
    /// The entity's blocks and links, without a ship core.
    pub piece: Entity,
    /// Whether it had a ship core at its origin, which was left out: a
    /// design has one core, and a station none.
    pub core_left_out: bool,
    /// How many entities were docked to it (turrets, docked ships...), which
    /// do not come with it.
    pub docked_left_out: usize,
}

/// The entity of `bp` at `path` ("" for the main one) as a piece.
pub fn take(bp: &crate::Blueprint, path: &str) -> Option<Taken> {
    fn docked(e: &crate::blueprint::BlueprintEntity) -> usize {
        e.children.iter().map(|c| 1 + docked(c)).sum()
    }
    let whole = bp.entity(path)?;
    let mut piece = whole.entity.clone();
    let origin = Pos::new(0, 0, 0);
    let core_left_out = piece.get(origin).ty() == crate::edit::CORE_TYPE;
    if core_left_out {
        let touching: Vec<(Pos, Pos)> = piece
            .links()
            .iter()
            .flat_map(|(c, groups)| groups.values().flat_map(move |targets| targets.iter().map(move |t| (*c, *t))))
            .filter(|(c, t)| *c == origin || *t == origin)
            .collect();
        for (c, t) in touching {
            piece.unlink_any(c, t);
        }
        piece.set(origin, Block::AIR);
    }
    Some(Taken { piece, core_left_out, docked_left_out: docked(whole) })
}

/// The piece's links where `fit` puts it: each controller with the blocks
/// it controls, in the cells they land in.
pub fn placed_links(piece: &Entity, fit: &Fit) -> Vec<(Pos, Vec<Pos>)> {
    piece
        .links()
        .iter()
        .map(|(c, groups)| (fit.cell(*c), groups.values().flat_map(|targets| targets.iter().map(|t| fit.cell(*t))).collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(ty: u16) -> (Shape, bool) {
        if ty == 6 {
            (Shape::Wedge, false)
        } else {
            (Shape::Cube, false)
        }
    }

    #[test]
    fn four_quarter_turns_come_back_and_each_turn_keeps_cells_on_the_grid() {
        for t in [YAW, PITCH, ROLL] {
            assert_eq!(power(&t, 4), NO_GRID_TURN);
            let det = t[0][0] * (t[1][1] * t[2][2] - t[1][2] * t[2][1]) - t[0][1] * (t[1][0] * t[2][2] - t[1][2] * t[2][0]) + t[0][2] * (t[1][0] * t[2][1] - t[1][1] * t[2][0]);
            assert_eq!(det, 1, "a turn, not a mirror");
        }
        let fit = Fit { yaw: 1, ..Fit::default() };
        assert_eq!(fit.cell(Pos::new(0, 0, 1)), Pos::new(1, 0, 0), "the front turns to the right");
        let fit = Fit { pitch: 1, ..Fit::default() };
        assert_eq!(fit.cell(Pos::new(0, 0, 1)), Pos::new(0, -1, 0), "nose down");
    }

    #[test]
    fn a_piece_is_mirrored_turned_and_moved_and_its_blocks_turn_with_it() {
        let mut piece = Entity::new();
        piece.set(Pos::new(2, 0, 0), Block::new(5, 0, false));
        piece.set(Pos::new(0, 1, 0), Block::new(6, 0, false));
        let fit = Fit { at: [10, 0, 0], yaw: 2, mirror: true, ..Fit::default() };
        let got = placed(&piece, &fit, &shapes);
        // Mirrored, x 2 goes to -2; half a turn about Y takes it back to 2; then 10 along.
        assert!(got.contains(&(Pos::new(12, 0, 0), Block::new(5, 0, false))), "{got:?}");
        let wedge = got.iter().find(|(p, _)| *p == Pos::new(10, 1, 0)).map(|(_, b)| *b).unwrap();
        let expected = ShapeSet::turned(Shape::Wedge, ShapeSet::mirrored(Shape::Wedge, 0, [true, false, false], false), false, &fit.turn());
        assert_eq!(wedge.orientation(), expected);
        assert_eq!(placed_bounds(&piece, &fit), Some((Pos::new(10, 0, 0), Pos::new(12, 1, 0))));
    }

    #[test]
    fn the_mirrored_copy_drawn_with_the_placement_lands_where_the_blocks_are_written() {
        let mut piece = Entity::new();
        for (x, y, z) in [(1, 0, 0), (0, 2, 0), (0, 0, 3), (-1, -1, 2)] {
            piece.set(Pos::new(x, y, z), Block::new(5, 0, false));
        }
        for yaw in 0..4 {
            for pitch in 0..4 {
                let fit = Fit { at: [3, -4, 5], yaw, pitch, roll: 1, mirror: true };
                let drawn: std::collections::BTreeSet<Pos> = {
                    let copy = mirrored(&piece, &shapes);
                    let mut cells = std::collections::BTreeSet::new();
                    copy.for_each(|p, _| {
                        cells.insert(fit.placement().cell(p).unwrap());
                    });
                    cells
                };
                let written: std::collections::BTreeSet<Pos> = placed(&piece, &fit, &shapes).into_iter().map(|(p, _)| p).collect();
                assert_eq!(drawn, written, "yaw {yaw} pitch {pitch}");
            }
        }
    }

    #[test]
    fn a_blueprint_entity_is_taken_without_its_core_and_its_links_move_with_it() {
        let mut bp = crate::Blueprint::new_ship("Turret section", "test");
        let e = &mut bp.root.entity;
        e.set(Pos::new(0, 0, 1), Block::new(6, 0, false)); // cannon computer
        e.set(Pos::new(0, 0, 2), Block::new(16, 0, false)); // cannon barrel
        e.link(Pos::new(0, 0, 1), Pos::new(0, 0, 2), 16);
        e.link(Pos::new(0, 0, 0), Pos::new(0, 0, 1), 6);
        let taken = take(&bp, "").unwrap();
        assert!(taken.core_left_out);
        assert_eq!(taken.docked_left_out, 0);
        assert!(taken.piece.get(Pos::new(0, 0, 0)).is_air());
        assert_eq!(taken.piece.block_count(), 2);
        let fit = Fit { at: [5, 0, 0], yaw: 1, ..Fit::default() };
        let links = placed_links(&taken.piece, &fit);
        assert_eq!(links, vec![(fit.cell(Pos::new(0, 0, 1)), vec![fit.cell(Pos::new(0, 0, 2))])], "only the computer's link, turned and moved");
        assert!(take(&bp, "ATTACHED_9").is_none());
    }
}
