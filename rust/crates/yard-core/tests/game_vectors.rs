//! Compares what this crate reads with what the game itself reads.
//!
//! The expected hashes come from `tools/game-verify`, which loads a blueprint
//! with the game's own classes and hashes every block and link. To add a
//! vector, run `tools/game-verify/run.sh --summary <blueprint>` and copy the
//! entity's `totalBlocks`, `contentHash`, `rawHash` and `linksHash`.

mod common;

use common::hashes;
use std::fs;
use std::path::{Path, PathBuf};
use yard_core::blueprint::FileTree;
use yard_core::catalog::Catalog;
use yard_core::Blueprint;

/// (entity path, blocks, contentHash, rawHash, linksHash)
type Vector = (&'static str, u64, &'static str, &'static str, &'static str);

const NO_LINKS: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const CORE_ONLY: (&str, &str) = (
    "b9d6ba0b20886fe88492396eb29768948e50cd0f5a74576533907f8bdd2f9e8d",
    "4496b8ff3300fc486da1ddf2979effdb26efaaa9f97ae1c1847900dd5cef6f7c",
);

fn check(label: &str, bp: &Blueprint, expected: &[Vector]) {
    let entities = bp.entities();
    assert_eq!(entities.len(), expected.len(), "{label}: entity count");
    for (path, blocks, content, raw, links) in expected {
        let (_, e) = entities.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{label}: no entity {path:?}"));
        let got = hashes(&e.entity);
        assert_eq!(e.entity.block_count(), *blocks, "{label} {path:?}: block count");
        assert_eq!(got.1, *raw, "{label} {path:?}: stored block values differ from what the game read");
        assert_eq!(got.0, *content, "{label} {path:?}: block fields differ from what the game read");
        assert_eq!(got.2, *links, "{label} {path:?}: links differ from what the game read");
    }
}

fn sample(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/game-verify/samples").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn hand_built_minimal_blueprints_read_as_the_game_reads_them() {
    for name in ["minimal-core.sment", "plain-core.sment"] {
        let bp = Blueprint::from_sment(&sample(name)).unwrap();
        assert!(bp.warnings.is_empty(), "{name}: {:?}", bp.warnings);
        check(name, &bp, &[("", 1, CORE_ONLY.0, CORE_ONLY.1, NO_LINKS)]);
    }
}

/// A blueprint the game's own writer produced with a single-value segment and a
/// palette-bitmap segment instead of the usual zlib arrays.
#[test]
fn compressed_segment_encodings_read_as_the_game_reads_them() {
    let bp = Blueprint::from_sment(&sample("compressed-segments.sment")).unwrap();
    check(
        "compressed-segments",
        &bp,
        &[(
            "",
            32781,
            "acf1c056cba892b4ebfa91073ef54521e8422bd60262135ae182e665f9d6f1ad",
            "4a7ab395ce57a96ed3dda1f22d9a0d41c6481033d69f75824e14621dcd715971",
            NO_LINKS,
        )],
    );
}

/// A new ship made by this crate hashes the same as the game's reading of a
/// core-only blueprint, and its files are the smallest ones the game accepts.
#[test]
fn a_new_ship_is_what_the_game_expects() {
    let bp = Blueprint::new_ship("Fresh", "0.304.7_20260628_193755");
    let files = bp.to_files().unwrap();
    assert_eq!(files["Fresh/meta.smbpm"], [0, 0, 0, 5, 1]);
    assert_eq!(files["Fresh/logic.smbpl"], [0, 0, 0, 0, 0xff, 0xff, 0xfb, 0xfe, 0, 0, 0, 0]);
    let reference = yard_core::blueprint::unzip(&sample("minimal-core.sment")).unwrap();
    assert_eq!(files["Fresh/header.smbph"], reference["GV Minimal Core/header.smbph"], "header bytes");
    assert_eq!(files.keys().filter(|k| k.ends_with(".smd3")).collect::<Vec<_>>(), ["Fresh/DATA/Fresh.0.0.0.smd3"]);
    // Exactly one top-level directory, or the game refuses the import.
    assert!(files.keys().all(|k| k.starts_with("Fresh/")));
    let back = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
    check("new ship", &back, &[("", 1, CORE_ONLY.0, CORE_ONLY.1, NO_LINKS)]);
}

