//! A whole design put together: every docked entity where its dock holds it,
//! and how the parts can move from there.
//!
//! Each docked part has one number that says how far it has moved from how
//! the design was saved: degrees for what sits on a turret axis or a rotator,
//! blocks for what rides a rail. Moving a part carries along everything
//! docked to it, and stops where its blocks would run into another part's,
//! as the game stops it.

use crate::collide::{self, Body, Coarse, Shapes};
use crate::dock::{Dock, Motion, Placement, Track};
use crate::entity::{Bounds, Entity, Pos};
use crate::Blueprint;

/// Furthest a part's outermost block may travel between two checks for a
/// collision, in blocks. Less than a block, so nothing is stepped over.
const STRIDE: f32 = 0.3;
/// Most checks one move makes. A move too long for that takes longer strides.
const MOST_CHECKS: usize = 240;

/// How far a turret swings to either side in the demonstration, in degrees,
/// and how fast, in degrees a second.
pub const TURRET_SWEEP: f32 = 70.0;
pub const TURRET_SPEED: f32 = 50.0;
/// A rotator's quarter turn takes a second, then it rests for one.
pub const ROTATOR_SPEED: f32 = 90.0;
pub const ROTATOR_REST: f32 = 1.0;
/// Blocks a second along a rail.
pub const RAIL_SPEED: f32 = 2.0;

/// Where one part met another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// The part that was moving, or one carried by it, and its block.
    pub mover: usize,
    pub mover_cell: Pos,
    /// The part in the way, and its block.
    pub still: usize,
    pub still_cell: Pos,
}

/// Where a part is in its demonstration.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Demo {
    /// 1 or -1: toward the far end or back.
    heading: f32,
    /// Ends reached so far (turret, rail) or quarter turns made (rotator).
    legs: u32,
    /// Seconds still to wait before moving on.
    rest: f32,
    /// A rotator's next stop, in degrees.
    goal: f32,
    /// A rotator that met something is on its way back to where it came from.
    retreating: bool,
    done: bool,
}

/// One entity of the design.
#[derive(Clone, Debug)]
pub struct Part {
    pub path: String,
    /// How it is docked to its parent; None for the main entity.
    pub dock: Option<Dock>,
    /// Which part it is docked to. The main entity, part 0, names itself.
    pub parent: usize,
    /// Which of the design's entities it is, counting the main entity as 0
    /// and going through the docked ones as [`Blueprint::entities`] does.
    pub entity: usize,
    pub bounds: Option<Bounds>,
    /// Where its rail can carry it.
    pub track: Track,
    /// What of it can run into things. The main entity has none: it never moves.
    body: Option<Body>,
    /// Where it has blocks, coarsely, for what runs into it.
    coarse: Option<Coarse>,
    /// Where it sits now, in the main entity's coordinates.
    pub placed: Placement,
    /// How far it has moved from how it was saved.
    pub value: f32,
    /// Where its rail has carried and turned it, while the simulation of
    /// the game's rails moves it; then `value` does not count.
    pub driven: Option<Placement>,
    /// As saved, it already sits inside another part's blocks. Such a part
    /// would never get free, so it moves unhindered and hinders nothing.
    pub overlapping: bool,
    demo: Demo,
}

impl Part {
    pub fn motion(&self) -> Motion {
        self.dock.as_ref().map_or(Motion::Fixed, Dock::motion)
    }

    /// The least and greatest value it can take: degrees for what turns,
    /// blocks for what slides. Equal where it cannot move.
    pub fn range(&self) -> (f32, f32) {
        match self.motion() {
            Motion::Turret => (-180.0, 180.0),
            Motion::Rotator { .. } => (0.0, 360.0),
            Motion::Rail => (-(self.track.back.len() as f32), self.track.ahead.len() as f32),
            Motion::Fixed => (0.0, 0.0),
        }
    }

    pub fn can_move(&self) -> bool {
        let (lo, hi) = self.range();
        hi > lo
    }
}

/// Every placed entity of a design, parents before what is docked to them.
#[derive(Clone, Debug, Default)]
pub struct Rig {
    pub parts: Vec<Part>,
    /// Docked entities left out: the design has no dock on record for them,
    /// or for what they are docked to.
    pub unplaced: usize,
}

