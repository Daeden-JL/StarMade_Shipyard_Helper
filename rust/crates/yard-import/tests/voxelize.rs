//! Turning models into blocks: how many, where, and in which colour.

mod common;

use common::{ball, painted_box, plain_box};
use yard_import::{voxelize, Blocks, Cell, ImportError, Model, Options, Paint, Triangle, Up, Voxels};

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn model(triangles: Vec<Triangle>) -> Model {
    Model::from_triangles(triangles, Vec::new(), Up::Y)
}

fn options(longest: u32, solid: bool) -> Options {
    Options { longest, solid, ..Options::default() }
}

fn cell(voxels: &Voxels, pos: [u16; 3]) -> Option<&Cell> {
    voxels.cells.iter().find(|c| c.pos == pos)
}

fn colour(voxels: &Voxels, pos: [u16; 3]) -> Option<[u8; 3]> {
    cell(voxels, pos).unwrap_or_else(|| panic!("no block at {pos:?}")).colour
}

#[test]
fn a_cube_of_side_n_is_n_cubed_blocks_solid_and_a_one_block_shell_hollow() {
    for n in [4u32, 5, 8, 17] {
        let cube = model(plain_box([0.0; 3], [1.0; 3]));
        let solid = voxelize(&cube, &options(n, true)).unwrap();
        assert_eq!(solid.size, [n; 3]);
        assert_eq!(solid.cells.len() as u32, n * n * n);
        assert_eq!(solid.cells.iter().filter(|c| c.inside).count() as u32, (n - 2).pow(3));
        let shell = voxelize(&cube, &options(n, false)).unwrap();
        assert_eq!(shell.cells.len() as u32, n * n * n - (n - 2).pow(3));
        assert!(shell.cells.iter().all(|c| !c.inside));
    }
}

#[test]
fn a_hollow_cube_can_have_a_thicker_shell() {
    let cube = model(plain_box([0.0; 3], [1.0; 3]));
    for shell in [2u8, 3] {
        let voxels = voxelize(&cube, &Options { shell, ..options(12, false) }).unwrap();
        let n = 12u32;
        let k = u32::from(shell);
        assert_eq!(voxels.cells.len() as u32, n.pow(3) - (n - 2 * k).pow(3), "shell {shell}");
        assert!(cell(&voxels, [6, 6, 6]).is_none(), "still hollow");
    }
    // A shell thicker than the model is the whole of it.
    let voxels = voxelize(&cube, &Options { shell: 16, ..options(8, false) }).unwrap();
    assert_eq!(voxels.cells.len(), 512);
}

#[test]
fn a_cube_with_its_faces_turned_inside_out_gives_the_same_blocks() {
    let mut triangles = plain_box([0.0; 3], [1.0; 3]);
    for t in &mut triangles {
        t.corners.swap(1, 2);
    }
    let solid = voxelize(&model(triangles), &options(8, true)).unwrap();
    assert_eq!(solid.cells.len(), 512);
}

#[test]
fn a_cube_lying_anywhere_at_any_scale_gives_the_same_blocks() {
    let cube = model(plain_box([-1234.5, 77.25, 0.001], [-1234.5 + 0.03, 77.25 + 0.03, 0.001 + 0.03]));
    let shell = voxelize(&cube, &options(10, false)).unwrap();
    assert_eq!(shell.size, [10; 3]);
    // Rounding at this distance from the origin may cost or add a block or
    // two along an edge, no more.
    assert!((shell.cells.len() as i64 - 488).abs() <= 40, "{}", shell.cells.len());
}

#[test]
fn a_box_keeps_its_proportions() {
    let voxels = voxelize(&model(plain_box([0.0; 3], [4.0, 2.0, 1.0])), &options(16, true)).unwrap();
    assert_eq!(voxels.size, [16, 8, 4]);
    assert_eq!(voxels.cells.len(), 16 * 8 * 4);
}

