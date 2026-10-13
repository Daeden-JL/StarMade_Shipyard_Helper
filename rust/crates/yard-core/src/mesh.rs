//! Turns blocks into triangles for the viewport, one segment at a time so an
//! edit only rebuilds the few segments it touched.

use crate::entity::{Entity, Pos, Segment, SEG};
use crate::shape::{cube_uv, opposite, side_uv, turned_side, Geom, SideUv, BACK, BOTTOM, FRONT, LEFT, RIGHT, SIDE_DIRS, TOP};
use crate::Block;

/// One vertex as the viewport's shader reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Position in design space.
    pub pos: [f32; 3],
    /// The block's flat colour, used where there is no texture, and its opacity.
    pub color: [u8; 4],
    /// Position within the face, 0..255 across one block: where to sample the
    /// block's texture, and where its edges are.
    pub uv: [u8; 2],
    /// Texture tile in the game's numbering (page * 256 + row * 16 + column), or
    /// [`NO_TILE`].
    pub tile: u16,
    /// Brightness of the face, 0..255.
    pub light: u8,
    /// For a tile at or above [`MODEL_TILE`]: the low bytes of a texture
    /// position that spans the whole sheet, `uv` being the high bytes.
    pub fine: [u8; 2],
}

/// Bytes per vertex in [`Mesh::vertex_bytes`].
pub const VERTEX_BYTES: usize = 24;
/// Marks a face that has no texture and is drawn in its flat colour.
pub const NO_TILE: u16 = u16::MAX;
/// Tiles from here up to [`NO_TILE`] name a whole texture sheet, the sheet
/// number added on: the skin of a block's 3D model.
pub const MODEL_TILE: u16 = 0xF000;

#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Per vertex: three little-endian f32 (offset 0), RGBA (12), two UV bytes
    /// (16), the tile as a little-endian u16 (18), light (20), the two fine
    /// texture bytes (21) and one byte of padding.
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * VERTEX_BYTES);
        for v in &self.vertices {
            for c in v.pos {
                out.extend_from_slice(&c.to_le_bytes());
            }
            out.extend_from_slice(&v.color);
            out.extend_from_slice(&v.uv);
            out.extend_from_slice(&v.tile.to_le_bytes());
            out.extend_from_slice(&[v.light, v.fine[0], v.fine[1], 0]);
        }
        out
    }

    pub fn index_bytes(&self) -> Vec<u8> {
        self.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    }
}

/// The triangles of one segment. See-through blocks are kept apart because they
/// are drawn after everything solid, blended.
#[derive(Default)]
pub struct ChunkMesh {
    pub opaque: Mesh,
    pub translucent: Mesh,
}

/// How one block is drawn.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    pub color: [u8; 3],
    /// Glass and the like: drawn blended, and does not hide what is behind it.
    pub transparent: bool,
    pub geom: &'a Geom,
    /// Texture tile per side of the block itself, in the game's slot order:
    /// front, back, top, bottom, right, left. [`NO_TILE`] where there is none.
    pub tiles: [u16; 6],
    /// How many of those differ: 1 (same all round), 3 (top, bottom, sides) or 6.
    pub sides: u8,
    pub rule: TexRule,
}

/// How a block's orientation moves its textures.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TexRule {
    /// Wedges and the other cut shapes: every face shows its texture upright.
    #[default]
    Upright,
    /// Cubes and slabs: the texture turns with the block, by the game's table.
    /// `pointing` is for blocks whose side textures point where the block does.
    Cube { pointing: bool },
    /// Blocks that turn 24 ways (rails, dockers): the orientation says which
    /// side the first texture faces and which way its top edge points.
    Turned,
}

/// For a block with six textures: which of its slots shows on each side of the
/// cell, by orientation value 0..=5. From the game's own tables.
const SIX_SIDED: [[usize; 6]; 6] =
    [[0, 1, 2, 3, 5, 4], [1, 0, 2, 3, 4, 5], [4, 5, 0, 1, 3, 2], [5, 4, 1, 0, 2, 3], [4, 5, 2, 3, 1, 0], [5, 4, 2, 3, 0, 1]];
/// For a block with three textures: top, bottom, and one for all four sides.
const THREE_SIDED: [usize; 6] = [5, 5, 2, 3, 5, 5];

