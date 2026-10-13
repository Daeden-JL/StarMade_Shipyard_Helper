//! The 3D models the game draws for some blocks in place of a cube (light
//! bars, consoles, pipes, capsules), read from the install the server is
//! pointed at. Like the block textures they belong to the game and are not
//! shipped with this project.
//!
//! A block names its model in `BlockConfig.xml` (`LodShape`); `mainConfig.xml`
//! maps that name to a folder and file below `data/models/lod`. There a
//! `.scene` file names the mesh and may move or turn it, the `.mesh.xml` holds
//! the triangles, and the `.material` file names the texture.

use anyhow::Context;
use axum::body::Bytes;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use yard_core::catalog::Catalog;
use yard_core::model::{Model, ModelSet, POINT_SCALE};

/// Pixels along the edge of a texture sheet as the browser stores them: the
/// size of the block texture sheets. Model textures are scaled to it.
const SHEET_SIZE: u32 = 1024;

pub struct Models {
    /// What `/api/models` answers.
    pub json: Bytes,
    /// Model name -> its number in the list.
    by_name: HashMap<String, u16>,
    /// The models' texture files, and each as a PNG sheet once it has been
    /// asked for (None if it could not be read). The first is page `first_page`.
    sheets: Vec<(PathBuf, std::sync::OnceLock<Option<Bytes>>)>,
    first_page: u16,
}

