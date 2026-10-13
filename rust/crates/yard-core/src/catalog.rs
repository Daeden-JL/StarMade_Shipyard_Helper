//! The game's block definitions, read from a StarMade install's
//! `data/config/BlockConfig.xml` and `BlockTypes.properties`.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Block geometry, in the order of the game's block style numbers (0..=6).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Cube,
    Wedge,
    Corner,
    Sprite,
    Tetra,
    Hepta,
    Rail,
}

impl Shape {
    /// The game's block style number for this shape.
    pub fn style(self) -> u8 {
        self as u8
    }

    pub fn from_style(style: i64) -> Self {
        match style {
            1 => Shape::Wedge,
            2 => Shape::Corner,
            3 => Shape::Sprite,
            4 => Shape::Tetra,
            5 => Shape::Hepta,
            6 => Shape::Rail,
            _ => Shape::Cube,
        }
    }
}

/// What reading a blueprint the way the game does needs to know about blocks:
/// old files are converted, and links are filtered, by block type.
pub trait BlockInfo {
    /// The block style number of a type (0 cube, 1 wedge, 2 corner, 3 sprite,
    /// 4 tetra, 5 hepta, 6 for blocks that turn 24 ways), or None if the game's
    /// block config has no such block.
    fn style(&self, ty: u16) -> Option<u8>;
}

impl BlockInfo for Catalog {
    fn style(&self, ty: u16) -> Option<u8> {
        self.get(ty).map(|d| d.shape.style())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockDef {
    pub id: u16,
    /// Config name such as `GREY_HULL`.
    pub key: String,
    pub name: String,
    /// Config path such as `General/Hulls/Basic/Grey`.
    pub category: String,
    pub shape: Shape,
    /// 0 = full block, 1..=3 = slabs (1 is three quarters high, 3 is one quarter).
    pub slab: u8,
    /// 1, 3 or 6 distinct side textures.
    pub sides: u8,
    /// Texture tile per side, in the game's side order: front, back, top, bottom, right, left.
    pub textures: [i32; 6],
    /// Block types this block can be linked to (as the controller).
    pub controlling: Vec<u16>,
    pub controlled_by: Vec<u16>,
    /// The full cube this is a shape or slab variant of, or 0.
    pub base: u16,
    pub style_ids: Vec<u16>,
    pub slab_ids: Vec<u16>,
    pub placeable: bool,
    pub deprecated: bool,
    pub can_activate: bool,
    pub logic: bool,
    pub system: bool,
    pub transparent: bool,
    pub light: bool,
    #[serde(default)]
    pub door: bool,
    /// Its side textures point where the block points
    /// (`SideTexturesPointToOrientation`).
    #[serde(default)]
    pub pointing: bool,
    /// The name of the 3D model the game draws in place of a cube (`LodShape`),
    /// or empty.
    #[serde(default)]
    pub model: String,
    /// The model drawn while the block is switched on, where that differs
    /// (`LodShapeSwitchStyleActive`), or empty.
    #[serde(default)]
    pub model_on: String,
    /// `SensorInput`: a sensor can read it.
    #[serde(default)]
    pub sensor: bool,
    /// `InventoryType`: 0 for none, 3 for storage and factories, 4 for shops.
    #[serde(default)]
    pub inventory: u8,
    /// The block is a factory (it has a `Factory` entry).
    #[serde(default)]
    pub factory: bool,
    /// The weapon computer roles of `MainCombinationController`,
    /// `SupportCombinationController` and `EffectCombinationController`.
    #[serde(default)]
    pub main_computer: bool,
    #[serde(default)]
    pub support_computer: bool,
    #[serde(default)]
    pub effect_computer: bool,
    /// A reactor chamber (`GeneralChamber`, or a `ChamberRoot` other than 0).
    #[serde(default)]
    pub chamber: bool,
    /// `LogicBlockButton`: a logic block that lets go by itself half a
    /// second after it is switched on.
    #[serde(default)]
    pub button: bool,
    /// `LogicSignaledByRail`: switched by a docked entity arriving over the
    /// rail block beside it.
    #[serde(default)]
    pub by_rail: bool,
    /// `HasActivationTexture`: its textures are those of the block switched
    /// on, and the next tile along is the block switched off.
    #[serde(default)]
    pub on_texture: bool,
    /// `Physical` is false: nothing runs into it.
    #[serde(default)]
    pub ghost: bool,
    pub description: String,
}

/// Storage, the factories, the shipyard computer and the pickup point: with
/// lights and doors, the blocks the game switches on the moment they are placed
/// (`ElementInformation.activateOnPlacement`).
const ON_WHEN_PLACED: [u16; 8] = [120, 211, 213, 215, 217, 259, 677, 937];

impl BlockDef {
    /// The state of the block's active bit when a player places it: lights are
    /// lit, doors are closed, storage is on; everything else starts off.
    pub fn active_when_placed(&self) -> bool {
        !self.logic && (self.light || self.door || ON_WHEN_PLACED.contains(&self.id))
    }
}

#[derive(Default)]
pub struct Catalog {
    by_id: BTreeMap<u16, BlockDef>,
    by_name: HashMap<String, u16>,
    links: HashMap<u16, crate::links::LinkTraits>,
}

impl crate::links::BlockLinks for Catalog {
    fn link_traits(&self, ty: u16) -> Option<&crate::links::LinkTraits> {
        self.links.get(&ty)
    }
}

impl Catalog {
    pub const CORE: u16 = 1;

    pub fn from_defs(defs: Vec<BlockDef>) -> Self {
        let mut c = Catalog::default();
        for d in defs {
            let lower = d.name.to_lowercase();
            // Several deprecated blocks share names with live ones; the live one wins.
            let replace = match c.by_name.get(&lower).and_then(|id| c.by_id.get(id)) {
                None => true,
                Some(existing) => existing.deprecated && !d.deprecated,
            };
            if replace {
                c.by_name.insert(lower, d.id);
            }
            c.links.insert(d.id, crate::links::LinkTraits::of(&d));
            c.by_id.insert(d.id, d);
        }
        c
    }

    /// Parses the two config files' contents.
    pub fn parse(block_config_xml: &str, block_types_properties: &str) -> Result<Self> {
        let mut ids: HashMap<&str, u16> = HashMap::new();
        for line in block_types_properties.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if let Ok(id) = v.trim().parse::<u16>() {
                    ids.insert(k.trim(), id);
                }
            }
        }

        let doc = roxmltree::Document::parse(block_config_xml)
            .map_err(|e| Error::Malformed { what: "BlockConfig.xml", detail: e.to_string() })?;
        let element = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name("Element"))
            .ok_or_else(|| Error::Malformed { what: "BlockConfig.xml", detail: "no <Element> section".into() })?;
        let mut defs = Vec::new();
        collect(element, String::new(), &ids, &mut defs);
        Ok(Self::from_defs(defs))
    }

    pub fn get(&self, id: u16) -> Option<&BlockDef> {
        self.by_id.get(&id)
    }

    pub fn by_name(&self, name: &str) -> Option<&BlockDef> {
        self.by_name.get(&name.trim().to_lowercase()).and_then(|id| self.by_id.get(id))
    }

    pub fn name(&self, id: u16) -> String {
        self.get(id).map_or_else(|| format!("unknown#{id}"), |d| d.name.clone())
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn all(&self) -> impl Iterator<Item = &BlockDef> {
        self.by_id.values()
    }

}