/// The texture tile a block shows on one side of its cell.
fn tile_for(look: &Look<'_>, orientation: u8, side: usize) -> u16 {
    let slot = match look.sides {
        6 => SIX_SIDED[(orientation as usize).min(5)][side],
        3 => THREE_SIDED[side],
        _ => 0,
    };
    look.tiles[slot]
}

/// The tile a block shows on a side of its cell, and how the game lays it
/// there; None for the layout where the block's shape has no table and the
/// texture is simply upright.
fn side_texture(look: &Look<'_>, orientation: u8, side: usize) -> (u16, Option<&'static SideUv>) {
    match look.rule {
        TexRule::Turned => {
            let (slot, uv) = turned_side(orientation, side);
            (look.tiles[if look.sides == 1 { 0 } else { slot }], Some(uv))
        }
        TexRule::Cube { pointing } => {
            // The game reads the orientation of blocks with several textures
            // as one of six directions, and never turns three-sided ones.
            let uv = match look.sides {
                3 => cube_uv(0, false, side),
                6 => cube_uv(orientation.min(5), pointing, side),
                _ => cube_uv(orientation % 6, pointing, side),
            };
            (tile_for(look, orientation, side), Some(uv))
        }
        TexRule::Upright => (tile_for(look, orientation, side), None),
    }
}

/// Where a corner of the cell falls in the texture of the given side, laid out
/// as the game lays out an unturned block: upright on the four walls, readable
/// from outside.
fn texture_uv(side: usize, c: [f32; 3]) -> [f32; 2] {
    match side {
        FRONT => [c[0], 1.0 - c[1]],
        BACK => [1.0 - c[0], 1.0 - c[1]],
        TOP => [1.0 - c[0], 1.0 - c[2]],
        BOTTOM => [1.0 - c[0], c[2]],
        RIGHT => [1.0 - c[2], 1.0 - c[1]],
        LEFT => [c[2], 1.0 - c[1]],
        _ => [0.5, 0.5],
    }
}

/// The side of the cell a direction mostly points at.
fn side_of(normal: [f32; 3]) -> usize {
    let n = normal.map(f32::abs);
    if n[0] >= n[1] && n[0] >= n[2] {
        if normal[0] > 0.0 { RIGHT } else { LEFT }
    } else if n[1] >= n[2] {
        if normal[1] > 0.0 { TOP } else { BOTTOM }
    } else if normal[2] > 0.0 {
        FRONT
    } else {
        BACK
    }
}

/// Supplies the look of every block: colour by type, geometry by type and
/// orientation. Called several times per block, so it should be a table lookup.
pub trait Looks {
    fn look(&self, block: Block) -> Look<'_>;
}

/// Opacity of see-through blocks.
const GLASS_ALPHA: u8 = 110;

/// Light by face direction, so shapes read without any lighting in the shader:
/// brightest from above, darker below, front a little brighter than back.
fn light(normal: [f32; 3]) -> f32 {
    (0.72 + 0.28 * normal[1] + 0.10 * normal[2]).clamp(0.35, 1.0)
}

/// Builds the mesh of one segment: every block face not hidden by a neighbour.
pub fn build_segment(entity: &Entity, key: (i32, i32, i32), looks: &impl Looks) -> ChunkMesh {
    let mut out = ChunkMesh::default();
    let Some(segment) = entity.segment(key) else { return out };
    if segment.count == 0 {
        return out;
    }
    for (index, &block) in segment.blocks.iter().enumerate() {
        if block.is_air() {
            continue;
        }
        let pos = Entity::position(key, index);
        let local = [index as i32 % SEG, (index as i32 / SEG) % SEG, index as i32 / (SEG * SEG)];
        let look = looks.look(block);
        let mesh = if look.transparent { &mut out.translucent } else { &mut out.opaque };
        for face in &look.geom.faces {
            if let Some(side) = face.side {
                let d = SIDE_DIRS[side];
                let n = [local[0] + d[0], local[1] + d[1], local[2] + d[2]];
                // Most neighbours are in the same segment; skip the map lookup for those.
                let neighbour = if n.iter().all(|c| (0..SEG).contains(c)) {
                    segment.blocks[Segment::index(n[0], n[1], n[2])]
                } else {
                    entity.get(pos.offset(d[0], d[1], d[2]))
                };
                if !neighbour.is_air() {
                    let other = looks.look(neighbour);
                    let covers = other.geom.fills(opposite(side));
                    // Glass hides glass of its own kind, but nothing else.
                    if covers && (!other.transparent || neighbour.ty() == block.ty()) {
                        continue;
                    }
                }
            }
            push_face(mesh, pos, face, &look, block.orientation());
        }
    }
    out
}

