//! Checks against files written by the game. They run only on a machine with a
//! StarMade install; point STARMADE_DIR at one, or the usual test install is used.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use yard_core::blueprint::FileTree;
use yard_core::catalog::Catalog;
use yard_core::Blueprint;

fn install() -> Option<PathBuf> {
    let dir = std::env::var_os("STARMADE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?;
    dir.join("data/config/BlockConfig.xml").exists().then_some(dir)
}

fn read_tree(dir: &Path, prefix: &str, out: &mut FileTree) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if entry.path().is_dir() {
            read_tree(&entry.path(), &path, out);
        } else {
            out.insert(path, fs::read(entry.path()).unwrap());
        }
    }
}

fn blueprints(install: &Path) -> Vec<(String, FileTree)> {
    let mut out = Vec::new();
    for folder in ["blueprints", "blueprints-default"] {
        let Ok(entries) = fs::read_dir(install.join(folder)) else { continue };
        for entry in entries.flatten().filter(|e| e.path().join("header.smbph").exists()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let mut tree = FileTree::new();
            read_tree(&entry.path(), &name, &mut tree);
            out.push((format!("{folder}/{name}"), tree));
        }
    }
    out
}

#[test]
fn block_counts_match_what_the_game_wrote_in_the_header() {
    let Some(install) = install() else { return };
    let all = blueprints(&install);
    assert!(!all.is_empty(), "install has no blueprints to test against");
    for (label, tree) in all {
        let bp = Blueprint::from_files(&tree).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert!(bp.warnings.is_empty(), "{label}: {:?}", bp.warnings);
        for (path, e) in bp.entities() {
            let parsed: BTreeMap<i16, i32> =
                e.entity.counts_by_type().into_iter().map(|(t, n)| (t as i16, n as i32)).collect();
            let header: BTreeMap<i16, i32> = e.header.counts.iter().copied().collect();
            assert_eq!(parsed, header, "{label} [{path}]: block counts differ from the header");
        }
    }
}

#[test]
fn every_link_joins_two_real_blocks_of_the_recorded_type() {
    let Some(install) = install() else { return };
    let mut links = 0;
    for (label, tree) in blueprints(&install) {
        let bp = Blueprint::from_files(&tree).unwrap();
        for (path, e) in bp.entities() {
            for (controller, groups) in e.entity.links() {
                assert!(!e.entity.get(*controller).is_air(), "{label} [{path}]: controller {controller:?} is empty");
                for (ty, targets) in groups {
                    for t in targets {
                        assert_eq!(e.entity.get(*t).ty(), *ty, "{label} [{path}]: link target {t:?}");
                        links += 1;
                    }
                }
            }
        }
    }
    assert!(links > 0, "expected the sample ships to contain links");
}

#[test]
fn writing_and_reading_back_changes_nothing() {
    let Some(install) = install() else { return };
    for (label, tree) in blueprints(&install) {
        let bp = Blueprint::from_files(&tree).unwrap();
        let written = bp.to_files().unwrap();

        // Everything except block data must come back byte for byte.
        for (path, original) in &tree {
            if path.ends_with(".smd3") {
                continue;
            }
            let ours = written.get(path).unwrap_or_else(|| panic!("{label}: {path} missing after write"));
            assert_eq!(ours, original, "{label}: {path} differs after a round trip");
        }
        // Block data is recompressed, so compare contents rather than bytes.
        let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
        let (a, b) = (bp.entities(), again.entities());
        assert_eq!(a.len(), b.len(), "{label}: entity count");
        for ((pa, ea), (pb, eb)) in a.iter().zip(b.iter()) {
            assert_eq!(pa, pb);
            let mut blocks_a = Vec::new();
            let mut blocks_b = Vec::new();
            ea.entity.for_each(|p, blk| blocks_a.push((p, blk)));
            eb.entity.for_each(|p, blk| blocks_b.push((p, blk)));
            blocks_a.sort_by_key(|(p, _)| *p);
            blocks_b.sort_by_key(|(p, _)| *p);
            assert_eq!(blocks_a, blocks_b, "{label} [{pa}]: blocks");
            assert_eq!(ea.entity.links(), eb.entity.links(), "{label} [{pa}]: links");
        }
        // Same region files as the game produced. (Game folders sometimes also hold a
        // stale second set named after the entity's in-world id; that one is dropped.)
        let ours: Vec<_> = written.keys().filter(|p| p.ends_with(".smd3")).cloned().collect();
        let theirs: Vec<_> = tree
            .keys()
            .filter(|p| p.ends_with(".smd3") && !p.rsplit('/').next().unwrap().starts_with("ENTITY_"))
            .cloned()
            .collect();
        assert_eq!(ours, theirs, "{label}: region file names");
    }
}

