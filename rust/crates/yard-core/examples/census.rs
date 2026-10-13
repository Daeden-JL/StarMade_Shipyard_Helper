//! Lists which stored designs hold given block types, with positions and
//! orientations of the first few, and how many links each design has.
//!
//!     cargo run -q -p yard-core --example census -- <designs dir> <type id>...

use std::collections::BTreeMap;
use yard_core::Blueprint;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("designs directory");
    let wanted: Vec<u16> = args.filter_map(|a| a.parse().ok()).collect();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("readable directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        let mut found: BTreeMap<u16, Vec<(String, yard_core::Pos, u8)>> = BTreeMap::new();
        let mut blocks = 0u64;
        let mut links = 0usize;
        for (entity_path, e) in bp.entities() {
            blocks += e.entity.block_count();
            links += e.entity.link_count();
            e.entity.for_each(|p, b| {
                if wanted.contains(&b.ty()) {
                    found.entry(b.ty()).or_default().push((entity_path.clone(), p, b.orientation()));
                }
            });
        }
        if !found.is_empty() || wanted.is_empty() {
            rows.push((blocks, path.file_stem().unwrap().to_string_lossy().to_string(), bp.name.clone(), links, found));
        }
    }
    rows.sort();
    for (blocks, id, name, links, found) in rows {
        let summary: Vec<String> = found
            .iter()
            .map(|(ty, list)| {
                let first: Vec<String> = list.iter().take(3).map(|(e, p, o)| format!("{e}@{},{},{} o{o}", p.x, p.y, p.z)).collect();
                format!("{ty} x{} [{}]", list.len(), first.join("; "))
            })
            .collect();
        println!("{id}\t{blocks}\t{links} links\t{name}\t{}", summary.join(" | "));
    }
}
