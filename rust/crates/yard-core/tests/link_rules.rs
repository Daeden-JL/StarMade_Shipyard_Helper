//! The linking rules against the game's own answers: for every pair of block
//! types of the 0.304.7 block list, whether the game lets a player link the
//! first to the second, on a ship and on a station. The answers were taken
//! from the game's classes (see `tools/link-rules/README.md`); the rules here
//! have to give the same for all 2.3 million pairs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use yard_core::catalog::Catalog;
use yard_core::links::{Kind, Rules};

fn catalog() -> Option<Catalog> {
    let dir = std::env::var_os("STARMADE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?
        .join("data/config");
    let xml = std::fs::read_to_string(dir.join("BlockConfig.xml")).ok()?;
    let props = std::fs::read_to_string(dir.join("BlockTypes.properties")).ok()?;
    Catalog::parse(&xml, &props).ok()
}

fn fixture(text: &str) -> BTreeMap<u16, BTreeSet<u16>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let (from, to) = l.split_once(':').expect("controller: targets");
            (from.trim().parse().unwrap(), to.split_whitespace().map(|t| t.parse().unwrap()).collect())
        })
        .collect()
}

fn compare(kind: Kind, text: &str) {
    let Some(catalog) = catalog() else { return };
    // Not checked against another game version's block list.
    if catalog.len() != 1516 {
        return;
    }
    let expected = fixture(text);
    // The game's answers were taken on a ship without factories.
    let mut rules = Rules::new(&catalog, kind);
    rules.ship_factories = false;
    let ids: Vec<u16> = catalog.all().map(|d| d.id).collect();
    let mut wrong = Vec::new();
    let mut legal = 0;
    for &a in &ids {
        let allowed = expected.get(&a);
        for &b in &ids {
            let (game, here) = (allowed.is_some_and(|set| set.contains(&b)), rules.can_link(a, b));
            legal += here as usize;
            if game != here {
                wrong.push(format!("{} -> {}: game {game}, here {here}", catalog.name(a), catalog.name(b)));
            }
        }
    }
    assert!(wrong.is_empty(), "{} pairs differ, such as: {:?}", wrong.len(), &wrong[..wrong.len().min(12)]);
    assert_eq!(legal, expected.values().map(BTreeSet::len).sum::<usize>());
}

#[test]
fn ships_link_what_the_game_links() {
    compare(Kind::Ship, include_str!("data/link_pairs_ship.txt"));
}

#[test]
fn stations_link_what_the_game_links() {
    compare(Kind::Station, include_str!("data/link_pairs_station.txt"));
}

#[test]
fn the_pairs_players_know_come_out_right() {
    let Some(c) = catalog() else { return };
    let ship = Rules::new(&c, Kind::Ship);
    let id = |name: &str| c.by_name(name).unwrap_or_else(|| panic!("no block {name}")).id;
    let (cannon, barrel, beam, missile_tube) = (id("Cannon Computer"), id("Cannon Barrel"), id("Damage Beam Computer"), 32);
    let (activation, light, hull, core, display) = (id("Activation Module"), 55, id("Grey Basic Armor"), 1, 479);
    assert!(ship.can_link(cannon, barrel) && !ship.can_link(barrel, cannon) && !ship.can_link(cannon, missile_tube));
    // A second weapon, an effect, a light for the shots.
    assert!(ship.can_link(cannon, beam) && ship.can_link(cannon, 349) && ship.can_link(cannon, light));
    assert!(!ship.can_link(349, cannon) && !ship.can_link(cannon, 350) && !ship.can_link(cannon, activation));
    // Logic reaches whatever can be switched, and nothing else.
    assert!(ship.can_link(activation, light) && ship.can_link(activation, activation) && ship.can_link(activation, cannon));
    assert!(ship.can_link(activation, 662) && ship.can_link(activation, 663) && ship.can_link(activation, display));
    assert!(!ship.can_link(activation, hull) && !ship.can_link(activation, core) && !ship.can_link(hull, hull));
    assert!(ship.can_link(core, cannon) && ship.can_link(core, light) && !ship.can_link(core, activation));
    // Rails: the speed controller, a docker's storage.
    assert!(ship.can_link(672, 662) && !ship.can_link(672, 663) && ship.can_link(663, 120));
    // What only one kind of entity has a system for.
    let station = Rules::new(&c, Kind::Station);
    assert!(ship.can_link(activation, 545) && !station.can_link(activation, 545));
    assert!(station.can_link(activation, 543) && !ship.can_link(activation, 543));
    // Who keeps every controller, and who leaves the old one for the new.
    assert!(ship.many_controllers(activation) && ship.many_controllers(122) && ship.many_controllers(display) && ship.many_controllers(662));
    assert!(!ship.many_controllers(barrel) && !ship.many_controllers(light) && !ship.many_controllers(cannon) && !ship.many_controllers(689));
    assert!(ship.displaces(cannon, barrel, cannon) && !ship.displaces(134, 121, activation));
    // Cargo space leaves a storage for a storage, but not a logic block for one.
    assert!(ship.displaces(120, 689, 120) && !ship.displaces(120, 689, activation));
    assert!(ship.can_control(cannon) && ship.can_control(activation) && !ship.can_control(barrel) && !ship.can_control(hull));
}
