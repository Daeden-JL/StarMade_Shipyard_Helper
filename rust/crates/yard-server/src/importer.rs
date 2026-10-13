//! Turns an uploaded 3D model into a new design: the model is read and cut
//! into blocks, and each block gets the hull block whose colour is nearest.

use std::collections::HashMap;
use yard_core::catalog::{Catalog, Shape};
use yard_core::{Block, Blueprint, Pos};
use yard_import::{Options, Up};

/// The hull block used where the model has no colour.
const PLAIN: &str = "Grey Basic Armor";

/// The blocks a model can be made of: one plain cube of every basic hull
/// colour, with the average colour of its textures.
pub fn hull_palette(catalog: &Catalog, colours: &HashMap<u16, [u8; 3]>) -> Vec<(u16, [u8; 3])> {
    catalog
        .all()
        .filter(|d| d.shape == Shape::Cube && d.slab == 0 && d.placeable && !d.deprecated && !d.transparent)
        .filter(|d| d.category.starts_with("General/Hulls/Basic/") && !d.category.ends_with("/Glass"))
        .filter_map(|d| colours.get(&d.id).map(|c| (d.id, *c)))
        .collect()
}

/// What the player chose in the import dialog.
#[derive(Clone, Debug)]
pub struct Choices {
    pub longest: u32,
    pub solid: bool,
    /// How thick the shell of a hollow result is, in blocks.
    pub shell: u8,
    /// None takes the file format's own habit.
    pub up: Option<Up>,
    pub turn: u8,
    pub mirror: bool,
    /// Wedges and corners where the model slants.
    pub smooth: bool,
    /// With smoothing: the edges of steps up to this many blocks high are
    /// beveled with wedges, where the model's own slope fits none. 0 for none.
    pub step: u8,
}

/// Reads a model file and builds a ship from it. Returns the blueprint and
/// the notes to show the player.
pub fn ship_from_model(
    catalog: &Catalog,
    palette: &[(u16, [u8; 3])],
    game_version: &str,
    name: &str,
    file_name: &str,
    bytes: &[u8],
    choices: &Choices,
) -> Result<(Blueprint, Vec<String>), String> {
    let model = yard_import::read(file_name, bytes).map_err(|e| e.to_string())?;
    let options = Options { longest: choices.longest, solid: choices.solid, shell: choices.shell, up: choices.up, turn: choices.turn % 4, mirror: choices.mirror, smooth: choices.smooth };
    let voxels = yard_import::voxelize(&model, &options).map_err(|e| e.to_string())?;
    if voxels.cells.is_empty() {
        return Err("The model came out with no blocks.".to_string());
    }
    let plain = catalog.by_name(PLAIN).map(|d| d.id).or_else(|| palette.first().map(|p| p.0)).ok_or("The server has no hull blocks to build with.")?;
    let matcher = yard_import::Matcher::new(palette);
    // The ship core goes in the filled cell nearest the middle of the grid,
    // so it sits inside the hull; its cell is the origin of the design.
    let middle = voxels.size.map(|n| (n as f32 - 1.0) / 2.0);
    let core = voxels
        .cells
        .iter()
        .filter(|c| c.partial.is_none())
        .min_by(|a, b| {
            let d = |c: &yard_import::Cell| (0..3).map(|i| (c.pos[i] as f32 - middle[i]).powi(2)).sum::<f32>();
            d(a).total_cmp(&d(b))
        })
        .map(|c| c.pos.map(|v| v as i32))
        .expect("there are cells");
    let mut bp = Blueprint::new_ship(name, game_version);
    let entity = &mut bp.root.entity;
    let mut by_colour: HashMap<[u8; 3], u16> = HashMap::new();
    let families = crate::smoothing::families(catalog);
    // How many blocks of each shape there are, for the note.
    let mut shaped = 0usize;
    let present: std::collections::HashSet<[i32; 3]> = if choices.smooth { voxels.cells.iter().map(|c| c.pos.map(i32::from)).collect() } else { Default::default() };
    // First the shape of every block on its own: what the model's slope fits,
    // or the bevel of a step's edge.
    let mut shapes: Vec<Option<yard_core::smooth::Form>> = Vec::with_capacity(voxels.cells.len());
    for cell in &voxels.cells {
        let open = |side: usize| {
            let d = yard_core::shape::SIDE_DIRS[side];
            !present.contains(&[cell.pos[0] as i32 + d[0], cell.pos[1] as i32 + d[1], cell.pos[2] as i32 + d[2]])
        };
        let planned = cell.partial.and_then(|samples| yard_core::smooth::best_form(samples, open));
        // Where the model's own slope fits no wedge (a gentle one), bevel the edge of the step.
        let planned = planned.or_else(|| {
            if cell.air == 0 || choices.step == 0 {
                return None;
            }
            let near = |d: [i32; 3]| present.contains(&[cell.pos[0] as i32 + d[0], cell.pos[1] as i32 + d[1], cell.pos[2] as i32 + d[2]]);
            yard_core::smooth::bevel(cell.air, near, choices.step).and_then(|mask| yard_core::smooth::best_form(mask, open))
        });
        shapes.push(planned);
    }
    // Then the inside corners, where two beveled rows meet and the block
    // between them stands up: decided by what the rows became.
    if choices.smooth && choices.step > 0 {
        let shaped_at: HashMap<[i32; 3], u64> =
            voxels.cells.iter().zip(&shapes).filter_map(|(c, f)| f.map(|f| (c.pos.map(i32::from), f.samples))).collect();
        for (cell, shape) in voxels.cells.iter().zip(shapes.iter_mut()) {
            if shape.is_some() || cell.air == 0 {
                continue;
            }
            let at = |d: [i32; 3]| [cell.pos[0] as i32 + d[0], cell.pos[1] as i32 + d[1], cell.pos[2] as i32 + d[2]];
            let neighbour = |d: [i32; 3]| shaped_at.get(&at(d)).copied().or_else(|| present.contains(&at(d)).then_some(u64::MAX));
            let open = |side: usize| !present.contains(&at(yard_core::shape::SIDE_DIRS[side]));
            *shape = yard_core::smooth::inside_corner(cell.air, neighbour).and_then(|mask| yard_core::smooth::best_form(mask, open));
        }
    }
    for (cell, planned) in voxels.cells.iter().zip(shapes) {
        let ty = match cell.colour {
            Some(colour) => *by_colour.entry(colour).or_insert_with(|| matcher.nearest(colour).unwrap_or(plain)),
            None => plain,
        };
        let at = Pos::new(cell.pos[0] as i32 - core[0], cell.pos[1] as i32 - core[1], cell.pos[2] as i32 - core[2]);
        if at == Pos::new(0, 0, 0) {
            continue;
        }
        let mut block = Block::new(ty, 0, false);
        if let Some(form) = planned {
            // A block of the shape in the colour's own family, or the whole block.
            if let Some(&shaped_ty) = families.get(&ty).and_then(|family| family.get(&(form.shape, form.slab))) {
                block = Block::new(shaped_ty, form.orientation, false);
                shaped += 1;
            }
        }
        entity.set(at, block);
    }
    let header_entity = &bp.root.entity;
    let mut header = bp.root.header.clone();
    header.refresh(header_entity);
    bp.root.header = header;
    let mut notes = voxels.notes;
    notes.extend(model.notes.iter().cloned());
    if choices.smooth {
        notes.push(format!("Smoothed: {shaped} blocks are wedges, corners and the like."));
    }
    notes.push(format!("Made of {} blocks, {} by {} by {}.", voxels.cells.len(), voxels.size[0], voxels.size[1], voxels.size[2]));
    Ok((bp, notes))
}