/// What one stretch of the demonstration did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Played {
    /// Where parts were stopped by other parts.
    pub contacts: Vec<Contact>,
    /// Every part has been through its motion once. Never true while looping.
    pub finished: bool,
}

fn toward(from: f32, to: f32, most: f32) -> f32 {
    if (to - from).abs() <= most {
        to
    } else {
        from + most * (to - from).signum()
    }
}

impl Rig {
    /// Puts a design together as it was saved. `is_track` says which block
    /// types are rail track.
    pub fn new(bp: &Blueprint, is_track: impl Fn(u16) -> bool, shapes: &dyn Shapes) -> Rig {
        let docks = bp.docks();
        let all = bp.entities();
        let mut listed: Vec<(String, Option<Dock>, &Entity)> = Vec::new();
        for (path, e) in &all {
            listed.push((path.clone(), docks.get(path).cloned(), &e.entity));
        }
        let mut rig = Rig::assemble(&listed, is_track, shapes);
        rig.unplaced = all.len() - rig.parts.len();
        rig
    }

    /// Puts together entities given by path, parents first, each with its
    /// dock. The first is the main entity.
    pub fn assemble(entities: &[(String, Option<Dock>, &Entity)], is_track: impl Fn(u16) -> bool, shapes: &dyn Shapes) -> Rig {
        let mut parts: Vec<Part> = Vec::new();
        let mut used: Vec<&Entity> = Vec::new();
        for (i, (path, dock, entity)) in entities.iter().enumerate() {
            let parent_path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            // An entity with no dock on record cannot be placed, nor can what
            // is docked to it.
            let parent = if i == 0 { Some(0) } else { parts.iter().position(|p| p.path == parent_path) };
            let (Some(parent), true) = (parent, i == 0 || dock.is_some()) else { continue };
            let track = match (dock, i) {
                (Some(dock), 1..) => dock.track(used[parent], &is_track),
                _ => Track::default(),
            };
            parts.push(Part {
                path: path.clone(),
                dock: dock.clone().filter(|_| i > 0),
                parent,
                entity: i,
                bounds: entity.bounds(),
                track,
                body: if i == 0 { None } else { Body::of(entity) },
                coarse: Coarse::of(entity),
                placed: Placement::SAME,
                value: 0.0,
                driven: None,
                overlapping: false,
                demo: Demo::default(),
            });
            used.push(entity);
        }
        let mut rig = Rig { parts, unplaced: 0 };
        rig.place();
        for i in 1..rig.parts.len() {
            let all: Vec<&Entity> = entities.iter().map(|(_, _, e)| *e).collect();
            rig.parts[i].overlapping = rig.own_hit(&all, i, shapes).is_some();
        }
        rig
    }

