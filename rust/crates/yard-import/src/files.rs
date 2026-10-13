//! The files that came along with a model: its materials, buffers and
//! pictures, when they were uploaded together in a zip.

/// Companion files by name. A model names them by path, often one that
/// only made sense on its author's machine, so they are looked up
/// forgivingly.
#[derive(Clone, Debug, Default)]
pub struct Files {
    entries: Vec<(String, Vec<u8>)>,
}

impl Files {
    pub fn new() -> Files {
        Files::default()
    }

    /// Adds a file under its path, as in the zip.
    pub fn add(&mut self, name: &str, bytes: Vec<u8>) {
        self.entries.push((tidy(name), bytes));
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The file a model in folder `dir` means by `reference`: first by the
    /// path as written, then by the bare file name anywhere, both without
    /// regard to upper and lower case.
    pub(crate) fn find(&self, dir: &str, reference: &str) -> Option<&[u8]> {
        let reference = tidy(&unescape(reference.trim().trim_start_matches("file://")));
        if reference.is_empty() {
            return None;
        }
        let joined = if dir.is_empty() { reference.clone() } else { tidy(&format!("{dir}/{reference}")) };
        let exact = |want: &str| self.entries.iter().find(|(name, _)| name.eq_ignore_ascii_case(want));
        let bare = reference.rsplit('/').next().unwrap_or(&reference);
        let by_name = || self.entries.iter().find(|(name, _)| name.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(bare)));
        exact(&joined).or_else(|| exact(&reference)).or_else(by_name).map(|(_, bytes)| bytes.as_slice())
    }
}

/// A path with forward slashes and no "." or ".." steps.
fn tidy(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Undoes the %20 kind of escapes that links carry.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One file being read, with what came along with it.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
    pub files: &'a Files,
}

impl<'a> Source<'a> {
    /// The folder the file lies in, as far as its name tells.
    pub fn dir(&self) -> String {
        let name = tidy(self.name);
        match name.rfind('/') {
            Some(slash) => name[..slash].to_string(),
            None => String::new(),
        }
    }

    pub fn companion(&self, reference: &str) -> Option<&'a [u8]> {
        self.files.find(&self.dir(), reference)
    }

    /// The file's contents as text, whatever stray bytes are in it.
    pub fn text(&self) -> std::borrow::Cow<'a, str> {
        String::from_utf8_lossy(self.bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(self.bytes))
    }
}

/// The lower-case ending of a file name, without the dot.
pub(crate) fn extension(name: &str) -> String {
    let bare = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match bare.rfind('.') {
        Some(dot) if dot > 0 => bare[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_companion_is_found_by_path_then_by_bare_name_whatever_the_case() {
        let mut files = Files::new();
        files.add("ship/Textures/Hull.PNG", vec![1]);
        files.add("ship/ship.mtl", vec![2]);
        assert_eq!(files.find("ship", "ship.mtl"), Some(&[2u8][..]));
        assert_eq!(files.find("ship", "textures/hull.png"), Some(&[1u8][..]));
        assert_eq!(files.find("ship", "C:\\Users\\me\\Desktop\\hull.png"), Some(&[1u8][..]));
        assert_eq!(files.find("ship/sub", "../Textures/Hull.PNG"), Some(&[1u8][..]));
        assert_eq!(files.find("", "missing.png"), None);
    }

    #[test]
    fn link_escapes_in_a_reference_are_undone() {
        let mut files = Files::new();
        files.add("my hull.png", vec![7]);
        assert_eq!(files.find("", "my%20hull.png"), Some(&[7u8][..]));
        assert_eq!(files.find("", "100%"), None);
    }

    #[test]
    fn the_extension_is_the_lower_case_ending() {
        assert_eq!(extension("a/b/Ship.OBJ"), "obj");
        assert_eq!(extension("noending"), "");
        assert_eq!(extension(".hidden"), "");
    }
}
