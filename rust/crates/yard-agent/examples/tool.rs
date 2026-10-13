//! Calls one tool on a design file, for seeing what a model would be told.
//!     YARD_STARMADE_DIR=<install> cargo run -q --release -p yard-agent --example tool -- <file.sment> <tool> '<json arguments>'
use std::sync::Arc;
use yard_agent::tools::Toolbox;
use yard_agent::{Conversation, MemoryWorld};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bp = yard_core::Blueprint::from_sment(&std::fs::read(&a[0]).unwrap()).unwrap();
    let cfg = std::path::Path::new(&std::env::var("YARD_STARMADE_DIR").expect("YARD_STARMADE_DIR")).join("data/config");
    let read = |f: &str| std::fs::read_to_string(cfg.join(f)).unwrap();
    let catalog = yard_core::catalog::Catalog::parse(&read("BlockConfig.xml"), &read("BlockTypes.properties")).unwrap();
    let world = MemoryWorld::of(Arc::new(catalog), bp);
    let args: serde_json::Value = serde_json::from_str(a.get(2).map_or("{}", |s| s.as_str())).unwrap();
    let t = std::time::Instant::now();
    let out = Toolbox::new().call(&world, &mut Conversation::new(), &a[1], &args);
    println!("{}\n({:.2?})", out.text, t.elapsed());
}
