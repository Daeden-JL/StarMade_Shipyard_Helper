//! What each block type looks like in the viewport, and which blocks are
//! shapes of which.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use yard_core::api::ClientBlock;
use yard_core::links::{self, BlockLinks, LinkTraits, Rules};
use yard_core::model::{self, Model, ModelSet};
use yard_core::shape::Geom;
use yard_core::catalog::Shape;
use yard_core::mesh::{Look, Looks, TexRule, NO_TILE};
use yard_core::shape::ShapeSet;
use yard_core::Block;

/// Colour for block types the catalog does not know (modded blocks).
const UNKNOWN_COLOUR: [u8; 3] = [200, 60, 200];
/// Block types are 11 bits in the file format.
const TYPES: usize = 2048;

#[derive(Clone, Copy)]
struct TypeLook {
    color: [u8; 3],
    transparent: bool,
    shape: Shape,
    slab: u8,
    tiles: [u16; 6],
    sides: u8,
    rule: TexRule,
    /// The 3D model drawn in place of a cube, and the one drawn while the
    /// block is switched on where that differs.
    model: Option<u16>,
    model_on: Option<u16>,
    /// A door or force field: not drawn while it stands open.
    door: bool,
    /// Its tiles show it switched on; switched off it shows the next tile along.
    on_texture: bool,
}

/// One of the game's 3D block models, with its faces in each turn it has been
/// drawn in so far.
struct ModelLook {
    model: Model,
    turns: [OnceCell<Geom>; model::TURNS],
}

/// Tiles per texture sheet.
const TILES_PER_PAGE: i32 = 256;

/// What placing needs to know about a block type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kind {
    pub shape: Shape,
    pub slab: u8,
    /// A slab, or a cube with a distinct front: its orientation is a direction.
    pub faces_matter: bool,
    /// The game places it switched on.
    pub active: bool,
}

pub struct BlockLooks {
    types: Vec<TypeLook>,
    shapes: ShapeSet,
    /// What an open door looks like.
    nothing: Geom,
    models: Vec<ModelLook>,
    kinds: HashMap<u16, Kind>,
    names: HashMap<u16, String>,
    /// What the linking rules need to know, by block type.
    links: HashMap<u16, LinkTraits>,
    /// (base cube, shape, slab) -> the block type that is that variant.
    variants: HashMap<(u16, Shape, u8), u16>,
    /// Block type -> (its base cube or itself, shape, slab).
    families: HashMap<u16, (u16, Shape, u8)>,
    /// The block types filed under the game's hulls (armour and glass) or
    /// lights: what a brush limited to armour and lights may paint.
    armor_or_light: HashSet<u16>,
}

impl Default for BlockLooks {
    fn default() -> Self {
        Self {
            types: vec![
                TypeLook { color: UNKNOWN_COLOUR, transparent: false, shape: Shape::Cube, slab: 0, tiles: [NO_TILE; 6], sides: 1, rule: TexRule::Upright, model: None, model_on: None, door: false, on_texture: false };
                TYPES
            ],
            shapes: ShapeSet::new(),
            nothing: Geom::default(),
            models: Vec::new(),
            kinds: HashMap::new(),
            names: HashMap::new(),
            links: HashMap::new(),
            variants: HashMap::new(),
            families: HashMap::new(),
            armor_or_light: HashSet::new(),
        }
    }
}

/// What moving a core needs to know about blocks, as the server's catalog
/// gives it: how a block turns, and which keep data by position.
impl yard_core::relocate::Aid for BlockLooks {
    fn turned(&self, ty: u16, o: u8, turn: &yard_core::shape::GridTurn) -> u8 {
        match self.kinds.get(&ty) {
            Some(k) => ShapeSet::turned(k.shape, o, k.faces_matter, turn),
            None => o,
        }
    }

    fn keeps_data(&self, ty: u16) -> bool {
        // Systems, logic, computers, storage, factories, chambers and
        // sensors, and the display and cargo blocks: what the server counts.
        const DISPLAY: u16 = 479;
        const CARGO: u16 = 689;
        use links::flag;
        match self.links.get(&ty) {
            None => ty != 0,
            Some(t) => {
                t.inventory != 0
                    || ty == DISPLAY
                    || ty == CARGO
                    || !t.controls.is_empty()
                    || !t.controlled_by.is_empty()
                    || t.is(flag::SIGNAL | flag::CAN_ACTIVATE | flag::SENSOR | flag::FACTORY | flag::MAIN | flag::SUPPORT | flag::EFFECT | flag::CHAMBER | flag::BUTTON)
            }
        }
    }
}

