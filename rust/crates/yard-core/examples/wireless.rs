//! Lists the wireless logic pairs of the designs in a folder.
//!
//!     cargo run -q -p yard-core --example wireless -- <designs dir>

use yard_core::Blueprint;

fn main() {
    let dir = std::env::args().nth(1).expect("designs directory");
    let (mut designs, mut pairs) = (0, 0);
    for entry in std::fs::read_dir(&dir).expect("readable directory").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sment") {
            continue;
        }
        let Ok(bp) = Blueprint::from_sment(&std::fs::read(&path).unwrap_or_default()) else { continue };
        let found = bp.wireless();
        let names: Vec<String> = bp.entities().iter().map(|(_, e)| yard_core::meta::wireless(&e.meta).map(|(n, p)| format!("{n:?}:{}", p.len())).unwrap_or_else(|e| format!("ERR {e}"))).collect();
        let entities = bp.entities();
        let real = found.iter().filter(|((a, p), (b, q))| entities[*a].1.entity.get(*p).ty() == 668 && entities[*b].1.entity.get(*q).ty() == 668).count();
        if !found.is_empty() || names.iter().any(|n| n.starts_with("ERR")) {
            designs += 1;
            pairs += found.len();
            println!("{}: {} pairs, {real} between two wireless modules; entities {}", bp.name, found.len(), names.join(" "));
        }
    }
    println!("{designs} designs with wireless logic, {pairs} pairs");
}
