//! Reading each format: a small file is built, read, and its triangles,
//! bounds and colours checked.

mod common;

use common::samples::{self, TOP};
use common::{bounds, close};
use yard_import::{formats, read, read_with, voxelize, Files, ImportError, Model, Options, Paint, Up};

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// Checks a model is the two-coloured cube: twelve triangles between
/// `low` and `high`, red where `is_top` holds for all three corners of a
/// triangle and blue elsewhere.
fn assert_cube(model: &Model, low: [f32; 3], high: [f32; 3], is_top: impl Fn([f32; 3]) -> bool) {
    assert_eq!(model.triangles.len(), 12, "{}", model.format);
    let (found_low, found_high) = bounds(&model.triangles);
    assert!(close(found_low, low) && close(found_high, high), "{}: bounds {found_low:?} to {found_high:?}", model.format);
    let mut tops = 0;
    for triangle in &model.triangles {
        let top = triangle.corners.iter().all(|&c| is_top(c));
        tops += usize::from(top);
        assert_eq!(triangle.paint, Paint::Flat(if top { RED } else { BLUE }), "{}: {:?}", model.format, triangle.corners);
    }
    assert_eq!(tops, 2, "{}", model.format);
}

fn unit_cube(model: &Model) {
    assert_cube(model, [0.0; 3], [1.0; 3], |c| c[1] == 1.0);
}

#[test]
fn obj_with_its_material_file_in_a_zip_gives_a_two_coloured_cube() {
    let (obj, mtl) = samples::obj();
    let model = read("cube.zip", &samples::zip(&[("model/cube.obj", obj.as_bytes()), ("model/cube.mtl", mtl.as_bytes())])).unwrap();
    assert_eq!(model.format, "Wavefront OBJ");
    assert_eq!(model.up, Up::Y);
    unit_cube(&model);
    assert!(model.notes.is_empty(), "{:?}", model.notes);
}

#[test]
fn obj_alone_reads_without_colours_and_says_where_they_are() {
    let (obj, _) = samples::obj();
    let model = read("cube.obj", obj.as_bytes()).unwrap();
    assert_eq!(model.triangles.len(), 12);
    assert!(model.triangles.iter().all(|t| t.paint == Paint::None));
    assert!(model.notes.iter().any(|n| n.contains("cube.mtl") && n.contains("zip")), "{:?}", model.notes);
    let voxels = voxelize(&model, &Options::default()).unwrap();
    assert!(voxels.notes.contains(&"The file has no colours.".to_string()));
}

#[test]
fn obj_reads_colours_on_points_negative_indices_long_faces_and_merged_groups() {
    let obj = "o first\nv 0 0 0 1 0 0\nv 1 0 0 1 0 0\nv 1 1 0 1 0 0\nv 0 1 0 1 0 0\nf -4 -3 -2 -1\n\
               o second\ng a group in it\nv 0 0 5 0 1 0\nv 2 0 5 0 1 0\nv 2 1 5 0 1 0\nv 1 1 5 0 1 0\nv 1 2 5 0 1 0\nv 0 2 5 0 1 0\n\
               f 5/1/1 6//1 7/2 8 9 \\\n 10\no third\nf 1 2 99\n";
    let model = read("shapes.obj", obj.as_bytes()).unwrap();
    // A square is two triangles, the six-cornered L four; the face with a
    // corner that does not exist is skipped.
    assert_eq!(model.triangles.len(), 6);
    assert!(model.triangles[..2].iter().all(|t| t.paint == Paint::Flat(RED)));
    assert!(model.triangles[2..].iter().all(|t| t.paint == Paint::Flat([0, 255, 0])));
    assert!(model.notes.contains(&"3 objects were merged.".to_string()), "{:?}", model.notes);
    assert!(model.notes.contains(&"1 broken face was skipped.".to_string()), "{:?}", model.notes);
    // The L's dent at (2,2) is not covered: its triangles' area is 3.
    let area: f32 = model.triangles[2..].iter().map(|t| ((t.corners[1][0] - t.corners[0][0]) * (t.corners[2][1] - t.corners[0][1]) - (t.corners[1][1] - t.corners[0][1]) * (t.corners[2][0] - t.corners[0][0])).abs() / 2.0).sum();
    assert!((area - 3.0).abs() < 1e-5, "{area}");
}