impl BlockLooks {
    pub fn load(&mut self, blocks: &[ClientBlock]) {
        for b in blocks {
            if let Some(slot) = self.types.get_mut(b.id as usize) {
                let tiles = b.textures.map(|t| u16::try_from(t).ok().filter(|t| *t != NO_TILE).unwrap_or(NO_TILE));
                let rule = match b.shape {
                    Shape::Cube => TexRule::Cube { pointing: b.pointing },
                    Shape::Rail => TexRule::Turned,
                    _ => TexRule::Upright,
                };
                *slot = TypeLook { color: b.color, transparent: b.transparent, shape: b.shape, slab: b.slab, tiles, sides: b.sides, rule, model: b.model, model_on: b.model_on, door: b.links.is(links::flag::DOOR), on_texture: b.links.is(links::flag::ON_TEXTURE) };
            }
            let faces_matter = b.shape == Shape::Cube && (b.slab > 0 || b.sides == 6);
            self.kinds.insert(b.id, Kind { shape: b.shape, slab: b.slab, faces_matter, active: b.active });
            self.names.insert(b.id, b.name.clone());
            self.links.insert(b.id, b.links.clone());
            let base = if b.base != 0 { b.base } else { b.id };
            self.variants.insert((base, b.shape, b.slab), b.id);
            self.families.insert(b.id, (base, b.shape, b.slab));
            if ["General/Hulls", "General/Light"].iter().any(|c| b.category == *c || b.category.starts_with(&format!("{c}/"))) {
                self.armor_or_light.insert(b.id);
            }
        }
    }

    /// Takes in the game's 3D block models. Blocks whose model is missing
    /// from the set are drawn as cubes.
    pub fn load_models(&mut self, set: ModelSet) {
        self.models = set.models.into_iter().map(|model| ModelLook { model, turns: std::array::from_fn(|_| OnceCell::new()) }).collect();
    }

    /// How many texture sheets the loaded blocks and models draw from.
    pub fn texture_pages(blocks: &[ClientBlock], models: &ModelSet) -> u32 {
        let highest = blocks.iter().flat_map(|b| b.textures).max().unwrap_or(-1);
        let for_blocks = if highest < 0 { 0 } else { (highest / TILES_PER_PAGE + 1) as u32 };
        models.pages.iter().map(|page| *page as u32 + 1).max().unwrap_or(0).max(for_blocks)
    }

    pub fn kind(&self, ty: u16) -> Kind {
        self.kinds.get(&ty).copied().unwrap_or(Kind { shape: Shape::Cube, slab: 0, faces_matter: false, active: false })
    }

    pub fn shapes(&self) -> &ShapeSet {
        &self.shapes
    }

    pub fn name(&self, ty: u16) -> String {
        self.known_name(ty).unwrap_or_else(|| format!("Block {ty}"))
    }

    /// The block's name, or None for a type the game's block list does not have.
    pub fn known_name(&self, ty: u16) -> Option<String> {
        self.names.get(&ty).cloned()
    }

    /// The game's linking rules for an entity of the given header type.
    pub fn rules(&self, entity_type: i32) -> Rules<'_> {
        Rules::new(self, links::Kind::of_entity_type(entity_type))
    }

    /// The material of a block, named by the plain full block of its colour
    /// (the block it is a shape or slab of), or nothing for a block that is
    /// not painted, a computer or a door for example.
    /// Whether the block type is armour (a hull block, glass among them) or a light.
    pub fn is_armor_or_light(&self, ty: u16) -> bool {
        self.armor_or_light.contains(&ty)
    }

    pub fn material(&self, ty: u16) -> Option<u16> {
        self.families.get(&ty).map(|(base, _, _)| *base)
    }

    /// For swapping material `from` for material `to` (each named by its plain
    /// full block): each block type of `from` with the type of `to` in the same
    /// shape, and the types of `from` that `to` has no shape for.
    pub fn swap_pairs(&self, from: u16, to: u16) -> (Vec<(u16, u16)>, Vec<u16>) {
        let (mut pairs, mut missing) = (Vec::new(), Vec::new());
        let mut own: Vec<(u16, Shape, u8)> = self.families.iter().filter(|(_, f)| f.0 == from).map(|(id, f)| (*id, f.1, f.2)).collect();
        own.sort_by_key(|f| f.0);
        for (id, shape, slab) in own {
            match self.variants.get(&(to, shape, slab)) {
                Some(&target) => pairs.push((id, target)),
                None => missing.push(id),
            }
        }
        (pairs, missing)
    }

    /// The block `existing` becomes when given another shape of its own
    /// material, turned as asked. None if the material has no such shape.
    pub fn reshape(&self, existing: Block, shape: Shape, slab: u8, orientation: u8) -> Option<Block> {
        let (base, _, _) = *self.families.get(&existing.ty())?;
        let ty = *self.variants.get(&(base, shape, slab))?;
        let mut out = existing;
        out.set_type(ty);
        out.set_orientation(orientation);
        Some(out)
    }

    /// The block `existing` becomes when painted with `paint`: the paint's
    /// material in the existing block's shape, keeping its rotation. None when
    /// the material has no such shape.
    pub fn repaint(&self, existing: Block, paint: u16) -> Option<Block> {
        let (paint_base, _, _) = *self.families.get(&paint)?;
        let (_, shape, slab) = self.families.get(&existing.ty()).copied().unwrap_or((0, Shape::Cube, 0));
        let ty = *self.variants.get(&(paint_base, shape, slab))?;
        let mut out = existing;
        out.set_type(ty);
        Some(out)
    }
}

