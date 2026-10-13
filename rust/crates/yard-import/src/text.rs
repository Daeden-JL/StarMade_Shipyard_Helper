//! Small helpers for the formats written as text.

/// The next number of a run of words, or nothing if the run ends or the
/// word is not a number.
pub(crate) fn next_f32<'a>(words: &mut impl Iterator<Item = &'a str>) -> Option<f32> {
    words.next()?.parse().ok()
}

/// Three numbers in a row.
pub(crate) fn next_point<'a>(words: &mut impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    Some([next_f32(words)?, next_f32(words)?, next_f32(words)?])
}

/// All the numbers of a text, split at blanks and commas. A word that is
/// not a number is an error in the caller's words.
pub(crate) fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split(|c: char| c.is_ascii_whitespace() || c == ',').filter(|w| !w.is_empty()).map(|w| w.parse().ok()).collect()
}

/// A 1-based or 0-based index list entry to a slice position: `index` as
/// written, `base` what the format counts from, negative counting back
/// from the end where the format allows it.
pub(crate) fn resolve(index: i64, base: i64, len: usize) -> Option<usize> {
    let at = if index < 0 { len as i64 + index } else { index - base };
    (0..len as i64).contains(&at).then_some(at as usize)
}

/// A colour written as "#RRGGBB" or "#RRGGBBAA".
pub(crate) fn hex_colour(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() < 6 || !hex.is_ascii() {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}
