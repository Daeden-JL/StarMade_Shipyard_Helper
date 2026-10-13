//! Broken and hostile files: an error, never a panic.

mod common;

use common::samples;
use yard_import::{read, voxelize, ImportError, Options};

/// Formats whose files say how much is to come, or must close what they
/// open, so that a file cut short can always be told from a whole one.
/// The others (OBJ, OFF, VRML, text FBX and text PLY) are lists of lines
/// that may end anywhere; cut short they are smaller models or errors,
/// which is checked too, but they need not be errors.
fn must_fail_when_cut(name: &str) -> bool {
    !matches!(name, "cube.obj" | "cube.off" | "cube-text.ply" | "cube.wrl" | "cube-text.fbx" | "cube-old.fbx")
}

#[test]
fn every_sample_cut_short_anywhere_is_an_error_or_a_smaller_model_never_a_panic() {
    for (name, bytes) in samples::all() {
        let whole = read(name, &bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let step = (bytes.len() / 400).max(1);
        let mut lengths: Vec<usize> = (0..bytes.len()).step_by(step).collect();
        lengths.extend(bytes.len().saturating_sub(40)..bytes.len());
        for length in lengths {
            match read(name, &bytes[..length]) {
                Err(_) => {}
                Ok(model) => {
                    // A binary FBX ends in a footer no reader needs; cut
                    // within it the file is still whole.
                    // Likewise a text STL cut within the name that follows
                    // its closing word.
                    let in_footer = (name.ends_with(".fbx") && length >= bytes.len() - 16) || (name == "cube-text.stl" && length >= bytes.len() - 6) || bytes[length..].iter().all(u8::is_ascii_whitespace);
                    assert!(!must_fail_when_cut(name) || in_footer, "{name} cut to {length} of {} bytes was read", bytes.len());
                    assert!(model.triangles.len() <= whole.triangles.len(), "{name} cut to {length}");
                    // Whatever was read must also survive being built.
                    let _ = voxelize(&model, &Options { longest: 8, ..Options::default() });
                }
            }
        }
    }
}

#[test]
fn every_sample_with_bytes_changed_is_read_or_refused_never_a_panic() {
    // A fixed scramble, so that a failure can be found again.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (name, bytes) in samples::all() {
        for _ in 0..300 {
            let mut changed = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let at = (next() % changed.len() as u64) as usize;
                changed[at] = match next() % 4 {
                    0 => 0,
                    1 => 0xff,
                    2 => changed[at].wrapping_add(1),
                    _ => next() as u8,
                };
            }
            if let Ok(model) = read(name, &changed) {
                let _ = voxelize(&model, &Options { longest: 8, ..Options::default() });
            }
        }
    }
}

#[test]
fn random_bytes_under_every_name_are_refused() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut noise = |length: usize| -> Vec<u8> {
        (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect()
    };
    let names = ["a.obj", "a.stl", "a.ply", "a.glb", "a.gltf", "a.zip", "a.vox", "a.dae", "a.3mf", "a.off", "a.fbx", "a.3ds", "a.amf", "a.x3d", "a.wrl", "a.bin", "a"];
    for length in [1, 7, 64, 84, 134, 1000, 5000] {
        for name in names {
            let bytes = noise(length);
            assert!(read(name, &bytes).is_err(), "{length} random bytes were read as {name}");
        }
    }
}

#[test]
fn random_bytes_behind_each_formats_first_bytes_are_refused() {
    let starts: [(&str, &[u8]); 12] = [
        ("a.glb", b"glTF\x02\0\0\0"),
        ("a.fbx", b"Kaydara FBX Binary  \0\x1a\0\xe8\x1c\0\0"),
        ("a.vox", b"VOX \x96\0\0\0MAIN"),
        ("a.ply", b"ply\nformat binary_little_endian 1.0\nelement vertex 1000000000\nproperty float x\nelement face 4000000000\nproperty list uchar int vertex_indices\nend_header\n"),
        ("a.3ds", b"MM\xff\xff\0\0\x02\0"),
        ("a.zip", b"PK\x03\x04"),
        ("a.stl", b"solid x\nfacet normal 0 0 0\nouter loop\nvertex 0 0"),
        ("a.off", b"OFF\n4000000000 4000000000 0\n"),
        ("a.dae", b"<?xml version=\"1.0\"?><COLLADA><library_geometries><geometry><mesh>"),
        ("a.gltf", b"{\"asset\":{\"version\":\"2.0\"},\"accessors\":[{\"count\":4000000000,\"componentType\":5126,\"type\":\"VEC3\"}],\"meshes\":[{\"primitives\":[{\"attributes\":{\"POSITION\":0}}]}],\"nodes\":[{\"mesh\":0}]"),
        ("a.amf", b"<amf><object><mesh><vertices>"),
        ("a.x3d", b"<X3D><Scene><Shape><IndexedFaceSet coordIndex='0 1 2 -1'>"),
    ];
    let mut seed = 0x1234_5678_9abc_def1u64;
    for (name, start) in starts {
        for length in [0usize, 3, 50, 600] {
            let mut bytes = start.to_vec();
            for _ in 0..length {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                bytes.push(seed as u8);
            }
            assert!(read(name, &bytes).is_err(), "{name} with {length} random bytes was read");
        }
    }
}

