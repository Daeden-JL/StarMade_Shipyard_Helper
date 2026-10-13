//! Turning a model's description of a block ("Grey Basic Armor", wedge,
//! slope up and back) into the block to place.

use super::orient::{self, Dir};
use super::Args;
use crate::schema;
use serde_json::Value;
use yard_core::catalog::{BlockDef, Catalog, Shape};
use yard_core::Block;

/// The shape words the tools take: the block styles, and the three slabs.
pub const SHAPES: [&str; 8] = ["cube", "wedge", "corner", "tetra", "hepta", "slab_3/4", "slab_1/2", "slab_1/4"];

/// Most candidates listed when a name fits several blocks.
const MAX_SUGGESTIONS: usize = 8;

/// The arguments that say which block to place and how it is turned. Every
/// tool that places blocks takes them.
pub fn properties() -> Vec<(&'static str, Value)> {
    vec![
        ("block", schema::string("The block: its exact name from find_blocks (such as \"Grey Basic Armor\") or its id.")),
        ("shape", schema::choice("Shape variant of the block, where it has one (find_blocks says). Default cube. Slabs are thin plates.", &SHAPES)),
        (
            "slope",
            schema::array(
                "For wedge, corner, tetra and hepta: the directions the slanted face looks toward. Wedge: two, e.g. [\"up\", \"back\"] is a ramp rising toward the front (solid bottom and front). Tetra and hepta: three, naming the cut-off corner. Corner: three, first the direction its point rises in. Remember left is +X.",
                schema::choice("A direction.", &orient::WORDS),
                3,
            ),
        ),
        ("facing", schema::choice("For blocks with a front (computers, slabs, rails): the direction the front, a slab's open side or a rail's arrow face looks toward. left is +X, right is -X.", &orient::WORDS)),
        ("pointing", schema::choice("For rails and dockers only: where the arrow on the arrow face points. Default front.", &orient::WORDS)),
        ("orientation", schema::integer_in("The game's raw orientation number (0-23), instead of slope and facing. Only if you know it, e.g. from get_block.", 0, 23)),
    ]
}

/// Whether a block shows which way it is turned although it is a cube: a
/// slab, or a block with six different sides.
pub fn faces_matter(def: &BlockDef) -> bool {
    def.shape == Shape::Cube && (def.slab > 0 || def.sides == 6)
}

pub fn is_air_word(text: &str) -> bool {
    matches!(text.trim().to_ascii_lowercase().as_str(), "air" | "empty" | "none" | "nothing" | "0")
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '/').filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// Whether a block can be chosen for placing at all.
fn usable(def: &BlockDef) -> bool {
    def.placeable && !def.deprecated && def.id != Catalog::CORE
}

/// Blocks whose name, config key or category holds every word of `query`,
/// best first: full blocks before their shape variants, short names before
/// long ones.
pub fn search<'a>(catalog: &'a Catalog, query: &str, limit: usize) -> Vec<&'a BlockDef> {
    let wanted = words(query);
    let mut hits: Vec<(usize, &BlockDef)> = catalog
        .all()
        .filter(|d| usable(d))
        .filter_map(|d| {
            let name = words(&d.name);
            let other = words(&format!("{} {}", d.key.replace('_', " "), d.category.replace('/', " ")));
            let in_name = wanted.iter().filter(|w| name.iter().any(|n| n.contains(w.as_str()))).count();
            let all = wanted.iter().all(|w| name.iter().chain(other.iter()).any(|n| n.contains(w.as_str())));
            // Fewer words missing from the name and fewer extra words in it rank higher.
            (all && !wanted.is_empty()).then(|| ((wanted.len() - in_name) * 100 + name.len() + if is_variant(d) { 50 } else { 0 }, d))
        })
        .collect();
    hits.sort_by_key(|(score, d)| (*score, d.id));
    hits.into_iter().take(limit).map(|(_, d)| d).collect()
}

fn is_variant(def: &BlockDef) -> bool {
    def.base != 0 || def.slab > 0 || def.shape != Shape::Cube && def.shape != Shape::Rail && def.shape != Shape::Sprite
}