#[test]
fn a_balls_solid_volume_is_within_a_few_percent_of_the_true_one() {
    let ball = model(ball(1.0, 48, 96));
    let solid = voxelize(&ball, &options(100, true)).unwrap();
    let expected = 4.0 / 3.0 * std::f64::consts::PI * 50.0f64.powi(3);
    let off = (solid.cells.len() as f64 - expected) / expected;
    assert!(off.abs() < 0.04, "volume is off by {:.1} %", off * 100.0);
    assert!(solid.cells.iter().any(|c| c.inside));
    // The middle is filled, the corners of the grid are not.
    assert!(cell(&solid, [50, 50, 50]).is_some_and(|c| c.inside));
    assert!(cell(&solid, [1, 1, 1]).is_none());
}

#[test]
fn a_hollow_ball_has_a_thin_shell_and_no_inside_blocks() {
    let ball = model(ball(1.0, 48, 96));
    let shell = voxelize(&ball, &options(100, false)).unwrap();
    assert!(shell.cells.iter().all(|c| !c.inside));
    assert!(cell(&shell, [50, 50, 50]).is_none());
    // The thinnest gapless shell has one block per column along the axis
    // the surface faces most: on a ball 0.83 blocks per unit of area. A
    // shell of every cell the surface touches would have 1.5.
    let area = 4.0 * std::f64::consts::PI * 2500.0;
    let per_area = shell.cells.len() as f64 / area;
    assert!((0.8..0.9).contains(&per_area), "{per_area} blocks per unit of area");
}

#[test]
fn a_flat_plate_is_one_layer_of_blocks() {
    let plate = vec![
        Triangle { corners: [[0.0, 0.3, 0.0], [1.0, 0.3, 0.0], [1.0, 0.3, 1.0]], paint: Paint::None },
        Triangle { corners: [[0.0, 0.3, 0.0], [1.0, 0.3, 1.0], [0.0, 0.3, 1.0]], paint: Paint::None },
    ];
    let voxels = voxelize(&model(plate), &options(10, true)).unwrap();
    assert_eq!(voxels.size, [10, 1, 10]);
    assert_eq!(voxels.cells.len(), 100);
}

#[test]
fn a_plate_lying_exactly_between_two_layers_of_cells_is_still_one_layer() {
    // A box of eight cells a side with a floor through its middle, at
    // height four: right on the border between two layers.
    let mut triangles = plain_box([0.0; 3], [8.0; 3]);
    triangles.push(Triangle { corners: [[0.0, 4.0, 0.0], [8.0, 4.0, 8.0], [8.0, 4.0, 0.0]], paint: Paint::None });
    triangles.push(Triangle { corners: [[0.0, 4.0, 0.0], [0.0, 4.0, 8.0], [8.0, 4.0, 8.0]], paint: Paint::None });
    let voxels = voxelize(&model(triangles), &options(8, false)).unwrap();
    let column: Vec<u16> = voxels.cells.iter().filter(|c| c.pos[0] == 4 && c.pos[2] == 4).map(|c| c.pos[1]).collect();
    assert_eq!(column, vec![0, 3, 7]);
}

#[test]
fn a_slanted_plate_has_no_gaps_and_one_block_per_column() {
    // A plate rising one in three: seen from above every column has
    // exactly one block.
    let plate = vec![
        Triangle { corners: [[0.0, 0.0, 0.0], [9.0, 3.0, 0.0], [9.0, 3.0, 9.0]], paint: Paint::None },
        Triangle { corners: [[0.0, 0.0, 0.0], [9.0, 3.0, 9.0], [0.0, 0.0, 9.0]], paint: Paint::None },
    ];
    let voxels = voxelize(&model(plate), &options(9, false)).unwrap();
    for x in 0..9 {
        for z in 0..9 {
            assert_eq!(voxels.cells.iter().filter(|c| c.pos[0] == x && c.pos[2] == z).count(), 1, "column {x},{z}");
        }
    }
}

