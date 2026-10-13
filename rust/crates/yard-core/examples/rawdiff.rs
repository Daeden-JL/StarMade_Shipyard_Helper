//! Compares two blueprints cell by cell at the raw level (type, facing,
//! active bit and hit points), which the sync's content fingerprint does
//! not: it ignores the active bit and hit points. Either side may be a
//! `.sment` file or a blueprint folder.
//!
//!     cargo run -q --release -p yard-core --example rawdiff -- a.sment b-folder [n]

use std::path::Path;
use yard_core::blueprint::FileTree;
use yard_core::{Blueprint, Pos};

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

fn load(p: &str) -> Blueprint {
    let files = if Path::new(p).is_dir() {
        tree(&Path::new(p).canonicalize().unwrap()).unwrap()
    } else {
        yard_core::blueprint::unzip(&std::fs::read(p).unwrap()).unwrap()
    };
    Blueprint::from_files(&files).unwrap()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let (x, y) = (load(&a[0]), load(&a[1]));
    for (path, ea) in x.entities() {
        let Some(eb) = y.entity(&path) else {
            println!("entity {path:?} only in a");
            continue;
        };
        let (ea, eb) = (&ea.entity, &eb.entity);
        let mut diffs: Vec<Pos> = Vec::new();
        ea.for_each(|p, b| {
            if eb.get(p) != b {
                diffs.push(p);
            }
        });
        eb.for_each(|p, b| {
            if ea.get(p).is_air() && !b.is_air() {
                diffs.push(p);
            }
        });
        if diffs.is_empty() {
            continue;
        }
        let (mut lo, mut hi) = (diffs[0], diffs[0]);
        for p in &diffs {
            lo = lo.lo(*p);
            hi = hi.hi(*p);
        }
        println!("entity {path:?}: {} cells differ, within ({},{},{})..({},{},{})", diffs.len(), lo.x, lo.y, lo.z, hi.x, hi.y, hi.z);
        let mut kinds = std::collections::BTreeMap::new();
        for p in &diffs {
            let (ba, bb) = (ea.get(*p), eb.get(*p));
            let what = match (ba.ty() == bb.ty(), ba.orientation() == bb.orientation(), ba.active_bit() == bb.active_bit(), ba.hp() == bb.hp()) {
                (false, _, _, _) => "type",
                (_, false, _, _) => "facing",
                (_, _, false, _) => "active",
                (_, _, _, false) => "hp",
                _ => "same?",
            };
            *kinds.entry((what, ba.ty(), bb.ty())).or_insert(0usize) += 1;
        }
        for ((what, ta, tb), count) in &kinds {
            println!("  {count} x {what}: a type {ta}, b type {tb}");
        }
        for p in diffs.iter().take(n) {
            let (ba, bb) = (ea.get(*p), eb.get(*p));
            println!(
                "  ({},{},{}) a {}o{} act{} hp{}  b {}o{} act{} hp{}",
                p.x, p.y, p.z, ba.ty(), ba.orientation(), ba.active_bit() as u8, ba.hp(), bb.ty(), bb.orientation(), bb.active_bit() as u8, bb.hp()
            );
        }
    }
}