impl yard_core::collide::Shapes for BlockLooks {
    fn geom(&self, block: Block) -> Option<&Geom> {
        let traits = self.links.get(&block.ty());
        let (door, ghost) = (traits.is_some_and(|t| t.is(links::flag::DOOR)), traits.is_some_and(|t| t.is(links::flag::GHOST)));
        (!yard_core::collide::passes_through(block, door, ghost)).then(|| self.look(block).geom)
    }
}

impl BlockLinks for BlockLooks {
    fn link_traits(&self, ty: u16) -> Option<&LinkTraits> {
        self.links.get(&ty)
    }
}

impl Looks for BlockLooks {
    fn look(&self, block: Block) -> Look<'_> {
        let mut t = self.types[block.ty() as usize % TYPES];
        if !block.active_bit() {
            if t.door {
                // The game does not draw a door that stands open, and what
                // is behind it shows.
                return Look { color: t.color, transparent: true, geom: &self.nothing, tiles: t.tiles, sides: t.sides, rule: t.rule };
            }
            if t.on_texture {
                t.tiles = t.tiles.map(|tile| if tile == NO_TILE { tile } else { tile + 1 });
            }
        }
        // The game picks the switched-on model by the block's active bit.
        let model = if block.active_bit() { t.model_on.or(t.model) } else { t.model };
        if let Some(look) = model.and_then(|m| self.models.get(m as usize)) {
            let turn = model::turn(t.shape, block.ty(), block.orientation());
            return Look {
                color: t.color,
                // Up close the game draws these models solid, whatever the block's own setting.
                transparent: false,
                geom: look.turns[turn].get_or_init(|| look.model.geom(turn)),
                tiles: t.tiles,
                sides: t.sides,
                rule: TexRule::Upright,
            };
        }
        Look {
            color: t.color,
            transparent: t.transparent,
            geom: self.shapes.geom(t.shape, t.slab, block.orientation()),
            tiles: t.tiles,
            sides: t.sides,
            rule: t.rule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: u16, name: &str, shape: Shape, slab: u8, base: u16) -> ClientBlock {
        ClientBlock {
            id,
            name: name.into(),
            category: "General/Hulls".into(),
            shape,
            color: [1, 2, 3],
            transparent: false,
            light: false,
            placeable: true,
            slab,
            base,
            sides: 1,
            active: false,
            textures: [id as i32, -1, -1, -1, -1, -1],
            pointing: false,
            links: Default::default(),
            model: None,
            model_on: None,
        }
    }

    fn looks() -> BlockLooks {
        let mut looks = BlockLooks::default();
        looks.load(&[
            block(5, "Grey Armor", Shape::Cube, 0, 0),
            block(293, "Grey Armor Wedge", Shape::Wedge, 0, 5),
            block(702, "Grey Armor 1/2", Shape::Cube, 2, 5),
            block(77, "Blue Armor", Shape::Cube, 0, 0),
            block(298, "Blue Armor Wedge", Shape::Wedge, 0, 77),
            block(6, "Cannon Computer", Shape::Cube, 0, 0),
        ]);
        looks
    }