#[test]
fn obj_with_a_texture_in_the_zip_paints_blocks_from_the_picture() {
    let obj = "mtllib plate.mtl\nv 0 0 0\nv 8 0 0\nv 8 0 8\nv 0 0 8\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nusemtl skin\nf 1/1 2/2 3/3 4/4\n";
    let mtl = "newmtl skin\nKd 0.5 0.5 0.5\nmap_Kd -s 1 1 1 textures\\Hull Paint.png\n";
    let packed = samples::zip(&[("plate.obj", obj.as_bytes()), ("plate.mtl", mtl.as_bytes()), ("textures/hull paint.png", &samples::png())]);
    let model = read("plate.zip", &packed).unwrap();
    assert_eq!(model.textures.len(), 1);
    assert!(model.triangles.iter().all(|t| matches!(t.paint, Paint::Textured { texture: 0, .. })));
    let voxels = voxelize(&model, &Options { longest: 8, ..Options::default() }).unwrap();
    assert_eq!(voxels.cells.len(), 64);
    // The picture is red on its left half, where u is small: low x.
    for cell in &voxels.cells {
        assert_eq!(cell.colour, Some(if cell.pos[0] < 4 { RED } else { BLUE }), "{:?}", cell.pos);
    }
    // Without the picture the material's own colour is used, and noted.
    let bare = read("plate.zip", &samples::zip(&[("plate.obj", obj.as_bytes()), ("plate.mtl", mtl.as_bytes())])).unwrap();
    assert!(bare.triangles.iter().all(|t| t.paint == Paint::Flat([128, 128, 128])));
    assert_eq!(bare.notes, vec!["Textures were not found in the upload; material colours were used."]);
}

#[test]
fn stl_as_text_and_as_bytes_both_give_the_cube() {
    for (name, bytes) in [("a.stl", samples::stl_text().into_bytes()), ("b.stl", samples::stl_binary(b"binary cube"))] {
        let model = read(name, &bytes).unwrap();
        assert_eq!(model.format, "STL");
        assert_eq!(model.up, Up::Z);
        assert_eq!(model.triangles.len(), 12);
        assert_eq!(bounds(&model.triangles), ([0.0; 3], [1.0; 3]));
        assert!(model.triangles.iter().all(|t| t.paint == Paint::None));
    }
}

#[test]
fn a_binary_stl_that_starts_with_the_word_solid_is_still_read_as_binary() {
    let model = read("tricky.stl", &samples::stl_binary(b"solid made by a CAD program that likes this word")).unwrap();
    assert_eq!(model.triangles.len(), 12);
    // And a text STL with no file name ending to go by is known too.
    assert_eq!(read("upload", samples::stl_text().as_bytes()).unwrap().triangles.len(), 12);
}

#[test]
fn a_binary_stl_with_colours_in_its_spare_bytes_keeps_them() {
    let mut bytes = samples::stl_binary(b"coloured");
    // Each record is 50 bytes; its last two are spare. Top bit set, then
    // five bits each of red, green, blue: all red.
    for record in bytes[84..].chunks_exact_mut(50) {
        record[48..].copy_from_slice(&(0x8000u16 | 31 << 10).to_le_bytes());
    }
    let model = read("coloured.stl", &bytes).unwrap();
    assert!(model.triangles.iter().all(|t| t.paint == Paint::Flat(RED)));
}

