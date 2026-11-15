//! Docks every turret of every blueprint in a folder onto a plain test
//! ship, writes it out and reads it back, and says what failed.
//!
//!     cargo run -q --release -p yard-core --example dockcheck -- <folder of .sment> <starmade config dir> [out folder]

use yard_core::catalog::Catalog;
use yard_core::clone::{dock_from, dockable, Place};
use yard_core::{Block, Blueprint, Pos};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("a folder"));
    let cfg = std::path::PathBuf::from(args.next().expect("config directory"));
    let out = args.next().map(std::path::PathBuf::from);
    let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).expect("BlockConfig.xml");
    let props = std::fs::read_to_string(cfg.join("BlockTypes.properties")).expect("BlockTypes.properties");
    let catalog = Catalog::parse(&xml, &props).expect("catalog");
    let mut target = Blueprint::new_ship("Dock Test", "test");
    for x in -40..=40 {
        for z in -40..=40 {
            target.root.entity.set(Pos::new(x, -1, z), Block::new(5, 0, false));
        }
    }
    let (mut tried, mut ok, mut alone, mut sources) = (0, 0, 0, 0);
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "sment")).collect();
    files.sort();
    for file in files {
        let Ok(bytes) = std::fs::read(&file) else { continue };
        if bytes.len() > 20_000_000 {
            continue;
        }
        let Ok(other) = Blueprint::from_sment_with(&bytes, &catalog) else { continue };
        let list = dockable(&other);
        if list.is_empty() {
            continue;
        }
        sources += 1;
        for d in list {
            tried += 1;
            alone += usize::from(d.path.is_empty());
            let mut bp = target.clone();
            let small = yard_core::clone::trimmed_to(&other, &d.path);
            let place = Place::New { parent: String::new(), at: Pos::new(20, -1, 20), facing: [0, 1, 0] };
            match dock_from(&mut bp, &catalog, &small, &d.path, &place) {
                Ok(made) => {
                    let back = Blueprint::from_sment(&bp.to_sment().unwrap());
                    match back {
                        Ok(back) if back.docks() == bp.docks() && back.docks().len() == 1 + d.docked => {
                            ok += 1;
                            if let Some(out) = &out {
                                if ok <= 3 {
                                    let mut named = back.clone();
                                    named.rename(&format!("Dock Test {ok}"));
                                    std::fs::write(out.join(format!("docktest{ok}.sment")), named.to_sment().unwrap()).unwrap();
                                    println!("wrote docktest{ok}: {} {:?} -> {}", other.name, d.path, made.notes.join(" | "));
                                }
                            }
                        }
                        Ok(back) => println!("{} {:?}: docks differ after reading back ({} vs {} expected)", other.name, d.path, back.docks().len(), 1 + d.docked),
                        Err(e) => println!("{} {:?}: written but unreadable: {e}", other.name, d.path),
                    }
                }
                Err(e) => println!("{} {:?} (rail {}): {e}", other.name, d.path, d.rail_ty),
            }
        }
    }
    println!("{sources} blueprints with something to dock; {tried} tried ({alone} turrets saved on their own), {ok} docked and read back");
}
