//! Helpers for the formats written as XML.

use std::borrow::Cow;

use roxmltree::{Document, Node, ParsingOptions};

use crate::error::{malformed, Result};

/// How deep elements may be nested. The parser calls itself for every
/// level, so a file of a hundred thousand nested tags would run it out of
/// stack; real models stay far below this.
const MAX_DEPTH: usize = 200;

/// Parses a file as XML, after making sure it is safe to: not nested too
/// deeply, and without entity definitions of its own. A bare document
/// type line, as X3D files have, is let through.
pub(crate) fn parse<'a>(text: &'a str, format: &str) -> Result<Document<'a>> {
    check(text.as_bytes(), format)?;
    Document::parse_with_options(text, ParsingOptions { allow_dtd: true, nodes_limit: 40_000_000 }).map_err(|_| malformed(format!("The {format} file is cut short or damaged.")))
}

/// Walks over the tags without building anything, counting how deep they
/// nest.
fn check(bytes: &[u8], format: &str) -> Result<()> {
    let find = |from: usize, what: &[u8]| bytes[from..].windows(what.len()).position(|w| w == what).map_or(bytes.len(), |i| from + i + what.len());
    let (mut at, mut depth) = (0, 0usize);
    while at < bytes.len() {
        if bytes[at] != b'<' {
            at += 1;
            continue;
        }
        let rest = &bytes[at..];
        if rest.starts_with(b"<!--") {
            at = find(at, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            at = find(at, b"]]>");
        } else if rest.starts_with(b"<?") {
            at = find(at, b"?>");
        } else if rest.starts_with(b"<!") {
            let end = find(at, b">");
            if bytes[at..end].contains(&b'[') {
                return Err(malformed(format!("The {format} file defines text entities of its own, which is not supported.")));
            }
            at = end;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            at = find(at, b">");
        } else {
            // An opening tag: find its end, minding quoted values.
            let mut quote = None;
            let mut end = at + 1;
            while end < bytes.len() {
                match (quote, bytes[end]) {
                    (None, b'>') => break,
                    (None, q @ (b'"' | b'\'')) => quote = Some(q),
                    (Some(q), b) if b == q => quote = None,
                    _ => {}
                }
                end += 1;
            }
            if bytes.get(end.wrapping_sub(1)) != Some(&b'/') {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(malformed(format!("The {format} file is nested too deeply; it is damaged.")));
                }
            }
            at = end + 1;
        }
    }
    Ok(())
}

/// The child elements of a node with a given name, whatever namespace.
pub(crate) fn children<'a, 'i: 'a>(node: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    node.children().filter(move |child| child.is_element() && child.tag_name().name() == name)
}

pub(crate) fn child<'a, 'i: 'a>(node: Node<'a, 'i>, name: &'a str) -> Option<Node<'a, 'i>> {
    children(node, name).next()
}

/// All the text right inside a node.
pub(crate) fn text<'a>(node: Node<'a, '_>) -> Cow<'a, str> {
    let mut parts = node.children().filter_map(|child| child.text().filter(|_| child.is_text()));
    match (parts.next(), parts.next()) {
        (None, _) => Cow::Borrowed(""),
        (Some(only), None) => Cow::Borrowed(only),
        (Some(first), Some(second)) => Cow::Owned(parts.fold(format!("{first}{second}"), |all, part| all + part)),
    }
}

/// The numbers a node's text is made of. Words that are not numbers are
/// read as "not a number" and caught where points are checked.
pub(crate) fn floats(node: Node) -> Vec<f32> {
    text(node).split_ascii_whitespace().map(|w| w.parse().unwrap_or(f32::NAN)).collect()
}

/// The whole numbers a node's text is made of; nothing if one is not.
pub(crate) fn indices(node: Node) -> Option<Vec<usize>> {
    text(node).split_ascii_whitespace().map(|w| w.parse().ok()).collect()
}

/// A link to something in the same file, without its "#".
pub(crate) fn link(text: &str) -> &str {
    text.trim().trim_start_matches('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_nesting_and_entity_definitions_are_refused_before_parsing() {
        let deep = "<a>".repeat(100_000);
        assert!(parse(&deep, "test").is_err());
        let entities = r#"<!DOCTYPE a [<!ENTITY e "<b><b><b></b></b></b>">]><a>&e;</a>"#;
        assert!(parse(entities, "test").is_err());
    }

    #[test]
    fn a_document_type_line_comments_and_self_closing_tags_pass() {
        let wide = format!(r#"<?xml version="1.0"?><!DOCTYPE a PUBLIC "x" "y"><!-- <a><a> --><a>{}<c d="e>f"/><![CDATA[<a><a>]]></a>"#, "<b/>".repeat(1000));
        let doc = parse(&wide, "test").unwrap();
        assert_eq!(doc.root_element().children().filter(|n| n.is_element()).count(), 1001);
    }
}