impl Models {
    /// The number of the model a block config names, if it could be read.
    pub fn index(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    /// Whether a texture page is one of the models' sheets.
    pub fn has_sheet(&self, page: u32) -> bool {
        page.checked_sub(self.first_page as u32).is_some_and(|index| (index as usize) < self.sheets.len())
    }

    /// The PNG of one of the models' texture pages. The first call for a page
    /// scales and packs the image, which takes a moment: call it off the
    /// async threads.
    pub fn sheet(&self, page: u32) -> Option<Bytes> {
        let index = page.checked_sub(self.first_page as u32)?;
        let (path, png) = self.sheets.get(index as usize)?;
        png.get_or_init(|| match sheet_png(path) {
            Ok(bytes) => Some(Bytes::from(bytes)),
            Err(e) => {
                tracing::warn!("model texture {} could not be read: {e:#}", path.display());
                None
            }
        })
        .clone()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn empty(first_page: u16) -> Self {
        Self { json: Bytes::from_static(b"{\"models\":[],\"pages\":[]}"), by_name: HashMap::new(), sheets: Vec::new(), first_page }
    }
}

/// Reads every model the block list names. `first_page` is the first texture
/// page number the block textures leave free. A model that cannot be read is
/// left out with a warning; its block is then drawn as a cube.
pub fn load(starmade_dir: &Path, catalog: &Catalog, first_page: u16) -> Models {
    match read_all(starmade_dir, catalog, first_page) {
        Ok(models) => models,
        Err(e) => {
            tracing::warn!("block models are not available, such blocks are drawn as cubes: {e:#}");
            Models::empty(first_page)
        }
    }
}

fn read_all(starmade_dir: &Path, catalog: &Catalog, first_page: u16) -> anyhow::Result<Models> {
    let data = starmade_dir.join("data");
    let config = std::fs::read_to_string(data.join("config/mainConfig.xml")).context("cannot read data/config/mainConfig.xml")?;
    let doc = roxmltree::Document::parse(&config).context("mainConfig.xml")?;
    let section = doc.descendants().find(|n| n.has_tag_name("LOD")).context("mainConfig.xml has no LOD section")?;
    let root = data.join(section.attribute("path").unwrap_or("/models/lod/").trim_matches('/'));
    // Model name -> the scene file without its extension.
    let files: HashMap<&str, PathBuf> = section
        .children()
        .filter(|n| n.is_element())
        .filter_map(|n| Some((n.tag_name().name(), root.join(n.attribute("relpath")?).join(n.attribute("filename")?))))
        .collect();

    let mut names: Vec<&str> = catalog.all().flat_map(|d| [d.model.as_str(), d.model_on.as_str()]).filter(|n| !n.is_empty()).collect();
    names.sort_unstable();
    names.dedup();

    let mut set = ModelSet::default();
    let mut by_name = HashMap::new();
    // The install keeps a copy of a texture in every folder that uses it;
    // the same picture is one sheet here, whichever folder it came from.
    let mut textures: Vec<PathBuf> = Vec::new();
    let mut pictures: Vec<[u8; 32]> = Vec::new();
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    for name in names {
        let read = files.get(name).with_context(|| format!("mainConfig.xml does not list model {name}")).and_then(|base| read_model(base));
        match read {
            Ok((mut model, texture)) => {
                let sheet = *seen.entry(texture.clone()).or_insert_with(|| {
                    use sha2::Digest;
                    let picture: [u8; 32] = sha2::Sha256::digest(std::fs::read(&texture).unwrap_or_default()).into();
                    pictures.iter().position(|p| *p == picture).unwrap_or_else(|| {
                        pictures.push(picture);
                        textures.push(texture);
                        textures.len() - 1
                    })
                });
                model.page = first_page + sheet as u16;
                by_name.insert(name.to_string(), set.models.len() as u16);
                set.models.push(model);
            }
            Err(e) => tracing::warn!("model {name} could not be read: {e:#}"),
        }
    }

    set.pages = (0..textures.len()).map(|i| first_page + i as u16).collect();
    let sheets = textures.into_iter().map(|path| (path, std::sync::OnceLock::new())).collect();
    Ok(Models { json: Bytes::from(serde_json::to_vec(&set)?), by_name, sheets, first_page })
}

/// A model texture as a PNG of the size the browser keeps its sheets in.
fn sheet_png(path: &Path) -> anyhow::Result<Vec<u8>> {
    let image = image::open(path)?.to_rgba8();
    let image = if image.dimensions() == (SHEET_SIZE, SHEET_SIZE) {
        image
    } else {
        image::imageops::resize(&image, SHEET_SIZE, SHEET_SIZE, image::imageops::FilterType::Triangle)
    };
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
}

fn number(node: roxmltree::Node, attribute: &str) -> anyhow::Result<f32> {
    node.attribute(attribute)
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .with_context(|| format!("<{}> has no number {attribute}", node.tag_name().name()))
}

/// How a scene file places its mesh: turned by `rows`, then moved by `shift`.
struct Placement {
    rows: [[f32; 3]; 3],
    shift: [f32; 3],
    mesh: String,
}

/// Reads the single node of a `.scene` file. The game applies its position
/// and rotation and ignores its scale.
fn read_scene(text: &str) -> anyhow::Result<Placement> {
    let doc = roxmltree::Document::parse(text)?;
    let node = doc.descendants().find(|n| n.has_tag_name("node")).context("no <node>")?;
    let child = |tag: &str| node.children().find(|n| n.has_tag_name(tag));
    let shift = match child("position") {
        Some(p) => [number(p, "x")?, number(p, "y")?, number(p, "z")?],
        None => [0.0; 3],
    };
    let rows = match child("rotation") {
        Some(q) => {
            let (w, x, y, z) = (number(q, "qw")?, number(q, "qx")?, number(q, "qy")?, number(q, "qz")?);
            [
                [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
                [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
                [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
            ]
        }
        None => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    let mesh = child("entity").and_then(|e| e.attribute("meshFile")).context("no mesh file named")?.to_string();
    Ok(Placement { rows, shift, mesh })
}

/// The diffuse texture a `.material` file names: the first texture that is
/// not an added glow map.
fn diffuse_texture(material: &str) -> Option<String> {
    material.split("texture_unit").skip(1).filter(|unit| !unit.contains("colour_op_ex add")).find_map(|unit| {
        unit.lines().map(str::trim).find_map(|line| line.strip_prefix("texture ").map(|name| name.trim().to_string()))
    })
}

/// Reads the first submesh of a `.mesh.xml`, the only one the game draws.
fn read_mesh(text: &str, placement: &Placement) -> anyhow::Result<Model> {
    let doc = roxmltree::Document::parse(text)?;
    let submesh = doc.descendants().find(|n| n.has_tag_name("submesh")).context("no <submesh>")?;
    let shared = submesh.attribute("usesharedvertices") == Some("true");
    let geometry = if shared {
        doc.descendants().find(|n| n.has_tag_name("sharedgeometry"))
    } else {
        submesh.children().find(|n| n.has_tag_name("geometry"))
    }
    .context("no vertices")?;

    let mut model = Model::default();
    // Positions and texture positions may come in one buffer or in two.
    for buffer in geometry.children().filter(|n| n.has_tag_name("vertexbuffer")) {
        for vertex in buffer.children().filter(|n| n.has_tag_name("vertex")) {
            if let Some(p) = vertex.children().find(|n| n.has_tag_name("position")) {
                let v = [number(p, "x")?, number(p, "y")?, number(p, "z")?];
                for axis in 0..3 {
                    let placed = (0..3).map(|k| placement.rows[axis][k] * v[k]).sum::<f32>() + placement.shift[axis];
                    model.points.push((placed * POINT_SCALE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16);
                }
            }
            if let Some(t) = vertex.children().find(|n| n.has_tag_name("texcoord")) {
                for attribute in ["u", "v"] {
                    model.uvs.push((number(t, attribute)?.clamp(0.0, 1.0) * 65535.0).round() as u16);
                }
            }
        }
    }
    let corners = model.points.len() / 3;
    anyhow::ensure!(corners > 0 && corners <= u16::MAX as usize, "{corners} vertices");
    anyhow::ensure!(model.uvs.len() == corners * 2, "{} texture positions for {corners} vertices", model.uvs.len() / 2);
    let faces = submesh.children().find(|n| n.has_tag_name("faces")).context("no <faces>")?;
    for face in faces.children().filter(|n| n.has_tag_name("face")) {
        for attribute in ["v1", "v2", "v3"] {
            let index: usize = face.attribute(attribute).and_then(|v| v.parse().ok()).context("a face without three corners")?;
            anyhow::ensure!(index < corners, "a face names vertex {index} of {corners}");
            model.triangles.push(index as u16);
        }
    }
    anyhow::ensure!(!model.triangles.is_empty(), "no faces");
    Ok(model)
}

/// Reads the model whose scene file is `base` + ".scene". Returns it with the
/// path of its texture; the caller numbers the texture sheets.
fn read_model(base: &Path) -> anyhow::Result<(Model, PathBuf)> {
    let dir = base.parent().context("no folder")?;
    let with = |ending: &str| {
        let mut name = base.file_name().unwrap_or_default().to_os_string();
        name.push(ending);
        dir.join(name)
    };
    let scene = std::fs::read_to_string(with(".scene")).context("scene file")?;
    let placement = read_scene(&scene).context("scene file")?;
    let mesh = std::fs::read_to_string(dir.join(format!("{}.xml", placement.mesh))).context("mesh file")?;
    let model = read_mesh(&mesh, &placement).context("mesh file")?;
    let material = std::fs::read_to_string(with(".material")).context("material file")?;
    let texture = dir.join(diffuse_texture(&material).context("the material names no texture")?);
    anyhow::ensure!(texture.is_file(), "texture {} is missing", texture.display());
    Ok((model, texture))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESH: &str = r#"<mesh><submeshes>
      <submesh material="Lights" usesharedvertices="false" operationtype="triangle_list">
        <faces count="1"><face v1="0" v2="1" v3="2"/></faces>
        <geometry vertexcount="3"><vertexbuffer positions="true" normals="true" texture_coords="1">
          <vertex><position x="-0.125" y="-0.5" z="-0.5"/><normal x="0" y="1" z="0"/><texcoord u="0" v="0.5"/></vertex>
          <vertex><position x="0.125" y="-0.5" z="0.5"/><normal x="0" y="1" z="0"/><texcoord u="1" v="0.25"/></vertex>
          <vertex><position x="0.125" y="-0.375" z="0.5"/><normal x="0" y="1" z="0"/><texcoord u="0.5" v="1.5"/></vertex>
        </vertexbuffer></geometry>
      </submesh>
    </submeshes></mesh>"#;

    fn scene(position: &str, rotation: &str) -> String {
        format!(r#"<scene><nodes><node name="n">{position}<scale x="3" y="3" z="3"/>{rotation}<entity meshFile="Bar.mesh"/></node></nodes></scene>"#)
    }

    #[test]
    fn a_mesh_is_read_as_the_game_reads_it() {
        let plain = read_scene(&scene("", "")).unwrap();
        assert_eq!(plain.mesh, "Bar.mesh");
        let model = read_mesh(MESH, &plain).unwrap();
        // 1/1024 of a block; the scale in the scene file is ignored, as in the game.
        assert_eq!(model.points, vec![-128, -512, -512, 128, -512, 512, 128, -384, 512]);
        assert_eq!(model.triangles, vec![0, 1, 2]);
        // Texture positions are kept as they are, measured from the top left; out of range is clamped.
        assert_eq!(model.uvs, vec![0, 32768, 65535, 16384, 32768, 65535]);
    }

    #[test]
    fn the_scene_files_turn_and_shift_are_applied() {
        // The light rods' scene: a quarter turn that brings the mesh's +Z to +Y.
        let turned = read_scene(&scene("", r#"<rotation qw="0.7071068" qx="-0.7071068" qy="0" qz="0"/>"#)).unwrap();
        let model = read_mesh(MESH, &turned).unwrap();
        assert_eq!(&model.points[0..3], &[-128, -512, 512]);
        // The ladder's scene: moved half a block toward -X.
        let moved = read_scene(&scene(r#"<position x="-0.5" y="0" z="0"/>"#, "")).unwrap();
        assert_eq!(&read_mesh(MESH, &moved).unwrap().points[0..3], &[-640, -512, -512]);
    }

    #[test]
    fn the_glow_map_is_not_taken_for_the_texture() {
        let material = "material Consoles\n{\n pass\n {\n  texture_unit\n  {\n   texture consoles_EM.png\n   colour_op_ex add src_texture src_current\n  }\n  texture_unit\n  {\n   texture Consoles.png\n  }\n }\n}";
        assert_eq!(diffuse_texture(material).as_deref(), Some("Consoles.png"));
        assert_eq!(diffuse_texture("material X { }"), None);
    }

    #[test]
    fn a_broken_mesh_is_refused() {
        let plain = read_scene(&scene("", "")).unwrap();
        assert!(read_mesh(&MESH.replace(r#"v3="2""#, r#"v3="7""#), &plain).is_err());
        assert!(read_mesh("<mesh/>", &plain).is_err());
        assert!(read_scene("<scene/>").is_err());
    }

    /// With a game install at hand: every model the block list names is found.
    #[test]
    fn the_installs_models_are_all_readable() {
        let Some(dir) = std::env::var_os("YARD_STARMADE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".smtest2/vol")))
            .filter(|d| d.join("data/config/mainConfig.xml").is_file())
        else {
            return;
        };
        let config = dir.join("data/config");
        let catalog = Catalog::parse(
            &std::fs::read_to_string(config.join("BlockConfig.xml")).unwrap(),
            &std::fs::read_to_string(config.join("BlockTypes.properties")).unwrap(),
        )
        .unwrap();
        let wanted: std::collections::BTreeSet<&str> =
            catalog.all().flat_map(|d| [d.model.as_str(), d.model_on.as_str()]).filter(|n| !n.is_empty()).collect();
        let models = read_all(&dir, &catalog, 40).unwrap();
        assert_eq!(models.len(), wanted.len(), "every named model was read");
        let set: ModelSet = serde_json::from_slice(&models.json).unwrap();
        assert!(set.models.iter().all(|m| m.points.iter().all(|c| c.abs() <= 1024)), "models fit in their cell");
        // The white light bar: lying on the floor of its cell along Z.
        let bar = &set.models[models.index("WhiteLightBar").unwrap() as usize];
        let ys: Vec<i16> = bar.points.chunks(3).map(|p| p[1]).collect();
        assert_eq!((ys.iter().min(), ys.iter().max()), (Some(&-512), Some(&-384)));
        assert_eq!(set.pages.first(), Some(&40));
        assert!(models.has_sheet(40) && !models.has_sheet(39) && !models.has_sheet(40 + set.pages.len() as u32));
        // Light bars, corners and rods are skinned from one picture, kept in
        // three folders: one sheet.
        let page = |name: &str| set.models[models.index(name).unwrap() as usize].page;
        assert_eq!((page("WhiteLightBar"), page("WhiteLightBar")), (page("WhiteLightCorner"), page("WhiteLightRod")));
        assert!(set.pages.len() <= 12, "{} sheets for the models", set.pages.len());
        // A sheet is a PNG of the size of the block texture sheets.
        let sheet = image::load_from_memory(&models.sheet(40).unwrap()).unwrap();
        assert_eq!((sheet.width(), sheet.height()), (SHEET_SIZE, SHEET_SIZE));
        assert!(models.sheet(39).is_none());
    }
}