fn push_face(mesh: &mut Mesh, pos: Pos, face: &crate::shape::Face, look: &Look<'_>, orientation: u8) {
    // A slanted face takes the texture of the side it mostly points at.
    let side = face.side.unwrap_or_else(|| side_of(face.normal));
    let color = [look.color[0], look.color[1], look.color[2], if look.transparent { GLASS_ALPHA } else { 255 }];
    let light = (light(face.normal) * 255.0) as u8;
    let base = mesh.vertices.len() as u32;
    let position = |c: &[f32; 3]| [pos.x as f32 + c[0] - 0.5, pos.y as f32 + c[1] - 0.5, pos.z as f32 + c[2] - 0.5];
    if let Some((page, skin)) = &face.skin {
        // A face of a 3D model: textured from a sheet of its own.
        for (c, uv) in face.corners.iter().zip(skin) {
            let wide = uv.map(|t| (t.clamp(0.0, 1.0) * 65535.0).round() as u16);
            mesh.vertices.push(Vertex {
                pos: position(c),
                color,
                uv: [(wide[0] >> 8) as u8, (wide[1] >> 8) as u8],
                tile: MODEL_TILE + page,
                light,
                fine: [wide[0] as u8, wide[1] as u8],
            });
        }
    } else {
        let (tile, layout) = match face.side {
            Some(side) => side_texture(look, orientation, side),
            None => (tile_for(look, orientation, side), None),
        };
        for c in &face.corners {
            let uv = layout.map_or_else(|| texture_uv(side, *c), |table| side_uv(table, side, *c));
            mesh.vertices.push(Vertex {
                // Blocks are centred on their integer position, as in the game.
                pos: position(c),
                color,
                uv: [(uv[0].clamp(0.0, 1.0) * 255.0) as u8, (uv[1].clamp(0.0, 1.0) * 255.0) as u8],
                tile,
                light,
                fine: [0, 0],
            });
        }
    }
    for i in 1..face.corners.len() as u32 - 1 {
        mesh.indices.extend_from_slice(&[base, base + i, base + i + 1]);
    }
}