#[test]
fn ply_as_text_and_in_both_byte_orders_gives_the_cube_with_point_colours() {
    for format in ["ascii", "binary_little_endian", "binary_big_endian"] {
        let model = read("cube.ply", &samples::ply(format)).unwrap();
        assert_eq!(model.format, "PLY");
        assert_eq!(model.triangles.len(), 12, "{format}");
        assert_eq!(bounds(&model.triangles), ([0.0; 3], [1.0; 3]), "{format}");
        for triangle in &model.triangles {
            let heights: Vec<f32> = triangle.corners.iter().map(|c| c[1]).collect();
            match &triangle.paint {
                Paint::Flat(colour) => assert_eq!(*colour, if heights.iter().all(|&h| h == 1.0) { RED } else { BLUE }, "{format}"),
                Paint::Corners(colours) => {
                    for (colour, height) in colours.iter().zip(&heights) {
                        assert_eq!(*colour, if *height == 1.0 { RED } else { BLUE }, "{format}");
                    }
                }
                other => panic!("{format}: {other:?}"),
            }
        }
    }
}

#[test]
fn a_ply_with_points_but_no_faces_is_an_error_that_explains_itself() {
    let cloud = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 0 0\n0 1 0\n";
    let error = read("cloud.ply", cloud.as_bytes()).unwrap_err();
    assert!(matches!(error, ImportError::Empty(_)));
    assert!(error.to_string().contains("only points"), "{error}");
}

#[test]
fn a_ply_passes_over_what_it_does_not_need_and_reads_face_colours() {
    let ply = "ply\nformat ascii 1.0\nelement vertex 4\nproperty double x\nproperty double y\nproperty double z\nproperty float nx\nproperty list uchar float extras\n\
               element edge 1\nproperty int vertex1\nproperty int vertex2\n\
               element face 2\nproperty list uint8 uint vertex_index\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n\
               0 0 0 1 2 7 7\n1 0 0 1 0\n1 1 0 1 0\n0 1 0 1 1 9\n0 1\n3 0 1 2 255 0 0\n3 0 2 3 0 0 255\n";
    let model = read("odd.ply", ply.as_bytes()).unwrap();
    assert_eq!(model.triangles.len(), 2);
    assert_eq!(model.triangles[0].paint, Paint::Flat(RED));
    assert_eq!(model.triangles[1].paint, Paint::Flat(BLUE));
}

#[test]
fn off_with_colours_gives_the_cube_painted_by_its_faces() {
    let model = read("cube.off", samples::off().as_bytes()).unwrap();
    assert_eq!(model.format, "OFF");
    unit_cube(&model);
    // A plain OFF with the counts on the heading's line and no colours.
    let plain = read("tri.off", b"OFF 3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n").unwrap();
    assert_eq!(plain.triangles.len(), 1);
    assert_eq!(plain.triangles[0].paint, Paint::None);
}

#[test]
fn glb_places_its_meshes_through_the_nodes_and_takes_material_colours() {
    let model = read("cube.glb", &samples::glb()).unwrap();
    assert_eq!(model.format, "glTF");
    assert_eq!(model.up, Up::Y);
    // Doubled, shifted by 3 along z, then turned a quarter about y.
    assert_cube(&model, [3.0, 0.0, -2.0], [5.0, 2.0, 0.0], |c| (c[1] - 2.0).abs() < 1e-5);
}

#[test]
fn gltf_as_text_reads_its_data_from_inside_it_or_from_the_zip() {
    let embedded = read("cube.gltf", &samples::gltf_embedded()).unwrap();
    assert_cube(&embedded, [3.0, 0.0, -2.0], [5.0, 2.0, 0.0], |c| (c[1] - 2.0).abs() < 1e-5);
    let (gltf, bin) = samples::gltf_separate();
    let zipped = read("cube.zip", &samples::zip(&[("cube.gltf", &gltf), ("cube data.bin", &bin)])).unwrap();
    assert_eq!(zipped.triangles, embedded.triangles);
    // Without its data file the error says which file is missing.
    let error = read("cube.gltf", &gltf).unwrap_err();
    assert!(matches!(error, ImportError::Missing(_)));
    assert!(error.to_string().contains("cube data.bin"), "{error}");
    // The companion files can also be handed over one by one.
    let mut files = Files::new();
    files.add("cube data.bin", bin);
    assert_eq!(read_with("cube.gltf", &gltf, &files).unwrap().triangles.len(), 12);
}

