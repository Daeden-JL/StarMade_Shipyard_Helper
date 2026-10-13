//! How a docked entity (a turret, a rail-docked part) sits on its parent.
//!
//! The parent's meta file holds, for each docked entity, the rail block on
//! the parent, the docker block on the child, and two transforms: the turn a
//! turret axis has given the child and where a rail has carried it. The game
//! places the child from those (`RailRelation.getBlockTransform`), and so
//! does this: the docker comes to rest in the cell in front of the rail
//! block's arrow side, arrow side to arrow side, arrows pointing the same way.

use crate::entity::{Pos, CORE_OFFSET};
use crate::shape_data::RAIL_BASIS;
use crate::tag::{self, Tag};
use crate::{Error, Result};

/// Three rows; a point is turned by taking row . point on each axis.
pub type Turn = [[f32; 3]; 3];

pub const NO_TURN: Turn = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The anchor a shipyard holds a design by. What is docked to it sits in the
/// anchor's own cell, not in front of it.
const SHIPYARD_ANCHOR: u16 = 679;
pub const TURRET_AXIS: u16 = 665;
pub const ROTATORS: [u16; 2] = [664, 669];

pub fn times(a: &Turn, b: &Turn) -> Turn {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn turned(turn: &Turn, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| (0..3).map(|k| turn[i][k] * v[k]).sum())
}

fn transposed(t: &Turn) -> Turn {
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| t[j][i]))
}

/// The turn that takes a 24-way block's own frame (up is its arrow side,
/// forward its arrow) to where its orientation value points them.
fn frame(orientation: u8) -> Turn {
    RAIL_BASIS[orientation as usize % 24].map(|row| row.map(f32::from))
}

/// The orientation value of the same block facing the opposite way with its
/// arrow unchanged: values come in groups of four per side, sides in pairs.
pub(crate) fn facing_back(orientation: u8) -> u8 {
    let o = orientation % 24;
    ((o / 4) ^ 1) * 4 + o % 4
}

/// A turn about an axis by an angle in radians (right-handed).
pub fn about(axis: [f32; 3], angle: f32) -> Turn {
    let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt().max(1e-9);
    let (x, y, z) = (axis[0] / length, axis[1] / length, axis[2] / length);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Where the blocks of one entity are in another's coordinates:
/// there = turn * here + shift. Block centres are whole numbers on both sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub turn: Turn,
    pub shift: [f32; 3],
}

impl Placement {
    pub const SAME: Placement = Placement { turn: NO_TURN, shift: [0.0; 3] };

    pub fn at(&self, p: [f32; 3]) -> [f32; 3] {
        let t = turned(&self.turn, p);
        [t[0] + self.shift[0], t[1] + self.shift[1], t[2] + self.shift[2]]
    }

    /// The placement of something placed by `inner` inside what `self` places.
    pub fn then(&self, inner: &Placement) -> Placement {
        Placement { turn: times(&self.turn, &inner.turn), shift: self.at(inner.shift) }
    }

    /// The way back: here = turn * there + shift. Turns have no stretch, so
    /// the transposed turn undoes one.
    pub fn back(&self) -> Placement {
        let turn = transposed(&self.turn);
        let shift = turned(&turn, self.shift).map(|c| -c);
        Placement { turn, shift }
    }

    /// The cell a block lands in, if the placement keeps blocks on the grid
    /// (quarter turns, whole steps), as every dock at rest does.
    pub fn cell(&self, p: Pos) -> Option<Pos> {
        let at = self.at([p.x as f32, p.y as f32, p.z as f32]);
        let near = at.map(f32::round);
        (0..3).all(|i| (at[i] - near[i]).abs() < 1e-3).then(|| Pos::new(near[0] as i32, near[1] as i32, near[2] as i32))
    }

    /// As a column-major 4x4, the form the viewport's shader takes.
    pub fn matrix(&self) -> [f32; 16] {
        let (t, s) = (&self.turn, &self.shift);
        [t[0][0], t[1][0], t[2][0], 0.0, t[0][1], t[1][1], t[2][1], 0.0, t[0][2], t[1][2], t[2][2], 0.0, s[0], s[1], s[2], 1.0]
    }
}

