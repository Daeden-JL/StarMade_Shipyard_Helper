//! X3D (XML) and its forerunner VRML 2.0 (text): only the faces of
//! `IndexedFaceSet` and its triangle cousins, with their transforms and
//! plain colours.

use std::collections::HashMap;
use std::rc::Rc;

use crate::colour::byte;
use crate::error::{malformed, unsupported, Result};
use crate::files::Source;
use crate::math::Mat4;
use crate::model::{Corner, Model, Skin, Up};

const MAX_DEPTH: usize = 100;
const MAX_VISITS: usize = 1_000_000;

/// A node of either format: its type and its fields.
#[derive(Debug, Default)]
struct Node {
    kind: String,
    fields: Vec<(String, Field)>,
}

#[derive(Debug)]
enum Field {
    Numbers(Vec<f64>),
    Nodes(Vec<Rc<Node>>),
    Text(String),
}

impl Node {
    fn numbers(&self, name: &str) -> &[f64] {
        self.fields.iter().find_map(|(n, f)| match f {
            Field::Numbers(numbers) if n == name => Some(numbers.as_slice()),
            _ => None,
        }).unwrap_or_default()
    }

    /// Whether a yes-or-no field says no.
    fn denies(&self, name: &str) -> bool {
        self.fields.iter().any(|(n, f)| n == name && matches!(f, Field::Text(t) if t.eq_ignore_ascii_case("false")))
    }

    fn nodes(&self) -> impl Iterator<Item = &Rc<Node>> {
        self.fields.iter().filter_map(|(_, f)| match f {
            Field::Nodes(nodes) => Some(nodes.iter()),
            _ => None,
        }).flatten()
    }

    fn find(&self, kinds: &[&str]) -> Option<&Rc<Node>> {
        self.nodes().find(|n| kinds.contains(&n.kind.as_str()))
    }
}

pub(crate) fn read(source: Source) -> Result<Model> {
    let content = source.text();
    let doc = crate::xml::parse(&content, "X3D")?;
    let mut named = HashMap::new();
    let root = from_xml(doc.root_element(), &mut named, 0)?;
    build("X3D", &root)
}

fn from_xml(element: roxmltree::Node, named: &mut HashMap<String, Rc<Node>>, depth: usize) -> Result<Rc<Node>> {
    if depth > MAX_DEPTH {
        return Err(malformed("The X3D file is nested too deeply; it is damaged."));
    }
    if let Some(known) = element.attribute("USE").and_then(|name| named.get(name)) {
        return Ok(known.clone());
    }
    let mut node = Node { kind: element.tag_name().name().to_string(), fields: Vec::new() };
    for attribute in element.attributes() {
        let field = match crate::text::numbers(attribute.value()) {
            Some(numbers) if !numbers.is_empty() => Field::Numbers(numbers),
            _ => Field::Text(attribute.value().to_string()),
        };
        node.fields.push((attribute.name().to_string(), field));
    }
    let mut below = Vec::new();
    for child in element.children().filter(roxmltree::Node::is_element) {
        below.push(from_xml(child, named, depth + 1)?);
    }
    node.fields.push(("children".into(), Field::Nodes(below)));
    let node = Rc::new(node);
    if let Some(name) = element.attribute("DEF") {
        named.insert(name.to_string(), node.clone());
    }
    Ok(node)
}

