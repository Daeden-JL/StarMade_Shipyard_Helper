//! Colours: the sRGB curve and finding the block nearest to a colour.

use std::sync::OnceLock;

/// The light a colour byte stands for, 0 to 1.
pub(crate) fn to_linear(byte: u8) -> f32 {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *slot = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        table
    })[byte as usize]
}

/// The colour byte for an amount of light, 0 to 1.
pub(crate) fn from_linear(light: f32) -> u8 {
    let l = if light.is_finite() { light.clamp(0.0, 1.0) } else { 0.0 };
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (c * 255.0).round() as u8
}

/// The byte for a colour part written as 0 to 1 that is already on the
/// sRGB curve, as most text formats write their material colours.
pub(crate) fn byte(part: f32) -> u8 {
    if part.is_finite() {
        (part.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// Two colours multiplied, as a tint on a texture.
pub(crate) fn tinted(colour: [u8; 3], tint: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| ((u16::from(colour[i]) * u16::from(tint[i]) + 127) / 255) as u8)
}

/// A colour in Oklab, where straight-line distance is close to how
/// different two colours look.
fn oklab(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(to_linear);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// A palette made ready for many lookups: the caller's (id, rgb) pairs with
/// their Oklab values worked out once.
#[derive(Clone, Debug)]
pub struct Matcher {
    entries: Vec<(u16, [f32; 3])>,
}

impl Matcher {
    pub fn new(palette: &[(u16, [u8; 3])]) -> Matcher {
        Matcher { entries: palette.iter().map(|&(id, rgb)| (id, oklab(rgb))).collect() }
    }

    /// The id of the palette entry that looks most like the colour; the
    /// first of equals. Nothing for an empty palette.
    pub fn nearest(&self, colour: [u8; 3]) -> Option<u16> {
        let want = oklab(colour);
        let mut best: Option<(u16, f32)> = None;
        for &(id, lab) in &self.entries {
            let d = (0..3).map(|i| (lab[i] - want[i]) * (lab[i] - want[i])).sum::<f32>();
            if best.is_none_or(|(_, least)| d < least) {
                best = Some((id, d));
            }
        }
        best.map(|(id, _)| id)
    }
}

/// Nearest palette entry for a colour, by how different the two look (the
/// distance in Oklab), for mapping model colours to block types. For many
/// colours against one palette, make a [`Matcher`] once instead.
pub fn nearest(palette: &[(u16, [u8; 3])], colour: [u8; 3]) -> Option<u16> {
    Matcher::new(palette).nearest(colour)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PALETTE: [(u16, [u8; 3]); 6] = [(1, [0, 0, 0]), (2, [255, 255, 255]), (3, [200, 30, 30]), (4, [30, 160, 40]), (5, [40, 60, 200]), (6, [128, 128, 128])];

    #[test]
    fn an_exact_palette_colour_finds_itself() {
        for (id, rgb) in PALETTE {
            assert_eq!(nearest(&PALETTE, rgb), Some(id));
        }
    }

    #[test]
    fn a_colour_finds_the_entry_that_looks_closest() {
        assert_eq!(nearest(&PALETTE, [255, 80, 70]), Some(3));
        assert_eq!(nearest(&PALETTE, [10, 12, 8]), Some(1));
        assert_eq!(nearest(&PALETTE, [120, 125, 130]), Some(6));
        assert_eq!(nearest(&PALETTE, [90, 110, 250]), Some(5));
        assert_eq!(nearest(&PALETTE, [240, 240, 235]), Some(2));
    }

    #[test]
    fn an_empty_palette_has_no_nearest_entry() {
        assert_eq!(nearest(&[], [1, 2, 3]), None);
    }

    #[test]
    fn the_srgb_curve_goes_there_and_back() {
        for byte in 0..=255u8 {
            assert_eq!(from_linear(to_linear(byte)), byte);
        }
    }
}
