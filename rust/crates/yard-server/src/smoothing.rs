//! The catalog side of smoothing: which block makes which shape in each
//! colour. The shapes themselves and the choosing are in `yard_core::smooth`.

use std::collections::HashMap;

use yard_core::catalog::{Catalog, Shape};

/// For each full cube, the block of every shape and slab in its family.
pub fn families(catalog: &Catalog) -> HashMap<u16, HashMap<(Shape, u8), u16>> {
    let mut out: HashMap<u16, HashMap<(Shape, u8), u16>> = HashMap::new();
    for d in catalog.all().filter(|d| d.placeable && !d.deprecated) {
        let cube = if d.base == 0 { d.id } else { d.base };
        out.entry(cube).or_default().insert((d.shape, d.slab), d.id);
    }
    out
}
