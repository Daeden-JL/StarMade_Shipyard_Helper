//! Prints the blocks of one entity of a design around a position.
//!     cargo run -q -p yard-core --example around -- <file.sment> <entity path or -> <x> <y> <z> [radius]
use yard_core::{Blueprint, Pos};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bp = Blueprint::from_sment(&std::fs::read(&a[0]).unwrap()).unwrap();
    let path = if a[1] == "-" { "" } else { a[1].as_str() };
    let e = &bp.entity(path).expect("entity").entity;
    let c: Vec<i32> = a[2..5].iter().map(|v| v.parse().unwrap()).collect();
    let r: i32 = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(2);
    for y in (c[1] - r..=c[1] + r).rev() {
        for z in c[2] - r..=c[2] + r {
            let row: Vec<String> = (c[0] - r..=c[0] + r)
                .map(|x| {
                    let b = e.get(Pos::new(x, y, z));
                    if b.is_air() { "      .".to_string() } else { format!("{:4}o{:<2}", b.ty(), b.orientation()) }
                })
                .collect();
            println!("y{y:3} z{z:3} | {}", row.join(" "));
        }
        println!();
    }
}