    /// Every entity of the design the rig was made from, in the order
    /// [`Part::entity`] counts them.
    pub fn entities<'a>(&self, bp: &'a Blueprint) -> Vec<&'a Entity> {
        bp.entities().into_iter().map(|(_, e)| &e.entity).collect()
    }

    /// Works out where every part sits from how far each has moved.
    pub fn place(&mut self) {
        for i in 0..self.parts.len() {
            let part = &self.parts[i];
            let on_parent = match (&part.dock, &part.driven) {
                (Some(dock), Some(moved)) => dock.posed(&dock.turret, moved),
                (Some(dock), None) => dock.moved_on(part.value, &part.track),
                (None, _) => Placement::SAME,
            };
            self.parts[i].placed = if i == 0 { on_parent } else { self.parts[part.parent].placed.then(&on_parent) };
        }
    }

    /// Which parts are the given one or docked to it, directly or not.
    fn carried(&self, part: usize) -> Vec<bool> {
        let mut inside = vec![false; self.parts.len()];
        for i in part..self.parts.len() {
            inside[i] = i == part || (i > 0 && inside[self.parts[i].parent]);
        }
        inside
    }

    /// Where one part's own blocks sit inside another part's, if they do.
    fn own_hit(&self, entities: &[&Entity], part: usize, shapes: &dyn Shapes) -> Option<Contact> {
        let inside = self.carried(part);
        self.hit_between(entities, part, &inside, shapes)
    }

    fn hit_between(&self, entities: &[&Entity], mover: usize, inside: &[bool], shapes: &dyn Shapes) -> Option<Contact> {
        let body = self.parts[mover].body.as_ref()?;
        for (still, other) in self.parts.iter().enumerate() {
            let (false, Some(coarse), Some(entity), Some(own)) = (inside[still], &other.coarse, entities.get(other.entity), entities.get(self.parts[mover].entity)) else { continue };
            let into_other = other.placed.back().then(&self.parts[mover].placed);
            if let Some((mover_cell, still_cell)) = collide::hit(body, own, &into_other, entity, coarse, shapes) {
                return Some(Contact { mover, mover_cell, still, still_cell });
            }
        }
        None
    }

    /// Where the part, or anything it carries, sits inside a part it does
    /// not carry. Parts that overlap as saved are passed over on both sides.
    pub fn hit(&self, entities: &[&Entity], part: usize, shapes: &dyn Shapes) -> Option<Contact> {
        let inside = self.carried(part);
        let mut solid = inside.clone();
        // What overlaps as saved is in nobody's way: count it as carried.
        for (i, p) in self.parts.iter().enumerate() {
            solid[i] = inside[i] || p.overlapping;
        }
        (part..self.parts.len()).filter(|&i| inside[i] && !self.parts[i].overlapping).find_map(|i| self.hit_between(entities, i, &solid, shapes))
    }

    /// How far from its pivot the part and what it carries reach, in blocks.
    fn reach(&self, part: usize) -> f32 {
        let Some(dock) = &self.parts[part].dock else { return 0.0 };
        let pivot = self.parts[part].placed.at(dock.docker.pos.to_array().map(|c| c as f32));
        let inside = self.carried(part);
        let mut furthest = 0.0f32;
        for (i, p) in self.parts.iter().enumerate() {
            let (true, Some(b)) = (inside[i], p.bounds) else { continue };
            let (min, max) = (b.min.to_array(), b.max.to_array());
            for corner in 0..8 {
                let c = [0, 1, 2].map(|a| if corner >> a & 1 == 0 { min[a] as f32 - 0.5 } else { max[a] as f32 + 0.5 });
                let at = p.placed.at(c);
                let d = [0, 1, 2].map(|a| at[a] - pivot[a]);
                furthest = furthest.max((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
            }
        }
        furthest
    }

    /// Moves a part toward a value, a little at a time, and stops it short
    /// of the first thing in its way. Says what stopped it, if anything did.
    pub fn move_to(&mut self, entities: &[&Entity], part: usize, target: f32, shapes: &dyn Shapes) -> Option<Contact> {
        let from = self.parts[part].value;
        if self.parts[part].dock.is_none() || target == from {
            return None;
        }
        // A hair's breadth is not worth a check; neither is a part that is
        // in nobody's way.
        if (target - from).abs() < 1e-4 || self.parts[part].overlapping {
            self.parts[part].value = target;
            self.place();
            return None;
        }
        let travel = match self.parts[part].motion() {
            Motion::Turret | Motion::Rotator { .. } => (target - from).abs().to_radians() * self.reach(part),
            _ => (target - from).abs(),
        };
        let checks = ((travel / STRIDE).ceil() as usize).clamp(1, MOST_CHECKS);
        let mut last = from;
        for k in 1..=checks {
            let value = if k == checks { target } else { from + (target - from) * k as f32 / checks as f32 };
            self.parts[part].value = value;
            self.place();
            if let Some(contact) = self.hit(entities, part, shapes) {
                self.parts[part].value = last;
                self.place();
                return Some(contact);
            }
            last = value;
        }
        None
    }

    /// Puts every part back the way the design was saved.
    pub fn reset(&mut self) {
        for part in &mut self.parts {
            part.value = 0.0;
            part.driven = None;
            part.demo = Demo::default();
        }
        self.place();
    }

    /// Starts the demonstration over from where the parts are now.
    pub fn start_demo(&mut self) {
        let mut turrets = 0;
        for part in &mut self.parts {
            part.demo = Demo { heading: 1.0, ..Demo::default() };
            if part.motion() == Motion::Turret {
                // Not all in step: each starts a little later than the one before.
                part.demo.rest = (turrets % 4) as f32 * 0.35;
                turrets += 1;
            }
            part.demo.done = !part.can_move();
        }
    }

    /// Moves every part on by `seconds` of its demonstration: turrets swing
    /// from side to side, rotators go round a quarter turn at a time, and
    /// what rides a rail goes to one end of its track and back. A part that
    /// meets another turns back there. Unless `looping`, each part stops
    /// once it has been through its motion and is back where it started.
    pub fn play(&mut self, entities: &[&Entity], seconds: f32, looping: bool, shapes: &dyn Shapes) -> Played {
        let mut played = Played::default();
        for i in 1..self.parts.len() {
            let mut demo = self.parts[i].demo;
            if demo.done {
                continue;
            }
            if demo.rest > 0.0 {
                demo.rest -= seconds;
                self.parts[i].demo = demo;
                continue;
            }
            let value = self.parts[i].value;
            let (lo, hi) = self.parts[i].range();
            match self.parts[i].motion() {
                Motion::Turret | Motion::Rail => {
                    let (far, near, speed) = match self.parts[i].motion() {
                        Motion::Turret => (TURRET_SWEEP, -TURRET_SWEEP, TURRET_SPEED),
                        _ => (hi, lo, RAIL_SPEED),
                    };
                    // Out, back past the start to the other end, and home.
                    let homing = !looping && demo.legs >= 2;
                    let goal = if homing { 0.0 } else if demo.heading > 0.0 { far } else { near };
                    let next = toward(value, goal, speed * seconds);
                    let contact = self.move_to(entities, i, next, shapes);
                    if contact.is_some() || next == goal {
                        demo.heading = -demo.heading;
                        demo.legs += 1;
                        demo.done = homing;
                    }
                    played.contacts.extend(contact);
                }
                Motion::Rotator { .. } => {
                    if !demo.retreating && (value - demo.goal).abs() < 1e-3 {
                        demo.goal = (value / 90.0).round() * 90.0 + 90.0;
                    }
                    let next = toward(value, demo.goal, ROTATOR_SPEED * seconds);
                    let contact = self.move_to(entities, i, next, shapes);
                    if contact.is_some() && !demo.retreating {
                        // Something is in the way: back to the quarter it left.
                        demo.goal -= 90.0;
                        demo.retreating = true;
                    } else if contact.is_some() || next == demo.goal {
                        demo.retreating = false;
                        demo.legs += 1;
                        demo.rest = ROTATOR_REST;
                        demo.goal = self.parts[i].value;
                        demo.done = !looping && demo.legs >= 4;
                        if (self.parts[i].value / 360.0).fract().abs() < 1e-4 {
                            // A full turn is no turn.
                            self.parts[i].value = 0.0;
                            demo.goal = 0.0;
                            self.place();
                        }
                    }
                    played.contacts.extend(contact);
                }
                Motion::Fixed => demo.done = true,
            }
            self.parts[i].demo = demo;
        }
        played.finished = !looping && self.parts.iter().skip(1).all(|p| p.demo.done);
        played
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Piece, NO_TURN, ROTATORS, TURRET_AXIS};
    use crate::Block;

    /// Every block a full cube.
    struct Solid(std::sync::LazyLock<crate::shape::Geom>);
    impl Shapes for Solid {
        fn geom(&self, _: Block) -> Option<&crate::shape::Geom> {
            Some(&self.0)
        }
    }
    static SOLID: Solid = Solid(std::sync::LazyLock::new(crate::shape::Geom::cube));

    const HULL: u16 = 5;
    const RAIL: u16 = 662;

    fn dock(rail: (i32, i32, i32), rail_type: u16, docker: (i32, i32, i32)) -> Dock {
        Dock {
            rail: Piece { pos: Pos::new(rail.0, rail.1, rail.2), ty: rail_type, orientation: 14 },
            docker: Piece { pos: Pos::new(docker.0, docker.1, docker.2), ty: 663, orientation: 10 },
            turret: NO_TURN,
            moved: Placement::SAME,
            rotated: false,
        }
    }

    fn fill(e: &mut Entity, from: (i32, i32, i32), to: (i32, i32, i32), ty: u16) {
        for x in from.0..=to.0 {
            for y in from.1..=to.1 {
                for z in from.2..=to.2 {
                    e.set(Pos::new(x, y, z), Block::new(ty, 14, false));
                }
            }
        }
    }

    /// A deck with a turret axis in the middle and a post standing on it,
    /// and a turret: a foot on the axis and a barrel four blocks long.
    fn deck_and_turret() -> (Entity, Entity) {
        let mut ship = Entity::new();
        fill(&mut ship, (-6, 0, -6), (6, 0, 6), HULL);
        ship.set(Pos::new(0, 0, 0), Block::new(TURRET_AXIS, 14, false));
        // The post is where the barrel comes to after a quarter turn: a
        // turret turns about its docker's arrow side, which faces down here,
        // so a turn by a positive angle swings the barrel from +Z toward -X.
        fill(&mut ship, (-3, 1, 0), (-3, 3, 0), HULL);
        let mut turret = Entity::new();
        turret.set(Pos::new(0, -1, 0), Block::new(663, 10, false));
        turret.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        fill(&mut turret, (0, 0, 1), (0, 0, 4), HULL);
        (ship, turret)
    }

    fn rig_of(ship: &Entity, turret: &Entity, dock: Dock) -> Rig {
        Rig::assemble(&[(String::new(), None, ship), ("ATTACHED_0".to_string(), Some(dock), turret)], |ty| ty == RAIL, &SOLID)
    }

    #[test]
    fn a_turret_turns_until_its_barrel_meets_the_post() {
        let (ship, turret) = deck_and_turret();
        let mut rig = rig_of(&ship, &turret, dock((0, 0, 0), TURRET_AXIS, (0, -1, 0)));
        let entities = [&ship, &turret];
        assert!(!rig.parts[1].overlapping);
        assert_eq!(rig.parts[1].range(), (-180.0, 180.0));
        // The barrel points along +Z, two blocks above the deck. Away from
        // the post it turns as far as asked.
        assert_eq!(rig.move_to(&entities, 1, -120.0, &SOLID), None);
        assert_eq!(rig.parts[1].value, -120.0);
        // Toward the post it is stopped short of a quarter turn.
        assert_eq!(rig.move_to(&entities, 1, 0.0, &SOLID), None);
        let contact = rig.move_to(&entities, 1, 170.0, &SOLID).expect("the post is in the way");
        assert_eq!((contact.mover, contact.still), (1, 0));
        assert_eq!(contact.still_cell.x, -3);
        let stopped = rig.parts[1].value;
        assert!(stopped > 50.0 && stopped < 90.0, "stopped at {stopped}");
        assert_eq!(rig.hit(&entities, 1, &SOLID), None, "it is left clear of the post");
        // Pushing on changes nothing; turning back is free.
        assert!(rig.move_to(&entities, 1, 170.0, &SOLID).is_some());
        assert!((rig.parts[1].value - stopped).abs() < 2.0);
        assert_eq!(rig.move_to(&entities, 1, 0.0, &SOLID), None);
    }

    #[test]
    fn a_part_that_overlaps_as_saved_moves_unhindered() {
        let (mut ship, turret) = deck_and_turret();
        // A block right where the barrel is.
        ship.set(Pos::new(0, 2, 2), Block::new(HULL, 0, false));
        let mut rig = rig_of(&ship, &turret, dock((0, 0, 0), TURRET_AXIS, (0, -1, 0)));
        assert!(rig.parts[1].overlapping);
        assert_eq!(rig.move_to(&[&ship, &turret], 1, 170.0, &SOLID), None);
        assert_eq!(rig.parts[1].value, 170.0);
    }

    #[test]
    fn what_is_docked_to_a_part_is_carried_and_checked_with_it() {
        // A turret with a short foot and a barrel docked on top of it.
        let (ship, _) = deck_and_turret();
        let mut base = Entity::new();
        base.set(Pos::new(0, -1, 0), Block::new(663, 10, false));
        base.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        base.set(Pos::new(0, 1, 0), Block::new(TURRET_AXIS, 14, false));
        let mut barrel = Entity::new();
        barrel.set(Pos::new(0, -1, 0), Block::new(663, 10, false));
        fill(&mut barrel, (0, 0, 0), (0, 0, 4), HULL);
        let on_ship = dock((0, 0, 0), TURRET_AXIS, (0, -1, 0));
        let on_base = dock((0, 1, 0), TURRET_AXIS, (0, -1, 0));
        let mut rig = Rig::assemble(
            &[(String::new(), None, &ship), ("ATTACHED_0".to_string(), Some(on_ship), &base), ("ATTACHED_0/ATTACHED_0".to_string(), Some(on_base), &barrel)],
            |ty| ty == RAIL,
            &SOLID,
        );
        let entities = [&ship, &base, &barrel];
        assert_eq!(rig.parts[2].parent, 1);
        // The base is at y 1 to 3 over the deck, the barrel's docker at 4 and
        // the barrel itself at 5: above the post, which ends at 3.
        assert_eq!(rig.parts[2].placed.cell(Pos::new(0, 0, 4)), Some(Pos::new(0, 5, 4)));
        assert_eq!(rig.move_to(&entities, 1, 90.0, &SOLID), None, "over the post");
        // Turning the base turned the barrel with it.
        assert_eq!(rig.parts[2].placed.cell(Pos::new(0, 0, 4)), Some(Pos::new(-4, 5, 0)));
        // A taller post: now the base is stopped by what it carries.
        let mut tall = Entity::new();
        ship.for_each(|p, b| tall.set(p, b));
        fill(&mut tall, (-3, 4, 0), (-3, 5, 0), HULL);
        let mut rig = Rig::assemble(
            &[(String::new(), None, &tall), ("ATTACHED_0".to_string(), Some(dock((0, 0, 0), TURRET_AXIS, (0, -1, 0))), &base), ("ATTACHED_0/ATTACHED_0".to_string(), Some(dock((0, 1, 0), TURRET_AXIS, (0, -1, 0))), &barrel)],
            |ty| ty == RAIL,
            &SOLID,
        );
        let contact = rig.move_to(&[&tall, &base, &barrel], 1, 90.0, &SOLID).expect("the barrel meets the post");
        assert_eq!((contact.mover, contact.still), (2, 0));
        // The barrel by itself is stopped the same way, and never by its own base.
        rig.reset();
        assert!(rig.move_to(&[&tall, &base, &barrel], 2, 90.0, &SOLID).is_some());
        assert_eq!(rig.move_to(&[&tall, &base, &barrel], 2, -90.0, &SOLID), None);
    }

    #[test]
    fn an_entity_with_no_dock_is_left_out_with_what_it_carries() {
        let (ship, turret) = deck_and_turret();
        let rig = Rig::assemble(
            &[
                (String::new(), None, &ship),
                ("ATTACHED_0".to_string(), None, &turret),
                ("ATTACHED_0/ATTACHED_0".to_string(), Some(dock((0, 1, 0), TURRET_AXIS, (0, -1, 0))), &turret),
                ("ATTACHED_1".to_string(), Some(dock((0, 0, 0), TURRET_AXIS, (0, -1, 0))), &turret),
            ],
            |ty| ty == RAIL,
            &SOLID,
        );
        assert_eq!(rig.parts.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(), ["", "ATTACHED_1"]);
    }

    /// Plays until the demonstration is over or the time is up; returns the
    /// values the part went through.
    fn play_through(rig: &mut Rig, entities: &[&Entity], part: usize, looping: bool, most_seconds: f32) -> (Vec<f32>, bool) {
        rig.start_demo();
        let mut values = Vec::new();
        let mut time = 0.0;
        while time < most_seconds {
            let played = rig.play(entities, 0.05, looping, &SOLID);
            values.push(rig.parts[part].value);
            if played.finished {
                return (values, true);
            }
            time += 0.05;
        }
        (values, false)
    }

    #[test]
    fn played_once_a_turret_swings_both_ways_and_comes_home() {
        let (mut ship, turret) = deck_and_turret();
        // Something nearer than the post, half a quarter turn round.
        ship.set(Pos::new(-2, 2, 2), Block::new(HULL, 0, false));
        let mut rig = rig_of(&ship, &turret, dock((0, 0, 0), TURRET_AXIS, (0, -1, 0)));
        let entities = [&ship, &turret];
        let (values, finished) = play_through(&mut rig, &entities, 1, false, 60.0);
        assert!(finished);
        assert_eq!(rig.parts[1].value, 0.0);
        let (least, most) = values.iter().fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        // The block stops the swing toward -X before the full sweep; the
        // other way there is nothing.
        assert!(most > 10.0 && most < 45.0, "swung to {most}");
        assert_eq!(least, -TURRET_SWEEP);
        // Looping, it never finishes.
        let (_, finished) = play_through(&mut rig, &entities, 1, true, 30.0);
        assert!(!finished);
    }

    #[test]
    fn played_once_a_rotator_goes_round_in_quarters_and_a_rail_rider_there_and_back() {
        let (mut ship, _) = deck_and_turret();
        let mut door = Entity::new();
        door.set(Pos::new(0, -1, 0), Block::new(663, 10, false));
        fill(&mut door, (0, 0, 0), (0, 0, 2), HULL);
        // No post in the way of this one.
        fill(&mut ship, (-3, 1, 0), (-3, 3, 0), 0);
        ship.set(Pos::new(0, 0, 0), Block::new(ROTATORS[0], 14, false));
        let mut rig = rig_of(&ship, &door, dock((0, 0, 0), ROTATORS[0], (0, -1, 0)));
        let entities = [&ship, &door];
        let (values, finished) = play_through(&mut rig, &entities, 1, false, 60.0);
        assert!(finished);
        assert_eq!(rig.parts[1].value, 0.0, "four quarter turns are a full turn");
        for quarter in [90.0, 180.0, 270.0] {
            let resting = values.iter().filter(|v| (**v - quarter).abs() < 1e-3).count();
            assert!(resting >= 10, "it rests at {quarter} degrees");
        }
        // A rotator with a wall beside it: every quarter turn is turned back.
        fill(&mut ship, (-2, 1, 1), (-2, 3, 1), HULL);
        let mut rig = rig_of(&ship, &door, dock((0, 0, 0), ROTATORS[0], (0, -1, 0)));
        let (values, finished) = play_through(&mut rig, &[&ship, &door], 1, false, 60.0);
        assert!(finished);
        assert!(values.iter().all(|v| *v > -1e-3 && *v < 89.0), "never past the wall");
        assert!(values.iter().any(|v| *v > 10.0), "but it tries");

        // Rail: track from z = -2 to z = 3, arrows forward, the rider on z = 0.
        let mut ship = Entity::new();
        fill(&mut ship, (0, 0, -2), (0, 0, 3), RAIL);
        let mut rider = Entity::new();
        rider.set(Pos::new(0, -1, 0), Block::new(663, 10, false));
        rider.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        let mut rig = rig_of(&ship, &rider, dock((0, 0, 0), RAIL, (0, -1, 0)));
        assert_eq!(rig.parts[1].range(), (-2.0, 3.0));
        let (values, finished) = play_through(&mut rig, &[&ship, &rider], 1, false, 60.0);
        assert!(finished);
        assert_eq!(rig.parts[1].value, 0.0);
        let (least, most) = values.iter().fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert_eq!((least, most), (-2.0, 3.0));
        // With a block standing over the track, it turns back before it.
        ship.set(Pos::new(0, 2, 2), Block::new(HULL, 0, false));
        let mut rig = rig_of(&ship, &rider, dock((0, 0, 0), RAIL, (0, -1, 0)));
        let (values, finished) = play_through(&mut rig, &[&ship, &rider], 1, false, 60.0);
        assert!(finished);
        let most = values.iter().fold(0.0f32, |hi, v| hi.max(*v));
        assert!(most < 1.2, "stopped at {most}");
    }
}
