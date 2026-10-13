//! How much empty space an entity's blocks enclose, for each seal width.
//!     cargo run -q --release -p yard-core --example inside -- <file.sment> [entity path or -]
use yard_core::interior::Interior;
use yard_core::Blueprint;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bp = Blueprint::from_sment(&std::fs::read(&a[0]).unwrap()).unwrap();
    let path = a.get(1).map_or("", |p| if p == "-" { "" } else { p.as_str() });
    let e = &bp.entity(path).expect("entity").entity;
    for seal in 0..=4 {
        let t = std::time::Instant::now();
        let inside = Interior::of(e, seal).unwrap();
        let took = t.elapsed();
        let spaces = inside.spaces();
        let biggest: Vec<String> = spaces.iter().take(3).map(|s| format!("{} at {:?}..{:?}", s.volume, s.min.to_array(), s.max.to_array())).collect();
        println!("seal {seal}: {} enclosed in {} spaces ({took:.2?}, spaces {:.2?}); {}", inside.volume, spaces.len(), t.elapsed() - took, biggest.join("; "));
    }
}
