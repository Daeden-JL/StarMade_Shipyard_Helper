//! A small model in every format, built here so that no test depends on a
//! file from elsewhere. Unless said otherwise each is the unit cube with
//! its top side (+y in the file) red and the rest blue.

use std::io::Write;

use serde_json::json;

use super::{cube_triangles, CUBE_POINTS, CUBE_SIDES};

/// Which of the cube's sides is the red one: +y.
pub const TOP: usize = 3;

fn join<T: ToString>(items: impl IntoIterator<Item = T>) -> String {
    items.into_iter().map(|i| i.to_string()).collect::<Vec<_>>().join(" ")
}

fn flat_points() -> Vec<f32> {
    CUBE_POINTS.iter().flatten().copied().collect()
}

/// A picture two texels wide: red on the left, blue on the right.
pub fn png() -> Vec<u8> {
    let picture = image::RgbImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A green picture, as JPEG.
pub fn jpeg() -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(16, 16, image::Rgb([0, 200, 0]));
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// The cube as OBJ with quads, and its material file.
pub fn obj() -> (String, String) {
    let mut obj = String::from("# a cube\nmtllib cube.mtl\no cube\n");
    for p in CUBE_POINTS {
        obj += &format!("v {} {} {}\n", p[0], p[1], p[2]);
    }
    for (side, corners) in CUBE_SIDES.iter().enumerate() {
        obj += if side == TOP { "usemtl red\n" } else { "usemtl blue\n" };
        obj += &format!("f {}\n", join(corners.iter().map(|c| c + 1)));
    }
    (obj, "newmtl red\nKd 1 0 0\nnewmtl blue\nKd 0.0 0.0 1.0\n".into())
}

pub fn stl_text() -> String {
    let mut out = String::from("solid cube\n");
    for t in cube_triangles() {
        out += " facet normal 0 0 0\n  outer loop\n";
        for c in t {
            out += &format!("   vertex {}\n", join(CUBE_POINTS[c]));
        }
        out += "  endloop\n endfacet\n";
    }
    out + "endsolid cube\n"
}

/// A binary STL; `head` is what its 80 free bytes start with.
pub fn stl_binary(head: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out[..head.len()].copy_from_slice(head);
    out.extend(12u32.to_le_bytes());
    for t in cube_triangles() {
        out.extend([0u8; 12]);
        for c in t {
            out.extend(CUBE_POINTS[c].iter().flat_map(|v| v.to_le_bytes()));
        }
        out.extend([0u8; 2]);
    }
    out
}

/// The cube as PLY with a colour per point (top points red) and quads.
/// `format` is "ascii", "binary_little_endian" or "binary_big_endian".
pub fn ply(format: &str) -> Vec<u8> {
    let head = format!("ply\nformat {format} 1.0\ncomment made by a test\nelement vertex 8\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 6\nproperty list uchar int vertex_indices\nend_header\n");
    let mut out = head.into_bytes();
    let colour = |p: [f32; 3]| if p[1] == 1.0 { [255u8, 0, 0] } else { [0, 0, 255] };
    let big = format == "binary_big_endian";
    for p in CUBE_POINTS {
        if format == "ascii" {
            out.extend(format!("{} {}\n", join(p), join(colour(p))).bytes());
        } else {
            out.extend(p.iter().flat_map(|v| if big { v.to_be_bytes() } else { v.to_le_bytes() }));
            out.extend(colour(p));
        }
    }
    for side in CUBE_SIDES {
        if format == "ascii" {
            out.extend(format!("4 {}\n", join(side)).bytes());
        } else {
            out.push(4);
            out.extend(side.iter().flat_map(|&c| if big { (c as i32).to_be_bytes() } else { (c as i32).to_le_bytes() }));
        }
    }
    out
}

pub fn off() -> String {
    let mut out = String::from("COFF\n# points faces edges\n8 6 12\n");
    for p in CUBE_POINTS {
        out += &format!("{} 128 128 128 255\n", join(p));
    }
    for (side, corners) in CUBE_SIDES.iter().enumerate() {
        out += &format!("4 {} {}\n", join(corners), if side == TOP { "255 0 0" } else { "0 0 255" });
    }
    out
}

/// The bytes and description of a glTF cube: two primitives, red top and
/// blue rest, in a node that is doubled and shifted by 3 along z, under a
/// node turned a quarter about y. Its corners end at x 3..5, y 0..2,
/// z -2..0.
fn gltf_parts() -> (serde_json::Value, Vec<u8>) {
    let mut bin: Vec<u8> = flat_points().iter().flat_map(|v| v.to_le_bytes()).collect();
    let order: Vec<u16> = cube_triangles().iter().flatten().map(|&c| c as u16).collect();
    // The top is triangles 6 and 7; put them first.
    let top: Vec<u16> = order[18..24].to_vec();
    let rest: Vec<u16> = order.iter().enumerate().filter(|(i, _)| !(18..24).contains(i)).map(|(_, &c)| c).collect();
    bin.extend(top.iter().chain(&rest).flat_map(|c| c.to_le_bytes()));
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            {"rotation": [0.0, half, 0.0, half], "children": [1]},
            {"mesh": 0, "translation": [0.0, 0.0, 3.0], "scale": [2.0, 2.0, 2.0]}
        ],
        "meshes": [{"primitives": [
            {"attributes": {"POSITION": 0}, "indices": 1, "material": 0},
            {"attributes": {"POSITION": 0}, "indices": 2, "material": 1}
        ]}],
        "materials": [
            {"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}},
            {"pbrMetallicRoughness": {"baseColorFactor": [0.0, 0.0, 1.0, 1.0]}}
        ],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 96},
            {"buffer": 0, "byteOffset": 96, "byteLength": 12},
            {"buffer": 0, "byteOffset": 108, "byteLength": 60}
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"},
            {"bufferView": 2, "componentType": 5123, "count": 30, "type": "SCALAR"}
        ]
    });
    (doc, bin)
}

