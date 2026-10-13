//! AMF: the 3D printing format written as XML, sometimes inside a zip.

use std::collections::HashMap;

use roxmltree::Node;

use crate::colour::byte;
use crate::error::Result;
use crate::files::Source;
use crate::model::{Corner, Model, Skin, Up};
use crate::xml::{child, children, text};

const NAME: &str = "AMF";

pub(crate) fn read(source: Source) -> Result<Model> {
    let content = source.text();
    let doc = crate::xml::parse(&content, NAME)?;
    let root = doc.root_element();
    let mut model = Model::new(NAME, Up::Z);
    let materials: HashMap<&str, [u8; 3]> = children(root, "material").filter_map(|m| Some((m.attribute("id")?, colour(m)?))).collect();
    let mut objects = 0;
    for object in children(root, "object") {
        objects += 1;
        let whole = colour(object);
        for mesh in children(object, "mesh") {
            let mut corners = Vec::new();
            for vertex in child(mesh, "vertices").into_iter().flat_map(|v| children(v, "vertex")) {
                let at = child(vertex, "coordinates").map_or([f32::NAN; 3], |c| ["x", "y", "z"].map(|axis| number(c, axis)));
                corners.push(Corner { at, colour: colour(vertex), uv: None });
            }
            for volume in children(mesh, "volume") {
                let material = volume.attribute("materialid").and_then(|id| materials.get(id)).copied();
                let skin = material.or(whole).map_or(Skin::None, Skin::Flat);
                let own = colour(volume);
                for triangle in children(volume, "triangle") {
                    let three = ["v1", "v2", "v3"].map(|name| child(triangle, name).and_then(|v| text(v).trim().parse::<usize>().ok()).and_then(|i| corners.get(i).copied()));
                    let [Some(a), Some(b), Some(c)] = three else {
                        model.skip();
                        continue;
                    };
                    // A triangle's or volume's own colour goes before the
                    // corners' colours, those before the material's.
                    match colour(triangle).or(own) {
                        Some(flat) => model.face(&[a, b, c].map(|corner| Corner::bare(corner.at)), Skin::Flat(flat))?,
                        None => model.face(&[a, b, c], skin)?,
                    }
                }
            }
        }
    }
    if objects > 1 {
        model.note(format!("{objects} objects were merged."));
    }
    model.finish()
}

fn number(node: Node, name: &str) -> f32 {
    child(node, name).and_then(|n| text(n).trim().parse().ok()).unwrap_or(f32::NAN)
}

/// The colour written right inside a node, as parts of one.
fn colour(node: Node) -> Option<[u8; 3]> {
    let colour = child(node, "color")?;
    let part = |name: &str| child(colour, name).and_then(|n| text(n).trim().parse::<f32>().ok());
    Some([byte(part("r")?), byte(part("g")?), byte(part("b")?)])
}