/// One block as a dock record names it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// In design space: the entity's core is the origin.
    pub pos: Pos,
    pub ty: u16,
    pub orientation: u8,
}

/// How one entity is docked to its parent, as the blueprint records it.
#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    /// The rail block on the parent.
    pub rail: Piece,
    /// The docker block on the docked entity.
    pub docker: Piece,
    /// The turn a turret axis has given the entity, about its docker.
    pub turret: Turn,
    /// Where a rail or rotator has carried it: a turn and a shift in the
    /// parent's coordinates.
    pub moved: Placement,
    /// The rotator it sits on has given it its turn already and will not
    /// turn it again until something arms it anew.
    pub rotated: bool,
}

/// Where a rail can carry a docked entity from where it is, as shifts in the
/// parent's coordinates: the cells behind it, nearest first, and ahead of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub back: Vec<[i32; 3]>,
    pub ahead: Vec<[i32; 3]>,
}

impl Track {
    /// The shift for a place along the track: 0 is where the entity is,
    /// whole numbers are cells ahead (or, below zero, behind), and anything
    /// between is on the way from one cell to the next.
    pub fn shift(&self, along: f32) -> [f32; 3] {
        let cell = |k: i32| -> [f32; 3] {
            let list = if k < 0 { &self.back } else { &self.ahead };
            match k.unsigned_abs() as usize {
                0 => [0.0; 3],
                n => list.get(n - 1).or(list.last()).map_or([0.0; 3], |c| c.map(|v| v as f32)),
            }
        };
        let along = along.clamp(-(self.back.len() as f32), self.ahead.len() as f32);
        let (from, part) = (along.floor(), along - along.floor());
        let (a, b) = (cell(from as i32), cell(from as i32 + 1));
        [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * part)
    }
}

/// Most cells of track followed either way.
const MAX_TRACK: usize = 512;

/// How a rail block lets what is docked to it move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// A turret axis: turns freely about the axis through the rail block.
    Turret,
    /// A rotator: quarter turns, one way.
    Rotator { clockwise: bool },
    /// Plain rail and the like: carried along the track.
    Rail,
    /// A shipyard anchor: held still.
    Fixed,
}

impl Dock {
    /// Where the docked entity's blocks are in the parent's coordinates, with
    /// the turret turn and rail movement the blueprint was saved with.
    pub fn placement(&self) -> Placement {
        self.posed(&self.turret, &self.moved)
    }

    /// The placement with every moving part at rest.
    pub fn at_rest(&self) -> Placement {
        self.posed(&NO_TURN, &Placement::SAME)
    }

    /// The placement for a given turret turn and rail movement.
    pub fn posed(&self, turret: &Turn, moved: &Placement) -> Placement {
        // A ship docked by its core rather than a docker is taken as it stands.
        let docker_frame = if self.docker.ty == crate::edit::CORE_TYPE { NO_TURN } else { frame(facing_back(self.docker.orientation)) };
        let at_rest = times(&frame(self.rail.orientation), &transposed(&docker_frame));
        let turn = times(&times(&moved.turn, &at_rest), turret);
        let (up, _) = crate::shape::turned_axes(self.rail.orientation);
        let ahead = if self.rail.ty == SHIPYARD_ANCHOR { [0; 3] } else { up };
        let docker = turned(&turn, [self.docker.pos.x as f32, self.docker.pos.y as f32, self.docker.pos.z as f32]);
        let rail = self.rail.pos.to_array();
        Placement { turn, shift: [0, 1, 2].map(|i| (rail[i] + ahead[i]) as f32 + moved.shift[i] - docker[i]) }
    }

    /// The direction, in the parent's coordinates, the rail block's arrow
    /// side faces: the axis a turret axis or rotator turns about.
    pub fn axis(&self) -> [f32; 3] {
        crate::shape::turned_axes(self.rail.orientation).0.map(|c| c as f32)
    }

