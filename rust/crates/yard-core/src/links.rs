//! Reading an entity's links: what runs a block, which blocks hang together,
//! and how to outline many linked blocks with few boxes.

use crate::catalog::BlockDef;
use crate::entity::{Entity, Links, Pos};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet, VecDeque};

/// What the game's linking rules need to know about a block type. Travels
/// with the block list, so the field names are kept short.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTraits {
    /// Block types its block config lists under `Controlling`.
    #[serde(rename = "c", default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<u16>,
    /// Block types its block config lists under `ControlledBy`. The game
    /// keeps the two lists apart and consults both.
    #[serde(rename = "b", default, skip_serializing_if = "Vec::is_empty")]
    pub controlled_by: Vec<u16>,
    /// The `flag` constants that apply.
    #[serde(rename = "f", default, skip_serializing_if = "is_zero")]
    pub flags: u16,
    /// The block config's `InventoryType`: 3 for storage and factories, 4 for shops.
    #[serde(rename = "i", default, skip_serializing_if = "is_zero_u8")]
    pub inventory: u8,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

/// Bits of [`LinkTraits::flags`].
pub mod flag {
    /// A logic block (`LogicBlock`): activation modules, buttons, gates, consoles.
    pub const SIGNAL: u16 = 1;
    pub const CAN_ACTIVATE: u16 = 1 << 1;
    pub const LIGHT: u16 = 1 << 2;
    pub const DOOR: u16 = 1 << 3;
    pub const SENSOR: u16 = 1 << 4;
    pub const FACTORY: u16 = 1 << 5;
    /// The weapon computer roles: cannon, missile and beam computers are
    /// "main", salvage and astrotech "support", the three effect computers "effect".
    pub const MAIN: u16 = 1 << 6;
    pub const SUPPORT: u16 = 1 << 7;
    pub const EFFECT: u16 = 1 << 8;
    /// A rail that docked entities travel on or turn on: a 24-way block
    /// without a 3D model that is not a docker, turret axis or shipyard part.
    pub const RAIL_TRACK: u16 = 1 << 9;
    pub const CHAMBER: u16 = 1 << 10;
    /// A logic block that lets go by itself half a second after it is
    /// switched on: buttons, consoles.
    pub const BUTTON: u16 = 1 << 11;
    /// Switched by a docked entity arriving over the rail block beside it:
    /// the activation module and the button.
    pub const BY_RAIL: u16 = 1 << 12;
    /// Drawn with the next texture tile along while it is switched off.
    pub const ON_TEXTURE: u16 = 1 << 13;
    /// Nothing runs into it: pickup rails, trigger areas, plants.
    pub const GHOST: u16 = 1 << 14;
}

/// Block types the rules name outright, by the game's block numbers.
mod id {
    pub const CORE: u16 = 1;
    pub const SALVAGE_COMPUTER: u16 = 4;
    pub const THRUSTER: u16 = 8;
    pub const ASTROTECH_COMPUTER: u16 = 39;
    pub const MINE_LAYER: u16 = 41;
    pub const GRAVITY: u16 = 56;
    pub const STABILIZER_NODE: u16 = 66;
    pub const AI_MODULE: u16 = 121;
    pub const COMMAND_MODULE: u16 = 134;
    pub const SHOP: u16 = 347;
    pub const TRACTOR_COMPUTER: u16 = 360;
    pub const ACTIVATION: u16 = 405;
    pub const AND: u16 = 408;
    pub const OR: u16 = 409;
    pub const TRIGGER_CONTROLLER: u16 = 413;
    pub const DISPLAY: u16 = 479;
    pub const WARP_GATE_COMPUTER: u16 = 542;
    pub const RAIL_DOCKER: u16 = 663;
    pub const ROTATORS: [u16; 2] = [664, 669];
    pub const TURRET_AXIS: u16 = 665;
    pub const RAIL_SPEED: u16 = 672;
    pub const SHIPYARD_COMPUTER: u16 = 677;
    pub const SHIPYARD_MODULE: u16 = 678;
    pub const SHIPYARD_CORE_ANCHOR: u16 = 679;
    pub const ACTIVATION_GATE_COMPUTER: u16 = 685;
    pub const CARGO: u16 = 689;
    pub const PICKUP_POINT: u16 = 937;
    pub const SENSOR: u16 = 980;
    pub const REACTOR: u16 = 1008;
    pub const GRAVITY_EXIT: u16 = 1738;
}

