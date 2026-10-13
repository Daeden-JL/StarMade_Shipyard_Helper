//! Compares the content of two designs entity by entity: the blocks that
//! differ (type and orientation) and the links each has that the other lacks.
//!     cargo run -q --release -p yard-core --example diff -- <a.sment> <b.sment> [shown per entity]
//! Docked entities are compared by path, so two copies the game numbered
//! differently show as different there.
use yard_core::{Blueprint, Pos};
use std::collections::BTreeSet;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let load = |p: &str| Blueprint::from_sment(&std::fs::read(p).unwrap()).unwrap();
    let (x, y) = (load(&a[0]), load(&a[1]));
    let shown: usize = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(10);
    let paths = |bp: &Blueprint| bp.entities().into_iter().map(|(p, _)| p.to_string()).collect::<BTreeSet<_>>();
    let (px, py) = (paths(&x), paths(&y));
    if px != py {
        println!("entities differ: only in a {:?}, only in b {:?}", px.difference(&py).collect::<Vec<_>>(), py.difference(&px).collect::<Vec<_>>());
    }
    let (dx, dy) = (x.docks(), y.docks());
    for path in px.intersection(&py) {
        let at = |d: Option<&yard_core::dock::Dock>| d.map(|d| (d.rail.pos, d.docker.pos));
        if at(dx.get(path)) != at(dy.get(path)) {
            println!("entity {path:?} docks differently: a {:?}, b {:?}", at(dx.get(path)), at(dy.get(path)));
        }
        let (ex, ey) = (&x.entity(path).unwrap().entity, &y.entity(path).unwrap().entity);
        let mut cells = BTreeSet::new();
        for e in [ex, ey] {
            e.for_each(|p, b| {
                if !b.is_air() {
                    cells.insert(p);
                }
            });
        }
        let differ: Vec<_> = cells.iter().filter(|p| {
            let (bx, by) = (ex.get(**p), ey.get(**p));
            (bx.is_air(), bx.ty(), bx.orientation()) != (by.is_air(), by.ty(), by.orientation())
        }).collect();
        let flat = |e: &yard_core::entity::Entity| -> BTreeSet<(Pos, u16, Pos)> {
            e.links().iter().flat_map(|(f, k)| k.iter().flat_map(move |(t, tos)| tos.iter().map(move |to| (*f, *t, *to)))).collect()
        };
        let (lx, ly) = (flat(ex), flat(ey));
        let (only_a, only_b): (Vec<_>, Vec<_>) = (lx.difference(&ly).collect(), ly.difference(&lx).collect());
        if differ.is_empty() && only_a.is_empty() && only_b.is_empty() {
            continue;
        }
        println!("entity {path:?}: {} cells differ, {} links only in a, {} only in b", differ.len(), only_a.len(), only_b.len());
        for p in differ.iter().take(shown) {
            let (bx, by) = (ex.get(**p), ey.get(**p));
            println!("  ({},{},{}) a {}o{} b {}o{}", p.x, p.y, p.z, bx.ty(), bx.orientation(), by.ty(), by.orientation());
        }
        for l in only_a.iter().take(shown) { println!("  link only in a {l:?}"); }
        for l in only_b.iter().take(shown) { println!("  link only in b {l:?}"); }
    }
}