/// The links this crate writes for an edited design must mean the same as the
/// game's, and the header it recomputes must follow the game's bounds margins.
#[test]
fn regenerated_logic_and_header_agree_with_the_games() {
    let Some(install) = install() else { return };
    for (label, tree) in blueprints(&install) {
        let mut bp = Blueprint::from_files(&tree).unwrap();
        let original_bounds = bp.root.header.bounds;
        let original_links = bp.root.entity.links().clone();
        let original_counts = bp.root.header.counts.clone();

        // Touch a block and a link so both files are regenerated, then undo the block.
        let far = yard_core::Pos::new(200, 200, 200);
        bp.root.entity.set(far, yard_core::Block::new(5, 0, false));
        bp.root.entity.set(far, yard_core::Block::AIR);
        bp.root.entity.link(far, far, 5);
        bp.root.entity.unlink(far, far, 5);
        assert!(bp.root.entity.blocks_changed() && bp.root.entity.links_changed());

        let again = Blueprint::from_files(&bp.to_files().unwrap()).unwrap();
        assert_eq!(again.root.entity.links(), &original_links, "{label}: regenerated links");
        assert_eq!(again.root.header.counts, original_counts, "{label}: regenerated counts");
        assert_eq!(again.root.header.bounds, original_bounds, "{label}: regenerated bounds");
    }
}

#[test]
fn real_block_config_parses() {
    let Some(install) = install() else { return };
    let xml = fs::read_to_string(install.join("data/config/BlockConfig.xml")).unwrap();
    let props = fs::read_to_string(install.join("data/config/BlockTypes.properties")).unwrap();
    let c = Catalog::parse(&xml, &props).unwrap();
    assert!(c.len() > 1000);
    assert_eq!(c.get(Catalog::CORE).unwrap().name, "Ship Core");
    let hull = c.by_name("Grey Basic Armor").unwrap();
    assert_eq!(hull.category, "General/Hulls/Basic/Grey");
    assert_eq!(c.get(hull.style_ids[0]).unwrap().base, hull.id);

    // Which blocks the game switches on when placed. The game itself
    // (ElementInformation.activateOnPlacement on 0.304.7) says 240 of them:
    // lights, doors, storage and factories, but no logic blocks.
    let on: Vec<_> = c.all().filter(|d| d.active_when_placed()).map(|d| d.id).collect();
    for (name, expected) in [("Plex Door", true), ("Storage Unit", true), ("Activation Module", false), ("Grey Basic Armor", false)] {
        let def = c.by_name(name).unwrap();
        assert_eq!(def.active_when_placed(), expected, "{name}");
    }
    assert!(on.contains(&977), "light bars are lit when placed");
    if c.len() == 1516 {
        assert_eq!(on.len(), 240, "blocks on when placed");
    } else {
        eprintln!("{} blocks, {} on when placed (count only checked on the 1516 blocks of 0.304.7)", c.len(), on.len());
    }
}

/// Every stock meta file must pass through the meta reader unchanged, and the
/// docked entities it names must be the folders that are there.
#[test]
fn stock_meta_files_are_understood_and_name_their_docked_entities() {
    let Some(install) = install() else { return };
    let mut metas = 0;
    let mut with_children = 0;
    for folder in ["blueprints", "blueprints-default", "blueprints-stations", "blueprints-npc"] {
        let mut tree = FileTree::new();
        if !install.join(folder).exists() {
            continue;
        }
        read_tree(&install.join(folder), "", &mut tree);
        for (path, meta) in tree.iter().filter(|(p, _)| p.ends_with("/meta.smbpm")) {
            let version = i32::from_be_bytes(meta[..4].try_into().unwrap());
            if version < 2 {
                continue;
            }
            metas += 1;
            let named = yard_core::meta::children(meta).unwrap_or_else(|e| panic!("{folder}/{path}: {e}"));
            // Rewriting to the name it already has changes nothing.
            if let Some(root) = named.first().and_then(|c| c.split('/').next()) {
                assert_eq!(&yard_core::meta::with_root(meta, root).unwrap(), meta, "{folder}/{path}");
                with_children += 1;
            }
            // Each named child is a folder next to this meta file, under the
            // name of the blueprint folder itself.
            let dir = path.strip_suffix("/meta.smbpm").unwrap();
            for child in &named {
                let (root, rest) = child.split_once('/').unwrap_or((child, ""));
                let blueprint_dir = dir.split("/ATTACHED_").next().unwrap();
                assert_eq!(root, blueprint_dir.rsplit('/').next().unwrap(), "{folder}/{path} names {child}");
                let expected = format!("{blueprint_dir}/{rest}/header.smbph");
                assert!(tree.contains_key(&expected), "{folder}/{path} names {child}, but {expected} is missing");
            }
        }
    }
    assert!(metas > 20 && with_children > 5, "only {metas} meta files, {with_children} with docked entities");
}

