//! Collada (.dae): XML with geometry in one place, materials in another
//! and a scene of nodes that places the geometry.

use std::collections::HashMap;

use roxmltree::Node;

use crate::colour::byte;
use crate::error::Result;
use crate::files::Source;
use crate::math::Mat4;
use crate::model::{Corner, Model, Skin, Up};
use crate::xml::{child, children, floats, indices, link, text};

const NAME: &str = "Collada";
const MAX_DEPTH: usize = 64;
const MAX_VISITS: usize = 1_000_000;

struct Reader<'a, 'i> {
    source: Source<'a>,
    /// Everything in the file that has an id.
    ids: HashMap<&'a str, Node<'a, 'i>>,
    skins: HashMap<&'a str, Skin>,
    model: Model,
    visits: usize,
    meshes: usize,
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let content = source.text();
    let doc = crate::xml::parse(&content, NAME)?;
    let root = doc.root_element();
    let up_axis = child(root, "asset").and_then(|asset| child(asset, "up_axis")).map(|axis| text(axis).trim().to_string());
    let (up, upright) = match up_axis.as_deref() {
        Some("Z_UP") => (Up::Z, Mat4::SAME),
        // With x up, right is -y: turned upright that is (-y, x, z).
        Some("X_UP") => (Up::Y, Mat4([[0.0, -1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])),
        _ => (Up::Y, Mat4::SAME),
    };
    let ids = doc.descendants().filter_map(|node| Some((node.attribute("id")?, node))).collect();
    let mut reader = Reader { source, ids, skins: HashMap::new(), model: Model::new(NAME, up), visits: 0, meshes: 0 };
    let scene = child(root, "scene").and_then(|s| child(s, "instance_visual_scene")).and_then(|i| i.attribute("url")).and_then(|url| reader.ids.get(link(url)).copied());
    let scene = scene.or_else(|| child(root, "library_visual_scenes").and_then(|l| child(l, "visual_scene")));
    match scene {
        Some(scene) => {
            for node in children(scene, "node") {
                reader.node(node, &upright, 0)?;
            }
        }
        // No scene: every geometry where it was modelled.
        None => {
            for geometry in children(root, "library_geometries").flat_map(|l| children(l, "geometry")) {
                reader.geometry(geometry, &upright, &HashMap::new())?;
            }
        }
    }
    if reader.meshes > 1 {
        let note = format!("{} objects were merged.", reader.meshes);
        reader.model.note(note);
    }
    reader.model.finish()
}

/// The numbers a source holds and how its entries lie in them.
struct Data {
    values: Vec<f32>,
    stride: usize,
    offset: usize,
}

impl Data {
    fn entry(&self, i: usize, n: usize) -> Option<&[f32]> {
        let start = self.offset.checked_add(i.checked_mul(self.stride)?)?;
        self.values.get(start..start.checked_add(n.min(self.stride))?).filter(|e| e.len() == n)
    }
}

/// One input of a primitive: what it is, where its numbers are, and which
/// of each corner's indices picks from them.
struct Input {
    data: Data,
    offset: usize,
}

impl<'a, 'i> Reader<'a, 'i> {
    fn node(&mut self, node: Node<'a, 'i>, above: &Mat4, depth: usize) -> Result<()> {
        self.visits += 1;
        if depth > MAX_DEPTH || self.visits > MAX_VISITS {
            return Ok(());
        }
        let mut placed = *above;
        for step in node.children().filter(Node::is_element) {
            let n: Vec<f64> = floats(step).iter().map(|&v| f64::from(v)).collect();
            let all = |count: usize| n.len() >= count && n.iter().all(|v| v.is_finite());
            match step.tag_name().name() {
                "matrix" => placed = placed.times(&Mat4::from_rows(&n).unwrap_or(Mat4::SAME)),
                "translate" if all(3) => placed = placed.times(&Mat4::shift([n[0], n[1], n[2]])),
                "rotate" if all(4) => placed = placed.times(&Mat4::about([n[0], n[1], n[2]], n[3].to_radians())),
                "scale" if all(3) => placed = placed.times(&Mat4::stretch([n[0], n[1], n[2]])),
                _ => {}
            }
        }
        for part in node.children().filter(Node::is_element) {
            let target = part.attribute("url").and_then(|url| self.ids.get(link(url)).copied());
            match (part.tag_name().name(), target) {
                ("node", _) => self.node(part, &placed, depth + 1)?,
                ("instance_node", Some(target)) => self.node(target, &placed, depth + 1)?,
                ("instance_geometry", Some(target)) => self.geometry(target, &placed, &bindings(part))?,
                ("instance_controller", Some(controller)) => {
                    // A skinned or morphed mesh: its rest shape is used.
                    let skin = child(controller, "skin").or_else(|| child(controller, "morph"));
                    let shape = skin.and_then(|s| child(s, "bind_shape_matrix")).and_then(|m| Mat4::from_rows(&floats(m).iter().map(|&v| f64::from(v)).collect::<Vec<_>>()));
                    let target = skin.and_then(|s| s.attribute("source")).and_then(|id| self.ids.get(link(id)).copied());
                    if let Some(target) = target {
                        self.geometry(target, &placed.times(&shape.unwrap_or(Mat4::SAME)), &bindings(part))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn data(&self, id: &str) -> Option<Data> {
        let source = self.ids.get(link(id))?;
        let accessor = child(*source, "technique_common").and_then(|t| child(t, "accessor"));
        let attribute = |name: &str, default: usize| accessor.and_then(|a| a.attribute(name)).and_then(|v| v.parse().ok()).unwrap_or(default);
        Some(Data { values: floats(child(*source, "float_array")?), stride: attribute("stride", 1).max(1), offset: attribute("offset", 0) })
    }

    /// The inputs of a primitive or of a `vertices` element by what they
    /// are; of several texture sets the first.
    fn inputs(&self, node: Node<'a, 'i>) -> HashMap<&'a str, (&'a str, usize)> {
        let mut out = HashMap::new();
        for input in children(node, "input") {
            let (Some(semantic), Some(source)) = (input.attribute("semantic"), input.attribute("source")) else { continue };
            let offset = input.attribute("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
            out.entry(semantic).or_insert((source, offset));
        }
        out
    }

    fn geometry(&mut self, geometry: Node<'a, 'i>, placed: &Mat4, bound: &HashMap<&'a str, &'a str>) -> Result<()> {
        let Some(mesh) = child(geometry, "mesh") else { return Ok(()) };
        self.meshes += 1;
        for primitive in mesh.children().filter(Node::is_element) {
            let kind = primitive.tag_name().name();
            if !matches!(kind, "triangles" | "polylist" | "polygons" | "trifans" | "tristrips") {
                continue;
            }
            let wanted = self.inputs(primitive);
            let stride = children(primitive, "input").filter_map(|i| i.attribute("offset")?.parse::<usize>().ok()).max().unwrap_or(0) + 1;
            let Some(&(vertices, at)) = wanted.get("VERTEX") else { continue };
            // The points themselves, and whatever else is given per point.
            let per_point = self.ids.get(link(vertices)).map(|v| self.inputs(*v)).unwrap_or_default();
            let input = |name: &str, offset: usize| per_point.get(name).and_then(|(id, _)| self.data(id)).map(|data| Input { data, offset });
            let Some(points) = input("POSITION", at) else { continue };
            let own = |name: &str| wanted.get(name).and_then(|(id, offset)| self.data(id).map(|data| Input { data, offset: *offset }));
            let colours = own("COLOR").or_else(|| input("COLOR", at));
            let places = own("TEXCOORD").or_else(|| input("TEXCOORD", at));
            let material = primitive.attribute("material").map(|symbol| bound.get(symbol).copied().unwrap_or(symbol));
            let skin = material.map_or(Skin::None, |id| self.skin(link(id)));
            let corner = |tuple: &[usize]| -> Option<Corner> {
                let p = points.data.entry(*tuple.get(points.offset)?, 3)?;
                let colour = colours.as_ref().and_then(|c| c.data.entry(*tuple.get(c.offset)?, 3)).map(|c| [byte(c[0]), byte(c[1]), byte(c[2])]);
                // Collada counts a picture's rows from the bottom.
                let uv = places.as_ref().and_then(|t| t.data.entry(*tuple.get(t.offset)?, 2)).map(|t| [t[0], 1.0 - t[1]]);
                Some(Corner { at: placed.point([p[0], p[1], p[2]]), colour, uv })
            };
            let counts = if kind == "polylist" { child(primitive, "vcount").and_then(indices) } else { None };
            for list in children(primitive, "p") {
                let Some(list) = indices(list) else {
                    self.model.skip();
                    continue;
                };
                let corners: Vec<Option<Corner>> = list.chunks_exact(stride).map(corner).collect();
                let mut face = |face: &[Option<Corner>]| match face.iter().copied().collect::<Option<Vec<Corner>>>() {
                    Some(face) => self.model.face(&face, skin),
                    None => {
                        self.model.skip();
                        Ok(())
                    }
                };
                match (kind, &counts) {
                    ("triangles", _) => corners.chunks_exact(3).try_for_each(&mut face)?,
                    ("polylist", Some(counts)) => {
                        let mut rest = corners.as_slice();
                        for &n in counts {
                            let Some((now, later)) = rest.split_at_checked(n) else { break };
                            face(now)?;
                            rest = later;
                        }
                    }
                    ("trifans", _) => corners.windows(2).skip(1).try_for_each(|pair| face(&[corners[0], pair[0], pair[1]]))?,
                    ("tristrips", _) => corners.windows(3).enumerate().try_for_each(|(i, t)| if i % 2 == 0 { face(t) } else { face(&[t[1], t[0], t[2]]) })?,
                    _ => face(&corners)?,
                }
            }
        }
        Ok(())
    }

    /// What a material gives: the diffuse colour or picture of its effect.
    fn skin(&mut self, id: &'a str) -> Skin {
        if let Some(known) = self.skins.get(id) {
            return *known;
        }
        let skin = self.find_skin(id).unwrap_or(Skin::None);
        self.skins.insert(id, skin);
        skin
    }

    fn find_skin(&mut self, id: &str) -> Option<Skin> {
        let material = *self.ids.get(id)?;
        let effect = *self.ids.get(link(child(material, "instance_effect")?.attribute("url")?))?;
        let named = |name: &str| effect.descendants().find(|n| n.is_element() && n.tag_name().name() == name);
        let diffuse = named("diffuse").or_else(|| named("emission"))?;
        if let Some(texture) = child(diffuse, "texture").and_then(|t| t.attribute("texture")) {
            // The picture is named through a sampler and a surface, each a
            // parameter of the effect; some writers name the image at once.
            let param = |sid: &str, inner: &str| effect.descendants().find(|n| n.attribute("sid") == Some(sid)).and_then(|p| p.descendants().find(|n| n.is_element() && n.tag_name().name() == inner)).map(|n| text(n).trim().to_string());
            let surface = param(texture, "source").or_else(|| param(texture, "init_from"));
            let image = surface.as_deref().and_then(|s| param(s, "init_from")).or(surface).unwrap_or_else(|| texture.to_string());
            // The image names its file in `init_from`, in newer files in a
            // `ref` inside that.
            let file = self.ids.get(image.as_str()).and_then(|i| child(*i, "init_from")).map(|from| text(child(from, "ref").unwrap_or(from)).trim().to_string());
            if let Some(file) = file {
                match self.model.texture(&file, self.source.companion(&file)) {
                    Some(texture) => return Some(Skin::Texture { texture, tint: [255; 3] }),
                    None => self.model.note("Textures were not found in the upload; material colours were used."),
                }
            }
            return None;
        }
        let c = floats(child(diffuse, "color")?);
        (c.len() >= 3).then(|| Skin::Flat([byte(c[0]), byte(c[1]), byte(c[2])]))
    }
}

/// Which material each of a geometry's material names stands for where it
/// is placed.
fn bindings<'a>(instance: Node<'a, '_>) -> HashMap<&'a str, &'a str> {
    instance.descendants().filter(|n| n.is_element() && n.tag_name().name() == "instance_material").filter_map(|n| Some((n.attribute("symbol")?, n.attribute("target")?))).collect()
}