/// A glTF plate of two triangles, 8 by 8 in x and z, described by `extra`
/// attributes and material on top of positions; `bin` follows the
/// positions in the buffer.
fn gltf_plate(attributes: serde_json::Value, material: serde_json::Value, mut views: Vec<serde_json::Value>, mut accessors: Vec<serde_json::Value>, extra: &[u8], images: serde_json::Value) -> Vec<u8> {
    use serde_json::json;
    let points: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [8.0, 0.0, 0.0], [0.0, 0.0, 8.0], [8.0, 0.0, 8.0]];
    let mut bin: Vec<u8> = points.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    bin.extend(extra);
    let mut all_views = vec![json!({"buffer": 0, "byteOffset": 0, "byteLength": 48})];
    all_views.append(&mut views);
    let mut all_accessors = vec![json!({"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"})];
    all_accessors.append(&mut accessors);
    let mut all_attributes = json!({"POSITION": 0});
    for (key, value) in attributes.as_object().unwrap() {
        all_attributes[key] = value.clone();
    }
    let doc = json!({
        "asset": {"version": "2.0"},
        "nodes": [{"mesh": 0}],
        // A strip of four corners: two triangles.
        "meshes": [{"primitives": [{"attributes": all_attributes, "material": 0, "mode": 5}]}],
        "materials": [material],
        "textures": [{"source": 0}],
        "images": images,
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": all_views,
        "accessors": all_accessors
    });
    samples::glb_of(&doc, &bin)
}