/// Block types that are never rail track although they turn 24 ways.
const NOT_TRACK: [u16; 5] = [id::RAIL_DOCKER, id::TURRET_AXIS, id::SHIPYARD_CORE_ANCHOR, id::PICKUP_POINT, id::SHIPYARD_MODULE];

/// Block types every entity keeps a system for, besides the reactor
/// chambers: a logic block may be linked to these whether or not they can be
/// switched. From the game's own list of systems.
const SYSTEMS: [u16; 52] = [
    2, 3, 4, 6, 14, 16, 24, 30, 32, 38, 39, 120, 122, 134, 177, 290, 331, 344, 345, 349, 350, 351, 352, 353, 354, 362, 405, 411,
    413, 414, 415, 478, 654, 655, 671, 672, 677, 678, 681, 682, 685, 686, 687, 688, 689, 937, 978, 980, 1008, 1009, 1010, 1286,
];
/// The systems only ships have, and only stations have.
const SHIP_SYSTEMS: [u16; 9] = [8, 15, 22, 37, 41, 360, 361, 544, 545];
const STATION_SYSTEMS: [u16; 13] = [112, 113, 211, 212, 213, 215, 217, 259, 347, 542, 543, 683, 684];
/// The factory blocks, which ships have systems for too where the server
/// allows factories on ships (`ALLOW_FACTORY_ON_SHIPS`, on by default).
const FACTORY_SYSTEMS: [u16; 7] = [112, 211, 212, 213, 215, 217, 259];

impl LinkTraits {
    pub fn of(def: &BlockDef) -> Self {
        let track = def.shape == crate::catalog::Shape::Rail && def.model.is_empty() && !NOT_TRACK.contains(&def.id);
        let flags = [
            (def.logic, flag::SIGNAL),
            (def.can_activate, flag::CAN_ACTIVATE),
            (def.light, flag::LIGHT),
            (def.door, flag::DOOR),
            (def.sensor, flag::SENSOR),
            (def.factory, flag::FACTORY),
            (def.main_computer, flag::MAIN),
            (def.support_computer, flag::SUPPORT),
            (def.effect_computer, flag::EFFECT),
            (track, flag::RAIL_TRACK),
            (def.chamber, flag::CHAMBER),
            (def.button, flag::BUTTON),
            (def.by_rail, flag::BY_RAIL),
            (def.on_texture, flag::ON_TEXTURE),
            (def.ghost, flag::GHOST),
        ];
        Self {
            controls: def.controlling.clone(),
            controlled_by: def.controlled_by.clone(),
            flags: flags.into_iter().filter(|(on, _)| *on).fold(0, |all, (_, bit)| all | bit),
            inventory: def.inventory,
        }
    }

    pub fn is(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn holds_items(&self) -> bool {
        self.inventory != 0 || self.is(flag::FACTORY)
    }

    /// What a sensor can read: inventories, doors, and what the config marks.
    fn sensed(&self) -> bool {
        self.holds_items() || self.is(flag::DOOR | flag::SENSOR)
    }
}

/// A rail a docker can dock to: track, rotators and the turret axis.
fn dockable(ty: u16, traits: &LinkTraits) -> bool {
    traits.is(flag::RAIL_TRACK) || ty == id::TURRET_AXIS
}

/// Where the rules look block types up: the game's block list on the server,
/// the list the browser was sent in the editor.
pub trait BlockLinks {
    fn link_traits(&self, ty: u16) -> Option<&LinkTraits>;
}

/// Which systems the entity has, which decides a few pairs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ship,
    /// Stations, and everything else that is not a ship.
    Station,
}

impl Kind {
    /// From the entity type in a blueprint header, where 0 is a ship.
    pub fn of_entity_type(entity_type: i32) -> Self {
        if entity_type == 0 { Kind::Ship } else { Kind::Station }
    }
}