#[test]
fn counts_that_promise_more_than_the_file_holds_are_refused_without_reserving_the_room() {
    // A glTF whose only accessor has no data at all but claims four
    // thousand million points.
    let gltf = br#"{"asset":{"version":"2.0"},"accessors":[{"count":4000000000,"componentType":5126,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}]}"#;
    assert!(read("huge.gltf", gltf).is_err());
    // A binary STL that claims more triangles than it has bytes for.
    let mut stl = samples::stl_binary(b"short");
    stl[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read("huge.stl", &stl).is_err());
    // A vox file with a block list longer than the file.
    let mut vox = samples::vox(true);
    let at = vox.windows(4).position(|w| w == b"XYZI").unwrap() + 12;
    vox[at..at + 4].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    assert!(read("huge.vox", &vox).is_err());
    // A 3D Studio point list that claims more points than follow.
    let mut tds = samples::tds();
    let at = tds.windows(2).position(|w| w == [0x10, 0x41]).unwrap() + 6;
    tds[at..at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(read("huge.3ds", &tds).is_err());
}

#[test]
fn a_packed_fbx_array_that_unpacks_to_more_than_it_says_is_cut_off() {
    use samples::{fbx_binary_of, Fbx, FbxRecord};
    // The point list says 3 numbers but packs 30000: only what is said is
    // unpacked, which is not a whole model.
    let mut tree = samples::fbx_tree();
    let geometry = &mut tree[2].children[0];
    geometry.children[0] = FbxRecord { name: "Vertices", values: vec![Fbx::Doubles(vec![0.0; 30_000])], children: vec![] };
    let mut bytes = fbx_binary_of(&tree, 7400, true);
    let at = bytes.windows(8).position(|w| w == b"Vertices").unwrap() + 8 + 1;
    assert_eq!(&bytes[at..at + 4], &30_000u32.to_le_bytes());
    bytes[at..at + 4].copy_from_slice(&3u32.to_le_bytes());
    match read("bomb.fbx", &bytes) {
        Ok(model) => assert!(model.triangles.is_empty() || model.notes.iter().any(|n| n.contains("skipped"))),
        Err(error) => assert!(!matches!(error, ImportError::UnknownFormat(_)), "{error}"),
    }
}

#[test]
fn a_scene_that_contains_itself_does_not_run_forever() {
    let gltf = br#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[0]}],"nodes":[{"children":[1]},{"children":[0]}]}"#;
    assert!(matches!(read("loop.gltf", gltf), Err(ImportError::Empty(_))));
    let dae = r##"<COLLADA><library_nodes><node id="a"><instance_node url="#b"/></node><node id="b"><instance_node url="#a"/></node></library_nodes>
        <library_visual_scenes><visual_scene id="s"><node><instance_node url="#a"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#s"/></scene></COLLADA>"##;
    assert!(matches!(read("loop.dae", dae.as_bytes()), Err(ImportError::Empty(_))));
    let deep = format!("#VRML V2.0 utf8\n{}", "Group { children [ ".repeat(5000));
    assert!(read("deep.wrl", deep.as_bytes()).is_err());
    let deep = format!("<X3D><Scene>{}</Scene></X3D>", "<Group>".repeat(3000) + &"</Group>".repeat(3000));
    assert!(read("deep.x3d", deep.as_bytes()).is_err());
}

#[test]
fn a_zip_that_unpacks_to_far_more_than_it_weighs_is_refused() {
    use std::io::Write;
    // 600 MB of zeros pack to well under a megabyte.
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer.start_file("bomb.obj", zip::write::SimpleFileOptions::default()).unwrap();
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..600 {
        writer.write_all(&zeros).unwrap();
    }
    let packed = writer.finish().unwrap().into_inner();
    assert!(packed.len() < 2 << 20);
    let error = read("bomb.zip", &packed).unwrap_err();
    assert!(matches!(error, ImportError::TooLarge(_)), "{error}");
}