fn collect(parent: roxmltree::Node, path: String, ids: &HashMap<&str, u16>, out: &mut Vec<BlockDef>) {
    for node in parent.children().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        if tag == "Block" {
            if let Some(def) = parse_block(node, &path, ids) {
                out.push(def);
            }
        } else {
            let child = if path.is_empty() { tag.to_string() } else { format!("{path}/{tag}") };
            collect(node, child, ids, out);
        }
    }
}

fn parse_block(node: roxmltree::Node, category: &str, ids: &HashMap<&str, u16>) -> Option<BlockDef> {
    let key = node.attribute("type")?;
    let id = *ids.get(key)?;
    let text = |tag: &str| -> String {
        node.children()
            .find(|n| n.has_tag_name(tag))
            .map(|n| n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let int = |tag: &str| text(tag).parse::<i64>().unwrap_or(0);
    let flag = |tag: &str| text(tag).eq_ignore_ascii_case("true");
    let id_list = |tag: &str| -> Vec<u16> { text(tag).split(',').filter_map(|p| p.trim().parse().ok()).collect() };
    let refs = |tag: &str| -> Vec<u16> {
        node.children()
            .find(|n| n.has_tag_name(tag))
            .map(|list| {
                list.children()
                    .filter(|n| n.has_tag_name("Element"))
                    .filter_map(|n| n.text().and_then(|t| ids.get(t.trim()).copied()))
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut textures = [0i32; 6];
    for (i, part) in node.attribute("textureId").unwrap_or("").split(',').take(6).enumerate() {
        textures[i] = part.trim().parse().unwrap_or(0);
    }

    Some(BlockDef {
        id,
        key: key.to_string(),
        name: node.attribute("name").unwrap_or(key).to_string(),
        category: category.to_string(),
        shape: Shape::from_style(int("BlockStyle")),
        slab: int("Slab") as u8,
        sides: int("IndividualSides").max(1) as u8,
        textures,
        controlling: refs("Controlling"),
        controlled_by: refs("ControlledBy"),
        base: int("SourceReference") as u16,
        style_ids: id_list("StyleIds"),
        slab_ids: id_list("SlabIds"),
        placeable: flag("Placable"),
        deprecated: flag("Deprecated"),
        can_activate: flag("CanActivate"),
        logic: flag("LogicBlock"),
        system: flag("SystemBlock"),
        transparent: flag("Transparency"),
        light: flag("LightSource"),
        door: flag("Door"),
        pointing: flag("SideTexturesPointToOrientation"),
        model: text("LodShape"),
        model_on: text("LodShapeSwitchStyleActive"),
        sensor: flag("SensorInput"),
        inventory: int("InventoryType").clamp(0, 255) as u8,
        factory: !text("Factory").is_empty(),
        main_computer: flag("MainCombinationController"),
        support_computer: flag("SupportCombinationController"),
        effect_computer: flag("EffectCombinationController"),
        chamber: flag("GeneralChamber") || int("ChamberRoot") != 0,
        button: flag("LogicBlockButton"),
        by_rail: flag("LogicSignaledByRail"),
        on_texture: flag("HasActivationTexture"),
        ghost: text("Physical").eq_ignore_ascii_case("false"),
        description: text("Description"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Config><Element>
      <General><Hulls>
        <Block icon="1" name="Grey Basic Armor" textureId="33, 33, 33, 33, 33, 33" type="GREY_HULL">
          <Slab>0</Slab><StyleIds>599</StyleIds><SourceReference>0</SourceReference>
          <Placable>true</Placable><IndividualSides>1</IndividualSides>
          <BlockStyle>0<!--normal: A normal 6 sided block--></BlockStyle>
        </Block>
        <Block icon="2" name="Grey Basic Armor Wedge" textureId="33, 33, 33, 33, 33, 33" type="GREY_HULL_WEDGE">
          <SourceReference>598</SourceReference><Placable>true</Placable><BlockStyle>1<!--wedge--></BlockStyle>
        </Block>
      </Hulls></General>
      <Ship><Weapons>
        <Block name="Cannon Computer" textureId="373, 304, 371, 377, 305, 305" type="CANNON_COMPUTER">
          <ControlledBy><Element>SHIP_CORE</Element></ControlledBy>
          <Controlling><Element>CANNON_BARREL</Element></Controlling>
          <Placable>true</Placable><IndividualSides>6</IndividualSides><BlockStyle>0</BlockStyle>
        </Block>
        <Block name="Cannon Barrel" textureId="1,1,1,1,1,1" type="CANNON_BARREL">
          <ControlledBy><Element>CANNON_COMPUTER</Element></ControlledBy><Placable>true</Placable>
        </Block>
      </Weapons></Ship>
    </Element></Config>"#;
    const PROPS: &str = "# ids\nSHIP_CORE=1\nCANNON_COMPUTER=6\nCANNON_BARREL=16\nGREY_HULL=598\nGREY_HULL_WEDGE=599\n";

    #[test]
    fn parses_ids_categories_shapes_and_links() {
        let c = Catalog::parse(XML, PROPS).unwrap();
        assert_eq!(c.len(), 4);
        let hull = c.by_name("grey basic armor").unwrap();
        assert_eq!((hull.id, hull.category.as_str(), hull.shape), (598, "General/Hulls", Shape::Cube));
        assert_eq!(hull.style_ids, vec![599]);
        let wedge = c.get(599).unwrap();
        assert_eq!((wedge.shape, wedge.base), (Shape::Wedge, 598));
        assert_eq!(c.get(6).unwrap().textures, [373, 304, 371, 377, 305, 305]);
        assert_eq!((c.get(6).unwrap().controlling.clone(), c.get(16).unwrap().controlled_by.clone()), (vec![16], vec![6]));
    }
}