    /// Where the rail can carry the entity from where it is. The game moves
    /// what sits on a rail one block at a time along that block's arrow, for
    /// as long as the next block is track facing the same side; the entity
    /// keeps its own turn round corners. Behind it are the blocks whose
    /// arrows lead here. Read from the parent's blocks as they are now, since
    /// logic may have turned a rail since the entity docked. `is_track` says
    /// which block types are rail track.
    pub fn track(&self, parent: &crate::Entity, is_track: impl Fn(u16) -> bool) -> Track {
        let up = crate::shape::turned_axes(self.rail.orientation).0;
        let here = self.rail.pos.to_array();
        let on = [0, 1, 2].map(|i| here[i] + self.moved.shift[i].round() as i32);
        // The arrow of the track block in a cell, if there is one facing the same side.
        let arrow_at = |cell: [i32; 3]| {
            let b = parent.get(Pos::new(cell[0], cell[1], cell[2]));
            let (side, arrow) = crate::shape::turned_axes(b.orientation());
            (is_track(b.ty()) && side == up).then_some(arrow)
        };
        let mut track = Track::default();
        if self.motion() != Motion::Rail || self.moved.turn != NO_TURN {
            return track;
        }
        let Some(mut arrow) = arrow_at(on) else { return track };
        let mut seen = vec![on];
        let mut at = on;
        while track.ahead.len() < MAX_TRACK {
            let next = [at[0] + arrow[0], at[1] + arrow[1], at[2] + arrow[2]];
            let (Some(onward), false) = (arrow_at(next), seen.contains(&next)) else { break };
            track.ahead.push([0, 1, 2].map(|i| next[i] - on[i]));
            seen.push(next);
            (at, arrow) = (next, onward);
        }
        at = on;
        while track.back.len() < MAX_TRACK {
            // The neighbour whose arrow points at this cell.
            let from = crate::shape::SIDE_DIRS.iter().map(|d| [at[0] - d[0], at[1] - d[1], at[2] - d[2]]).find(|cell| {
                !seen.contains(cell) && arrow_at(*cell).is_some_and(|a| [cell[0] + a[0], cell[1] + a[1], cell[2] + a[2]] == at)
            });
            let Some(from) = from else { break };
            track.back.push([0, 1, 2].map(|i| from[i] - on[i]));
            seen.push(from);
            at = from;
        }
        track
    }

    /// The placement after the docked entity has moved on from how it was
    /// saved: `value` is degrees for a turret axis or a rotator, cells along
    /// `track` for what rides a rail.
    pub fn moved_on(&self, value: f32, track: &Track) -> Placement {
        match self.motion() {
            // A turret turns about its docker's arrow-side axis, in its own coordinates.
            Motion::Turret => {
                let axis = crate::shape::turned_axes(self.docker.orientation).0.map(|c| c as f32);
                self.posed(&times(&self.turret, &about(axis, value.to_radians())), &self.moved)
            }
            // A rotator turns what it carries about its own axis, in the parent's.
            Motion::Rotator { clockwise } => {
                let angle = if clockwise { -value } else { value }.to_radians();
                let moved = Placement { turn: times(&about(self.axis(), angle), &self.moved.turn), shift: self.moved.shift };
                self.posed(&self.turret, &moved)
            }
            Motion::Rail => {
                let carried = track.shift(value);
                let shift = [0, 1, 2].map(|i| self.moved.shift[i] + carried[i]);
                self.posed(&self.turret, &Placement { turn: self.moved.turn, shift })
            }
            Motion::Fixed => self.placement(),
        }
    }

    pub fn motion(&self) -> Motion {
        // A turret turns about its docker's arrow-side axis, and the game
        // only knows the upright and the sideways one: a docker facing front
        // or back makes a turret that never turns.
        let docker_faces_along_z = crate::shape::turned_axes(self.docker.orientation).0[2] != 0;
        match self.rail.ty {
            TURRET_AXIS if docker_faces_along_z => Motion::Fixed,
            TURRET_AXIS => Motion::Turret,
            ty if ROTATORS.contains(&ty) => Motion::Rotator { clockwise: ty == ROTATORS[0] },
            SHIPYARD_ANCHOR => Motion::Fixed,
            _ => Motion::Rail,
        }
    }