/// The block a model means by a name or id. A name that fits several
/// blocks equally well is refused with the candidates listed.
pub fn lookup<'a>(catalog: &'a Catalog, text: &str) -> Result<&'a BlockDef, String> {
    let text = text.trim();
    let found = if let Ok(id) = text.parse::<u16>() {
        catalog.get(id).ok_or_else(|| format!("There is no block with id {id}. Use find_blocks to search by name."))?
    } else if let Some(def) = catalog.by_name(text) {
        def
    } else if let Some(def) = catalog.all().find(|d| d.key.eq_ignore_ascii_case(&text.replace(' ', "_"))) {
        def
    } else {
        let hits = search(catalog, text, MAX_SUGGESTIONS);
        let plain: Vec<&&BlockDef> = hits.iter().filter(|d| !is_variant(d)).collect();
        match (hits.len(), plain.len()) {
            (0, _) => return Err(format!("There is no block called \"{text}\". Use find_blocks to search by keyword, then use the exact name.")),
            (1, _) => hits[0],
            (_, 1) => plain[0],
            _ => {
                let names: Vec<String> = hits.iter().map(|d| format!("{} (id {})", d.name, d.id)).collect();
                return Err(format!("\"{text}\" fits several blocks: {}. Use the exact name or id.", names.join(", ")));
            }
        }
    };
    if found.id == Catalog::CORE {
        return Err("The Ship Core is already at (0, 0, 0); a ship has exactly one and it cannot be placed, moved or removed.".to_string());
    }
    if !usable(found) {
        return Err(format!("{} cannot be placed: the game no longer uses it or it is not a building block. Use find_blocks to pick another.", found.name));
    }
    Ok(found)
}

/// The block style and slab number a shape word stands for.
fn shape_of(word: &str) -> Option<(Shape, u8)> {
    Some(match word {
        "cube" => (Shape::Cube, 0),
        "wedge" => (Shape::Wedge, 0),
        "corner" => (Shape::Corner, 0),
        "tetra" => (Shape::Tetra, 0),
        "hepta" => (Shape::Hepta, 0),
        "slab_3/4" => (Shape::Cube, 1),
        "slab_1/2" => (Shape::Cube, 2),
        "slab_1/4" => (Shape::Cube, 3),
        _ => return None,
    })
}

/// The full block a shape variant belongs to; a full block is its own.
fn base_of<'a>(catalog: &'a Catalog, def: &'a BlockDef) -> &'a BlockDef {
    if def.base != 0 { catalog.get(def.base).unwrap_or(def) } else { def }
}

fn find_variant<'a>(catalog: &'a Catalog, base: &'a BlockDef, wanted: (Shape, u8)) -> Option<&'a BlockDef> {
    let fits = |d: &&BlockDef| (d.shape, d.slab) == wanted && usable(d);
    let listed = base.style_ids.iter().chain(&base.slab_ids).filter_map(|id| catalog.get(*id));
    std::iter::once(base)
        .chain(listed)
        .find(fits)
        // Block lists that do not name their variants still point back at the base.
        .or_else(|| catalog.all().filter(|d| d.base == base.id).find(fits))
}

/// The variant of a block in another shape: the wedge of a hull colour, its
/// half slab. `shape` is one of [`SHAPES`].
pub fn variant<'a>(catalog: &'a Catalog, def: &'a BlockDef, shape: &str) -> Result<&'a BlockDef, String> {
    let base = base_of(catalog, def);
    let wanted = shape_of(shape).ok_or_else(|| format!("\"{shape}\" is not a shape. Use one of: {}.", SHAPES.join(", ")))?;
    find_variant(catalog, base, wanted).ok_or_else(|| format!("{} has no {shape} variant.{}", base.name, variants_note(catalog, base)))
}

/// The shape words a block's variants answer to, other than its own.
pub fn variant_words(catalog: &Catalog, def: &BlockDef) -> Vec<&'static str> {
    let base = base_of(catalog, def);
    SHAPES.into_iter().filter(|word| shape_of(word).and_then(|wanted| find_variant(catalog, base, wanted)).is_some_and(|v| v.id != def.id)).collect()
}

fn variants_note(catalog: &Catalog, def: &BlockDef) -> String {
    let have = variant_words(catalog, def);
    if have.is_empty() { " It comes in one shape only.".to_string() } else { format!(" It comes as: {}.", have.join(", ")) }
}

fn dir_arg(args: &Args<'_>, name: &str) -> Result<Option<Dir>, String> {
    args.text(name).map(Dir::parse).transpose()
}