    #[test]
    fn painting_keeps_shape_and_rotation_and_changes_material() {
        let looks = looks();
        let wedge = Block::new(293, 7, false);
        let painted = looks.repaint(wedge, 77).unwrap();
        assert_eq!((painted.ty(), painted.orientation()), (298, 7));
        // Choosing the wedge of a material paints with that material, not with wedges.
        assert_eq!(looks.repaint(Block::new(5, 0, false), 298).unwrap().ty(), 77);
        // Blue armor has no half slab here, and a computer has no wedge: left alone.
        assert_eq!(looks.repaint(Block::new(702, 2, false), 77), None);
        assert_eq!(looks.repaint(wedge, 6), None);
        // A block the catalog does not know is treated as a plain cube.
        assert_eq!(looks.repaint(Block::new(1999, 0, false), 77).unwrap().ty(), 77);
    }

    #[test]
    fn armour_and_lights_are_told_from_other_blocks_by_their_category() {
        let mut looks = BlockLooks::default();
        let filed = |id: u16, category: &str| ClientBlock { category: category.into(), ..block(id, "Block", Shape::Cube, 0, 0) };
        looks.load(&[
            filed(5, "General/Hulls/Basic/Grey"),
            filed(63, "General/Hulls/Basic/Glass"),
            filed(55, "General/Light/Cube"),
            filed(6, "Ship/Weapons"),
            filed(405, "General/Doors/Basic"),
            filed(9, "General/HullsOfOld"),
        ]);
        assert!(looks.is_armor_or_light(5) && looks.is_armor_or_light(63) && looks.is_armor_or_light(55));
        assert!(!looks.is_armor_or_light(6) && !looks.is_armor_or_light(405) && !looks.is_armor_or_light(9));
        assert!(!looks.is_armor_or_light(1999));
    }

    #[test]
    fn a_block_with_a_model_is_drawn_as_that_model_turned_by_its_orientation() {
        let mut looks = BlockLooks::default();
        let mut bar = block(977, "White Light Bar", Shape::Rail, 0, 0);
        bar.transparent = true;
        bar.model = Some(0);
        let mut button = block(399, "Small Button", Shape::Rail, 0, 0);
        (button.model, button.model_on) = (Some(0), Some(1));
        // A model the server could not read: the block falls back to a cube.
        let mut lost = block(1003, "Grate", Shape::Rail, 0, 0);
        lost.model = Some(9);
        looks.load(&[bar, button, lost]);
        let triangle = |page: u16, y: i16| Model { page, points: vec![-512, y, -512, 512, y, -512, 0, y, 512], uvs: vec![0, 0, 65535, 0, 32768, 65535], triangles: vec![0, 2, 1] };
        looks.load_models(ModelSet { models: vec![triangle(40, -384), triangle(40, -448)], pages: vec![40] });
        // As placed, the triangle lies an eighth of a block above the floor.
        let placed = looks.look(Block::new(977, 14, false));
        assert_eq!(placed.geom.faces.len(), 1);
        assert!(placed.geom.faces[0].corners.iter().all(|c| c[1] == 0.125));
        assert!(!placed.transparent && placed.geom.full_sides == 0);
        // Turned to hang from the ceiling.
        assert!(looks.look(Block::new(977, 8, false)).geom.faces[0].corners.iter().all(|c| c[1] == 0.875));
        // The button swaps to its other model while switched on.
        assert!(looks.look(Block::new(399, 14, true)).geom.faces[0].corners.iter().all(|c| c[1] == 0.0625));
        assert!(looks.look(Block::new(399, 14, false)).geom.faces[0].corners.iter().all(|c| c[1] == 0.125));
        assert_eq!(looks.look(Block::new(1003, 14, false)).geom.faces.len(), 6);
        assert_eq!(BlockLooks::texture_pages(&[], &ModelSet { models: vec![], pages: vec![40, 41] }), 42);
    }

    #[test]
    fn shapes_come_from_type_and_rotation() {
        let looks = looks();
        assert_eq!(looks.look(Block::new(5, 3, false)).geom.faces.len(), 6);
        assert_eq!(looks.look(Block::new(293, 0, false)).geom.faces.len(), 5);
        let slab = looks.look(Block::new(702, 2, false)).geom;
        assert!(slab.fills(yard_core::shape::BOTTOM) && !slab.fills(yard_core::shape::TOP));
        assert!(looks.kind(702).faces_matter && !looks.kind(5).faces_matter);
        assert_eq!(looks.name(1999), "Block 1999");
    }
}