#[test]
fn gltf_blends_point_colours_with_the_material_and_reads_strips_without_indices() {
    use serde_json::json;
    // Colours as bytes standing for parts of one: white, white, black, black.
    let colours = [255u8, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255];
    let glb = gltf_plate(
        json!({"COLOR_0": 1}),
        json!({"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}}),
        vec![json!({"buffer": 0, "byteOffset": 48, "byteLength": 16})],
        vec![json!({"bufferView": 1, "componentType": 5121, "normalized": true, "count": 4, "type": "VEC4"})],
        &colours,
        json!([]),
    );
    let model = read("plate.glb", &glb).unwrap();
    assert_eq!(model.triangles.len(), 2);
    // White times the red material is red; black stays black.
    assert_eq!(model.triangles[0].paint, Paint::Corners([RED, RED, [0, 0, 0]]));
}

#[test]
fn gltf_samples_a_texture_packed_into_the_file() {
    use serde_json::json;
    let mut extra: Vec<u8> = [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let png = samples::png();
    extra.extend(&png);
    let glb = gltf_plate(
        json!({"TEXCOORD_0": 1}),
        json!({"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}),
        vec![json!({"buffer": 0, "byteOffset": 48, "byteLength": 32}), json!({"buffer": 0, "byteOffset": 80, "byteLength": png.len()})],
        vec![json!({"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"})],
        &extra,
        json!([{"bufferView": 2, "mimeType": "image/png"}]),
    );
    let model = read("plate.glb", &glb).unwrap();
    assert_eq!(model.textures.len(), 1);
    assert!(model.notes.is_empty(), "{:?}", model.notes);
    let voxels = voxelize(&model, &Options { longest: 8, ..Options::default() }).unwrap();
    for cell in &voxels.cells {
        assert_eq!(cell.colour, Some(if cell.pos[0] < 4 { RED } else { BLUE }), "{:?}", cell.pos);
    }
}

#[test]
fn gltf_reads_a_jpeg_texture_from_a_link_written_into_the_file() {
    use base64::Engine;
    use serde_json::json;
    let extra: Vec<u8> = [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let link = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(samples::jpeg()));
    let glb = gltf_plate(
        json!({"TEXCOORD_0": 1}),
        json!({"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}),
        vec![json!({"buffer": 0, "byteOffset": 48, "byteLength": 32})],
        vec![json!({"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"})],
        &extra,
        json!([{"uri": link}]),
    );
    let model = read("plate.glb", &glb).unwrap();
    let voxels = voxelize(&model, &Options { longest: 8, ..Options::default() }).unwrap();
    // JPEG does not keep colours exactly; the picture is plain green.
    for cell in &voxels.cells {
        let [r, g, b] = cell.colour.unwrap();
        assert!(r < 12 && (188..212).contains(&g) && b < 12, "{:?}", cell.colour);
    }
}

#[test]
fn gltf_packed_with_draco_or_meshopt_is_refused_with_advice() {
    for (extension, word) in [("KHR_draco_mesh_compression", "Draco"), ("EXT_meshopt_compression", "Meshopt")] {
        let doc = serde_json::json!({"asset": {"version": "2.0"}, "extensionsRequired": [extension], "extensionsUsed": [extension], "meshes": []});
        let error = read("packed.gltf", &serde_json::to_vec(&doc).unwrap()).unwrap_err();
        assert!(matches!(error, ImportError::Unsupported(_)));
        assert!(error.to_string().contains(word) && error.to_string().contains("without compression"), "{error}");
    }
}

#[test]
fn a_zip_with_several_models_uses_the_largest_and_says_so() {
    let (obj, mtl) = samples::obj();
    let packed = samples::zip(&[("small.off", b"OFF 3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n"), ("big/cube.obj", obj.as_bytes()), ("big/cube.mtl", mtl.as_bytes()), ("__MACOSX/big/._cube.obj", b"junk"), ("readme.txt", b"hello")]);
    let model = read("models.zip", &packed).unwrap();
    assert_eq!(model.format, "Wavefront OBJ");
    unit_cube(&model);
    assert_eq!(model.notes[0], "The zip holds 2 models; the largest, big/cube.obj, was used.");
}

#[test]
fn a_zip_without_a_model_says_what_it_should_hold() {
    let error = read("stuff.zip", &samples::zip(&[("readme.txt", b"hello")])).unwrap_err();
    assert!(matches!(error, ImportError::UnknownFormat(_)));
    assert!(error.to_string().contains(".obj"), "{error}");
    let error = read("stuff.zip", &samples::zip(&[("ship.blend", b"BLENDER-v300")])).unwrap_err();
    assert!(error.to_string().contains("ship.blend") && error.to_string().contains("Export it from Blender"), "{error}");
}

#[test]
fn vox_gives_ready_made_blocks_with_the_files_palette_or_the_default_one() {
    let model = read("blocks.vox", &samples::vox(true)).unwrap();
    assert_eq!(model.format, "MagicaVoxel");
    assert_eq!(model.up, Up::Z);
    assert!(model.triangles.is_empty());
    let blocks = model.blocks.as_ref().unwrap();
    assert_eq!(blocks.cells.len(), 12);
    assert!(blocks.cells.iter().all(|(at, colour)| *colour == if at[2] == blocks.cells.iter().map(|(at, _)| at[2]).max().unwrap() { RED } else { BLUE }));
    // z is up: the file's 3 x 2 x 2 becomes 3 wide, 2 high, 2 deep, red on top.
    let voxels = voxelize(&model, &Options::default()).unwrap();
    assert_eq!(voxels.size, [3, 2, 2]);
    assert!(voxels.cells.iter().all(|c| c.colour == Some(if c.pos[1] == 1 { RED } else { BLUE })));
    // Without a palette: colour 1 of the default palette is white, 2 a pale yellow.
    let plain = voxelize(&read("blocks.vox", &samples::vox(false)).unwrap(), &Options::default()).unwrap();
    assert!(plain.cells.iter().all(|c| c.colour == Some(if c.pos[1] == 1 { [255, 255, 255] } else { [255, 255, 204] })));
}

#[test]
fn vox_with_a_scene_places_every_model_and_without_one_takes_the_first() {
    let model = read("scene.vox", &samples::vox_scene(true)).unwrap();
    assert_eq!(model.notes, vec!["2 objects were merged."]);
    let voxels = voxelize(&model, &Options { solid: false, ..Options::default() }).unwrap();
    // The cube about the origin and the bar at x = 10, turned to run
    // along the file's y, which is the ship's depth.
    assert_eq!(voxels.size, [12, 2, 4]);
    assert_eq!(voxels.cells.len(), 12);
    let bar: Vec<_> = voxels.cells.iter().filter(|c| c.colour == Some(BLUE)).collect();
    assert_eq!(bar.len(), 4);
    assert!(bar.iter().all(|c| c.pos[0] == 11 && c.pos[1] == 1));
    let first = read("frames.vox", &samples::vox_scene(false)).unwrap();
    assert_eq!(first.blocks.as_ref().unwrap().cells.len(), 8);
    assert_eq!(first.notes, vec!["The file holds 2 models; only the first was used."]);
}

#[test]
fn collada_places_geometry_through_its_nodes_and_binds_material_colours() {
    let model = read("cube.dae", samples::dae().as_bytes()).unwrap();
    assert_eq!(model.format, "Collada");
    assert_eq!(model.up, Up::Z);
    assert_cube(&model, [5.0, 0.0, 0.0], [7.0, 2.0, 2.0], |c| c[1] == 2.0);
}

#[test]
fn collada_reads_polygons_point_colours_and_textures_from_the_zip() {
    let dae = r##"<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
 <library_images><image id="paint"><init_from>file:///C:/work/Hull%20Paint.png</init_from></image></library_images>
 <library_effects><effect id="fx"><profile_COMMON>
  <newparam sid="paint-surface"><surface type="2D"><init_from>paint</init_from></surface></newparam>
  <newparam sid="paint-sampler"><sampler2D><source>paint-surface</source></sampler2D></newparam>
  <technique sid="common"><lambert><diffuse><texture texture="paint-sampler" texcoord="UV"/></diffuse></lambert></technique>
 </profile_COMMON></effect></library_effects>
 <library_materials><material id="skin"><instance_effect url="#fx"/></material></library_materials>
 <library_geometries>
  <geometry id="plate"><mesh>
   <source id="p"><float_array id="pa" count="12">0 0 0 8 0 0 8 0 8 0 0 8</float_array><technique_common><accessor source="#pa" count="4" stride="3"/></technique_common></source>
   <source id="t"><float_array id="ta" count="8">0 1 1 1 1 0 0 0</float_array><technique_common><accessor source="#ta" count="4" stride="2"/></technique_common></source>
   <vertices id="v"><input semantic="POSITION" source="#p"/></vertices>
   <polygons material="skin" count="1"><input semantic="VERTEX" source="#v" offset="0"/><input semantic="TEXCOORD" source="#t" offset="1" set="0"/><p>0 0 1 1 2 2 3 3</p></polygons>
  </mesh></geometry>
  <geometry id="flag"><mesh>
   <source id="fp"><float_array id="fpa" count="9">0 5 0 1 5 0 0 6 0</float_array><technique_common><accessor source="#fpa" count="3" stride="3"/></technique_common></source>
   <source id="fc"><float_array id="fca" count="8">1 0 0 1 0 0 1 1</float_array><technique_common><accessor source="#fca" count="2" stride="4"/></technique_common></source>
   <vertices id="fv"><input semantic="POSITION" source="#fp"/></vertices>
   <triangles count="1"><input semantic="VERTEX" source="#fv" offset="0"/><input semantic="COLOR" source="#fc" offset="1"/><p>0 0 1 0 2 1</p></triangles>
  </mesh></geometry>
 </library_geometries>
 <library_visual_scenes><visual_scene id="s">
  <node><instance_geometry url="#plate"><bind_material><technique_common><instance_material symbol="skin" target="#skin"/></technique_common></bind_material></instance_geometry></node>
  <node><instance_geometry url="#flag"/></node>
 </visual_scene></library_visual_scenes>
 <scene><instance_visual_scene url="#s"/></scene>
</COLLADA>"##;
    let model = read("ship.zip", &samples::zip(&[("ship.dae", dae.as_bytes()), ("tex/Hull Paint.png", &samples::png())])).unwrap();
    assert_eq!(model.up, Up::Y);
    assert_eq!(model.triangles.len(), 3);
    assert_eq!(model.triangles[2].paint, Paint::Corners([RED, RED, BLUE]));
    // The plate's picture places are written bottom-up, as Collada does:
    // the corner at x = 0 lies on the picture's left, which is red.
    match model.triangles[0].paint {
        Paint::Textured { texture: 0, uv, .. } => assert_eq!(uv, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]),
        other => panic!("{other:?}"),
    }
    assert_eq!(model.notes, vec!["2 objects were merged."]);
}

#[test]
fn threemf_builds_its_items_with_their_transforms_components_and_colours() {
    let model = read("cube.3mf", &samples::threemf()).unwrap();
    assert_eq!(model.format, "3MF");
    assert_eq!(model.up, Up::Z);
    // The cube as it is, and once more shifted by 10 and then doubled.
    assert_eq!(model.triangles.len(), 24);
    assert_eq!(bounds(&model.triangles), ([0.0; 3], [22.0, 2.0, 2.0]));
    for (i, triangle) in model.triangles.iter().enumerate() {
        assert_eq!(triangle.paint, Paint::Flat(if i % 12 / 2 == TOP { RED } else { BLUE }));
    }
    assert_eq!(model.notes, vec!["2 objects were merged."]);
}

#[test]
fn fbx_as_bytes_of_both_offset_widths_packed_or_not_and_as_text_gives_the_cube() {
    let files = [("7400 packed", samples::fbx_binary(7400, true)), ("7400 plain", samples::fbx_binary(7400, false)), ("7500 packed", samples::fbx_binary(7500, true)), ("7700 plain", samples::fbx_binary(7700, false)), ("text", samples::fbx_text().into_bytes())];
    for (name, bytes) in files {
        let model = read("cube.fbx", &bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(model.format, "FBX");
        assert_eq!(model.up, Up::Z, "{name}");
        // Doubled, shifted by 10 along x, under a parent turned a quarter
        // about z: (x, y, z) ends at (-2y, 10 + 2x, 2z).
        assert_cube(&model, [-2.0, 10.0, 0.0], [0.0, 12.0, 2.0], |c| (c[0] + 2.0).abs() < 1e-4);
    }
}

#[test]
fn fbx_of_the_older_kind_with_geometry_inside_its_model_is_read() {
    let model = read("old.fbx", samples::fbx_text_old().as_bytes()).unwrap();
    assert_eq!(model.up, Up::Y);
    assert_eq!(model.triangles.len(), 12);
    assert_eq!(bounds(&model.triangles), ([0.0, 1.0, 0.0], [1.0, 2.0, 1.0]));
    assert!(model.triangles.iter().all(|t| t.paint == Paint::Flat(BLUE)));
}

#[test]
fn tds_gives_the_cube_with_a_material_colour_per_group_of_faces() {
    let model = read("cube.3ds", &samples::tds()).unwrap();
    assert_eq!(model.format, "3D Studio");
    assert_eq!(model.up, Up::Z);
    unit_cube(&model);
}

#[test]
fn amf_gives_the_cube_with_volume_and_material_colours_also_from_a_zip() {
    let amf = samples::amf();
    let model = read("cube.amf", amf.as_bytes()).unwrap();
    assert_eq!(model.format, "AMF");
    assert_eq!(model.up, Up::Z);
    unit_cube(&model);
    let zipped = read("cube.amf", &samples::zip(&[("cube.amf", amf.as_bytes())])).unwrap();
    assert_eq!(zipped.triangles, model.triangles);
}

#[test]
fn x3d_gives_the_cube_placed_by_its_transform_with_a_colour_per_face() {
    let model = read("cube.x3d", samples::x3d().as_bytes()).unwrap();
    assert_eq!(model.format, "X3D");
    assert_cube(&model, [0.0, 0.0, 4.0], [2.0, 2.0, 6.0], |c| c[1] == 2.0);
}

#[test]
fn vrml_gives_the_cube_from_two_shapes_sharing_their_points_by_name() {
    let model = read("cube.wrl", samples::vrml().as_bytes()).unwrap();
    assert_eq!(model.format, "VRML");
    // Turned a quarter about y, (x, y, z) becomes (z, y, -x), then shifted.
    assert_cube(&model, [4.0, 0.0, -1.0], [5.0, 1.0, 0.0], |c| c[1] == 1.0);
    assert_eq!(model.notes, vec!["2 objects were merged."]);
}

#[test]
fn every_sample_is_known_by_its_contents_whatever_it_is_called() {
    for (name, bytes) in samples::all() {
        // OBJ has no mark to know it by but its lines; everything else
        // must be told from its first bytes.
        let by_contents = read("upload.dat", &bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let by_name = read(name, &bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(by_contents.format, by_name.format, "{name}");
        assert_eq!(by_contents.triangles.len(), by_name.triangles.len(), "{name}");
    }
}

#[test]
fn every_sample_turns_into_blocks() {
    for (name, bytes) in samples::all() {
        let model = read(name, &bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let voxels = voxelize(&model, &Options { longest: 12, ..Options::default() }).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(!voxels.cells.is_empty(), "{name}");
        assert!(voxels.size.iter().all(|&s| (1..=12).contains(&s)), "{name}: {:?}", voxels.size);
    }
}

#[test]
fn files_that_cannot_be_read_are_named_for_what_they_are() {
    let cases: [(&str, &[u8], &str); 12] = [
        ("ship.blend", b"BLENDER-v293", "Export it from Blender as glTF (.glb) or OBJ."),
        ("renamed.obj", b"BLENDER-v293", "Blender files cannot be read directly"),
        ("part.step", b"ISO-10303-21;", "export as STL"),
        ("part.IGS", b"                                                                        S      1", "export as STL"),
        ("scene.usdz", b"PK\x03\x04rest", "USD files cannot be read"),
        ("scene.max", b"\xd0\xcf\x11\xe0", "3ds Max"),
        ("scene.c4d", b"QC4DC4D6", "Cinema 4D"),
        ("house.skp", b"\xff\xfe\xff\x0eS\0k\0", "SketchUp"),
        ("scene.mb", b"FOR4", "Maya"),
        ("hull.3dm", b"3D Geometry File Format", "Rhino"),
        ("castle.schematic", b"\x1f\x8b\x08\0", "Minecraft schematics cannot be read"),
        ("castle.litematic", b"\x1f\x8b\x08\0", "Minecraft"),
    ];
    for (name, bytes, expected) in cases {
        let error = read(name, bytes).unwrap_err();
        assert!(matches!(error, ImportError::Unsupported(_)), "{name}: {error:?}");
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn an_unknown_file_lists_what_can_be_read() {
    let error = read("notes.txt", b"just some words").unwrap_err();
    assert!(matches!(error, ImportError::UnknownFormat(_)));
    for format in formats() {
        for extension in format.extensions {
            assert!(error.to_string().contains(&format!(".{extension}")), "{extension} is missing from: {error}");
        }
    }
    assert!(matches!(read("empty.obj", b""), Err(ImportError::Empty(_))));
}

#[test]
fn the_list_of_formats_names_each_ending_once() {
    let mut endings: Vec<&str> = formats().iter().flat_map(|f| f.extensions.iter().copied()).collect();
    assert!(endings.contains(&"obj") && endings.contains(&"glb") && endings.contains(&"fbx") && endings.contains(&"vox"));
    let count = endings.len();
    endings.sort_unstable();
    endings.dedup();
    assert_eq!(endings.len(), count);
}