/// The game's rules for linking, for one entity.
#[derive(Clone, Copy)]
pub struct Rules<'a> {
    /// None makes every link as asked, with no rule applied: for replaying
    /// what was decided elsewhere, and for tests of other things.
    blocks: Option<&'a dyn BlockLinks>,
    pub kind: Kind,
    /// Whether ships have factory systems. The game's default, and what a
    /// blueprint from a server with the default expects.
    pub ship_factories: bool,
}

/// One of the single places a weapon computer has for another block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// The second weapon that shapes how this one fires.
    Slave,
    Effect,
    /// The light that colours the weapon's shots.
    Light,
}

impl<'a> Rules<'a> {
    pub fn new(blocks: &'a dyn BlockLinks, kind: Kind) -> Self {
        Self { blocks: Some(blocks), kind, ship_factories: true }
    }

    pub const fn unchecked() -> Rules<'static> {
        Rules { blocks: None, kind: Kind::Ship, ship_factories: true }
    }

    pub fn is_unchecked(&self) -> bool {
        self.blocks.is_none()
    }

    fn traits(&self, ty: u16) -> Option<&'a LinkTraits> {
        self.blocks.and_then(|b| b.link_traits(ty))
    }

    /// Whether the entity keeps a system for this block type.
    fn has_system(&self, ty: u16, traits: &LinkTraits) -> bool {
        traits.is(flag::CHAMBER)
            || SYSTEMS.contains(&ty)
            || match self.kind {
                Kind::Ship => SHIP_SYSTEMS.contains(&ty) || (self.ship_factories && FACTORY_SYSTEMS.contains(&ty)),
                Kind::Station => STATION_SYSTEMS.contains(&ty),
            }
    }

    /// Whether the game lets a player link a block of type `a` to one of type
    /// `b`: select `a` with C, press V on `b`. The game asks two questions and
    /// both must hold; the pair rules below are its own, quirks included (a
    /// factory may be linked to anything).
    pub fn can_link(&self, a: u16, b: u16) -> bool {
        if self.is_unchecked() {
            return a != 0 && b != 0;
        }
        let (Some(at), Some(bt)) = (self.traits(a), self.traits(b)) else { return false };
        let paired = pair_rule(a, at, b, bt);
        let weapon = at.is(flag::MAIN) && bt.is(flag::MAIN | flag::EFFECT | flag::LIGHT) || at.is(flag::SUPPORT) && bt.is(flag::MAIN);
        (self.has_system(b, bt) || paired) && (bt.controlled_by.contains(&a) || at.is(flag::SIGNAL) || weapon || paired)
    }

    /// Whether the game links a block of type `b` to the selected block of
    /// type `a` when it is placed. Almost what V allows: an effect computer
    /// is not taken by the weapon computer that happens to be selected.
    pub fn links_when_placed(&self, a: u16, b: u16) -> bool {
        let (Some(at), Some(bt)) = (self.traits(a), self.traits(b)) else { return false };
        let paired = pair_rule(a, at, b, bt);
        (self.has_system(b, bt) || paired) && (bt.controlled_by.contains(&a) || at.is(flag::SIGNAL) || paired)
    }

    /// Whether a ship's core takes a block of this type the moment it is
    /// placed: computers, which is how they come to be on the pilot's bar.
    pub fn core_takes(&self, ty: u16) -> bool {
        self.traits(ty).is_some_and(|t| t.controlled_by.contains(&id::CORE)) && self.can_link(id::CORE, ty)
    }

    /// Whether blocks of this type are rail track, which hangs together only
    /// where it also points the same way.
    pub fn is_track(&self, ty: u16) -> bool {
        self.traits(ty).is_some_and(|t| t.is(flag::RAIL_TRACK))
    }

    /// Whether the game lets this kind of block control anything on this
    /// kind of entity. For the editor's wording only.
    pub fn can_control(&self, a: u16) -> bool {
        let Some(at) = self.traits(a) else { return self.is_unchecked() && a != 0 };
        !at.controls.is_empty()
            || at.is(flag::SIGNAL | flag::MAIN | flag::SUPPORT | flag::RAIL_TRACK | flag::FACTORY)
            || at.inventory != 0
            || [
                id::CORE,
                id::GRAVITY,
                id::GRAVITY_EXIT,
                id::STABILIZER_NODE,
                id::MINE_LAYER,
                id::COMMAND_MODULE,
                id::TURRET_AXIS,
                id::TRACTOR_COMPUTER,
                id::TRIGGER_CONTROLLER,
            ]
            .contains(&a)
    }

    /// Whether a block of type `b` keeps every controller it is given.
    /// Everything else has one controller: linked from another, it leaves
    /// the one it had.
    pub fn many_controllers(&self, b: u16) -> bool {
        let Some(bt) = self.traits(b) else { return true };
        b != id::CARGO && (bt.is(flag::SIGNAL | flag::RAIL_TRACK) || bt.sensed() || b == id::DISPLAY)
    }

    /// The place a controller of type `a` has for a block of type `b`, if it
    /// has only one such place.
    pub fn slot(&self, a: u16, b: u16) -> Option<Slot> {
        let (at, bt) = (self.traits(a)?, self.traits(b)?);
        if a == id::CORE {
            return bt.is(flag::LIGHT).then_some(Slot::Light);
        }
        if !at.is(flag::MAIN | flag::SUPPORT) {
            return None;
        }
        if bt.is(flag::MAIN) {
            Some(Slot::Slave)
        } else if bt.is(flag::EFFECT) && at.is(flag::MAIN) {
            Some(Slot::Effect)
        } else if bt.is(flag::LIGHT) && at.is(flag::MAIN) {
            Some(Slot::Light)
        } else {
            None
        }
    }

    /// Whether linking to a block of type `b` takes it away from the
    /// controller of type `other` it already has.
    pub fn displaces(&self, a: u16, b: u16, other: u16) -> bool {
        if self.is_unchecked() || a == id::COMMAND_MODULE {
            return false;
        }
        if b == id::CARGO {
            // Cargo space belongs to one inventory at a time.
            return self.traits(other).is_some_and(LinkTraits::holds_items);
        }
        !self.many_controllers(b)
    }

    /// The one kind of block a block of type `b` must be linked to before it
    /// does anything: a cannon barrel's cannon computer, a transporter
    /// module's controller. None for blocks that work alone or can answer to
    /// several kinds (rails, logic, storage, factories) and for computers,
    /// which the ship core takes.
    pub fn needs_controller(&self, b: u16) -> Option<u16> {
        let bt = self.traits(b)?;
        let [a] = bt.controlled_by[..] else { return None };
        let at = self.traits(a)?;
        let alone = bt.is(flag::RAIL_TRACK | flag::SIGNAL) || bt.holds_items() || at.is(flag::SIGNAL);
        (a != id::CORE && !alone).then_some(a)
    }

    /// Whether blocks of type `a` run modules that need them: the computers
    /// of [`Rules::needs_controller`].
    pub fn runs_modules(&self, a: u16) -> bool {
        self.traits(a).is_some_and(|at| at.controls.iter().any(|&b| self.needs_controller(b) == Some(a)))
    }
}

