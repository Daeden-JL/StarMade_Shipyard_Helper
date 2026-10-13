//! What the tools act on: a design's entities, read and edited through a
//! small trait, and an in-memory design for tests and examples.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use yard_core::catalog::{BlockDef, Catalog, Shape};
use yard_core::edit::{self, Change, Journal, Op};
use yard_core::links::{Kind, Rules};
use yard_core::{Blueprint, Entity, Pos};

/// Whether an entity is a ship, which decides a few linking rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Ship,
    Station,
}

impl EntityKind {
    pub fn of_entity_type(entity_type: i32) -> Self {
        if entity_type == 0 { Self::Ship } else { Self::Station }
    }

    /// The same distinction as the linking rules draw it.
    pub fn rules_kind(self) -> Kind {
        match self {
            Self::Ship => Kind::Ship,
            Self::Station => Kind::Station,
        }
    }
}

/// One entity of a design in a few facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityInfo {
    /// "" for the main entity, "ATTACHED_0", "ATTACHED_0/ATTACHED_1"... for
    /// docked ones.
    pub path: String,
    pub kind: EntityKind,
    pub blocks: u64,
    /// Lowest and highest block position, both included; None if empty.
    pub bounds: Option<(Pos, Pos)>,
    pub links: usize,
    /// The path of the entity this one is docked to; None for the main one.
    pub docked_to: Option<String>,
}

/// What the tools act on. The web server implements this over its open
/// designs; tests use [`MemoryWorld`].
///
/// Every method is called from inside [`crate::run`], between two model
/// calls, and may block for as long as a large edit takes. An implementation
/// must not hold a lock across calls: the model reads, thinks for seconds,
/// then edits.
pub trait World: Send + Sync {
    fn catalog(&self) -> &Catalog;

    /// The design's entities, the main one first.
    fn entities(&self) -> Vec<EntityInfo>;

    /// Runs `read` on one entity. Err if there is no such entity.
    fn with_entity(&self, path: &str, read: &mut dyn FnMut(&Entity)) -> Result<(), String>;

    /// Applies ops to one entity as a single undoable step under the game's
    /// linking rules, and says what changed. A lone [`Op::Undo`] or
    /// [`Op::Redo`] moves through the entity's history instead. Nothing is
    /// changed if the step is refused; the error is a sentence for the model.
    fn step(&self, path: &str, ops: Vec<Op>) -> Result<Change, String>;
}

/// The facts of every entity of a blueprint, for [`World::entities`].
pub fn entity_infos(blueprint: &Blueprint) -> Vec<EntityInfo> {
    blueprint
        .entities()
        .into_iter()
        .map(|(path, e)| EntityInfo {
            docked_to: (!path.is_empty()).then(|| path.rsplit_once('/').map_or(String::new(), |(parent, _)| parent.to_string())),
            kind: EntityKind::of_entity_type(e.header.entity_type),
            blocks: e.entity.block_count(),
            bounds: e.entity.bounds().map(|b| (b.min, b.max)),
            links: e.entity.link_count(),
            path,
        })
        .collect()
}

/// A design held in memory, with undo history: the world of the tests, the
/// example program, and anything else that wants an agent without a server.
pub struct MemoryWorld {
    catalog: Arc<Catalog>,
    state: Mutex<(Blueprint, HashMap<String, Journal>)>,
}

impl MemoryWorld {
    /// A new ship: just its core.
    pub fn new_ship(catalog: Arc<Catalog>, name: &str) -> Self {
        Self::of(catalog, Blueprint::new_ship(name, yard_core::blueprint::FALLBACK_GAME_VERSION))
    }

    pub fn of(catalog: Arc<Catalog>, blueprint: Blueprint) -> Self {
        Self { catalog, state: Mutex::new((blueprint, HashMap::new())) }
    }

    /// Runs `read` on the whole design.
    pub fn with_blueprint<T>(&self, read: impl FnOnce(&Blueprint) -> T) -> T {
        read(&self.lock().0)
    }

    /// Runs `edit` on the whole design, outside the undo history: for
    /// setting a scene up.
    pub fn with_blueprint_mut<T>(&self, edit: impl FnOnce(&mut Blueprint) -> T) -> T {
        edit(&mut self.lock().0)
    }

    /// The block at a position of an entity; air if there is no such entity.
    pub fn block(&self, path: &str, at: Pos) -> yard_core::Block {
        self.lock().0.entity(path).map_or(yard_core::Block::AIR, |e| e.entity.get(at))
    }

