//! One representative colour per block, averaged from the game's block textures.
//!
//! The textures belong to the game and are not shipped with this project; they are
//! read from the StarMade install the server is pointed at.

use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;
use yard_core::catalog::Catalog;

const TILES_PER_ROW: u32 = 16;
const TILES_PER_PAGE: i32 = 256;

/// Colour for blocks whose texture cannot be read.
pub const FALLBACK: [u8; 3] = [150, 150, 160];

/// Where an install keeps its block texture sheets. The 64-pixel set is the
/// smallest the game ships: plenty for an average colour and for a viewport.
pub fn texture_dir(starmade_dir: &Path) -> std::path::PathBuf {
    starmade_dir.join("data/textures/block/Default/64")
}

pub fn load(starmade_dir: &Path, catalog: &Catalog) -> HashMap<u16, [u8; 3]> {
    let dir = texture_dir(starmade_dir);
    let mut pages: HashMap<i32, Option<RgbaImage>> = HashMap::new();
    let mut tiles: HashMap<i32, Option<[u8; 3]>> = HashMap::new();
    let mut out = HashMap::new();

    for def in catalog.all() {
        // Sides are front, back, top, bottom, right, left. Average them all so a
        // block with one odd face (a computer's screen) still reads as its body.
        let mut sum = [0u32; 3];
        let mut n = 0u32;
        for &texture in &def.textures {
            let colour = *tiles.entry(texture).or_insert_with(|| {
                let page = pages.entry(texture / TILES_PER_PAGE).or_insert_with_key(|p| {
                    image::open(dir.join(format!("t{p:03}.png"))).ok().map(|i| i.to_rgba8())
                });
                page.as_ref().and_then(|img| average_tile(img, (texture % TILES_PER_PAGE) as u32))
            });
            if let Some(c) = colour {
                for i in 0..3 {
                    sum[i] += c[i] as u32;
                }
                n += 1;
            }
        }
        if n > 0 {
            out.insert(def.id, [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]);
        }
    }
    out
}

/// Average of a tile's visible pixels, or None for an empty tile.
fn average_tile(img: &RgbaImage, tile: u32) -> Option<[u8; 3]> {
    let size = img.width() / TILES_PER_ROW;
    let (tx, ty) = (tile % TILES_PER_ROW * size, tile / TILES_PER_ROW * size);
    if size == 0 || ty + size > img.height() {
        return None;
    }
    let mut sum = [0u64; 3];
    let mut weight = 0u64;
    for y in ty..ty + size {
        for x in tx..tx + size {
            let p = img.get_pixel(x, y).0;
            let a = p[3] as u64;
            for i in 0..3 {
                sum[i] += p[i] as u64 * a;
            }
            weight += a;
        }
    }
    (weight > 0).then(|| [(sum[0] / weight) as u8, (sum[1] / weight) as u8, (sum[2] / weight) as u8])
}