/// Meshes a whole entity in one go. The viewport builds per segment instead;
/// this is for tests and one-off renders.
pub fn build(entity: &Entity, looks: &impl Looks) -> ChunkMesh {
    let mut out = ChunkMesh::default();
    let mut keys: Vec<_> = entity.segments().map(|(k, _)| *k).collect();
    keys.sort();
    for key in keys {
        let chunk = build_segment(entity, key, looks);
        for (from, to) in [(chunk.opaque, &mut out.opaque), (chunk.translucent, &mut out.translucent)] {
            let base = to.vertices.len() as u32;
            to.vertices.extend(from.vertices);
            to.indices.extend(from.indices.into_iter().map(|i| i + base));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLASS: u16 = 63;
    const SLAB: u16 = 700;
    const COMPUTER: u16 = 6;
    const DOCKER: u16 = 663;

    struct TestLooks {
        cube: Geom,
        slab: Geom,
    }

    impl TestLooks {
        fn new() -> Self {
            Self { cube: Geom::cube(), slab: Geom::cuboid([0.0; 3], [1.0, 0.5, 1.0]) }
        }
    }

    impl Looks for TestLooks {
        fn look(&self, block: Block) -> Look<'_> {
            Look {
                color: [200, 200, 200],
                transparent: block.ty() == GLASS,
                geom: if block.ty() == SLAB { &self.slab } else { &self.cube },
                // A computer: its own front, back, top, bottom, right, left tiles.
                // A rail docker: arrow side, underside, and one tile for the rest.
                tiles: match block.ty() {
                    COMPUTER => [10, 11, 12, 13, 14, 15],
                    DOCKER => [454, 455, 453, 453, 453, 453],
                    _ => [7, NO_TILE, NO_TILE, NO_TILE, NO_TILE, NO_TILE],
                },
                sides: if matches!(block.ty(), COMPUTER | DOCKER) { 6 } else { 1 },
                rule: match block.ty() {
                    DOCKER => TexRule::Turned,
                    SLAB => TexRule::Upright,
                    _ => TexRule::Cube { pointing: false },
                },
            }
        }
    }

    fn faces(m: &Mesh) -> usize {
        m.indices.len() / 6
    }

    #[test]
    fn hidden_faces_are_not_drawn() {
        let looks = TestLooks::new();
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
        assert_eq!(faces(&build(&e, &looks).opaque), 6);
        // Two touching cubes share a face on each: 10 faces, not 12.
        e.set(Pos::new(1, 0, 0), Block::new(5, 0, false));
        let m = build(&e, &looks).opaque;
        assert_eq!(faces(&m), 10);
        assert_eq!(m.vertices.len(), 10 * 4);
        assert_eq!(m.vertex_bytes().len(), m.vertices.len() * VERTEX_BYTES);
        assert_eq!(m.index_bytes().len(), m.indices.len() * 4);
    }

    #[test]
    fn faces_are_hidden_across_segment_borders_too() {
        let looks = TestLooks::new();
        let mut e = Entity::new();
        // Design x = 15 and 16 are in different segments.
        e.set(Pos::new(15, 0, 0), Block::new(5, 0, false));
        e.set(Pos::new(16, 0, 0), Block::new(5, 0, false));
        assert_eq!(e.segments().count(), 2);
        assert_eq!(faces(&build(&e, &looks).opaque), 10);
        let (key, _) = Entity::locate(Pos::new(15, 0, 0));
        assert_eq!(faces(&build_segment(&e, key, &looks).opaque), 5);
        assert!(build_segment(&e, (9, 9, 9), &looks).opaque.is_empty());
    }

    #[test]
    fn glass_is_kept_apart_and_does_not_hide_solid_blocks() {
        let looks = TestLooks::new();
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(5, 0, false));
        e.set(Pos::new(1, 0, 0), Block::new(GLASS, 0, false));
        e.set(Pos::new(2, 0, 0), Block::new(GLASS, 0, false));
        let m = build(&e, &looks);
        // The hull keeps all six faces: one is seen through the glass.
        assert_eq!(faces(&m.opaque), 6);
        // Glass against hull is hidden and so is glass against its own kind:
        // four faces left on the middle block, five on the end one.
        assert_eq!(faces(&m.translucent), 4 + 5);
        assert!(m.translucent.vertices.iter().all(|v| v.color[3] < 255));
    }

    /// The four vertices of the face of a lone block that points along `normal`.
    fn face_on(block: Block, normal: [f32; 3]) -> Vec<Vertex> {
        let looks = TestLooks::new();
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), block);
        let mesh = build(&e, &looks).opaque;
        // Face centres sit half a block from the block's centre along the normal.
        mesh.vertices
            .chunks(4)
            .find(|q| (0..3).all(|a| (q.iter().map(|v| v.pos[a]).sum::<f32>() / 4.0 - normal[a] * 0.5).abs() < 1e-4))
            .unwrap()
            .to_vec()
    }

    /// The tile drawn on the face of a lone block that points along `normal`.
    fn tile_on(block: Block, normal: [f32; 3]) -> u16 {
        face_on(block, normal)[0].tile
    }

    /// Where on a face the top edge of its texture lies: the middle of the
    /// two corners drawn with v = 0, relative to the block's centre.
    fn texture_top(block: Block, normal: [f32; 3]) -> [f32; 3] {
        let top: Vec<_> = face_on(block, normal).into_iter().filter(|v| v.uv[1] == 0).collect();
        assert_eq!(top.len(), 2, "two corners of a face are at the top of its texture");
        [0, 1, 2].map(|a| (top[0].pos[a] + top[1].pos[a]) / 2.0)
    }

    #[test]
    fn a_rail_docker_shows_its_arrow_side_where_its_orientation_says() {
        let up = [0.0, 1.0, 0.0];
        // As the game places it: arrow side up, arrow pointing forward (+Z).
        let placed = Block::new(DOCKER, 14, false);
        assert_eq!(tile_on(placed, up), 454);
        assert_eq!(tile_on(placed, [0.0, -1.0, 0.0]), 455);
        assert_eq!(tile_on(placed, [0.0, 0.0, 1.0]), 453);
        assert_eq!(texture_top(placed, up), [0.0, 0.5, 0.5]);
        // Same side up, turned a quarter each time: forward becomes back, +X, -X.
        assert_eq!(texture_top(Block::new(DOCKER, 12, false), up), [0.0, 0.5, -0.5]);
        assert_eq!(texture_top(Block::new(DOCKER, 13, false), up), [0.5, 0.5, 0.0]);
        assert_eq!(texture_top(Block::new(DOCKER, 15, false), up), [-0.5, 0.5, 0.0]);
        // The arrow side on each of the other five sides of the cell.
        for (orientation, normal) in [(0, [0.0, 0.0, 1.0]), (4, [0.0, 0.0, -1.0]), (8, [0.0, -1.0, 0.0]), (16, [-1.0, 0.0, 0.0]), (20, [1.0, 0.0, 0.0])] {
            let docker = Block::new(DOCKER, orientation, false);
            assert_eq!(tile_on(docker, normal), 454, "orientation {orientation}");
            assert_eq!(tile_on(docker, normal.map(|c: f32| -c)), 455, "orientation {orientation}");
        }
    }

    #[test]
    fn a_cube_turns_its_texture_with_its_orientation() {
        // Unturned, the texture on the top of a cube has its top edge toward +Z.
        assert_eq!(texture_top(Block::new(5, 0, false), [0.0, 1.0, 0.0]), [0.0, 0.5, 0.5]);
        // Orientation 1 (facing back) turns it half way round.
        assert_eq!(texture_top(Block::new(5, 1, false), [0.0, 1.0, 0.0]), [0.0, 0.5, -0.5]);
        // The walls of an unturned cube stay upright.
        assert_eq!(texture_top(Block::new(5, 0, false), [1.0, 0.0, 0.0]), [0.5, 0.5, 0.0]);
    }

    #[test]
    fn a_block_with_a_front_shows_it_where_its_orientation_points() {
        // Orientation 0: front texture toward +Z, and the game's left/right
        // quirk puts the "right" slot on -X.
        let forward = Block::new(COMPUTER, 0, false);
        assert_eq!(tile_on(forward, [0.0, 0.0, 1.0]), 10);
        assert_eq!(tile_on(forward, [0.0, 0.0, -1.0]), 11);
        assert_eq!(tile_on(forward, [0.0, 1.0, 0.0]), 12);
        assert_eq!(tile_on(forward, [1.0, 0.0, 0.0]), 15);
        assert_eq!(tile_on(forward, [-1.0, 0.0, 0.0]), 14);
        // Orientation 2: front toward +Y. Orientation 5: front toward +X.
        assert_eq!(tile_on(Block::new(COMPUTER, 2, false), [0.0, 1.0, 0.0]), 10);
        assert_eq!(tile_on(Block::new(COMPUTER, 5, false), [1.0, 0.0, 0.0]), 10);
        assert_eq!(tile_on(Block::new(COMPUTER, 4, false), [-1.0, 0.0, 0.0]), 10);
        // A block with one texture shows it everywhere, whatever its orientation.
        assert_eq!(tile_on(Block::new(5, 3, false), [0.0, -1.0, 0.0]), 7);
    }

    #[test]
    fn wall_textures_are_upright_and_readable_from_outside() {
        // Top corners of a wall are the top row of the texture (v = 0).
        for side in [FRONT, BACK, LEFT, RIGHT] {
            assert_eq!(texture_uv(side, [0.3, 1.0, 0.7])[1], 0.0);
            assert_eq!(texture_uv(side, [0.3, 0.0, 0.7])[1], 1.0);
        }
        // Seen from in front (+Z, looking back), +X is on the viewer's right:
        // the texture's left edge is at x = 0.
        assert_eq!(texture_uv(FRONT, [0.0, 0.5, 1.0])[0], 0.0);
        // Seen from behind, it is the other way round.
        assert_eq!(texture_uv(BACK, [0.0, 0.5, 0.0])[0], 1.0);
        assert_eq!(side_of([0.0, 0.7, -0.7]), TOP);
        assert_eq!(side_of([-0.6, 0.5, 0.5]), LEFT);
    }

    #[test]
    fn a_slab_only_hides_the_side_it_fills() {
        let looks = TestLooks::new();
        let mut e = Entity::new();
        e.set(Pos::new(0, 0, 0), Block::new(5, 0, false));
        // A half slab on top of the cube: it rests on the cube's top face and hides it.
        e.set(Pos::new(0, 1, 0), Block::new(SLAB, 0, false));
        assert_eq!(faces(&build(&e, &looks).opaque), 5 + 5);
        // A cube on top of the slab: the slab's top does not reach it, so both faces show.
        e.set(Pos::new(0, 2, 0), Block::new(5, 0, false));
        assert_eq!(faces(&build(&e, &looks).opaque), 5 + 5 + 6);
    }
}