/// Packs a description and its bytes as one binary glTF.
pub fn glb_of(doc: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut text = serde_json::to_vec(doc).unwrap();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend((28 + text.len() as u32 + bin.len() as u32).to_le_bytes());
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(text);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}

pub fn glb() -> Vec<u8> {
    let (doc, bin) = gltf_parts();
    glb_of(&doc, &bin)
}

/// The same cube as text with its bytes written into it.
pub fn gltf_embedded() -> Vec<u8> {
    use base64::Engine;
    let (mut doc, bin) = gltf_parts();
    doc["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bin)));
    serde_json::to_vec_pretty(&doc).unwrap()
}

/// The same cube as text that names its bytes as a second file, and that
/// file.
pub fn gltf_separate() -> (Vec<u8>, Vec<u8>) {
    let (mut doc, bin) = gltf_parts();
    doc["buffers"][0]["uri"] = json!("cube data.bin");
    (serde_json::to_vec_pretty(&doc).unwrap(), bin)
}

fn vox_chunk(id: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend((content.len() as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(content);
    out
}

fn vox_file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"VOX ".to_vec();
    out.extend(150u32.to_le_bytes());
    out.extend(b"MAIN");
    out.extend(0u32.to_le_bytes());
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn vox_model(size: [u32; 3], blocks: &[[u8; 4]]) -> [Vec<u8>; 2] {
    let size: Vec<u8> = size.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut list = (blocks.len() as u32).to_le_bytes().to_vec();
    list.extend(blocks.iter().flatten());
    [vox_chunk(b"SIZE", &size), vox_chunk(b"XYZI", &list)]
}

fn vox_palette() -> Vec<u8> {
    // Colour 1 is red, colour 2 blue, the rest grey.
    let mut colours = vec![128u8; 1024];
    colours[..8].copy_from_slice(&[255, 0, 0, 255, 0, 0, 255, 255]);
    vox_chunk(b"RGBA", &colours)
}

/// A MagicaVoxel model 3 x 2 x 2 (z up): the upper layer red, the lower
/// blue. Without a palette of its own if `palette` is false.
pub fn vox(palette: bool) -> Vec<u8> {
    let mut blocks = Vec::new();
    for x in 0..3 {
        for y in 0..2 {
            for z in 0..2 {
                blocks.push([x, y, z, if z == 1 { 1 } else { 2 }]);
            }
        }
    }
    let mut chunks = vox_model([3, 2, 2], &blocks).to_vec();
    if palette {
        chunks.push(vox_palette());
    }
    vox_file(&chunks)
}

fn vox_text(text: &str) -> Vec<u8> {
    let mut out = (text.len() as u32).to_le_bytes().to_vec();
    out.extend(text.bytes());
    out
}

fn vox_dict(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut out = (entries.len() as u32).to_le_bytes().to_vec();
    for (key, value) in entries {
        out.extend(vox_text(key));
        out.extend(vox_text(value));
    }
    out
}

fn vox_place(node: i32, child: i32, frame: &[(&str, &str)]) -> Vec<u8> {
    let mut out = node.to_le_bytes().to_vec();
    out.extend(vox_dict(&[]));
    out.extend(child.to_le_bytes());
    out.extend((-1i32).to_le_bytes());
    out.extend(0i32.to_le_bytes());
    out.extend(1i32.to_le_bytes());
    out.extend(vox_dict(frame));
    vox_chunk(b"nTRN", &out)
}

fn vox_shape(node: i32, model: i32) -> Vec<u8> {
    let mut out = node.to_le_bytes().to_vec();
    out.extend(vox_dict(&[]));
    out.extend(1i32.to_le_bytes());
    out.extend(model.to_le_bytes());
    out.extend(vox_dict(&[]));
    vox_chunk(b"nSHP", &out)
}

/// Two models in a scene: a red 2 x 2 x 2 cube with its middle at the
/// origin, and a blue bar 4 x 1 x 1 placed at (10, 0, 0) and turned a
/// quarter about z so that it runs along y. `scene` false leaves the scene
/// out, as old files have it.
pub fn vox_scene(scene: bool) -> Vec<u8> {
    let cube: Vec<[u8; 4]> = (0..8).map(|i| [i & 1, i >> 1 & 1, i >> 2, 1]).collect();
    let bar: Vec<[u8; 4]> = (0..4).map(|x| [x, 0, 0, 2]).collect();
    let mut chunks = [vox_model([2, 2, 2], &cube), vox_model([4, 1, 1], &bar)].concat();
    if scene {
        let mut group = 1i32.to_le_bytes().to_vec();
        group.extend(vox_dict(&[]));
        group.extend(2i32.to_le_bytes());
        group.extend(2i32.to_le_bytes());
        group.extend(4i32.to_le_bytes());
        chunks.push(vox_place(0, 1, &[]));
        chunks.push(vox_chunk(b"nGRP", &group));
        chunks.push(vox_place(2, 3, &[("_t", "0 0 0")]));
        chunks.push(vox_shape(3, 0));
        // Turn 33: first row's entry in column 1, second row's in column 0
        // and negative: (x, y, z) becomes (y, -x, z).
        chunks.push(vox_place(4, 5, &[("_t", "10 0 0"), ("_r", "33")]));
        chunks.push(vox_shape(5, 1));
    }
    chunks.push(vox_palette());
    vox_file(&chunks)
}

/// The cube as Collada, z up, in a node that doubles it and shifts it by
/// 5 along x: its corners end at x 5..7, y and z 0..2. The top is a
/// polylist, the rest triangles.
pub fn dae() -> String {
    let effect = |id: &str, colour: &str| format!(r#"<effect id="{id}-fx"><profile_COMMON><technique sid="common"><phong><diffuse><color>{colour} 1</color></diffuse></phong></technique></profile_COMMON></effect>"#);
    let rest: Vec<usize> = cube_triangles().into_iter().enumerate().filter(|(i, _)| i / 2 != TOP).flat_map(|(_, t)| t).collect();
    format!(
        r##"<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
 <asset><unit name="meter" meter="1"/><up_axis>Z_UP</up_axis></asset>
 <library_effects>{}{}</library_effects>
 <library_materials>
  <material id="red"><instance_effect url="#red-fx"/></material>
  <material id="blue"><instance_effect url="#blue-fx"/></material>
 </library_materials>
 <library_geometries><geometry id="cube"><mesh>
  <source id="cube-pos"><float_array id="cube-pos-array" count="24">{}</float_array>
   <technique_common><accessor source="#cube-pos-array" count="8" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>
  <vertices id="cube-vtx"><input semantic="POSITION" source="#cube-pos"/></vertices>
  <polylist material="top" count="1"><input semantic="VERTEX" source="#cube-vtx" offset="0"/><vcount>4</vcount><p>{}</p></polylist>
  <triangles material="rest" count="10"><input semantic="VERTEX" source="#cube-vtx" offset="0"/><p>{}</p></triangles>
 </mesh></geometry></library_geometries>
 <library_visual_scenes><visual_scene id="scene"><node id="cube-node">
  <translate>5 0 0</translate><scale>2 2 2</scale>
  <instance_geometry url="#cube"><bind_material><technique_common>
   <instance_material symbol="top" target="#red"/><instance_material symbol="rest" target="#blue"/>
  </technique_common></bind_material></instance_geometry>
 </node></visual_scene></library_visual_scenes>
 <scene><instance_visual_scene url="#scene"/></scene>
</COLLADA>
"##,
        effect("red", "1 0 0"),
        effect("blue", "0 0 1"),
        join(flat_points()),
        join(CUBE_SIDES[TOP]),
        join(rest)
    )
}

/// The model file of a 3MF: the cube with its top red from a list of base
/// materials and the rest blue from a colour group, used once as it is and
/// once through a component shifted by 10 along x.
pub fn threemf_model() -> String {
    let mut triangles = String::new();
    for (i, t) in cube_triangles().iter().enumerate() {
        let colour = if i / 2 == TOP { r#"pid="1" p1="0""# } else { r#"pid="2" p1="1""# };
        triangles += &format!(r#"<triangle v1="{}" v2="{}" v3="{}" {colour}/>"#, t[0], t[1], t[2]);
    }
    let points: String = CUBE_POINTS.iter().map(|p| format!(r#"<vertex x="{}" y="{}" z="{}"/>"#, p[0], p[1], p[2])).collect();
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02">
 <resources>
  <basematerials id="1"><base name="red" displaycolor="#FF0000"/></basematerials>
  <m:colorgroup id="2"><m:color color="#00FF00FF"/><m:color color="#0000FFFF"/></m:colorgroup>
  <object id="3" type="model"><mesh><vertices>{points}</vertices><triangles>{triangles}</triangles></mesh></object>
  <object id="4" type="model"><components><component objectid="3" transform="1 0 0 0 1 0 0 0 1 10 0 0"/></components></object>
 </resources>
 <build><item objectid="3"/><item objectid="4" transform="2 0 0 0 2 0 0 0 2 0 0 0"/></build>
</model>
"##
    )
}

pub fn threemf() -> Vec<u8> {
    let types = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#;
    zip(&[("[Content_Types].xml", types.as_bytes()), ("3D/3dmodel.model", threemf_model().as_bytes())])
}

/// One value of an FBX record, for the writers below.
pub enum Fbx {
    Int(i32),
    Long(i64),
    Double(f64),
    Text(&'static str),
    Doubles(Vec<f64>),
    Ints(Vec<i32>),
}

pub struct FbxRecord {
    pub name: &'static str,
    pub values: Vec<Fbx>,
    pub children: Vec<FbxRecord>,
}

fn rec(name: &'static str, values: Vec<Fbx>, children: Vec<FbxRecord>) -> FbxRecord {
    FbxRecord { name, values, children }
}

fn fbx_property(name: &'static str, kind: &'static str, values: [f64; 3]) -> FbxRecord {
    let mut all = vec![Fbx::Text(name), Fbx::Text(kind), Fbx::Text(""), Fbx::Text("A")];
    all.extend(values.map(Fbx::Double));
    rec("P", all, vec![])
}

/// The cube as an FBX tree: z up; the model doubles it and shifts it by 10
/// along x, and hangs under a model turned a quarter about z, so its
/// corners end at x -2..0, y 10..12, z 0..2. Two materials by face.
pub fn fbx_tree() -> Vec<FbxRecord> {
    let order: Vec<i32> = CUBE_SIDES.iter().flat_map(|s| [s[0] as i32, s[1] as i32, s[2] as i32, -(s[3] as i32) - 1]).collect();
    let wearing: Vec<i32> = (0..6).map(|side| i32::from(side == TOP)).collect();
    let material = |id: i64, name: &'static str, colour: [f64; 3]| rec("Material", vec![Fbx::Long(id), Fbx::Text(name), Fbx::Text("")], vec![rec("Properties70", vec![], vec![fbx_property("DiffuseColor", "Color", colour)])]);
    let link = |child: i64, parent: i64| rec("C", vec![Fbx::Text("OO"), Fbx::Long(child), Fbx::Long(parent)], vec![]);
    vec![
        rec("FBXHeaderExtension", vec![], vec![rec("FBXVersion", vec![Fbx::Int(7400)], vec![])]),
        rec("GlobalSettings", vec![], vec![rec("Properties70", vec![], vec![rec("P", vec![Fbx::Text("UpAxis"), Fbx::Text("int"), Fbx::Text("Integer"), Fbx::Text(""), Fbx::Int(2)], vec![])])]),
        rec(
            "Objects",
            vec![],
            vec![
                rec(
                    "Geometry",
                    vec![Fbx::Long(100), Fbx::Text("Geometry::cube"), Fbx::Text("Mesh")],
                    vec![
                        rec("Vertices", vec![Fbx::Doubles(flat_points().iter().map(|&v| f64::from(v)).collect())], vec![]),
                        rec("PolygonVertexIndex", vec![Fbx::Ints(order)], vec![]),
                        rec(
                            "LayerElementMaterial",
                            vec![Fbx::Int(0)],
                            vec![rec("MappingInformationType", vec![Fbx::Text("ByPolygon")], vec![]), rec("ReferenceInformationType", vec![Fbx::Text("IndexToDirect")], vec![]), rec("Materials", vec![Fbx::Ints(wearing)], vec![])],
                        ),
                    ],
                ),
                rec(
                    "Model",
                    vec![Fbx::Long(200), Fbx::Text("Model::cube"), Fbx::Text("Mesh")],
                    vec![rec("Properties70", vec![], vec![fbx_property("Lcl Translation", "Lcl Translation", [10.0, 0.0, 0.0]), fbx_property("Lcl Scaling", "Lcl Scaling", [2.0, 2.0, 2.0])])],
                ),
                rec("Model", vec![Fbx::Long(201), Fbx::Text("Model::parent"), Fbx::Text("Null")], vec![rec("Properties70", vec![], vec![fbx_property("Lcl Rotation", "Lcl Rotation", [0.0, 0.0, 90.0])])]),
                material(300, "Material::blue", [0.0, 0.0, 1.0]),
                material(301, "Material::red", [1.0, 0.0, 0.0]),
            ],
        ),
        rec("Connections", vec![], vec![link(100, 200), link(200, 201), link(201, 0), link(300, 200), link(301, 200)]),
    ]
}

fn fbx_write(out: &mut Vec<u8>, record: &FbxRecord, wide: bool, pack: bool) {
    let offset = |out: &mut Vec<u8>, n: usize| if wide { out.extend((n as u64).to_le_bytes()) } else { out.extend((n as u32).to_le_bytes()) };
    let start = out.len();
    let width = if wide { 8 } else { 4 };
    offset(out, 0);
    offset(out, record.values.len());
    offset(out, 0);
    out.push(record.name.len() as u8);
    out.extend(record.name.bytes());
    let values_start = out.len();
    for value in &record.values {
        let array = |out: &mut Vec<u8>, kind: u8, count: usize, raw: Vec<u8>| {
            out.push(kind);
            out.extend((count as u32).to_le_bytes());
            let stored = if pack {
                let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                packer.write_all(&raw).unwrap();
                packer.finish().unwrap()
            } else {
                raw
            };
            out.extend(u32::from(pack).to_le_bytes());
            out.extend((stored.len() as u32).to_le_bytes());
            out.extend(stored);
        };
        match value {
            Fbx::Int(i) => {
                out.push(b'I');
                out.extend(i.to_le_bytes());
            }
            Fbx::Long(l) => {
                out.push(b'L');
                out.extend(l.to_le_bytes());
            }
            Fbx::Double(d) => {
                out.push(b'D');
                out.extend(d.to_le_bytes());
            }
            Fbx::Text(text) => {
                out.push(b'S');
                out.extend((text.len() as u32).to_le_bytes());
                out.extend(text.bytes());
            }
            Fbx::Doubles(numbers) => array(out, b'd', numbers.len(), numbers.iter().flat_map(|n| n.to_le_bytes()).collect()),
            Fbx::Ints(numbers) => array(out, b'i', numbers.len(), numbers.iter().flat_map(|n| n.to_le_bytes()).collect()),
        }
    }
    let values_length = out.len() - values_start;
    if !record.children.is_empty() {
        for child in &record.children {
            fbx_write(out, child, wide, pack);
        }
        out.extend(vec![0u8; width * 3 + 1]);
    }
    let end = out.len();
    let patch = |out: &mut Vec<u8>, at: usize, n: usize| if wide { out[at..at + 8].copy_from_slice(&(n as u64).to_le_bytes()) } else { out[at..at + 4].copy_from_slice(&(n as u32).to_le_bytes()) };
    patch(out, start, end);
    patch(out, start + 2 * width, values_length);
}

/// A binary FBX of a version (7400 writes offsets in four bytes, 7500 in
/// eight), its arrays packed with zlib or not.
pub fn fbx_binary_of(tree: &[FbxRecord], version: u32, pack: bool) -> Vec<u8> {
    let mut out = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
    out.extend(version.to_le_bytes());
    let wide = version >= 7500;
    for record in tree {
        fbx_write(&mut out, record, wide, pack);
    }
    out.extend(vec![0u8; if wide { 25 } else { 13 }]);
    // The footer: some bytes no reader needs.
    out.extend([0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e]);
    out
}

pub fn fbx_binary(version: u32, pack: bool) -> Vec<u8> {
    fbx_binary_of(&fbx_tree(), version, pack)
}

fn fbx_text_of(out: &mut String, record: &FbxRecord, indent: usize) {
    let pad = "\t".repeat(indent);
    let values: Vec<String> = record
        .values
        .iter()
        .map(|v| match v {
            Fbx::Int(i) => i.to_string(),
            Fbx::Long(l) => l.to_string(),
            Fbx::Double(d) => d.to_string(),
            Fbx::Text(t) => format!("\"{t}\""),
            Fbx::Doubles(n) => format!("*{} {{\n{pad}\ta: {}\n{pad}}}", n.len(), n.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")),
            Fbx::Ints(n) => format!("*{} {{\n{pad}\ta: {}\n{pad}}}", n.len(), n.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")),
        })
        .collect();
    *out += &format!("{pad}{}: {}", record.name, values.join(", "));
    if record.children.is_empty() {
        *out += "\n";
    } else {
        *out += " {\n";
        for child in &record.children {
            fbx_text_of(out, child, indent + 1);
        }
        *out += &format!("{pad}}}\n");
    }
}

/// The same tree as a text FBX.
pub fn fbx_text() -> String {
    let mut out = String::from("; FBX 7.4.0 project file\n; ----------------------------------------------------\n\n");
    for record in fbx_tree() {
        fbx_text_of(&mut out, &record, 0);
    }
    out
}

/// A text FBX of the older 6.1 kind: geometry inside the model, objects
/// known by name. The cube shifted by 1 along y, one blue material.
pub fn fbx_text_old() -> String {
    let order: Vec<String> = CUBE_SIDES.iter().flat_map(|s| [s[0] as i32, s[1] as i32, s[2] as i32, -(s[3] as i32) - 1]).map(|i| i.to_string()).collect();
    format!(
        r#"; FBX 6.1.0 project file
FBXHeaderExtension:  {{
	FBXHeaderVersion: 1003
	FBXVersion: 6100
}}
Objects:  {{
	Model: "Model::cube", "Mesh" {{
		Version: 232
		Properties60:  {{
			Property: "Lcl Translation", "Lcl Translation", "A+",0,1,0
			Property: "Lcl Scaling", "Lcl Scaling", "A+",1,1,1
		}}
		Vertices: {}
		PolygonVertexIndex: {}
	}}
	Material: "Material::blue", "" {{
		Properties60:  {{
			Property: "DiffuseColor", "ColorRGB", "",0,0,1
		}}
	}}
}}
Connections:  {{
	Connect: "OO", "Model::cube", "Model::Scene"
	Connect: "OO", "Material::blue", "Model::cube"
}}
"#,
        flat_points().iter().map(|v| format!("{v:.6}")).collect::<Vec<_>>().join(","),
        order.join(",")
    )
}

fn tds_chunk(id: u16, body: &[u8]) -> Vec<u8> {
    let mut out = id.to_le_bytes().to_vec();
    out.extend((body.len() as u32 + 6).to_le_bytes());
    out.extend(body);
    out
}

/// The cube as a 3D Studio file with a red and a blue material.
pub fn tds() -> Vec<u8> {
    let material = |name: &[u8], colour: [u8; 3]| tds_chunk(0xafff, &[tds_chunk(0xa000, name), tds_chunk(0xa020, &tds_chunk(0x0011, &colour))].concat());
    let mut points = 8u16.to_le_bytes().to_vec();
    points.extend(flat_points().iter().flat_map(|v| v.to_le_bytes()));
    let triangles = cube_triangles();
    let mut faces = 12u16.to_le_bytes().to_vec();
    for t in &triangles {
        faces.extend(t.iter().flat_map(|&c| (c as u16).to_le_bytes()));
        faces.extend(0u16.to_le_bytes());
    }
    let group = |name: &[u8], members: Vec<u16>| {
        let mut body = name.to_vec();
        body.extend((members.len() as u16).to_le_bytes());
        body.extend(members.iter().flat_map(|m| m.to_le_bytes()));
        tds_chunk(0x4130, &body)
    };
    faces.extend(group(b"red\0", (0..12).filter(|i| i / 2 == TOP as u16).collect()));
    faces.extend(group(b"blue\0", (0..12).filter(|i| i / 2 != TOP as u16).collect()));
    let mesh = tds_chunk(0x4100, &[tds_chunk(0x4110, &points), tds_chunk(0x4120, &faces)].concat());
    let object = tds_chunk(0x4000, &[b"cube\0".to_vec(), mesh].concat());
    let editor = tds_chunk(0x3d3d, &[material(b"red\0", [255, 0, 0]), material(b"blue\0", [0, 0, 255]), object].concat());
    tds_chunk(0x4d4d, &[tds_chunk(0x0002, &3u32.to_le_bytes()), editor].concat())
}

/// The cube as AMF: a blue volume for the sides and a red one for the top.
pub fn amf() -> String {
    let points: String = CUBE_POINTS.iter().map(|p| format!("<vertex><coordinates><x>{}</x><y>{}</y><z>{}</z></coordinates></vertex>", p[0], p[1], p[2])).collect();
    let volume = |top: bool, colour: &str| {
        let triangles: String = cube_triangles().iter().enumerate().filter(|(i, _)| (i / 2 == TOP) == top).map(|(_, t)| format!("<triangle><v1>{}</v1><v2>{}</v2><v3>{}</v3></triangle>", t[0], t[1], t[2])).collect();
        format!("<volume{colour}>{triangles}</volume>")
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<amf unit="millimeter" version="1.1">
 <material id="2"><color><r>0</r><g>0</g><b>1</b></color></material>
 <object id="1"><mesh><vertices>{points}</vertices>{}{}</mesh></object>
</amf>
"#,
        volume(false, r#" materialid="2""#).replace("</volume>", "</volume>\n"),
        volume(true, "").replace("<volume>", "<volume><color><r>1</r><g>0</g><b>0</b></color>")
    )
}

fn face_set_order() -> String {
    CUBE_SIDES.iter().map(|s| format!("{} -1", join(s))).collect::<Vec<_>>().join(" ")
}

/// The cube as X3D: one shape with a colour per face, in a transform that
/// doubles it and shifts it by 4 along z.
pub fn x3d() -> String {
    let colours: Vec<&str> = (0..6).map(|side| if side == TOP { "1 0 0" } else { "0 0 1" }).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE X3D PUBLIC "ISO//Web3D//DTD X3D 3.3//EN" "http://www.web3d.org/specifications/x3d-3.3.dtd">
<X3D profile="Interchange" version="3.3">
 <Scene>
  <Transform translation="0 0 4" scale="2 2 2">
   <Shape>
    <Appearance><Material diffuseColor="0.5 0.5 0.5"/></Appearance>
    <IndexedFaceSet colorPerVertex="false" coordIndex="{}">
     <Coordinate point="{}"/>
     <Color color="{}"/>
    </IndexedFaceSet>
   </Shape>
  </Transform>
 </Scene>
</X3D>
"#,
        face_set_order(),
        join(flat_points()),
        colours.join(", ")
    )
}

/// The cube as VRML 2.0: the top as a red shape, the rest as a blue shape
/// that uses the same points by name, in a transform turned a quarter
/// about y and shifted by 4 along x.
pub fn vrml() -> String {
    let side = |s: &[usize; 4]| format!("{}, -1", s.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "));
    let rest: Vec<String> = CUBE_SIDES.iter().enumerate().filter(|(i, _)| *i != TOP).map(|(_, s)| side(s)).collect();
    format!(
        r#"#VRML V2.0 utf8
# a cube in two shapes
DEF Whole Transform {{
  translation 4 0 0
  rotation 0 1 0 1.5707963
  children [
    Shape {{
      appearance Appearance {{ material Material {{ diffuseColor 1 0 0 }} }}
      geometry IndexedFaceSet {{
        solid TRUE
        coord DEF Points Coordinate {{ point [ {} ] }}
        coordIndex [ {} ]
      }}
    }}
    Shape {{
      appearance Appearance {{ material Material {{ diffuseColor 0 0 1 }} }}
      geometry IndexedFaceSet {{
        coord USE Points
        coordIndex [ {} ]
      }}
    }}
  ]
}}
ROUTE Whole.translation_changed TO Whole.set_translation
"#,
        CUBE_POINTS.iter().map(|p| join(*p)).collect::<Vec<_>>().join(", "),
        side(&CUBE_SIDES[TOP]),
        rest.join(" ")
    )
}

/// Every sample by a file name, for tests that run over all formats.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let (obj, mtl) = obj();
    let (gltf, bin) = gltf_separate();
    vec![
        ("cube.obj", obj.clone().into_bytes()),
        ("cube-text.stl", stl_text().into_bytes()),
        ("cube-binary.stl", stl_binary(b"made by a test")),
        ("cube-text.ply", ply("ascii")),
        ("cube-little.ply", ply("binary_little_endian")),
        ("cube-big.ply", ply("binary_big_endian")),
        ("cube.off", off().into_bytes()),
        ("cube.glb", glb()),
        ("cube.gltf", gltf_embedded()),
        ("cube-obj.zip", zip(&[("cube.obj", obj.as_bytes()), ("cube.mtl", mtl.as_bytes())])),
        ("cube-gltf.zip", zip(&[("cube.gltf", &gltf), ("cube data.bin", &bin)])),
        ("blocks.vox", vox(true)),
        ("scene.vox", vox_scene(true)),
        ("cube.dae", dae().into_bytes()),
        ("cube.3mf", threemf()),
        ("cube-7400.fbx", fbx_binary(7400, true)),
        ("cube-7500.fbx", fbx_binary(7500, false)),
        ("cube-text.fbx", fbx_text().into_bytes()),
        ("cube-old.fbx", fbx_text_old().into_bytes()),
        ("cube.3ds", tds()),
        ("cube.amf", amf().into_bytes()),
        ("cube.x3d", x3d().into_bytes()),
        ("cube.wrl", vrml().into_bytes()),
    ]
}
