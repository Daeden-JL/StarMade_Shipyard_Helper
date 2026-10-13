//! Reads 3D model files and turns them into blocks.
//!
//! A player uploads a model in one of many formats; [`read`] finds out what
//! it is and gives its triangles with their colours, [`voxelize`] lays them
//! into a grid of blocks of the size asked for, and [`nearest`] finds the
//! block whose colour looks most like a given one.
//!
//! Nothing here trusts the file: a broken or hostile upload gives an error
//! a player can read, never a panic, and never more memory than the limits
//! in [`model`], [`voxel`] and the archive reader allow.

mod amf;
mod archive;
mod colour;
mod dae;
mod error;
mod fbx;
mod files;
mod gltf;
mod math;
mod model;
mod obj;
mod off;
mod ply;
mod smooth;
mod stl;
mod tds;
mod text;
mod threemf;
mod vox;
mod voxel;
mod x3d;
mod xml;

pub use colour::{nearest, Matcher};
pub use error::ImportError;
pub use files::Files;
pub use model::{Blocks, Model, Paint, Texture, Triangle, Up, MAX_BLOCKS, MAX_TRIANGLES};
pub use smooth::SAMPLES;
pub use voxel::{voxelize, Cell, Options, Voxels, LONGEST_RANGE, MAX_SHELL};

use error::Result;
use files::{extension, Source};

/// A kind of file that can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// What the format is called, as in "Wavefront OBJ".
    pub name: &'static str,
    /// The file name endings it goes by, lower case, without the dot.
    pub extensions: &'static [&'static str],
}

const FORMATS: [Format; 13] = [
    Format { name: "Wavefront OBJ", extensions: &["obj"] },
    Format { name: "STL", extensions: &["stl"] },
    Format { name: "PLY", extensions: &["ply"] },
    Format { name: "glTF", extensions: &["glb", "gltf"] },
    Format { name: "Zip with a model and its textures", extensions: &["zip"] },
    Format { name: "MagicaVoxel", extensions: &["vox"] },
    Format { name: "Collada", extensions: &["dae"] },
    Format { name: "3MF", extensions: &["3mf"] },
    Format { name: "OFF", extensions: &["off", "coff"] },
    Format { name: "FBX", extensions: &["fbx"] },
    Format { name: "3D Studio", extensions: &["3ds"] },
    Format { name: "AMF", extensions: &["amf"] },
    Format { name: "X3D and VRML", extensions: &["x3d", "wrl"] },
];

/// The formats that can be read, for the upload dialog and for error
/// messages, most common first.
pub fn formats() -> &'static [Format] {
    &FORMATS
}

/// Reads a model file. What the file is is told by its contents first and
/// by the ending of its name second. A model whose colours live in other
/// files (an OBJ's materials, a glTF's buffers and pictures) should be
/// uploaded with them in one zip.
pub fn read(file_name: &str, bytes: &[u8]) -> Result<Model> {
    read_with(file_name, bytes, &Files::new())
}

/// Reads a model file whose companion files are at hand already, for a
/// caller that takes several files in one upload.
pub fn read_with(file_name: &str, bytes: &[u8], files: &Files) -> Result<Model> {
    read_source(Source { name: file_name, bytes, files }, true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Obj,
    Stl,
    Ply,
    Gltf,
    Zip,
    Vox,
    Dae,
    ThreeMf,
    Off,
    Fbx,
    Tds,
    Amf,
    X3d,
    Vrml,
}

pub(crate) fn kind_by_extension(extension: &str) -> Option<Kind> {
    Some(match extension {
        "obj" => Kind::Obj,
        "stl" => Kind::Stl,
        "ply" => Kind::Ply,
        "glb" | "gltf" => Kind::Gltf,
        "zip" | "3mf" => Kind::Zip,
        "vox" => Kind::Vox,
        "dae" => Kind::Dae,
        "model" => Kind::ThreeMf,
        "off" | "coff" => Kind::Off,
        "fbx" => Kind::Fbx,
        "3ds" => Kind::Tds,
        "amf" => Kind::Amf,
        "x3d" => Kind::X3d,
        "wrl" | "vrml" => Kind::Vrml,
        _ => return None,
    })
}

/// The endings that can be read, as a list for a sentence.
pub(crate) fn extension_list() -> String {
    FORMATS.iter().flat_map(|f| f.extensions).map(|e| format!(".{e}")).collect::<Vec<_>>().join(", ")
}

/// What to tell the player about a kind of file that is known but cannot
/// be read, by the ending of its name.
pub(crate) fn unreadable(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "blend" | "blend1" => "Blender files cannot be read directly. Export it from Blender as glTF (.glb) or OBJ.",
        "step" | "stp" | "iges" | "igs" => "STEP and IGES files hold CAD solids, not triangles. Open it in your CAD program and export as STL.",
        "sldprt" | "sldasm" | "f3d" | "ipt" | "iam" | "x_t" | "x_b" | "catpart" | "fcstd" | "scad" => "CAD files cannot be read directly. Open it in your CAD program and export as STL.",
        "usd" | "usdz" | "usda" | "usdc" => "USD files cannot be read. Convert it to glTF (.glb), for example by importing it into Blender and exporting from there.",
        "max" => "3ds Max scene files cannot be read. Export it from 3ds Max as FBX or OBJ.",
        "c4d" => "Cinema 4D files cannot be read. Export it from Cinema 4D as FBX or OBJ.",
        "skp" => "SketchUp files cannot be read. Export it from SketchUp as Collada (.dae) or OBJ.",
        "ma" | "mb" => "Maya scene files cannot be read. Export it from Maya as FBX or OBJ.",
        "3dm" => "Rhino files cannot be read. Export it from Rhino as OBJ or STL.",
        "lwo" | "lws" | "lxo" | "ztl" | "zpr" | "hip" | "hipnc" | "abc" | "x" | "ms3d" | "b3d" | "md2" | "md3" | "mdl" | "dxf" | "dwg" => "This kind of 3D file cannot be read. Export it from the program that made it as glTF (.glb), OBJ or FBX.",
        "schematic" | "schem" | "litematic" | "nbt" => "Minecraft schematics cannot be read. The only block format that can is MagicaVoxel (.vox); convert it to that, or to a mesh such as OBJ.",
        "rar" | "7z" | "gz" | "tgz" | "tar" | "bz2" | "xz" => "Only .zip archives can be opened. Unpack it and upload the model file, or pack it again as a .zip.",
        "mtl" => "This is the material file of an OBJ model, not the model. Upload the .obj, or both in one zip.",
        "bin" => "This is the data file of a glTF model, not the model. Upload the .gltf with it in one zip.",
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tga" | "webp" | "dds" | "psd" => "This is a picture, not a 3D model. Upload the model file, or the model with its pictures in one zip.",
        _ => return None,
    })
}