/// The blocks of an entity that should be linked and are not: modules that
/// no computer runs, and computers that run none of their modules.
pub fn unlinked(entity: &Entity, rules: &Rules) -> Vec<Pos> {
    let links = entity.links();
    let controlled: HashSet<Pos> = links.values().flat_map(|groups| groups.values().flatten().copied()).collect();
    let mut out = Vec::new();
    entity.for_each(|p, b| {
        let ty = b.ty();
        let alone = if rules.needs_controller(ty).is_some() {
            !controlled.contains(&p)
        } else if rules.runs_modules(ty) {
            !links.get(&p).is_some_and(|groups| groups.iter().any(|(t, set)| !set.is_empty() && rules.needs_controller(*t) == Some(ty)))
        } else {
            false
        };
        if alone {
            out.push(p);
        }
    });
    out
}

/// The game's table of pairs that may be linked whatever else holds
/// (`ElementInformation.canBeControlled`), by its own block numbers.
fn pair_rule(a: u16, at: &LinkTraits, b: u16, bt: &LinkTraits) -> bool {
    let signal = |t: &LinkTraits| t.is(flag::SIGNAL);
    at.controls.contains(&b)
        || ([id::GRAVITY, id::GRAVITY_EXIT].contains(&a) && [id::ACTIVATION, id::GRAVITY, id::GRAVITY_EXIT].contains(&b))
        || (a == id::SHIPYARD_COMPUTER && b == id::SHIPYARD_CORE_ANCHOR)
        || (a == id::SHOP && b == id::CARGO)
        || (a == id::STABILIZER_NODE && b == id::STABILIZER_NODE)
        || (a == id::CORE && bt.is(flag::LIGHT))
        || ([id::SHIPYARD_COMPUTER, id::SHOP, id::MINE_LAYER, id::ASTROTECH_COMPUTER, id::SALVAGE_COMPUTER].contains(&a) && bt.holds_items())
        || (at.holds_items() && bt.holds_items())
        || (at.is(flag::RAIL_TRACK) && bt.holds_items())
        || (a == id::SENSOR && bt.sensed())
        || (at.inventory == 3 && [id::ACTIVATION, id::AND, id::OR].contains(&b))
        || (id::ROTATORS.contains(&a) && b == id::ACTIVATION)
        || (a == id::ACTIVATION_GATE_COMPUTER && signal(bt))
        || (signal(at) && b == id::ACTIVATION_GATE_COMPUTER)
        || (at.is(flag::FACTORY) && at.inventory == 3)
        || (a == id::RAIL_SPEED && (bt.is(flag::RAIL_TRACK) || b == id::ACTIVATION))
        || (a == id::TRIGGER_CONTROLLER && signal(bt))
        || (signal(at) && b == id::SHIPYARD_COMPUTER)
        || (a == id::COMMAND_MODULE
            && ([id::SHIPYARD_COMPUTER, id::AI_MODULE, id::WARP_GATE_COMPUTER, id::REACTOR, id::THRUSTER].contains(&b)
                || dockable(b, bt)
                || bt.is(flag::CHAMBER)))
        || (dockable(a, at) && signal(bt))
        || (a == id::TRACTOR_COMPUTER && signal(bt))
        || (signal(at) && (b == id::RAIL_DOCKER || bt.is(flag::CAN_ACTIVATE | flag::RAIL_TRACK)))
        || (at.is(flag::MAIN) && bt.is(flag::LIGHT | flag::MAIN))
        || (at.is(flag::SUPPORT) && bt.is(flag::MAIN))
}