    /// Reads the record a parent's meta file keeps for one docked entity.
    /// `meta_version` is that meta file's version: before 4 the positions
    /// count from a core at (8, 8, 8) and orientations are in the old numbering.
    /// None for a record that is not a dock (a pending request to undock).
    pub fn read(record: &[u8], meta_version: i32) -> Result<Option<Dock>> {
        let bad = |what: &str| Error::Malformed { what: "dock record", detail: what.to_string() };
        let tag = tag::read(record)?;
        let top = tag.items().ok_or_else(|| bad("not a structure"))?;
        // 1 is docked, 3 a dock still being made; anything else is no dock.
        if !matches!(top.first().and_then(Tag::int), Some(1 | 3)) {
            return Ok(None);
        }
        let Some(request) = top.get(1).and_then(Tag::items).and_then(|wrap| wrap.first()).and_then(Tag::items) else {
            return Ok(None);
        };
        let shift = if meta_version < 4 { 8 } else { 0 };
        let piece = |tag: Option<&Tag>| -> Result<Piece> {
            let fields = tag.and_then(Tag::items).ok_or_else(|| bad("a block is missing"))?;
            // The current form leads with a byte; the old form starts at the id.
            let (fields, old) = match fields.first() {
                Some(Tag::Byte(_)) => (&fields[1..], false),
                _ => (fields, true),
            };
            let (Some(Tag::Vec3i(p)), Some(ty), Some(o), Some(active)) =
                (fields.get(1), fields.get(2).and_then(Tag::int), fields.get(3).and_then(Tag::int), fields.get(4).and_then(Tag::int))
            else {
                return Err(bad("a block is incomplete"));
            };
            // Old records hold the orientation as it was before the game
            // moved the fifth orientation bit out of the "active" bit.
            let orientation = if old { (o + if active > 0 { 0 } else { 16 }).rem_euclid(24) } else { o.rem_euclid(32) };
            Ok(Piece {
                pos: Pos::new(p[0] + shift - CORE_OFFSET, p[1] + shift - CORE_OFFSET, p[2] + shift - CORE_OFFSET),
                ty: ty as u16,
                orientation: orientation as u8,
            })
        };
        let placement = |tag: Option<&Tag>| -> Placement {
            match tag {
                Some(Tag::Matrix4(m)) => Placement {
                    turn: [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]],
                    shift: [m[3], m[7], m[11]],
                },
                _ => Placement::SAME,
            }
        };
        let finite = |p: &Placement| p.turn.iter().flatten().chain(p.shift.iter()).all(|c| c.is_finite());
        // Of the two rail transforms the game keeps, it is the one "at the
        // dock" it starts from when the blueprint is loaded: an entity saved
        // halfway between two rail blocks is back on the one it left.
        let at_dock = request.get(5).filter(|t| matches!(t, Tag::Matrix4(_))).or(request.get(3));
        let (turret, moved) = (placement(request.get(2)), placement(at_dock));
        if !finite(&turret) || !finite(&moved) {
            return Err(bad("a transform that is not a number"));
        }
        // After the transforms: where it is going (or a 0 byte for nowhere),
        // then whether its rotator has turned it.
        let rotated = request.get(7).and_then(Tag::int).is_some_and(|flag| flag != 0);
        Ok(Some(Dock { rail: piece(request.first())?, docker: piece(request.get(1))?, turret: turret.turn, moved, rotated }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dock(rail: (i32, i32, i32), rail_turn: u8, docker: (i32, i32, i32), docker_turn: u8) -> Dock {
        Dock {
            rail: Piece { pos: Pos::new(rail.0, rail.1, rail.2), ty: TURRET_AXIS, orientation: rail_turn },
            docker: Piece { pos: Pos::new(docker.0, docker.1, docker.2), ty: 663, orientation: docker_turn },
            turret: NO_TURN,
            moved: Placement::SAME,
            rotated: false,
        }
    }

    #[test]
    fn the_docker_rests_in_front_of_the_rails_arrow_side_arrows_aligned() {
        // A turret axis on top of the hull, arrow side up (14). The turret's
        // docker is under its core, arrow side down, arrow forward (10): the
        // commonest turret there is.
        let d = dock((0, 3, 5), 14, (0, -1, 0), 10);
        let p = d.placement();
        assert_eq!(p.cell(d.docker.pos), Some(Pos::new(0, 4, 5)), "the docker sits on the rail block");
        assert_eq!(p.cell(Pos::new(0, 0, 0)), Some(Pos::new(0, 5, 5)), "and the turret's core above it");
        assert_eq!(p.turn, NO_TURN, "nothing needs turning: the arrows already agree");
        // The same turret on a rail that faces +X instead: it lies on its side.
        let side = dock((2, 0, 0), 20, (0, -1, 0), 10).placement();
        assert_eq!(side.cell(Pos::new(0, -1, 0)), Some(Pos::new(3, 0, 0)));
        assert_eq!(side.cell(Pos::new(0, 0, 0)), Some(Pos::new(4, 0, 0)), "its up now points along +X");
        // Every pair of orientations: docker in the cell the rail faces,
        // and the child's cell behind the docker further out the same way.
        for rail_turn in 0..24u8 {
            for docker_turn in 0..24u8 {
                let d = dock((5, -2, 7), rail_turn, (1, 2, 3), docker_turn);
                let p = d.placement();
                let (up, forward) = crate::shape::turned_axes(rail_turn);
                assert_eq!(p.cell(d.docker.pos), Some(d.rail.pos.offset(up[0], up[1], up[2])), "{rail_turn} {docker_turn}");
                let (docker_up, docker_forward) = crate::shape::turned_axes(docker_turn);
                // The docker's arrow side faces the rail, and its arrow points with the rail's.
                assert_eq!(turned(&p.turn, docker_up.map(|c| c as f32)).map(|c| c.round() as i32), up.map(|c| -c));
                assert_eq!(turned(&p.turn, docker_forward.map(|c| c as f32)).map(|c| c.round() as i32), forward);
                // And back again.
                let back = p.back().at(p.at([4.0, 5.0, 6.0]));
                assert!(back.iter().zip([4.0, 5.0, 6.0]).all(|(a, b)| (a - b).abs() < 1e-4));
            }
        }
    }

    #[test]
    fn a_turret_turn_swings_the_entity_about_its_docker() {
        let mut d = dock((0, 3, 5), 14, (0, -1, 0), 10);
        // A quarter turn about the child's own up axis.
        d.turret = about([0.0, 1.0, 0.0], std::f32::consts::FRAC_PI_2);
        let p = d.placement();
        assert_eq!(p.cell(d.docker.pos), Some(Pos::new(0, 4, 5)), "the docker stays where it is");
        // The barrel that pointed forward (+Z) now points along +X.
        assert_eq!(p.cell(Pos::new(0, 0, 3)), Some(Pos::new(3, 5, 5)));
        // Halfway, blocks are off the grid.
        d.turret = about([0.0, 1.0, 0.0], 0.4);
        assert_eq!(d.placement().cell(Pos::new(0, 0, 3)), None);
        assert_eq!(d.at_rest().turn, NO_TURN);
        assert_eq!(d.axis(), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn docked_entities_move_the_way_their_rail_block_moves_them() {
        use crate::{Block, Entity};
        // A turret: a quarter turn swings the barrel, the docker stays.
        let turret = dock((0, 3, 5), 14, (0, -1, 0), 10);
        let nowhere = Track::default();
        assert_eq!(turret.moved_on(0.0, &nowhere), turret.placement());
        let swung = turret.moved_on(90.0, &nowhere);
        assert_eq!(swung.cell(turret.docker.pos), Some(Pos::new(0, 4, 5)));
        assert_ne!(swung.cell(Pos::new(0, 0, 3)), turret.placement().cell(Pos::new(0, 0, 3)));
        assert_eq!(swung.cell(Pos::new(0, 0, 3)).map(|p| p.y), Some(5), "it turns about the upright axis");
        // A rotator: the two kinds turn opposite ways, both about their own axis.
        let mut cw = dock((0, 3, 5), 14, (0, -1, 0), 10);
        cw.rail.ty = ROTATORS[0];
        let mut ccw = cw.clone();
        ccw.rail.ty = ROTATORS[1];
        assert_eq!(cw.motion(), Motion::Rotator { clockwise: true });
        assert_eq!(cw.moved_on(90.0, &nowhere).cell(Pos::new(0, 0, 3)), ccw.moved_on(-90.0, &nowhere).cell(Pos::new(0, 0, 3)));
        assert_eq!(cw.moved_on(90.0, &nowhere).cell(cw.docker.pos), Some(Pos::new(0, 4, 5)));
        // Seen from the docked side, the clockwise one turns clockwise: what
        // pointed forward (+Z) comes to point toward -X... which, with +X on
        // the left in this game, is the right-hand side.
        assert_eq!(cw.moved_on(90.0, &nowhere).cell(Pos::new(0, 0, 3)), Some(Pos::new(-3, 5, 5)));
        // Rail: five rail blocks along Z with their arrows forward, the
        // entity on the second, and a corner at the end: two more toward +X.
        let mut rail = dock((0, 3, 5), 14, (0, -1, 0), 10);
        rail.rail.ty = 662;
        let mut parent = Entity::new();
        for z in 4..=7 {
            parent.set(Pos::new(0, 3, z), Block::new(662, 14, false));
        }
        parent.set(Pos::new(0, 3, 8), Block::new(662, 13, false));
        parent.set(Pos::new(1, 3, 8), Block::new(662, 13, false));
        parent.set(Pos::new(2, 3, 8), Block::new(662, 13, false));
        // A piece that faces another side does not carry the run on.
        parent.set(Pos::new(3, 3, 8), Block::new(662, 0, false));
        let track = rail.track(&parent, |ty| ty == 662);
        assert_eq!(track.back, vec![[0, 0, -1]]);
        assert_eq!(track.ahead, vec![[0, 0, 1], [0, 0, 2], [0, 0, 3], [1, 0, 3], [2, 0, 3]]);
        assert_eq!(rail.moved_on(3.0, &track).cell(rail.docker.pos), Some(Pos::new(0, 4, 8)));
        // Round the corner it is carried sideways, still facing the way it did.
        let far = rail.moved_on(5.0, &track);
        assert_eq!(far.cell(rail.docker.pos), Some(Pos::new(2, 4, 8)));
        assert_eq!(far.turn, rail.placement().turn);
        assert_eq!(rail.moved_on(-1.0, &track).cell(Pos::new(0, 0, 0)), Some(Pos::new(0, 5, 4)));
        // Halfway between two cells, and no further than the track goes.
        assert_eq!(track.shift(0.5), [0.0, 0.0, 0.5]);
        assert_eq!(track.shift(3.5), [0.5, 0.0, 3.0]);
        assert_eq!((track.shift(99.0), track.shift(-99.0)), ([2.0, 0.0, 3.0], [0.0, 0.0, -1.0]));
        // A rail that logic has turned round carries the other way.
        for z in 4..=7 {
            parent.set(Pos::new(0, 3, z), Block::new(662, 12, false));
        }
        let turned_round = rail.track(&parent, |ty| ty == 662);
        assert_eq!((turned_round.ahead.clone(), turned_round.back.len()), (vec![[0, 0, -1]], 2));
        // Something that is not on track (the record is stale) does not move.
        assert_eq!(dock((9, 9, 9), 14, (0, -1, 0), 10).track(&parent, |ty| ty == 662), Track::default());
        assert_eq!(turret.track(&parent, |ty| ty == 662), Track::default(), "a turret axis is not a track");
        // A turret whose docker faces front or back never turns.
        assert_eq!(dock((0, 0, 5), 0, (0, 0, -1), 4).motion(), Motion::Fixed);
        assert_eq!(dock((0, 0, 5), 14, (0, -1, 0), 20).motion(), Motion::Turret);
    }

    #[test]
    fn placements_chain_and_fill_a_matrix_column_by_column() {
        let base = dock((0, 3, 5), 14, (0, -1, 0), 10).placement();
        let barrel = dock((0, 0, 1), 0, (0, 0, -1), 4).placement();
        let both = base.then(&barrel);
        let direct = base.at(barrel.at([1.0, 2.0, 3.0]));
        assert_eq!(both.at([1.0, 2.0, 3.0]), direct);
        let m = base.matrix();
        assert_eq!(&m[12..15], &base.shift);
        assert_eq!((m[0], m[5], m[10], m[15]), (1.0, 1.0, 1.0, 1.0));
        assert_eq!(dock((0, 0, 0), 14, (0, 0, 0), 10).motion(), Motion::Turret);
    }
}
