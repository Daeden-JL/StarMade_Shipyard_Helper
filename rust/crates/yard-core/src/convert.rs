//! Ships made into stations and stations into ships: what has to change in a
//! blueprint for the game to spawn it as the other kind.
//!
//! The game tells the two apart by the header's entity type (0 a ship, 2 a
//! space station) and files them under different classifications. A ship
//! is built round its core at the origin; a station has no core, and the
//! game will not let one be placed on it: a station is built from a Build
//! Block. So a ship becomes a station by its core giving way to a Build
//! Block, and a station becomes a ship by a core taking the place of what
//! is at its origin. Docked entities (turrets, docked ships) stay what they are.
//!
//! Blocks that only work on the other kind (thrusters on a station, factory
//! enhancers on a ship...) are left where they are and counted, so the
//! player can decide what to do with them.

use std::collections::BTreeMap;

use crate::edit::CORE_TYPE;
use crate::links::{ship_only, station_only};
use crate::{Block, Blueprint, Pos};

/// The header's entity type of a ship.
pub const SHIP: i32 = 0;
/// The header's entity type of a space station.
pub const STATION: i32 = 2;

/// The Build Block: what a station is built from, in the place a ship has its core.
pub const BUILD_BLOCK: u16 = 123;

/// The game's blueprint classifications (`BlueprintClassification`), by
/// their place in its list, which is what a header stores.
pub mod class {
    pub const NONE: i32 = 0;
    pub const MINING: i32 = 1;
    pub const DEFENSE: i32 = 5;
    pub const NONE_STATION: i32 = 9;
    pub const DEFENSE_STATION: i32 = 12;
    pub const MINING_STATION: i32 = 13;
}

/// Which kind of entity a design is to become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ship,
    Station,
}

impl Kind {
    /// The kind a header's entity type stands for, if it is one of the two.
    pub fn of_entity_type(entity_type: i32) -> Option<Kind> {
        match entity_type {
            SHIP => Some(Kind::Ship),
            STATION => Some(Kind::Station),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Ship => "ship",
            Kind::Station => "station",
        }
    }
}

/// What a conversion did, for the player.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Converted {
    /// The block the ship core took the place of at the origin, when a
    /// station became a ship and something was there.
    pub replaced: Option<u16>,
    /// Whether the core gave way to a Build Block (a ship made a station).
    pub core_removed: bool,
    /// Links that went with the core or the block it replaced.
    pub links_dropped: usize,
    /// Blocks of the main entity, by type, that only work on the other kind.
    pub idle: BTreeMap<u16, u32>,
}

/// A ship's classification as a station's: mining and defence have a
/// station of their own, everything else is a plain station.
pub fn station_class(ship: i32) -> i32 {
    match ship {
        class::MINING => class::MINING_STATION,
        class::DEFENSE => class::DEFENSE_STATION,
        _ => class::NONE_STATION,
    }
}

/// A station's classification as a ship's, the other way round.
pub fn ship_class(station: i32) -> i32 {
    match station {
        class::MINING_STATION => class::MINING,
        class::DEFENSE_STATION => class::DEFENSE,
        _ => class::NONE,
    }
}

/// Turns the main entity of a design into the other kind. Refuses, leaving
/// the blueprint untouched, when it is that kind already or neither kind
/// (a shop, an asteroid, a planet).
pub fn convert(bp: &mut Blueprint, to: Kind) -> Result<Converted, String> {
    let root = &mut bp.root;
    let from = Kind::of_entity_type(root.header.entity_type)
        .ok_or_else(|| "Only ships and stations can be converted; this blueprint is another kind of entity.".to_string())?;
    if from == to {
        return Err(format!("It is a {} already.", to.word()));
    }
    let origin = Pos::new(0, 0, 0);
    let entity = &mut root.entity;
    let mut done = Converted::default();
    let old = entity.get(origin);
    match to {
        Kind::Station => {
            if old.ty() == CORE_TYPE {
                done.links_dropped = drop_links(entity, origin);
                entity.set(origin, Block::new(BUILD_BLOCK, 0, false));
                done.core_removed = true;
            }
            root.header.classification = station_class(root.header.classification);
        }
        Kind::Ship => {
            if !old.is_air() {
                done.replaced = Some(old.ty());
                done.links_dropped = drop_links(entity, origin);
            }
            entity.set(origin, Block::new(CORE_TYPE, 0, false));
            root.header.classification = ship_class(root.header.classification);
        }
    }
    root.header.entity_type = match to {
        Kind::Ship => SHIP,
        Kind::Station => STATION,
    };
    let idle = |ty: u16| match to {
        Kind::Ship => station_only(ty),
        Kind::Station => ship_only(ty),
    };
    done.idle = entity.counts_by_type().into_iter().filter(|(ty, _)| idle(*ty)).collect();
    root.header.refresh(entity);
    Ok(done)
}

/// Takes out every link from and to the block at `at`. Returns how many.
fn drop_links(entity: &mut crate::Entity, at: Pos) -> usize {
    let mut pairs: Vec<(Pos, Pos)> = Vec::new();
    for (controller, groups) in entity.links() {
        for targets in groups.values() {
            for target in targets {
                if *controller == at || *target == at {
                    pairs.push((*controller, *target));
                }
            }
        }
    }
    pairs.into_iter().filter(|(c, t)| entity.unlink_any(*c, *t).is_some()).count()
}