    /// A panic in a tool must not take the design down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, (Blueprint, HashMap<String, Journal>)> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl World for MemoryWorld {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    fn entities(&self) -> Vec<EntityInfo> {
        entity_infos(&self.lock().0)
    }

    fn with_entity(&self, path: &str, read: &mut dyn FnMut(&Entity)) -> Result<(), String> {
        let state = self.lock();
        let entity = state.0.entity(path).ok_or_else(|| no_such_entity(path))?;
        read(&entity.entity);
        Ok(())
    }

    fn step(&self, path: &str, ops: Vec<Op>) -> Result<Change, String> {
        let mut state = self.lock();
        let (blueprint, journals) = &mut *state;
        let entity = blueprint.entity_mut(path).ok_or_else(|| no_such_entity(path))?;
        let rules = Rules::new(&*self.catalog, Kind::of_entity_type(entity.header.entity_type));
        let journal = journals.entry(path.to_string()).or_default();
        edit::run_with(&mut entity.entity, journal, &ops, &rules).map_err(|e| sentence(&e.to_string()))
    }
}

/// The error for a path that names no entity, as the model should read it.
pub fn no_such_entity(path: &str) -> String {
    format!("There is no entity \"{path}\" in this design. design_info lists the entities; \"\" is the main one.")
}

/// An error message as a sentence: capital first, full stop last.
pub fn sentence(message: &str) -> String {
    let mut chars = message.trim().chars();
    let Some(first) = chars.next() else { return String::new() };
    let text: String = first.to_uppercase().chain(chars).collect();
    if text.ends_with(['.', '!', '?']) { text } else { format!("{text}.") }
}

/// A small block list with the game's own ids and names: enough hull, shapes
/// and systems to build and link with, for tests that have no StarMade
/// install to read the real one from.
pub fn sample_catalog() -> Catalog {
    let mut defs = Vec::new();
    let plain = |id: u16, name: &str, key: &str, category: &str| def(id, name, key, category);
    defs.push(plain(1, "Ship Core", "SHIP_CORE", "Ship/Core"));
    defs.push(plain(63, "Glass", "GLASS", "General/Glass"));
    defs.push(plain(8, "Thruster Module", "THRUSTER", "Ship/Thrust"));
    defs.push(plain(1008, "Reactor Power", "REACTOR_MAIN", "General/Power"));
    defs.push(plain(1009, "Reactor Stabilizer", "REACTOR_STABILIZER", "General/Power"));
    defs.push(plain(3, "Shield Capacitor", "SHIELD_CAP", "General/Shields"));
    defs.push(plain(478, "Shield-Recharger", "SHIELD_REGEN", "General/Shields"));
    defs.push(plain(662, "Rail Basic", "RAIL_BASIC", "General/Rails"));
    defs.push(plain(663, "Rail Docker", "RAIL_DOCKER", "General/Rails"));
    for d in defs.iter_mut().filter(|d| d.id == 662 || d.id == 663) {
        d.shape = Shape::Rail;
    }
    // Hull in two colours, each with every shape and slab.
    for (base, colour, key) in [(598u16, "Grey", "GREY_HULL"), (620, "Red", "RED_HULL")] {
        let name = format!("{colour} Basic Armor");
        let shapes = [(1u16, Shape::Wedge, "Wedge"), (2, Shape::Corner, "Corner"), (3, Shape::Tetra, "Tetra"), (4, Shape::Hepta, "Hepta")];
        let slabs = [(5u16, 3u8, "1/4"), (6, 2, "1/2"), (7, 1, "3/4")];
        let mut cube = def(base, &name, key, "General/Hulls/Basic");
        cube.style_ids = shapes.iter().map(|(offset, ..)| base + offset).collect();
        cube.slab_ids = slabs.iter().map(|(offset, ..)| base + offset).collect();
        defs.push(cube);
        for (offset, shape, word) in shapes {
            let mut variant = def(base + offset, &format!("{name} {word}"), &format!("{key}_{}", word.to_uppercase()), "General/Hulls/Basic");
            (variant.shape, variant.base) = (shape, base);
            defs.push(variant);
        }
        for (offset, slab, word) in slabs {
            let mut variant = def(base + offset, &format!("{name} {word}"), &format!("{key}_SLAB_{slab}"), "General/Hulls/Basic");
            (variant.slab, variant.base) = (slab, base);
            defs.push(variant);
        }
    }
    // Weapons: computers the core takes, each with its modules.
    for (computer, module, name, part) in [(6u16, 16u16, "Cannon", "Cannon Barrel"), (414, 415, "Damage Beam", "Damage Beam Module"), (38, 32, "Missile", "Missile Tube")] {
        let mut c = def(computer, &format!("{name} Computer"), &format!("{}_COMPUTER", name.to_uppercase().replace(' ', "_")), "Ship/Weapons");
        (c.controlling, c.controlled_by, c.main_computer, c.can_activate, c.sides) = (vec![module], vec![1], true, true, 6);
        let mut m = def(module, part, &part.to_uppercase().replace(' ', "_"), "Ship/Weapons");
        (m.controlled_by, m.can_activate) = (vec![computer], true);
        defs.extend([c, m]);
    }
    if let Some(core) = defs.iter_mut().find(|d| d.id == 1) {
        core.controlling = vec![6, 414, 38];
    }
    let mut light = def(55, "White Light", "WHITE_LIGHT", "General/Lights");
    (light.light, light.can_activate) = (true, true);
    let mut activation = def(405, "Activation Module", "ACTIVATION_MODULE", "General/Logic");
    (activation.logic, activation.can_activate) = (true, true);
    let mut door = def(122, "Plex Door", "DOOR", "General/Doors");
    (door.door, door.can_activate) = (true, true);
    let mut old = def(2, "Power (Old)", "POWER_OLD", "General/Power");
    old.deprecated = true;
    defs.extend([light, activation, door, old]);
    Catalog::from_defs(defs)
}