/// The controllers that run the block at `target`, in position order: the
/// computer a module belongs to, the master of a slaved computer, the logic
/// blocks that switch a light.
pub fn controllers_of(links: &Links, target: Pos) -> Vec<Pos> {
    links.iter().filter(|(_, groups)| groups.values().any(|set| set.contains(&target))).map(|(c, _)| *c).collect()
}

/// Whether `controller` runs the block at `target`.
pub fn is_linked(links: &Links, controller: Pos, target: Pos) -> bool {
    links.get(&controller).is_some_and(|groups| groups.values().any(|set| set.contains(&target)))
}

/// The block at `start` together with every block of the same type that
/// hangs on to it through touching sides, nearest first, at most `limit`
/// blocks. This is what the game links at once when a whole group is taken.
/// With `same_turn` the blocks must also be turned the same way, as the game
/// asks of rail track.
pub fn touching_group(entity: &Entity, start: Pos, limit: usize, same_turn: bool) -> Vec<Pos> {
    let first = entity.get(start);
    let ty = first.ty();
    let alike = |b: crate::Block| b.ty() == ty && (!same_turn || b.orientation() == first.orientation());
    if ty == 0 || limit == 0 {
        return Vec::new();
    }
    let mut seen: HashSet<Pos> = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(p) = queue.pop_front() {
        out.push(p);
        if out.len() >= limit {
            break;
        }
        for d in crate::shape::SIDE_DIRS {
            let next = p.offset(d[0], d[1], d[2]);
            if alike(entity.get(next)) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    out
}

/// Covers a set of cells with boxes (low corner, high corner, both included)
/// that do not overlap. Rows, slabs and blocks of cells come out as one box
/// each, so a weapon of a thousand modules is outlined with a handful of boxes.
pub fn boxes(cells: &BTreeSet<Pos>) -> Vec<(Pos, Pos)> {
    // Runs along Z: positions sort by x, then y, then z.
    let mut runs: Vec<(Pos, Pos)> = Vec::new();
    for &p in cells {
        match runs.last_mut() {
            Some((lo, hi)) if lo.x == p.x && lo.y == p.y && hi.z + 1 == p.z => hi.z = p.z,
            _ => runs.push((p, p)),
        }
    }
    // Runs of the same length side by side along Y become sheets, and sheets
    // of the same outline side by side along X become boxes.
    runs.sort_by_key(|(lo, hi)| (lo.x, lo.z, hi.z, lo.y));
    let mut sheets: Vec<(Pos, Pos)> = Vec::new();
    for (lo, hi) in runs {
        match sheets.last_mut() {
            Some((a, b)) if a.x == lo.x && a.z == lo.z && b.z == hi.z && b.y + 1 == lo.y => b.y = hi.y,
            _ => sheets.push((lo, hi)),
        }
    }
    sheets.sort_by_key(|(lo, hi)| (lo.y, hi.y, lo.z, hi.z, lo.x));
    let mut out: Vec<(Pos, Pos)> = Vec::new();
    for (lo, hi) in sheets {
        match out.last_mut() {
            Some((a, b)) if a.y == lo.y && b.y == hi.y && a.z == lo.z && b.z == hi.z && b.x + 1 == lo.x => b.x = hi.x,
            _ => out.push((lo, hi)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    fn cells(list: &[(i32, i32, i32)]) -> BTreeSet<Pos> {
        list.iter().map(|&(x, y, z)| Pos::new(x, y, z)).collect()
    }

    fn volume(boxes: &[(Pos, Pos)]) -> i64 {
        boxes.iter().map(|(lo, hi)| (hi.x - lo.x + 1) as i64 * (hi.y - lo.y + 1) as i64 * (hi.z - lo.z + 1) as i64).sum()
    }

    struct Kinds(std::collections::HashMap<u16, LinkTraits>);

    impl BlockLinks for Kinds {
        fn link_traits(&self, ty: u16) -> Option<&LinkTraits> {
            self.0.get(&ty)
        }
    }

    #[test]
    fn modules_without_a_computer_and_computers_without_modules_are_unlinked() {
        const COMPUTER: u16 = 6;
        const BARREL: u16 = 16;
        const ARMOR: u16 = 5;
        const RAIL: u16 = 662;
        const SPEED: u16 = 7;
        let traits = |c: &[u16], b: &[u16], f: u16| LinkTraits { controls: c.to_vec(), controlled_by: b.to_vec(), flags: f, inventory: 0 };
        let kinds = Kinds(
            [
                (id::CORE, traits(&[COMPUTER], &[], 0)),
                (COMPUTER, traits(&[BARREL], &[id::CORE], flag::MAIN)),
                (BARREL, traits(&[], &[COMPUTER], flag::CAN_ACTIVATE)),
                (ARMOR, traits(&[], &[], 0)),
                (SPEED, traits(&[RAIL], &[], 0)),
                (RAIL, traits(&[], &[SPEED], flag::RAIL_TRACK)),
            ]
            .into(),
        );
        let rules = Rules::new(&kinds, Kind::Ship);
        assert_eq!((rules.needs_controller(BARREL), rules.needs_controller(RAIL), rules.needs_controller(COMPUTER)), (Some(COMPUTER), None, None));
        assert!(rules.runs_modules(COMPUTER) && !rules.runs_modules(SPEED) && !rules.runs_modules(id::CORE));

        let mut e = Entity::new();
        let p = |x| Pos::new(x, 0, 0);
        e.set(Pos::new(0, 0, 0), Block::new(id::CORE, 0, false));
        // A cannon with one of its two barrels linked, a lone computer, a
        // lone barrel, armour and an unlinked rail.
        e.set(p(1), Block::new(COMPUTER, 0, false));
        e.set(p(2), Block::new(BARREL, 0, false));
        e.set(p(3), Block::new(BARREL, 0, false));
        e.set(p(5), Block::new(COMPUTER, 0, false));
        e.set(p(7), Block::new(BARREL, 0, false));
        e.set(p(8), Block::new(ARMOR, 0, false));
        e.set(p(9), Block::new(RAIL, 0, false));
        e.link(p(1), p(2), BARREL);
        e.link(Pos::new(0, 0, 0), p(5), COMPUTER);
        let mut found = unlinked(&e, &rules);
        found.sort_by_key(|q| q.x);
        assert_eq!(found, vec![p(3), p(5), p(7)]);
    }

    #[test]
    fn solid_groups_become_single_boxes() {
        let mut block = BTreeSet::new();
        for x in -1..=1 {
            for y in 0..=1 {
                for z in 5..=8 {
                    block.insert(Pos::new(x, y, z));
                }
            }
        }
        assert_eq!(boxes(&block), vec![(Pos::new(-1, 0, 5), Pos::new(1, 1, 8))]);
        // A row along each axis.
        assert_eq!(boxes(&cells(&[(0, 0, 0), (1, 0, 0), (2, 0, 0)])), vec![(Pos::new(0, 0, 0), Pos::new(2, 0, 0))]);
        assert_eq!(boxes(&cells(&[(0, 3, 0), (0, 4, 0)])), vec![(Pos::new(0, 3, 0), Pos::new(0, 4, 0))]);
        assert_eq!(boxes(&BTreeSet::new()), vec![]);
    }

    #[test]
    fn every_cell_is_covered_exactly_once() {
        // An L, a gap and a lone cell.
        let set = cells(&[(0, 0, 0), (0, 0, 1), (0, 0, 2), (1, 0, 0), (2, 0, 0), (0, 0, 4), (9, 9, 9), (0, 1, 0)]);
        let out = boxes(&set);
        assert_eq!(volume(&out), set.len() as i64);
        for p in &set {
            let holding = out.iter().filter(|(lo, hi)| (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z));
            assert_eq!(holding.count(), 1, "{p:?}");
        }
        assert!(out.len() < set.len());
    }

    #[test]
    fn a_group_is_the_blocks_of_one_type_that_touch() {
        let mut e = Entity::new();
        let barrel = Block::new(16, 0, false);
        for z in 1..=4 {
            e.set(Pos::new(0, 0, z), barrel);
        }
        e.set(Pos::new(1, 0, 4), barrel);
        // Touches only at a corner: not part of the group.
        e.set(Pos::new(1, 1, 5), barrel);
        // Another type in between does not carry the group on.
        e.set(Pos::new(0, 0, 5), Block::new(5, 0, false));
        e.set(Pos::new(0, 0, 6), barrel);
        let group: BTreeSet<Pos> = touching_group(&e, Pos::new(0, 0, 2), 1000, false).into_iter().collect();
        assert_eq!(group, cells(&[(0, 0, 1), (0, 0, 2), (0, 0, 3), (0, 0, 4), (1, 0, 4)]));
        // The limit keeps the nearest blocks, the one asked about first.
        assert_eq!(touching_group(&e, Pos::new(0, 0, 2), 1, false), vec![Pos::new(0, 0, 2)]);
        assert_eq!(touching_group(&e, Pos::new(0, 0, 2), 3, false).len(), 3);
        assert!(touching_group(&e, Pos::new(7, 7, 7), 10, false).is_empty(), "nothing there");
        // Rail track: a piece turned another way ends the run.
        e.set(Pos::new(0, 0, 3), Block::new(16, 5, false));
        assert_eq!(touching_group(&e, Pos::new(0, 0, 1), 1000, true).len(), 2);
        assert_eq!(touching_group(&e, Pos::new(0, 0, 1), 1000, false).len(), 5);
    }

    #[test]
    fn what_runs_a_block_is_found_from_the_link_map() {
        let mut e = Entity::new();
        let (computer, other, barrel) = (Pos::new(0, 0, 1), Pos::new(0, 0, 2), Pos::new(0, 0, 3));
        e.link(computer, barrel, 16);
        e.link(other, computer, 6);
        assert_eq!(controllers_of(e.links(), barrel), vec![computer]);
        assert_eq!(controllers_of(e.links(), computer), vec![other]);
        assert!(controllers_of(e.links(), other).is_empty());
        assert!(is_linked(e.links(), computer, barrel) && !is_linked(e.links(), barrel, computer));
    }
}