/// The notes to show after a conversion, given the names of block types.
pub fn notes(done: &Converted, to: Kind, name_of: impl Fn(u16) -> String) -> Vec<String> {
    let mut notes = Vec::new();
    match to {
        Kind::Station => {
            notes.push("It is a station now: the game spawns it as one and files it with the stations in the catalog.".to_string());
            if done.core_removed {
                notes.push("The ship core gave way to a Build Block, which a station is built from.".to_string());
            }
        }
        Kind::Ship => {
            notes.push("It is a ship now: the game spawns it as one and files it with the ships in the catalog.".to_string());
            match done.replaced {
                Some(ty) => notes.push(format!("The ship core took the place of the {} at the centre of the station.", name_of(ty))),
                None => notes.push("The ship core was put at the centre of the station, which was empty. Use the Move core tool to put it where you want it.".to_string()),
            }
        }
    }
    if done.links_dropped > 0 {
        let what = if done.links_dropped == 1 { "link" } else { "links" };
        notes.push(format!("{} {what} to the block at the centre went with it.", done.links_dropped));
    }
    if !done.idle.is_empty() {
        let list: Vec<String> = done.idle.iter().map(|(ty, n)| format!("{n} {}", name_of(*ty))).collect();
        let other = match to {
            Kind::Ship => "station",
            Kind::Station => "ship",
        };
        notes.push(format!("These blocks only work on a {other} and do nothing on a {}: {}. They were left in place.", to.word(), list.join(", ")));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship_with_thrusters() -> Blueprint {
        let mut bp = Blueprint::new_ship("Test", "test");
        let e = &mut bp.root.entity;
        e.set(Pos::new(0, 0, 1), Block::new(6, 0, false)); // cannon computer
        e.set(Pos::new(0, 0, -1), Block::new(8, 0, false)); // thruster
        e.set(Pos::new(1, 0, 0), Block::new(5, 0, false)); // hull
        e.link(Pos::new(0, 0, 0), Pos::new(0, 0, 1), 6);
        bp.root.header.classification = class::MINING;
        bp
    }

    #[test]
    fn a_ship_becomes_a_station_without_its_core() {
        let mut bp = ship_with_thrusters();
        let done = convert(&mut bp, Kind::Station).unwrap();
        let root = &bp.root;
        assert_eq!(root.header.entity_type, STATION);
        assert_eq!(root.header.classification, class::MINING_STATION);
        assert_eq!(root.entity.get(Pos::new(0, 0, 0)).ty(), BUILD_BLOCK, "the core gives way to a Build Block");
        assert!(done.core_removed);
        assert_eq!(done.links_dropped, 1);
        assert_eq!(root.entity.link_count(), 0);
        assert_eq!(done.idle.get(&8), Some(&1), "the thruster does nothing on a station");
        assert!(!root.header.counts.iter().any(|(ty, _)| *ty == CORE_TYPE as i16), "the header no longer counts a core");
    }

    #[test]
    fn a_station_becomes_a_ship_with_a_core_at_its_centre() {
        let mut bp = ship_with_thrusters();
        convert(&mut bp, Kind::Station).unwrap();
        // Something at the centre of the station: the core takes its place.
        bp.root.entity.set(Pos::new(0, 0, 0), Block::new(291, 0, false));
        bp.root.entity.link(Pos::new(0, 0, 0), Pos::new(1, 0, 0), 5);
        let done = convert(&mut bp, Kind::Ship).unwrap();
        assert_eq!(bp.root.header.entity_type, SHIP);
        assert_eq!(bp.root.header.classification, class::MINING);
        assert_eq!(bp.root.entity.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
        assert_eq!(done.replaced, Some(291));
        assert_eq!(done.links_dropped, 1);
        assert!(done.idle.is_empty());
    }

    #[test]
    fn an_empty_centre_gets_the_core_and_nothing_is_replaced() {
        let mut bp = ship_with_thrusters();
        convert(&mut bp, Kind::Station).unwrap();
        // A station whose centre is empty, as some in the game are.
        bp.root.entity.set(Pos::new(0, 0, 0), Block::AIR);
        let done = convert(&mut bp, Kind::Ship).unwrap();
        assert_eq!(done.replaced, None);
        assert_eq!(bp.root.entity.get(Pos::new(0, 0, 0)).ty(), CORE_TYPE);
    }

    #[test]
    fn converting_to_what_it_is_already_is_refused_and_changes_nothing() {
        let mut bp = ship_with_thrusters();
        let before = bp.root.header.clone();
        assert!(convert(&mut bp, Kind::Ship).is_err());
        assert_eq!(bp.root.header, before);
        bp.root.header.entity_type = 3; // an asteroid
        assert!(convert(&mut bp, Kind::Station).is_err());
    }

    #[test]
    fn classifications_map_both_ways() {
        assert_eq!(station_class(class::DEFENSE), class::DEFENSE_STATION);
        assert_eq!(station_class(4), class::NONE_STATION);
        assert_eq!(ship_class(class::DEFENSE_STATION), class::DEFENSE);
        assert_eq!(ship_class(10), class::NONE);
    }
}