/// A full, placeable cube with no special traits. Read from JSON so that
/// traits `yard-core` adds later, which all default to off, need no change
/// here.
fn def(id: u16, name: &str, key: &str, category: &str) -> BlockDef {
    let plain = serde_json::json!({
        "id": id, "key": key, "name": name, "category": category, "shape": "cube", "slab": 0, "sides": 1,
        "textures": [0, 0, 0, 0, 0, 0], "controlling": [], "controlled_by": [], "base": 0, "style_ids": [], "slab_ids": [],
        "placeable": true, "deprecated": false, "can_activate": false, "logic": false, "system": false,
        "transparent": false, "light": false, "description": "",
    });
    serde_json::from_value(plain).expect("a plain block definition has every field the catalog requires")
}

#[cfg(test)]
mod tests {
    use super::*;
    use yard_core::edit::{FillMode, When};
    use yard_core::Block;

    fn world() -> MemoryWorld {
        MemoryWorld::new_ship(Arc::new(sample_catalog()), "Test Ship")
    }

    fn fill(min: Pos, max: Pos, ty: u16, link: Option<Pos>) -> Op {
        Op::Fill { min, max, block: Block::new(ty, 0, false), mode: FillMode::Solid, when: When::Empty, link }
    }

    #[test]
    fn a_new_ship_is_one_entity_holding_its_core() {
        let w = world();
        let entities = w.entities();
        assert_eq!(entities.len(), 1);
        let main = &entities[0];
        assert_eq!((main.path.as_str(), main.kind, main.blocks, main.docked_to.clone()), ("", EntityKind::Ship, 1, None));
        assert_eq!(main.bounds, Some((Pos::new(0, 0, 0), Pos::new(0, 0, 0))));
        assert_eq!(w.block("", Pos::new(0, 0, 0)).ty(), 1);
    }

    #[test]
    fn steps_follow_the_linking_rules_and_can_be_undone() {
        let w = world();
        let (computer, barrels) = (Pos::new(1, 0, 0), (Pos::new(1, 0, 1), Pos::new(1, 0, 4)));
        let change = w.step("", vec![fill(computer, computer, 6, Some(Pos::new(0, 0, 0))), fill(barrels.0, barrels.1, 16, Some(computer))]).unwrap();
        assert_eq!((change.cells.len(), change.linked.len()), (5, 5), "the core takes the computer, the computer its four barrels");
        // Hull is not something a cannon computer controls.
        let hull = Pos::new(2, 0, 0);
        w.step("", vec![fill(hull, hull, 598, None)]).unwrap();
        let refused = w.step("", vec![Op::Link { controller: computer, targets: vec![hull] }]).unwrap();
        assert!(refused.is_empty());
        w.step("", vec![Op::Undo]).unwrap();
        w.step("", vec![Op::Undo]).unwrap();
        assert_eq!(w.entities()[0].blocks, 1);
        assert_eq!(w.entities()[0].links, 0);
        assert_eq!(w.step("", vec![Op::Undo]).unwrap_err(), "There is nothing to undo.");
    }

    #[test]
    fn an_unknown_entity_is_an_error_the_model_can_act_on() {
        let w = world();
        assert!(w.step("ATTACHED_9", vec![]).unwrap_err().contains("no entity \"ATTACHED_9\""));
        assert!(w.with_entity("ATTACHED_9", &mut |_| {}).is_err());
    }

    #[test]
    fn the_sample_catalog_has_shapes_and_links_to_work_with() {
        let catalog = sample_catalog();
        let grey = catalog.by_name("Grey Basic Armor").unwrap();
        assert_eq!(grey.style_ids.len(), 4);
        assert_eq!(catalog.get(grey.style_ids[0]).unwrap().shape, Shape::Wedge);
        let rules = Rules::new(&catalog, Kind::Ship);
        assert!(rules.can_link(6, 16) && rules.core_takes(6) && !rules.can_link(6, 598));
    }
}