/// What the contents of a file say it is, for the formats that can be
/// known by their first bytes.
fn kind_by_contents(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"glTF") {
        return Some(Kind::Gltf);
    }
    if bytes.starts_with(b"Kaydara FBX Binary") {
        return Some(Kind::Fbx);
    }
    if bytes.starts_with(b"VOX ") {
        return Some(Kind::Vox);
    }
    if bytes.starts_with(b"ply") && bytes.get(3).is_some_and(|b| b.is_ascii_whitespace()) {
        return Some(Kind::Ply);
    }
    if tds::looks_like(bytes) {
        return Some(Kind::Tds);
    }
    // A binary STL is known by its length alone; its first bytes may say
    // anything, even "solid".
    if stl::is_binary(bytes) {
        return Some(Kind::Stl);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("#VRML") {
        return Some(Kind::Vrml);
    }
    if head.starts_with('<') {
        return [("<COLLADA", Kind::Dae), ("<amf", Kind::Amf), ("<X3D", Kind::X3d), ("<model", Kind::ThreeMf)].into_iter().filter_map(|(tag, kind)| head.find(tag).map(|at| (at, kind))).min_by_key(|(at, _)| *at).map(|(_, kind)| kind);
    }
    if head.starts_with('{') && bytes.windows(7).take(1 << 20).any(|w| w == b"\"asset\"") {
        return Some(Kind::Gltf);
    }
    if head.starts_with("; FBX") || head.contains("FBXHeaderExtension") {
        return Some(Kind::Fbx);
    }
    if head.split_ascii_whitespace().next().is_some_and(off::is_heading) {
        return Some(Kind::Off);
    }
    if stl::is_text(bytes) {
        return Some(Kind::Stl);
    }
    None
}

/// What to tell the player about a file that is known by its first bytes
/// to be of a kind that cannot be read.
fn unreadable_contents(bytes: &[u8]) -> Option<&'static str> {
    let starts: [(&[u8], &str); 7] = [
        (b"BLENDER", "blend"),
        (b"ISO-10303-21", "step"),
        (b"PXR-USDC", "usd"),
        (b"#usda", "usd"),
        (b"Rar!", "rar"),
        (b"7z\xbc\xaf", "7z"),
        (b"\x1f\x8b", "gz"),
    ];
    starts.iter().find(|(start, _)| bytes.starts_with(start)).and_then(|(_, extension)| unreadable(extension))
}

pub(crate) fn read_source(source: Source, zip_allowed: bool) -> Result<Model> {
    if source.bytes.is_empty() {
        return Err(ImportError::Empty("The file is empty.".into()));
    }
    let extension = extension(source.name);
    let is_zip = source.bytes.starts_with(b"PK\x03\x04") || source.bytes.starts_with(b"PK\x05\x06");
    let kind = if let Some(kind) = kind_by_contents(source.bytes) {
        kind
    } else if let Some(message) = unreadable(&extension).filter(|_| kind_by_extension(&extension).is_none()).or_else(|| unreadable_contents(source.bytes)) {
        return Err(ImportError::Unsupported(message.into()));
    } else if is_zip {
        Kind::Zip
    } else if let Some(kind) = kind_by_extension(&extension).filter(|kind| *kind != Kind::Zip) {
        kind
    } else if obj::looks_like(source.bytes) {
        Kind::Obj
    } else if kind_by_extension(&extension) == Some(Kind::Zip) {
        return Err(error::malformed(format!("The file is named .{extension} but is not a zip archive; it is cut short or damaged.")));
    } else {
        return Err(ImportError::UnknownFormat(format!("This is not a kind of 3D file that can be read. These can: {}.", extension_list())));
    };
    match kind {
        Kind::Obj => obj::read(source),
        Kind::Stl => stl::read(source),
        Kind::Ply => ply::read(source),
        Kind::Gltf => gltf::read(source),
        Kind::Zip if zip_allowed => archive::read(source),
        Kind::Zip => Err(error::unsupported("The zip holds another zip. Unpack it and upload the model with its files in one plain zip.")),
        Kind::Vox => vox::read(source),
        Kind::Dae => dae::read(source),
        Kind::ThreeMf => threemf::read_bare(source),
        Kind::Off => off::read(source),
        Kind::Fbx => fbx::read(source),
        Kind::Tds => tds::read(source),
        Kind::Amf => amf::read(source),
        Kind::X3d => x3d::read(source),
        Kind::Vrml => x3d::read_vrml(source),
    }
}
