//! 3MF: a zip with the model as XML inside, the format of 3D printing
//! slicers. Objects may be built from other objects, also from other
//! files in the same zip.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use crate::archive::{unpack, Zip, MAX_UNPACKED};
use crate::error::{malformed, Result};
use crate::files::Source;
use crate::math::Mat4;
use crate::model::{Corner, Model, Skin, Up};
use crate::text::{hex_colour, numbers};
use crate::xml::{child, children};

const NAME: &str = "3MF";
const MAX_DEPTH: usize = 32;
const MAX_VISITS: usize = 1_000_000;

/// One model file of the package: its objects and its lists of colours.
struct Part<'a, 'i> {
    objects: HashMap<&'a str, Node<'a, 'i>>,
    colours: HashMap<&'a str, Vec<Option<[u8; 3]>>>,
}

/// Reads a 3MF from its zip.
pub(crate) fn read(archive: &mut Zip, entries: &[(usize, String, u64)]) -> Result<Model> {
    let mut budget = MAX_UNPACKED;
    let mut texts: Vec<(String, String)> = Vec::new();
    for (index, name, _) in entries.iter().filter(|(_, name, _)| name.to_ascii_lowercase().ends_with(".model")) {
        let bytes = unpack(archive, *index, &mut budget)?;
        texts.push((path(name), String::from_utf8_lossy(&bytes).into_owned()));
    }
    // The root model is named by the package; by convention it is this.
    let main = texts.iter().position(|(name, _)| name == "3d/3dmodel.model").or_else(|| texts.iter().position(|(name, _)| name.matches('/').count() <= 1)).unwrap_or(0);
    read_parts(&texts, main)
}

/// Reads a model file that was taken out of its zip.
pub(crate) fn read_bare(source: Source) -> Result<Model> {
    read_parts(&[(path(source.name), source.text().into_owned())], 0)
}

/// A path inside the package as it is compared: lower case, no first slash.
fn path(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

fn read_parts(texts: &[(String, String)], main: usize) -> Result<Model> {
    let docs: Vec<Document> = texts.iter().map(|(_, text)| crate::xml::parse(text, NAME)).collect::<Result<_>>()?;
    let parts: HashMap<&str, Part> = texts.iter().zip(&docs).map(|((name, _), doc)| (name.as_str(), part(doc))).collect();
    let (name, doc) = (texts.get(main).map(|(name, _)| name.as_str()), docs.get(main));
    let (Some(name), Some(doc)) = (name, doc) else { return Err(malformed("The 3MF file holds no model.")) };
    let mut model = Model::new(NAME, Up::Z);
    let mut visits = 0;
    let items: Vec<Node> = child(doc.root_element(), "build").into_iter().flat_map(|build| children(build, "item")).collect();
    for item in &items {
        let Some(id) = item.attribute("objectid") else { continue };
        let home = attribute(*item, "path").map(path);
        object(&mut model, &parts, home.as_deref().unwrap_or(name), id, &transform(*item), 0, &mut visits)?;
    }
    if items.is_empty() {
        // Nothing set out to build: take every object as it lies.
        let all: Vec<&str> = parts.get(name).map(|part| part.objects.keys().copied().collect()).unwrap_or_default();
        for id in all {
            object(&mut model, &parts, name, id, &Mat4::SAME, 0, &mut visits)?;
        }
    }
    if items.len() > 1 {
        model.note(format!("{} objects were merged.", items.len()));
    }
    model.finish()
}

fn part<'a, 'i>(doc: &'a Document<'i>) -> Part<'a, 'i> {
    let mut part = Part { objects: HashMap::new(), colours: HashMap::new() };
    for resource in child(doc.root_element(), "resources").into_iter().flat_map(|r| r.children()).filter(Node::is_element) {
        let Some(id) = resource.attribute("id") else { continue };
        match resource.tag_name().name() {
            "object" => {
                part.objects.insert(id, resource);
            }
            "basematerials" => {
                part.colours.insert(id, children(resource, "base").map(|base| base.attribute("displaycolor").and_then(hex_colour)).collect());
            }
            "colorgroup" => {
                part.colours.insert(id, children(resource, "color").map(|colour| colour.attribute("color").and_then(hex_colour)).collect());
            }
            _ => {}
        }
    }
    part
}

/// An attribute by its bare name, whatever namespace it is written in.
fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name() == name).map(|a| a.value())
}

/// A transform as 3MF writes it: twelve numbers, the rows of a matrix
/// that a point is multiplied into from the left.
fn transform(node: Node) -> Mat4 {
    let Some(n) = node.attribute("transform").and_then(numbers).filter(|n| n.len() == 12 && n.iter().all(|v| v.is_finite())) else { return Mat4::SAME };
    let mut out = Mat4::SAME;
    for row in 0..3 {
        for column in 0..4 {
            out.0[row][column] = n[column * 3 + row];
        }
    }
    out
}

fn object(model: &mut Model, parts: &HashMap<&str, Part>, home: &str, id: &str, placed: &Mat4, depth: usize, visits: &mut usize) -> Result<()> {
    *visits += 1;
    if depth > MAX_DEPTH || *visits > MAX_VISITS {
        return Ok(());
    }
    let Some(part) = parts.get(home) else {
        model.note("Part of the model lies in a file that is missing from the 3MF.");
        return Ok(());
    };
    let Some(found) = part.objects.get(id) else { return Ok(()) };
    for component in child(*found, "components").into_iter().flat_map(|c| children(c, "component")) {
        let Some(id) = component.attribute("objectid") else { continue };
        let elsewhere = attribute(component, "path").map(path);
        object(model, parts, elsewhere.as_deref().unwrap_or(home), id, &placed.times(&transform(component)), depth + 1, visits)?;
    }
    let Some(mesh) = child(*found, "mesh") else { return Ok(()) };
    let number = |node: Node, name: &str| node.attribute(name).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(f32::NAN);
    let points: Vec<[f32; 3]> = child(mesh, "vertices").into_iter().flat_map(|v| children(v, "vertex")).map(|v| placed.point(["x", "y", "z"].map(|axis| number(v, axis)))).collect();
    let index = |node: Node, name: &str| node.attribute(name).and_then(|v| v.trim().parse::<usize>().ok());
    for triangle in child(mesh, "triangles").into_iter().flat_map(|t| children(t, "triangle")) {
        let three = ["v1", "v2", "v3"].map(|name| index(triangle, name).and_then(|i| points.get(i).copied()));
        let [Some(a), Some(b), Some(c)] = three else {
            model.skip();
            continue;
        };
        // A triangle names a list of colours and an entry for each
        // corner, or leaves both to its object.
        let (list, first) = match triangle.attribute("pid") {
            Some(list) => (Some(list), index(triangle, "p1")),
            None => (found.attribute("pid"), index(*found, "pindex")),
        };
        let colours = list.and_then(|list| part.colours.get(list));
        let colour = |entry: Option<usize>| colours.and_then(|c| c.get(entry?).copied().flatten());
        let first = colour(first);
        let corners = [(a, first), (b, colour(index(triangle, "p2")).or(first)), (c, colour(index(triangle, "p3")).or(first))];
        model.face(&corners.map(|(at, colour)| Corner { at, colour, uv: None }), Skin::None)?;
    }
    Ok(())
}