/// The block the arguments describe, turned as they say. `field` names the
/// argument holding the block's name. With `allow_air`, the word "air"
/// stands for removing.
pub fn resolve(catalog: &Catalog, args: &Args<'_>, field: &str, allow_air: bool) -> Result<Block, String> {
    let text = args.text(field).ok_or_else(|| format!("Missing argument \"{field}\"."))?;
    if is_air_word(text) {
        return if allow_air { Ok(Block::AIR) } else { Err(format!("\"{field}\" must be a block here, not air. Use the remove tool to take blocks away.")) };
    }
    let mut def = lookup(catalog, text)?;
    if let Some(shape) = args.text("shape") {
        def = variant(catalog, def, shape)?;
    }
    let orientation = orientation(def, args)?;
    Ok(Block::new(def.id, orientation, def.active_when_placed()))
}

/// How the arguments turn a block of this type.
fn orientation(def: &BlockDef, args: &Args<'_>) -> Result<u8, String> {
    if let Some(raw) = args.int("orientation") {
        return Ok(raw as u8);
    }
    let slope: Option<Vec<Dir>> = match args.get("slope").and_then(Value::as_array) {
        Some(words) => Some(words.iter().filter_map(Value::as_str).map(Dir::parse).collect::<Result<_, _>>()?),
        None => None,
    };
    let (facing, pointing) = (dir_arg(args, "facing")?, dir_arg(args, "pointing")?);
    match (def.shape, slope) {
        (shape, Some(slope)) if orient::is_sloped(shape) => orient::sloped(shape, &slope),
        (_, Some(_)) => Err(format!("{} has no slanted face, so \"slope\" does not apply. Add \"shape\": \"wedge\" (or corner, tetra, hepta), or leave slope out.", def.name)),
        (Shape::Rail, None) => orient::rail(facing.unwrap_or(Dir::Up), pointing),
        (shape, None) if orient::is_sloped(shape) => Ok(0),
        (_, None) => Ok(facing.map_or(0, orient::facing)),
    }
}

