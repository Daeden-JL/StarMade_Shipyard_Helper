//! Prints, for each entity of a blueprint, the block count and the three
//! hashes that `tools/game-verify` reports, as this crate reads the file.
//! `tools/game-verify/compare.sh` puts the two side by side.
//!
//! The blueprint is read the way the game reads it, which takes the game's
//! block list: from the install in `STARMADE_DIR`, or `~/.smtest2/vol`. Without
//! an install only current-format blueprints can be read.
//!
//!     cargo run -q -p yard-core --example bphash -- ship.sment
//!     cargo run -q -p yard-core --example bphash -- path/to/blueprint-folder

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use yard_core::blueprint::FileTree;
use yard_core::catalog::{BlockInfo, Catalog};
use yard_core::Blueprint;

/// The block list of the local StarMade install, if there is one.
fn catalog() -> Option<Catalog> {
    let dir = std::env::var_os("STARMADE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))?
        .join("data/config");
    let xml = std::fs::read_to_string(dir.join("BlockConfig.xml")).ok()?;
    let props = std::fs::read_to_string(dir.join("BlockTypes.properties")).ok()?;
    Catalog::parse(&xml, &props).ok()
}

/// A blueprint folder as a file tree rooted at the folder's own name.
fn tree(dir: &Path) -> std::io::Result<FileTree> {
    fn walk(root: &Path, dir: &Path, out: &mut FileTree) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&path)?);
            }
        }
        Ok(())
    }
    let mut out = FileTree::new();
    walk(dir.parent().unwrap_or(dir), dir, &mut out)?;
    Ok(out)
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: bphash <file.sment | blueprint folder>");
        std::process::exit(2);
    };
    let fail = |e: std::io::Error| -> ! {
        eprintln!("{path}: {e}");
        std::process::exit(2);
    };
    let catalog = catalog();
    let blocks = catalog.as_ref().map(|c| c as &dyn BlockInfo);
    let files = if Path::new(&path).is_dir() {
        let dir = Path::new(&path).canonicalize().unwrap_or_else(|e| fail(e));
        Ok(tree(&dir).unwrap_or_else(|e| fail(e)))
    } else {
        yard_core::blueprint::unzip(&std::fs::read(&path).unwrap_or_else(|e| fail(e)))
    };
    let read = files.and_then(|files| Blueprint::from_files_with(&files, blocks));
    let bp = match read {
        Ok(bp) => bp,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    for warning in &bp.warnings {
        eprintln!("warning: {warning}");
    }
    for (entity_path, e) in bp.entities() {
        let (content, raw, links) = common::hashes(&e.entity);
        // Tab-separated: path (empty for the main entity), blocks, hashes.
        println!("{entity_path}\t{}\t{content}\t{raw}\t{links}", e.entity.block_count());
    }
}
