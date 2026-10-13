//! Turns a model file into blocks and prints what came of it, so a model
//! can be checked by eye:
//!
//! `cargo run -p yard-import --release --example voxelize -- <file> [longest] [solid|shell] [smooth]`

use std::time::Instant;

use yard_import::{read, voxelize, Options, Voxels};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = arguments.first() else {
        eprintln!("usage: voxelize <file> [longest] [solid|shell] [smooth]");
        std::process::exit(2);
    };
    let longest = arguments.get(1).and_then(|n| n.parse().ok()).unwrap_or(32);
    let solid = arguments.get(2).is_none_or(|word| word != "shell");
    let smooth = arguments.get(3).is_some_and(|word| word == "smooth");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            std::process::exit(1);
        }
    };
    let started = Instant::now();
    let model = match read(path, &bytes) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("read failed: {error}");
            std::process::exit(1);
        }
    };
    let read_time = started.elapsed();
    println!("format     {}", model.format);
    println!("triangles  {}", model.triangles.len());
    println!("textures   {}", model.textures.len());
    println!("up         {:?}", model.up);
    let started = Instant::now();
    let voxels = match voxelize(&model, &Options { longest, solid, smooth, ..Options::default() }) {
        Ok(voxels) => voxels,
        Err(error) => {
            eprintln!("voxelize failed: {error}");
            std::process::exit(1);
        }
    };
    let voxel_time = started.elapsed();
    let inside = voxels.cells.iter().filter(|c| c.inside).count();
    let coloured = voxels.cells.iter().filter(|c| c.colour.is_some()).count();
    println!("grid       {} x {} x {}", voxels.size[0], voxels.size[1], voxels.size[2]);
    println!("blocks     {} ({} surface, {} inside, {} coloured)", voxels.cells.len(), voxels.cells.len() - inside, inside, coloured);
    for note in &voxels.notes {
        println!("note       {note}");
    }
    println!("partial    {}", voxels.cells.iter().filter(|c| c.partial.is_some()).count());
    println!("time       read {read_time:.2?}, voxelize {voxel_time:.2?}");
    if voxels.size.iter().all(|&s| s <= 80) {
        print_slices(&voxels);
    }
}

/// Prints a few level slices from the bottom up, seen from above: `#` for
/// a surface block, `+` for an inside block. The nose (+z) is at the top
/// of each slice and the pilot's left (+x) on the left.
fn print_slices(voxels: &Voxels) {
    let [sx, sy, sz] = voxels.size.map(|s| s as usize);
    let mut grid = vec![b' '; sx * sy * sz];
    for cell in &voxels.cells {
        let [x, y, z] = cell.pos.map(usize::from);
        grid[x + sx * (y + sy * z)] = if cell.inside { b'+' } else { b'#' };
    }
    let levels: Vec<usize> = if sy <= 5 { (0..sy).collect() } else { (0..5).map(|i| i * (sy - 1) / 4).collect() };
    for y in levels {
        println!("\nlevel y = {y}");
        for z in (0..sz).rev() {
            let row: String = (0..sx).rev().map(|x| grid[x + sx * (y + sy * z)] as char).collect();
            println!("  {}", row.trim_end());
        }
    }
}
