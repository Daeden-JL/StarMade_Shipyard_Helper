//! Smooths an entity of a blueprint as the shipyard's "Smooth whole entity"
//! does and prints how many blocks of each shape it makes, and where it puts
//! the corner pieces and tetras.
//!
//!     cargo run -q --release -p yard-core --example smoothcount -- <file.sment> <starmade config dir> <entity> [step] [passes] [inside] [sharpen] [fresh] [list]

use std::collections::{BTreeMap, HashMap};
use yard_core::catalog::{Catalog, Shape};
use yard_core::smooth::{resmooth_entity_opts, samples_of, smooth_entity_opts, Form, Smoothing};
use yard_core::{Block, Blueprint, Pos};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cfg = std::path::PathBuf::from(&a[1]);
    let xml = std::fs::read_to_string(cfg.join("BlockConfig.xml")).expect("BlockConfig.xml");
    let props = std::fs::read_to_string(cfg.join("BlockTypes.properties")).expect("BlockTypes.properties");
    let catalog = Catalog::parse(&xml, &props).expect("catalog");
    let bp = Blueprint::from_sment(&std::fs::read(&a[0]).expect("readable file")).expect("a blueprint");
    let path = if a[2] == "-" { "" } else { a[2].as_str() };
    let step: u8 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(1);
    let passes: usize = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(1);
    let inside = a.get(5).is_some_and(|v| v == "inside");
    let sharpen = a.iter().any(|v| v == "sharpen");
    let mut variants: HashMap<(u16, Shape, u8), u16> = HashMap::new();
    let mut family: HashMap<u16, (u16, Shape, u8)> = HashMap::new();
    for d in catalog.all().filter(|d| d.placeable && !d.deprecated) {
        let base = if d.base == 0 { d.id } else { d.base };
        variants.insert((base, d.shape, d.slab), d.id);
        family.insert(d.id, (base, d.shape, d.slab));
    }
    let kind = |b: Block| family.get(&b.ty()).copied().unwrap_or((0, Shape::Cube, 0));
    let smoothable = |b: Block| {
        let (base, shape, slab) = kind(b);
        b.ty() != 1 && shape == Shape::Cube && slab == 0 && variants.contains_key(&(base, Shape::Wedge, 0))
    };
    let reshape = |b: Block, f: &Form| {
        let (base, _, _) = kind(b);
        let ty = *variants.get(&(base, f.shape, f.slab))?;
        let mut out = b;
        out.set_type(ty);
        out.set_orientation(f.orientation);
        Some(out)
    };
    let samples = |b: Block| {
        if b.is_air() {
            return None;
        }
        let (_, shape, slab) = kind(b);
        Some(match shape {
            Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta => samples_of(shape, 0, b.orientation()),
            Shape::Cube if slab > 0 => samples_of(shape, slab, b.orientation()),
            _ => u64::MAX,
        })
    };
    let mut entity = bp.entity(path).expect("entity").entity.clone();
    for pass in 1..=passes {
        let fresh = a.iter().any(|v| v == "fresh");
        let unshape = |b: Block| {
            let (base, shape, _) = kind(b);
            if !matches!(shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta) {
                return None;
            }
            let mut out = b;
            out.set_type(*variants.get(&(base, Shape::Cube, 0))?);
            out.set_orientation(0);
            Some(out).filter(|c| smoothable(*c))
        };
        let done = if fresh {
            resmooth_entity_opts(&entity, &Smoothing { step, inside, sharpen }, smoothable, reshape, samples, unshape)
        } else {
            smooth_entity_opts(&entity, &Smoothing { step, inside, sharpen }, smoothable, reshape, samples)
        };
        let mut by_shape: BTreeMap<String, usize> = BTreeMap::new();
        for (p, b) in &done {
            let (_, shape, _) = kind(*b);
            *by_shape.entry(format!("{shape:?}")).or_default() += 1;
            if matches!(shape, Shape::Corner | Shape::Tetra) || a.iter().any(|v| v == "list") {
                println!("{shape:?} at {:?} o{}", (p.x, p.y, p.z), b.orientation());
            }
            entity.set(*p, *b);
        }
        println!("pass {pass}: {} blocks changed: {by_shape:?}", done.len());
    }
    let _ = Pos::new(0, 0, 0);
}