#[test]
fn a_renamed_blueprint_keeps_its_docked_entities_findable() {
    let Some(install) = install() else { return };
    let mut tree = FileTree::new();
    let name = "Isanth Type-PNR-25-B";
    read_tree(&install.join("blueprints").join(name), name, &mut tree);
    let mut bp = Blueprint::from_files(&tree).unwrap();
    assert_eq!(yard_core::meta::children(&bp.root.meta).unwrap(), vec![format!("{name}/ATTACHED_0")]);
    bp.rename("Isanth Copy");
    assert!(bp.warnings.is_empty(), "{:?}", bp.warnings);
    let files = bp.to_files().unwrap();
    assert_eq!(yard_core::meta::children(&files["Isanth Copy/meta.smbpm"]).unwrap(), vec!["Isanth Copy/ATTACHED_0"]);
    assert!(files.contains_key("Isanth Copy/ATTACHED_0/header.smbph"));
    // Apart from the name, nothing changed: back under the old name the files
    // are the ones the game wrote. (Block data is recompressed on writing; the
    // round-trip test above compares its contents.)
    bp.rename(name);
    let back = bp.to_files().unwrap();
    assert_eq!(back.keys().collect::<Vec<_>>(), tree.keys().collect::<Vec<_>>());
    for (path, data) in tree.iter().filter(|(path, _)| !path.ends_with(".smd3")) {
        assert!(&back[path] == data, "{path} changed");
    }
}

/// The dock records of the stock blueprints are read: every docked folder has
/// one, it names the docker block that is really there, and placing the
/// entity puts that docker in the cell the rail block's arrow side faces.
/// (The stock ships have few docked parts, and those were left on rails that
/// were built over since. The maths is tested pair by pair in `dock.rs`, and
/// `cargo run --example docks -- <folder of .sment files>` runs these checks
/// and an overlap check over any collection of blueprints.)
#[test]
fn stock_dock_records_are_read_and_put_dockers_on_their_rails() {
    let Some(install) = install() else { return };
    let mut docked = 0;
    for (label, tree) in blueprints(&install) {
        let bp = Blueprint::from_files(&tree).unwrap_or_else(|e| panic!("{label}: {e}"));
        let docks = bp.docks();
        for (path, dock) in &docks {
            let child = &bp.entity(path).unwrap().entity;
            docked += 1;
            assert_eq!(child.get(dock.docker.pos).ty(), dock.docker.ty, "{label} [{path}]: docker block");
            assert_eq!(child.get(dock.docker.pos).orientation(), dock.docker.orientation, "{label} [{path}]: docker orientation");
            let (up, _) = yard_core::shape::turned_axes(dock.rail.orientation);
            assert_eq!(dock.at_rest().cell(dock.docker.pos), Some(dock.rail.pos.offset(up[0], up[1], up[2])), "{label} [{path}]");
            // Where the game left it: the docker's cell, moved by what the
            // record says the rail has carried it.
            let carried = dock.moved.shift.map(|c| c.round() as i32);
            let rest = dock.rail.pos.offset(up[0], up[1], up[2]);
            assert_eq!(dock.placement().cell(dock.docker.pos), Some(rest.offset(carried[0], carried[1], carried[2])), "{label} [{path}]");
        }
        // Every docked folder has a dock, and the layout reaches all of them.
        let entities = bp.entities().len();
        assert_eq!(docks.len() + 1, entities, "{label}: docked entities without a dock on record");
        assert_eq!(bp.layout(|_, d| d.placement()).len(), entities, "{label}");
    }
    assert!(docked > 0, "the stock blueprints have docked entities");
}

/// The turn a docked entity gets, for every pair of rail and docker
/// orientations, is the one the game's own classes give.
#[test]
fn docked_entities_are_turned_as_the_game_turns_them() {
    use yard_core::dock::{Dock, Piece, Placement, NO_TURN};
    use yard_core::Pos;
    let table: serde_json::Value = serde_json::from_str(include_str!("data/dock_rest_turns.json")).unwrap();
    for rail in 0..24u8 {
        for docker in 0..24u8 {
            let dock = Dock {
                rail: Piece { pos: Pos::new(3, -4, 5), ty: 662, orientation: rail },
                docker: Piece { pos: Pos::new(-1, 2, 0), ty: 663, orientation: docker },
                turret: NO_TURN,
                moved: Placement::SAME,
                rotated: false,
            };
            let game: Vec<Vec<f32>> = serde_json::from_value(table["rest"][rail as usize][docker as usize].clone()).unwrap();
            let here = dock.placement().turn;
            for row in 0..3 {
                for column in 0..3 {
                    assert_eq!(here[row][column], game[row][column], "rail {rail}, docker {docker}");
                }
            }
        }
    }
}