#[test]
fn a_rod_thinner_than_a_block_that_passes_between_cell_centres_is_not_lost() {
    // Two small boxes pin the grid to ten cells a side; the rod runs along
    // x right on the border between four columns of cells.
    let mut triangles = plain_box([0.0; 3], [0.5; 3]);
    triangles.extend(plain_box([9.5; 3], [10.0; 3]));
    triangles.extend(plain_box([2.0, 4.9, 4.9], [8.0, 5.1, 5.1]));
    let voxels = voxelize(&model(triangles), &options(10, false)).unwrap();
    for x in 2..8 {
        let here = voxels.cells.iter().filter(|c| c.pos[0] == x && (4..=5).contains(&c.pos[1]) && (4..=5).contains(&c.pos[2])).count();
        assert!(here >= 1, "the rod is missing at x = {x}");
    }
    // And it has not grown beyond the four columns it touches.
    assert!(voxels.cells.iter().filter(|c| (2..8).contains(&c.pos[0])).all(|c| (4..=5).contains(&c.pos[1]) && (4..=5).contains(&c.pos[2])));
}

#[test]
fn a_model_drawn_on_whole_numbers_keeps_its_exact_shape() {
    // An L of three unit cubes, as a block model exported as a mesh, at its
    // own size: the blocks come back exactly.
    let mut triangles = plain_box([0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
    triangles.extend(plain_box([0.0, 1.0, 0.0], [1.0, 2.0, 1.0]));
    // Pin the grid to four cells so that one unit is one cell.
    triangles.extend(plain_box([3.0, 3.0, 0.0], [4.0, 4.0, 1.0]));
    let voxels = voxelize(&model(triangles), &options(4, true)).unwrap();
    let mut filled: Vec<[u16; 3]> = voxels.cells.iter().map(|c| c.pos).collect();
    filled.sort();
    assert_eq!(filled, vec![[0, 0, 0], [0, 1, 0], [1, 0, 0], [3, 3, 0]]);
}

#[test]
fn a_cubes_red_top_gives_red_blocks_on_top_and_nowhere_else() {
    let cube = model(painted_box([0.0; 3], [1.0; 3], |side| Paint::Flat(if side == 3 { RED } else { BLUE })));
    let voxels = voxelize(&cube, &options(8, false)).unwrap();
    for c in &voxels.cells {
        let [x, y, z] = c.pos;
        let rim = x == 0 || x == 7 || z == 0 || z == 7;
        if y == 7 && !rim {
            assert_eq!(c.colour, Some(RED), "{:?}", c.pos);
        } else if y < 7 {
            assert_eq!(c.colour, Some(BLUE), "{:?}", c.pos);
        }
    }
}

#[test]
fn corner_colours_blend_across_a_face() {
    let plate = vec![Triangle { corners: [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [0.0, 0.0, 8.0]], paint: Paint::Corners([[0, 0, 0], [240, 0, 0], [0, 0, 240]]) }];
    let voxels = voxelize(&model(plate), &options(8, false)).unwrap();
    assert_eq!(colour(&voxels, [0, 0, 0]), Some([15, 0, 15]));
    assert_eq!(colour(&voxels, [6, 0, 0]), Some([195, 0, 15]));
    assert_eq!(colour(&voxels, [1, 0, 5]), Some([45, 0, 165]));
}

#[test]
fn a_texture_is_sampled_where_each_block_lies_on_it() {
    use yard_import::Texture;
    // Two texels: red on the left, blue on the right.
    let texture = Texture { width: 2, height: 1, rgb: vec![255, 0, 0, 0, 0, 255] };
    let paint = |uv| Paint::Textured { texture: 0, tint: [255, 255, 255], uv };
    let plate = vec![
        Triangle { corners: [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [8.0, 0.0, 8.0]], paint: paint([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]) },
        Triangle { corners: [[0.0, 0.0, 0.0], [8.0, 0.0, 8.0], [0.0, 0.0, 8.0]], paint: paint([[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) },
    ];
    let voxels = voxelize(&Model::from_triangles(plate, vec![texture], Up::Y), &options(8, false)).unwrap();
    assert_eq!(voxels.cells.len(), 64);
    for c in &voxels.cells {
        assert_eq!(c.colour, Some(if c.pos[0] < 4 { RED } else { BLUE }), "{:?}", c.pos);
    }
}

/// A box four long, three high and two deep with +x red, +y green, +z blue.
fn marked_box() -> Model {
    model(painted_box([0.0; 3], [4.0, 3.0, 2.0], |side| match side {
        1 => Paint::Flat(RED),
        3 => Paint::Flat(GREEN),
        5 => Paint::Flat(BLUE),
        _ => Paint::None,
    }))
}

#[test]
fn with_y_up_the_files_axes_are_the_ships_axes() {
    let voxels = voxelize(&marked_box(), &Options { longest: 16, solid: false, up: Some(Up::Y), turn: 0, mirror: false, smooth: false, shell: 1 }).unwrap();
    assert_eq!(voxels.size, [16, 12, 8]);
    assert_eq!(colour(&voxels, [15, 6, 4]), Some(RED));
    assert_eq!(colour(&voxels, [8, 11, 4]), Some(GREEN));
    assert_eq!(colour(&voxels, [8, 6, 7]), Some(BLUE));
    assert_eq!(colour(&voxels, [0, 6, 4]), None);
}

#[test]
fn with_z_up_the_files_z_becomes_up_and_its_minus_y_the_nose() {
    let voxels = voxelize(&marked_box(), &Options { longest: 16, solid: false, up: Some(Up::Z), turn: 0, mirror: false, smooth: false, shell: 1 }).unwrap();
    assert_eq!(voxels.size, [16, 8, 12]);
    assert_eq!(colour(&voxels, [15, 4, 6]), Some(RED));
    // The file's +z (blue) is on top, its +y (green) at the back.
    assert_eq!(colour(&voxels, [8, 7, 6]), Some(BLUE));
    assert_eq!(colour(&voxels, [8, 4, 0]), Some(GREEN));
}

#[test]
fn each_quarter_turn_swings_the_nose_to_the_left() {
    let turned = |turn| voxelize(&marked_box(), &Options { longest: 16, solid: false, up: Some(Up::Y), turn, mirror: false, smooth: false, shell: 1 }).unwrap();
    // Once: what pointed forward (+z, blue) points left (+x); what pointed
    // left (+x, red) points back.
    let once = turned(1);
    assert_eq!(once.size, [8, 12, 16]);
    assert_eq!(colour(&once, [7, 6, 8]), Some(BLUE));
    assert_eq!(colour(&once, [4, 6, 0]), Some(RED));
    assert_eq!(colour(&once, [4, 11, 8]), Some(GREEN));
    // Twice: front and back, left and right have changed places.
    let twice = turned(2);
    assert_eq!(twice.size, [16, 12, 8]);
    assert_eq!(colour(&twice, [0, 6, 4]), Some(RED));
    assert_eq!(colour(&twice, [8, 6, 0]), Some(BLUE));
    // Three times: forward points right.
    let thrice = turned(3);
    assert_eq!(colour(&thrice, [0, 6, 8]), Some(BLUE));
    assert_eq!(colour(&thrice, [4, 6, 15]), Some(RED));
    // Four times is no turn at all.
    assert_eq!(turned(4), turned(0));
}

#[test]
fn mirroring_swaps_left_and_right_only() {
    let voxels = voxelize(&marked_box(), &Options { longest: 16, solid: true, up: Some(Up::Y), turn: 0, mirror: true, smooth: false, shell: 1 }).unwrap();
    assert_eq!(colour(&voxels, [0, 6, 4]), Some(RED));
    assert_eq!(colour(&voxels, [8, 11, 4]), Some(GREEN));
    assert_eq!(colour(&voxels, [8, 6, 7]), Some(BLUE));
    assert_eq!(voxels.cells.len(), 16 * 12 * 8);
}

#[test]
fn a_model_without_colours_says_so_in_a_note() {
    let voxels = voxelize(&model(plain_box([0.0; 3], [1.0; 3])), &options(4, false)).unwrap();
    assert!(voxels.notes.contains(&"The file has no colours.".to_string()));
    assert!(voxels.cells.iter().all(|c| c.colour.is_none()));
}

#[test]
fn the_size_asked_for_is_held_to_its_range() {
    let cube = model(plain_box([0.0; 3], [1.0; 3]));
    assert_eq!(voxelize(&cube, &options(0, false)).unwrap().size, [4; 3]);
    // A rod, since a cube that long would be far too many blocks.
    let rod = model(plain_box([0.0; 3], [1.0, 0.0001, 0.0001]));
    assert_eq!(voxelize(&rod, &options(100_000, false)).unwrap().size, [8192, 1, 1]);
}

#[test]
fn a_box_not_a_whole_number_of_pages_across_fills_exactly() {
    // The grid is kept in pages of 16 cells; sizes that stop part way into
    // a page must neither lose cells at the edge nor invent any.
    let long = model(plain_box([0.0; 3], [37.0, 21.0, 19.0]));
    let solid = voxelize(&long, &options(37, true)).unwrap();
    assert_eq!(solid.size, [37, 21, 19]);
    assert_eq!(solid.cells.len(), 37 * 21 * 19);
    assert_eq!(solid.cells.iter().filter(|c| !c.inside).count(), 37 * 21 * 19 - 35 * 19 * 17);
    // Ordered by z, then y, then x.
    assert!(solid.cells.windows(2).all(|w| (w[0].pos[2], w[0].pos[1], w[0].pos[0]) < (w[1].pos[2], w[1].pos[1], w[1].pos[0])));
    let hollow = voxelize(&long, &options(37, false)).unwrap();
    assert_eq!(hollow.cells.len(), 37 * 21 * 19 - 35 * 19 * 17);
}

#[test]
fn a_ship_thousands_of_blocks_long_is_kept_in_memory_by_its_surface() {
    // A 4096 long, thin plate: a dense grid of the box would be fine here,
    // but a hull-shaped shell of this length would not; the plate checks
    // the size is accepted and the surface is all that is listed.
    let plate = model(plain_box([0.0; 3], [4096.0, 64.0, 2.0]));
    let hollow = voxelize(&plate, &options(4096, false)).unwrap();
    assert_eq!(hollow.size, [4096, 64, 2]);
    assert_eq!(hollow.cells.len(), 4096 * 64 * 2);
    let solid = voxelize(&plate, &options(4096, true)).unwrap();
    assert_eq!(solid.cells.len(), 4096 * 64 * 2);
}

#[test]
fn a_result_with_too_many_blocks_is_an_error_that_says_so() {
    let cube = model(plain_box([0.0; 3], [1.0; 3]));
    let error = voxelize(&cube, &options(512, true)).unwrap_err();
    assert!(matches!(error, ImportError::TooLarge(_)), "{error}");
    assert!(error.to_string().contains("blocks"), "{error}");
}

#[test]
fn a_model_without_triangles_or_without_size_is_an_error() {
    assert!(matches!(voxelize(&model(Vec::new()), &options(8, true)), Err(ImportError::Empty(_))));
    let point = Triangle { corners: [[1.0; 3]; 3], paint: Paint::None };
    assert!(matches!(voxelize(&model(vec![point]), &options(8, true)), Err(ImportError::Empty(_))));
    // Three points in a row have a length but no surface.
    let line = Triangle { corners: [[0.0; 3], [1.0; 3], [2.0; 3]], paint: Paint::None };
    assert!(matches!(voxelize(&model(vec![line]), &options(8, true)), Err(ImportError::Empty(_))));
}

#[test]
fn triangles_with_corners_that_are_not_numbers_are_left_out() {
    let mut triangles = plain_box([0.0; 3], [1.0; 3]);
    triangles.push(Triangle { corners: [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], paint: Paint::None });
    triangles.push(Triangle { corners: [[f32::INFINITY, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], paint: Paint::None });
    assert_eq!(voxelize(&model(triangles), &options(8, true)).unwrap().cells.len(), 512);
}

#[test]
fn a_mesh_with_a_hole_smaller_than_a_block_still_fills() {
    // A cube whose top is two triangles that leave a slit a tenth of a
    // cell wide along the diagonal.
    let mut triangles: Vec<Triangle> = plain_box([0.0; 3], [8.0; 3]).into_iter().filter(|t| !t.corners.iter().all(|c| c[1] == 8.0)).collect();
    triangles.push(Triangle { corners: [[0.0, 8.0, 0.0], [0.0, 8.0, 8.0], [7.9, 8.0, 8.0]], paint: Paint::None });
    triangles.push(Triangle { corners: [[0.1, 8.0, 0.0], [8.0, 8.0, 8.0], [8.0, 8.0, 0.0]], paint: Paint::None });
    let voxels = voxelize(&model(triangles), &options(8, true)).unwrap();
    assert_eq!(voxels.cells.iter().filter(|c| c.inside).count(), 216);
}

fn block_model() -> Model {
    // An L lying in the file's x-y plane, two high in z; z is up.
    let mut cells = Vec::new();
    for z in 0..2 {
        for x in 0..3 {
            cells.push(([x, 0, z], RED));
        }
        cells.push(([0, 1, z], GREEN));
    }
    Model::from_blocks(Blocks { cells }, Up::Z)
}

#[test]
fn ready_made_blocks_are_laid_out_as_they_are_and_turned_as_asked() {
    let voxels = voxelize(&block_model(), &options(64, false)).unwrap();
    // (x, y, z) becomes (x, z, -y): the file's +y, where the green block
    // is, is the back.
    assert_eq!(voxels.size, [3, 2, 2]);
    assert_eq!(voxels.cells.len(), 8);
    assert_eq!(colour(&voxels, [0, 0, 0]), Some(GREEN));
    assert_eq!(colour(&voxels, [2, 1, 1]), Some(RED));
    assert!(cell(&voxels, [1, 0, 0]).is_none());
    let turned = voxelize(&block_model(), &Options { longest: 64, solid: false, up: Some(Up::Y), turn: 1, mirror: true, smooth: false, shell: 1 }).unwrap();
    // Y up: (x, y, z); a quarter turn: (z, y, -x); mirrored: (-z, y, -x).
    assert_eq!(turned.size, [2, 2, 3]);
    assert_eq!(colour(&turned, [0, 1, 2]), Some(GREEN));
    assert_eq!(colour(&turned, [1, 0, 0]), Some(RED));
}

#[test]
fn ready_made_blocks_are_never_resized_and_too_long_a_model_is_an_error() {
    let mut cells = Vec::new();
    for x in 0..40 {
        cells.push(([x, 0, 0], RED));
    }
    let long = Model::from_blocks(Blocks { cells }, Up::Z);
    assert_eq!(voxelize(&long, &options(64, false)).unwrap().size, [40, 1, 1]);
    let error = voxelize(&long, &options(32, false)).unwrap_err();
    assert!(matches!(error, ImportError::TooLarge(_)));
    assert!(error.to_string().contains("40 x 1 x 1"), "{error}");
}

#[test]
fn a_hollow_box_of_ready_made_blocks_is_filled_when_asked() {
    let mut cells = Vec::new();
    for x in 0..5 {
        for y in 0..5 {
            for z in 0..5 {
                if [x, y, z].iter().any(|&c| c == 0 || c == 4) {
                    cells.push(([x, y, z], BLUE));
                }
            }
        }
    }
    let hollow = Model::from_blocks(Blocks { cells }, Up::Z);
    assert_eq!(voxelize(&hollow, &options(8, false)).unwrap().cells.len(), 98);
    let solid = voxelize(&hollow, &options(8, true)).unwrap();
    assert_eq!(solid.cells.len(), 125);
    assert_eq!(solid.cells.iter().filter(|c| c.inside).count(), 27);
}

// ---- smoothing ----

fn smooth(longest: u32, solid: bool) -> Options {
    Options { longest, solid, smooth: true, ..Options::default() }
}

/// How much space the blocks fill, in whole blocks: a partial block counts
/// the share of its sample points that are filled.
fn volume(voxels: &Voxels) -> f64 {
    voxels.cells.iter().map(|c| c.partial.map_or(1.0, |m| f64::from(m.count_ones()) / 64.0)).sum()
}

fn partial_count(voxels: &Voxels) -> usize {
    voxels.cells.iter().filter(|c| c.partial.is_some()).count()
}

/// A solid with flat faces from its corners and the faces as corner
/// numbers; every face is wound to face away from the middle.
fn solid_of(points: &[[f32; 3]], faces: &[&[usize]]) -> Vec<Triangle> {
    let middle = [0, 1, 2].map(|k| points.iter().map(|p| p[k]).sum::<f32>() / points.len() as f32);
    let mut out = Vec::new();
    for face in faces {
        for i in 1..face.len() - 1 {
            let mut corners = [points[face[0]], points[face[i]], points[face[i + 1]]];
            let (a, b) = ([0, 1, 2].map(|k| corners[1][k] - corners[0][k]), [0, 1, 2].map(|k| corners[2][k] - corners[0][k]));
            let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            let out_of = (0..3).map(|k| n[k] * (corners[0][k] - middle[k])).sum::<f32>();
            if out_of < 0.0 {
                corners.swap(1, 2);
            }
            out.push(Triangle { corners, paint: Paint::None });
        }
    }
    out
}

#[test]
fn smoothing_a_box_with_faces_on_the_grid_changes_nothing() {
    let cube = model(plain_box([0.0; 3], [1.0; 3]));
    for solid in [true, false] {
        let plain = voxelize(&cube, &options(12, solid)).unwrap();
        let smoothed = voxelize(&cube, &smooth(12, solid)).unwrap();
        assert_eq!(partial_count(&smoothed), 0);
        assert_eq!(smoothed.cells.len(), plain.cells.len());
        assert_eq!(smoothed.cells.iter().map(|c| c.pos).collect::<Vec<_>>(), plain.cells.iter().map(|c| c.pos).collect::<Vec<_>>());
    }
}

#[test]
fn smoothing_a_slope_gives_partial_blocks_that_keep_its_volume() {
    // A wedge 16 long, 8 high at the back and 8 wide: its top slopes 1 in 2.
    let points = [[0.0, 0.0, 0.0], [16.0, 0.0, 0.0], [16.0, 8.0, 0.0], [0.0, 0.0, 8.0], [16.0, 0.0, 8.0], [16.0, 8.0, 8.0]];
    let wedge = model(solid_of(&points, &[&[0, 1, 2], &[3, 5, 4], &[0, 3, 4, 1], &[1, 4, 5, 2], &[0, 2, 5, 3]]));
    let truth = 0.5 * 16.0 * 8.0 * 8.0;
    let plain = voxelize(&wedge, &options(16, true)).unwrap();
    let smoothed = voxelize(&wedge, &smooth(16, true)).unwrap();
    assert!(partial_count(&smoothed) > 8, "{} partial blocks", partial_count(&smoothed));
    let (before, after) = ((volume(&plain) - truth).abs(), (volume(&smoothed) - truth).abs());
    assert!(after < before && after < 0.04 * truth, "volume off by {before} before and {after} after, of {truth}");
}

#[test]
fn smoothing_a_ball_is_closer_to_the_ball_than_the_blocks_alone() {
    let ball = model(ball(1.0, 48, 96));
    // What a ball cut like this fills: close to, a little under, 4/3 pi r^3.
    let truth = 4.0 / 3.0 * std::f64::consts::PI * 10.0f64.powi(3) * 0.995;
    let plain = voxelize(&ball, &options(20, true)).unwrap();
    let smoothed = voxelize(&ball, &smooth(20, true)).unwrap();
    assert!(partial_count(&smoothed) > 100, "{} partial blocks", partial_count(&smoothed));
    let (before, after) = ((volume(&plain) - truth).abs() / truth, (volume(&smoothed) - truth).abs() / truth);
    assert!(after < before / 2.0 && after < 0.02, "volume off by {:.1}% before and {:.1}% after", before * 100.0, after * 100.0);
    // Still a closed shape: every partial block has a neighbour that is whole or partial.
    let at: std::collections::HashSet<_> = smoothed.cells.iter().map(|c| c.pos).collect();
    for c in smoothed.cells.iter().filter(|c| c.partial.is_some()) {
        let [x, y, z] = c.pos;
        let near = [[1i32, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]].iter().filter(|d| at.contains(&[(x as i32 + d[0]) as u16, (y as i32 + d[1]) as u16, (z as i32 + d[2]) as u16])).count();
        assert!(near >= 1, "{:?} stands alone", c.pos);
    }
}

#[test]
fn smoothing_a_hollow_ball_has_the_same_shell_as_a_solid_one() {
    let ball = model(ball(1.0, 32, 64));
    let solid = voxelize(&ball, &smooth(16, true)).unwrap();
    let hollow = voxelize(&ball, &smooth(16, false)).unwrap();
    let shell = |v: &Voxels| v.cells.iter().filter(|c| !c.inside).map(|c| (c.pos, c.partial)).collect::<Vec<_>>();
    assert_eq!(shell(&solid), shell(&hollow));
    assert!(hollow.cells.iter().all(|c| !c.inside));
}

#[test]
fn a_thin_plate_is_not_smoothed_away() {
    // A plate a tenth of a block thick, tilted a little.
    let points = [[0.0, 0.0, 0.0], [16.0, 1.0, 0.0], [16.0, 1.1, 0.0], [0.0, 0.1, 0.0], [0.0, 0.0, 8.0], [16.0, 1.0, 8.0], [16.0, 1.1, 8.0], [0.0, 0.1, 8.0]];
    let plate = model(solid_of(&points, &[&[0, 1, 2, 3], &[4, 7, 6, 5], &[0, 4, 5, 1], &[3, 2, 6, 7], &[0, 3, 7, 4], &[1, 5, 6, 2]]));
    let plain = voxelize(&plate, &options(16, false)).unwrap();
    let smoothed = voxelize(&plate, &smooth(16, false)).unwrap();
    assert!(smoothed.cells.len() * 10 >= plain.cells.len() * 8, "{} of {} blocks left", smoothed.cells.len(), plain.cells.len());
}

#[test]
fn smoothing_does_nothing_to_a_model_that_is_blocks_already() {
    let blocks = Model::from_blocks(Blocks { cells: vec![([0, 0, 0], [9, 9, 9]), ([1, 0, 0], [9, 9, 9])] }, Up::Y);
    let voxels = voxelize(&blocks, &smooth(8, true)).unwrap();
    assert_eq!(voxels.cells.len(), 2);
    assert!(voxels.cells.iter().all(|c| c.partial.is_none()));
    assert!(voxels.notes.iter().any(|n| n.contains("not smoothed")), "{:?}", voxels.notes);
}