/// A block in words: its name, id, and how it is turned.
pub fn describe(catalog: &Catalog, block: Block) -> String {
    if block.is_air() {
        return "empty".to_string();
    }
    let Some(def) = catalog.get(block.ty()) else { return format!("unknown block (id {})", block.ty()) };
    let turned = orient::describe(def.shape, faces_matter(def), block.orientation());
    if turned.is_empty() { format!("{} (id {})", def.name, def.id) } else { format!("{} (id {}, {turned}, orientation {})", def.name, def.id, block.orientation()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::sample_catalog;
    use serde_json::json;

    fn resolved(args: Value) -> Result<Block, String> {
        resolve(&sample_catalog(), &Args::of(&args), "block", true)
    }

    #[test]
    fn blocks_are_found_by_name_id_key_and_unambiguous_words() {
        let catalog = sample_catalog();
        assert_eq!(lookup(&catalog, "Grey Basic Armor").unwrap().id, 598);
        assert_eq!(lookup(&catalog, " grey basic armor ").unwrap().id, 598);
        assert_eq!(lookup(&catalog, "598").unwrap().id, 598);
        assert_eq!(lookup(&catalog, "RED_HULL").unwrap().id, 620);
        assert_eq!(lookup(&catalog, "red hull").unwrap().id, 620, "the config key answers to words the name lacks");
        assert_eq!(lookup(&catalog, "cannon computer").unwrap().id, 6);
        assert_eq!(lookup(&catalog, "glass").unwrap().id, 63);
        assert_eq!(lookup(&catalog, "grey armor wedge").unwrap().id, 599);
    }

    #[test]
    fn names_that_fit_nothing_or_several_blocks_are_refused_with_help() {
        let catalog = sample_catalog();
        assert!(lookup(&catalog, "unobtainium").unwrap_err().contains("find_blocks"));
        let several = lookup(&catalog, "basic armor").unwrap_err();
        assert!(several.contains("fits several blocks") && several.contains("Grey Basic Armor (id 598)") && several.contains("Red Basic Armor (id 620)"), "{several}");
        assert!(lookup(&catalog, "9999").unwrap_err().contains("no block with id 9999"));
        assert!(lookup(&catalog, "Ship Core").unwrap_err().contains("exactly one"));
        assert!(lookup(&catalog, "Power (Old)").unwrap_err().contains("cannot be placed"));
    }

    #[test]
    fn a_search_lists_full_blocks_before_their_variants() {
        let catalog = sample_catalog();
        let names: Vec<&str> = search(&catalog, "grey", 3).into_iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names[0], "Grey Basic Armor");
        assert!(search(&catalog, "weapons", 10).iter().any(|d| d.name == "Cannon Barrel"), "categories are searched too");
        assert!(search(&catalog, "old", 10).is_empty(), "blocks the game dropped are not offered");
    }

    #[test]
    fn shape_variants_are_reached_from_the_base_block_or_from_each_other() {
        let catalog = sample_catalog();
        let grey = catalog.get(598).unwrap();
        assert_eq!(variant(&catalog, grey, "wedge").unwrap().id, 599);
        assert_eq!(variant(&catalog, grey, "slab_1/2").unwrap().slab, 2);
        let wedge = catalog.get(599).unwrap();
        assert_eq!(variant(&catalog, wedge, "corner").unwrap().id, 600);
        assert_eq!(variant(&catalog, wedge, "cube").unwrap().id, 598);
        let none = variant(&catalog, catalog.get(63).unwrap(), "wedge").unwrap_err();
        assert_eq!(none, "Glass has no wedge variant. It comes in one shape only.");
        assert_eq!(variant_words(&catalog, grey), ["wedge", "corner", "tetra", "hepta", "slab_3/4", "slab_1/2", "slab_1/4"]);
    }

    #[test]
    fn a_described_block_comes_out_with_the_right_type_and_turn() {
        assert_eq!(resolved(json!({"block": "Grey Basic Armor"})).unwrap(), Block::new(598, 0, false));
        let ramp = resolved(json!({"block": "Grey Basic Armor", "shape": "wedge", "slope": ["up", "back"]})).unwrap();
        assert_eq!((ramp.ty(), ramp.orientation()), (599, 0));
        let other = resolved(json!({"block": "Grey Basic Armor Wedge", "slope": ["down", "left"]})).unwrap();
        assert_eq!(orient::slope_of(Shape::Wedge, other.orientation()).unwrap(), [Dir::Left, Dir::Down]);
        let raw = resolved(json!({"block": "Grey Basic Armor", "shape": "corner", "orientation": 17})).unwrap();
        assert_eq!((raw.ty(), raw.orientation()), (600, 17));
        let slab = resolved(json!({"block": "Red Basic Armor", "shape": "slab_1/4", "facing": "up"})).unwrap();
        assert_eq!(slab.orientation(), orient::facing(Dir::Up));
        let computer = resolved(json!({"block": "Cannon Computer", "facing": "back"})).unwrap();
        assert_eq!((computer.ty(), computer.orientation()), (6, 1));
        let rail = resolved(json!({"block": "Rail Basic"})).unwrap();
        assert_eq!(rail.orientation(), 14, "arrow face up, pointing forward, as the editor places rails");
        let light = resolved(json!({"block": "White Light"})).unwrap();
        assert!(light.active_bit(), "lights are placed switched on, as in the game");
        assert_eq!(resolved(json!({"block": "air"})).unwrap(), Block::AIR);
    }

    #[test]
    fn a_turn_that_does_not_fit_the_block_is_explained() {
        assert!(resolved(json!({"block": "Glass", "slope": ["up", "back"]})).unwrap_err().contains("no slanted face"));
        assert!(resolved(json!({"block": "Grey Basic Armor", "shape": "wedge", "slope": ["up"]})).unwrap_err().contains("needs 2 slope directions"));
        assert!(resolved(json!({"block": "Glass", "shape": "wedge"})).unwrap_err().contains("no wedge variant"));
        assert!(resolve(&sample_catalog(), &Args::of(&json!({"block": "air"})), "block", false).unwrap_err().contains("remove tool"));
    }

    #[test]
    fn blocks_are_described_with_their_turn() {
        let catalog = sample_catalog();
        assert_eq!(describe(&catalog, Block::new(598, 0, false)), "Grey Basic Armor (id 598)");
        assert_eq!(describe(&catalog, Block::new(599, 0, false)), "Grey Basic Armor Wedge (id 599, slope up, back, orientation 0)");
        assert_eq!(describe(&catalog, Block::AIR), "empty");
    }
}