fn install() -> Option<PathBuf> {
    let dir = std::env::var_os("STARMADE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?;
    dir.join("blueprints").is_dir().then_some(dir)
}

fn tree(dir: &Path) -> FileTree {
    fn walk(root: &Path, dir: &Path, out: &mut FileTree) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = FileTree::new();
    walk(dir.parent().unwrap(), dir, &mut out);
    out
}

/// A stock ship with a docked turret, read from the install.
#[test]
fn a_stock_ship_reads_as_the_game_reads_it() {
    let Some(install) = install() else {
        eprintln!("skipped: no StarMade install (set STARMADE_DIR)");
        return;
    };
    let dir = install.join("blueprints/Isanth Type-PNR-25-B");
    if !dir.is_dir() {
        eprintln!("skipped: this install has no Isanth Type-PNR-25-B");
        return;
    }
    let bp = Blueprint::from_files(&tree(&dir)).unwrap();
    let expected: [Vector; 2] = [
        (
            "",
            3167,
            "9441cf5f444f44beb06fb3aad0695631b71cf6fffb38e81ac10f29c400987ce9",
            "bd7f3f1a3767d690cb059bf021035cba3a49e8197de53b6533811eec7178ab0c",
            "620d4e09dc7166e5ad40ce9927c1966503a8f1304cdd52bcb16efaaaeebf6a63",
        ),
        (
            "ATTACHED_0",
            33,
            "b913b41f6776e880a965922974a391fe0ce626e59e2592945e379988dbe4106f",
            "64cbb0787204b33e8aa477fd91ec1c047520bed10ff484285e263059e83515fb",
            NO_LINKS,
        ),
    ];
    check("Isanth Type-PNR-25-B", &bp, &expected);
    // Written out and read back, it is still what the game read.
    let again = Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap();
    check("Isanth Type-PNR-25-B rewritten", &again, &expected);
}

/// The install's block list. Old blueprints are converted by block type, so
/// they can only be read as the game reads them with this at hand.
fn catalog(install: &Path) -> Catalog {
    let config = install.join("data/config");
    let xml = fs::read_to_string(config.join("BlockConfig.xml")).unwrap();
    let props = fs::read_to_string(config.join("BlockTypes.properties")).unwrap();
    Catalog::parse(&xml, &props).unwrap()
}

/// Checks every entity of a blueprint against an entry of
/// `tools/game-verify/samples/vectors.json`, and again after writing it out in
/// the current format and reading that back without the block list.
fn check_against(label: &str, bp: &Blueprint, entry: &serde_json::Value) {
    let root = entry["entities"][0]["path"].as_str().unwrap();
    for (bp, stage) in [(bp, "read"), (&Blueprint::from_sment(&bp.to_sment().unwrap()).unwrap(), "rewritten")] {
        let entities = bp.entities();
        for expected in entry["entities"].as_array().unwrap() {
            let full = expected["path"].as_str().unwrap();
            let path = full.strip_prefix(root).unwrap().trim_start_matches('/');
            let (_, e) = entities.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{label}: no entity {path:?}"));
            let (content, raw, links) = hashes(&e.entity);
            let at = format!("{label} {path:?} ({stage})");
            assert_eq!(e.entity.block_count(), expected["totalBlocks"].as_u64().unwrap(), "{at}: block count");
            assert_eq!(raw, expected["rawHash"].as_str().unwrap(), "{at}: stored block values");
            assert_eq!(content, expected["contentHash"].as_str().unwrap(), "{at}: block fields");
            assert_eq!(links, expected["linksHash"].as_str().unwrap(), "{at}: links");
        }
    }
}

fn vectors() -> serde_json::Value {
    serde_json::from_slice(&sample("vectors.json")).unwrap()
}

/// Hand-built blueprints covering every old segment version and link file
/// layout, each checked against what the game read from it.
#[test]
fn old_format_samples_read_as_the_game_reads_them() {
    let Some(install) = install() else {
        eprintln!("skipped: no StarMade install (set STARMADE_DIR)");
        return;
    };
    let catalog = catalog(&install);
    let mut checked = 0;
    for entry in vectors()["samples"].as_array().unwrap() {
        let name = entry["sample"].as_str().unwrap().strip_prefix("samples/").unwrap();
        // The network link layout never occurs in a blueprint on disk.
        if name.starts_with("logic-network") {
            continue;
        }
        let bp = Blueprint::from_sment_with(&sample(name), &catalog).unwrap_or_else(|e| panic!("{name}: {e}"));
        check_against(name, &bp, entry);
        checked += 1;
    }
    assert!(checked >= 7, "only {checked} samples checked");
}

fn stock_blueprints_match(filter: impl Fn(&str, u64) -> bool) {
    let Some(install) = install() else {
        eprintln!("skipped: no StarMade install (set STARMADE_DIR)");
        return;
    };
    let catalog = catalog(&install);
    let mut checked = 0;
    for entry in vectors()["stock"].as_array().unwrap() {
        let rel = entry["blueprint"].as_str().unwrap();
        let blocks: u64 = entry["entities"].as_array().unwrap().iter().map(|e| e["totalBlocks"].as_u64().unwrap()).sum();
        let dir = install.join(rel);
        // Isanth M holds two files for one region; which the game keeps depends
        // on the order the file system lists them in.
        if !dir.is_dir() || rel.ends_with("Isanth Type-PNR-25-M") || !filter(rel, blocks) {
            continue;
        }
        let bp = Blueprint::from_files_with(&tree(&dir), Some(&catalog)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        check_against(rel, &bp, entry);
        checked += 1;
    }
    eprintln!("{checked} stock blueprints checked");
    assert!(checked > 0, "no stock blueprint from the vectors was found in the install");
}

/// Stock stations and faction ships saved by old game versions (the smaller ones).
#[test]
fn old_stock_blueprints_read_as_the_game_reads_them() {
    stock_blueprints_match(|_, blocks| blocks < 120_000);
}

/// All of them, including stations of over a million blocks. Slow in a debug
/// build: `cargo test --release -p yard-core --test game_vectors -- --ignored`.
#[test]
#[ignore]
fn every_stock_blueprint_reads_as_the_game_reads_it() {
    stock_blueprints_match(|_, _| true);
}