pub(crate) fn read_vrml(source: Source) -> Result<Model> {
    let content = source.text();
    if content.trim_start().starts_with("#VRML V1") {
        return Err(unsupported("This is a VRML 1.0 file; only VRML 2.0 (VRML97) can be read. Convert it, or export as OBJ."));
    }
    let mut lexer = Lexer { bytes: content.as_bytes(), at: 0, ahead: Vec::new() };
    let mut named = HashMap::new();
    let mut top = Vec::new();
    while let Some(token) = lexer.peek(0) {
        match token {
            Token::Word(word) if word == "ROUTE" => (0..4).for_each(|_| drop(lexer.next())),
            Token::Word(word) if word == "PROTO" || word == "EXTERNPROTO" => {
                lexer.next();
                lexer.next();
                while matches!(lexer.peek(0), Some(Token::Open(_))) {
                    lexer.skip_group();
                }
            }
            Token::Word(_) => top.extend(lexer.node(&mut named, 0)?),
            _ => drop(lexer.next()),
        }
    }
    build("VRML", &Node { kind: "Scene".into(), fields: vec![("children".into(), Field::Nodes(top))] })
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
    Text(String),
    /// An opening brace or bracket.
    Open(u8),
    Close(u8),
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    ahead: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&mut self, n: usize) -> Option<Token> {
        while self.ahead.len() <= n {
            let token = self.read()?;
            self.ahead.push(token);
        }
        self.ahead.get(n).cloned()
    }

    fn next(&mut self) -> Option<Token> {
        if self.ahead.is_empty() {
            self.read()
        } else {
            Some(self.ahead.remove(0))
        }
    }

    fn read(&mut self) -> Option<Token> {
        loop {
            match self.bytes.get(self.at)? {
                b'#' => self.at += self.bytes[self.at..].iter().position(|&b| b == b'\n').unwrap_or(self.bytes.len() - self.at),
                b if b.is_ascii_whitespace() || *b == b',' => self.at += 1,
                _ => break,
            }
        }
        let start = self.at;
        match self.bytes[start] {
            b @ (b'{' | b'[') => {
                self.at += 1;
                Some(Token::Open(b))
            }
            b @ (b'}' | b']') => {
                self.at += 1;
                Some(Token::Close(b))
            }
            b'"' => {
                let mut end = start + 1;
                while end < self.bytes.len() && self.bytes[end] != b'"' {
                    end += if self.bytes[end] == b'\\' { 2 } else { 1 };
                }
                let end = end.min(self.bytes.len());
                self.at = (end + 1).min(self.bytes.len());
                Some(Token::Text(String::from_utf8_lossy(&self.bytes[start + 1..end]).into_owned()))
            }
            _ => {
                let length = self.bytes[start..].iter().position(|&b| b.is_ascii_whitespace() || matches!(b, b',' | b'{' | b'}' | b'[' | b']' | b'"' | b'#')).unwrap_or(self.bytes.len() - start);
                self.at = start + length.max(1);
                let word = String::from_utf8_lossy(&self.bytes[start..self.at]);
                Some(match word.parse::<f64>() {
                    Ok(number) => Token::Number(number),
                    Err(_) => Token::Word(word.into_owned()),
                })
            }
        }
    }

    /// Passes over a braced or bracketed group, whatever is in it.
    fn skip_group(&mut self) {
        let mut depth = 0usize;
        while let Some(token) = self.next() {
            match token {
                Token::Open(_) => depth += 1,
                Token::Close(_) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether a node starts here: a name given or used, or a type and
    /// its opening brace.
    fn at_node(&mut self) -> bool {
        match self.peek(0) {
            Some(Token::Word(word)) => word == "DEF" || word == "USE" || self.peek(1) == Some(Token::Open(b'{')),
            _ => false,
        }
    }

    /// Reads a node. Nothing for a name used that was never given.
    fn node(&mut self, named: &mut HashMap<String, Rc<Node>>, depth: usize) -> Result<Option<Rc<Node>>> {
        if depth > MAX_DEPTH {
            return Err(malformed("The VRML file is nested too deeply; it is damaged."));
        }
        let cut = || malformed("The VRML file is cut short or damaged.");
        let Some(Token::Word(word)) = self.next() else { return Err(cut()) };
        match word.as_str() {
            "USE" => {
                let Some(Token::Word(name)) = self.next() else { return Err(cut()) };
                return Ok(named.get(&name).cloned());
            }
            "DEF" => {
                let Some(Token::Word(name)) = self.next() else { return Err(cut()) };
                let node = self.node(named, depth + 1)?;
                if let Some(node) = &node {
                    named.insert(name, node.clone());
                }
                return Ok(node);
            }
            _ => {}
        }
        if self.next() != Some(Token::Open(b'{')) {
            return Err(cut());
        }
        let mut node = Node { kind: word, fields: Vec::new() };
        loop {
            let name = match self.next().ok_or_else(cut)? {
                Token::Close(b'}') => break,
                Token::Word(name) if name == "ROUTE" => {
                    (0..3).for_each(|_| drop(self.next()));
                    continue;
                }
                Token::Word(name) => name,
                _ => continue,
            };
            let field = match self.peek(0).ok_or_else(cut)? {
                Token::Number(_) => {
                    let mut numbers = Vec::new();
                    while let Some(Token::Number(n)) = self.peek(0) {
                        numbers.push(n);
                        self.next();
                    }
                    Field::Numbers(numbers)
                }
                Token::Text(text) => {
                    self.next();
                    Field::Text(text)
                }
                Token::Open(b'[') => {
                    self.next();
                    let (mut numbers, mut nodes, mut texts) = (Vec::new(), Vec::new(), Vec::new());
                    loop {
                        if self.at_node() {
                            nodes.extend(self.node(named, depth + 1)?);
                            continue;
                        }
                        match self.next().ok_or_else(cut)? {
                            Token::Close(_) => break,
                            Token::Number(n) => numbers.push(n),
                            Token::Text(text) => texts.push(text),
                            Token::Open(_) => {
                                self.ahead.insert(0, Token::Open(b'['));
                                self.skip_group();
                            }
                            Token::Word(_) => {}
                        }
                    }
                    if !nodes.is_empty() {
                        Field::Nodes(nodes)
                    } else if !numbers.is_empty() {
                        Field::Numbers(numbers)
                    } else {
                        Field::Text(texts.join(" "))
                    }
                }
                _ if self.at_node() => Field::Nodes(self.node(named, depth + 1)?.into_iter().collect()),
                Token::Word(word) => {
                    self.next();
                    Field::Text(word)
                }
                _ => continue,
            };
            node.fields.push((name, field));
        }
        Ok(Some(Rc::new(node)))
    }
}

fn build(format: &'static str, root: &Node) -> Result<Model> {
    let mut model = Model::new(format, Up::Y);
    let (mut visits, mut shapes) = (0, 0);
    walk(&mut model, root, &Mat4::SAME, Skin::None, 0, &mut visits, &mut shapes)?;
    if shapes > 1 {
        model.note(format!("{shapes} objects were merged."));
    }
    model.finish()
}

fn walk(model: &mut Model, node: &Node, placed: &Mat4, skin: Skin, depth: usize, visits: &mut usize, shapes: &mut usize) -> Result<()> {
    *visits += 1;
    if depth > MAX_DEPTH || *visits > MAX_VISITS {
        return Ok(());
    }
    let (mut placed, mut skin) = (*placed, skin);
    match node.kind.as_str() {
        "Transform" => placed = placed.times(&transform(node)),
        "Shape" => {
            *shapes += 1;
            let colour = node.find(&["Appearance"]).and_then(|a| a.find(&["Material"])).map(|m| m.numbers("diffuseColor")).filter(|c| c.len() >= 3);
            skin = colour.map_or(Skin::None, |c| Skin::Flat([byte(c[0] as f32), byte(c[1] as f32), byte(c[2] as f32)]));
        }
        "IndexedFaceSet" | "IndexedTriangleSet" | "TriangleSet" => return faces(model, node, &placed, skin),
        // Of the same thing at several levels of detail, the finest.
        "LOD" => {
            return match node.nodes().next() {
                Some(finest) => walk(model, finest, &placed, skin, depth + 1, visits, shapes),
                None => Ok(()),
            };
        }
        _ => {}
    }
    for child in node.nodes() {
        walk(model, child, &placed, skin, depth + 1, visits, shapes)?;
    }
    Ok(())
}

/// A Transform node's placing: shift, then turn and stretch about a centre.
fn transform(node: &Node) -> Mat4 {
    let three = |name: &str, default: f64| match node.numbers(name) {
        [x, y, z, ..] if x.is_finite() && y.is_finite() && z.is_finite() => [*x, *y, *z],
        _ => [default; 3],
    };
    let turn = |name: &str| match node.numbers(name) {
        [x, y, z, angle, ..] => Mat4::about([*x, *y, *z], *angle),
        _ => Mat4::SAME,
    };
    let centre = three("center", 0.0);
    let stretch_turn = turn("scaleOrientation");
    [
        Mat4::shift(three("translation", 0.0)),
        Mat4::shift(centre),
        turn("rotation"),
        stretch_turn,
        Mat4::stretch(three("scale", 1.0)),
        stretch_turn.inverse().unwrap_or(Mat4::SAME),
        Mat4::shift(centre.map(|c| -c)),
    ]
    .iter()
    .fold(Mat4::SAME, |all, step| all.times(step))
}

fn faces(model: &mut Model, node: &Node, placed: &Mat4, skin: Skin) -> Result<()> {
    let Some(coordinate) = node.find(&["Coordinate"]) else { return Ok(()) };
    let points: Vec<[f32; 3]> = coordinate.numbers("point").chunks_exact(3).map(|p| placed.point([p[0] as f32, p[1] as f32, p[2] as f32])).collect();
    let colour_node = node.find(&["Color", "ColorRGBA"]);
    let each = if colour_node.is_some_and(|c| c.kind == "ColorRGBA") { 4 } else { 3 };
    let colours: Vec<[u8; 3]> = colour_node.map(|c| c.numbers("color").chunks_exact(each).map(|c| [byte(c[0] as f32), byte(c[1] as f32), byte(c[2] as f32)]).collect()).unwrap_or_default();
    let per_face = node.denies("colorPerVertex");
    let colour_order = node.numbers("colorIndex");
    let order: Vec<i64> = match node.kind.as_str() {
        "IndexedFaceSet" => node.numbers("coordIndex").iter().map(|&i| i as i64).collect(),
        // Bare triangles: every three entries, or every three points.
        "IndexedTriangleSet" => node.numbers("index").chunks_exact(3).flat_map(|t| [t[0] as i64, t[1] as i64, t[2] as i64, -1]).collect(),
        _ => (0..points.len() as i64 / 3).flat_map(|t| [t * 3, t * 3 + 1, t * 3 + 2, -1]).collect(),
    };
    let listed = node.kind == "IndexedFaceSet" && !colour_order.is_empty();
    let mut corners: Vec<Corner> = Vec::new();
    let (mut whole, mut face) = (true, 0usize);
    for (nth, &index) in order.iter().chain(std::iter::once(&-1)).enumerate() {
        if index < 0 {
            if !corners.is_empty() || !whole {
                if whole {
                    model.face(&corners, skin)?;
                } else {
                    model.skip();
                }
                face += 1;
            }
            corners.clear();
            whole = true;
            continue;
        }
        let which = match (per_face, listed) {
            (true, true) => colour_order.get(face).map(|&c| c as i64),
            (true, false) => Some(face as i64),
            (false, true) => colour_order.get(nth).map(|&c| c as i64),
            (false, false) => Some(index),
        };
        let colour = which.and_then(|c| usize::try_from(c).ok()).and_then(|c| colours.get(c).copied());
        match points.get(index as usize) {
            Some(at) => corners.push(Corner { at: *at, colour, uv: None }),
            None => whole = false,
        }
    }
    Ok(())
}
