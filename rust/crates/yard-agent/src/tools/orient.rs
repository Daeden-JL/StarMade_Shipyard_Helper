//! Saying which way a block is turned in words a model can use, and turning
//! those words into the game's orientation numbers.
//!
//! The numbers mean something different for every block shape, so nothing
//! here is a table of numbers: each answer is found by looking at the
//! shape's geometry in `yard_core::shape` and picking the orientation whose
//! faces lie where the words say.

use std::sync::OnceLock;
use yard_core::catalog::Shape;
use yard_core::shape::{turned_axes, ShapeSet};

/// A direction as the pilot sees it. StarMade's axes are right-handed with
/// +Y up and +Z forward, which puts +X on the pilot's left.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
    Front,
    Back,
}

/// The direction words the tools take.
pub const WORDS: [&str; 6] = ["up", "down", "left", "right", "front", "back"];

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right, Dir::Front, Dir::Back];

    /// One step in this direction.
    pub fn vector(self) -> [i32; 3] {
        match self {
            Dir::Up => [0, 1, 0],
            Dir::Down => [0, -1, 0],
            Dir::Left => [1, 0, 0],
            Dir::Right => [-1, 0, 0],
            Dir::Front => [0, 0, 1],
            Dir::Back => [0, 0, -1],
        }
    }

    pub fn of_vector(v: [i32; 3]) -> Option<Dir> {
        Self::ALL.into_iter().find(|d| d.vector() == v)
    }

    pub fn name(self) -> &'static str {
        WORDS[Self::ALL.iter().position(|d| *d == self).unwrap_or(0)]
    }

    pub fn opposite(self) -> Dir {
        let v = self.vector();
        Self::of_vector([-v[0], -v[1], -v[2]]).unwrap_or(self)
    }

    /// 0 for X, 1 for Y, 2 for Z.
    pub fn axis(self) -> usize {
        self.vector().iter().position(|c| *c != 0).unwrap_or(0)
    }

    /// The cell side on this side of a block, in the game's numbering.
    pub fn side(self) -> usize {
        let v = self.vector();
        yard_core::shape::SIDE_DIRS.iter().position(|d| *d == v).unwrap_or(0)
    }

    /// Reads a direction word, with the synonyms models reach for.
    pub fn parse(word: &str) -> Result<Dir, String> {
        match word.trim().to_ascii_lowercase().as_str() {
            "up" | "top" | "+y" | "above" => Ok(Dir::Up),
            "down" | "bottom" | "-y" | "below" => Ok(Dir::Down),
            "left" | "port" | "+x" => Ok(Dir::Left),
            "right" | "starboard" | "-x" => Ok(Dir::Right),
            "front" | "forward" | "fore" | "nose" | "bow" | "+z" => Ok(Dir::Front),
            "back" | "backward" | "rear" | "aft" | "stern" | "-z" => Ok(Dir::Back),
            other => Err(format!("\"{other}\" is not a direction. Use up, down, left (+X), right (-X), front (+Z) or back (-Z).")),
        }
    }
}

fn shapes() -> &'static ShapeSet {
    static SHAPES: OnceLock<ShapeSet> = OnceLock::new();
    SHAPES.get_or_init(ShapeSet::new)
}

/// Whether a shape has slanted faces, and so takes a slope.
pub fn is_sloped(shape: Shape) -> bool {
    matches!(shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta)
}

/// How many distinct orientations a sloped shape has.
fn turns(shape: Shape) -> u8 {
    ShapeSet::choices(shape, false)
}

/// Where the slanted faces of a shape look at one orientation, as whole
/// numbers per axis: (0, 1, -1) for a wedge sloping up and back; a 2 marks
/// the direction both slopes of a corner share.
fn slant(shape: Shape, orientation: u8) -> [i32; 3] {
    let geom = shapes().geom(shape, 0, orientation);
    let mut sum = [0.0f32; 3];
    for face in geom.faces.iter().filter(|f| f.side.is_none()) {
        for (total, part) in sum.iter_mut().zip(face.normal) {
            *total += part;
        }
    }
    let unit = sum.iter().map(|c| c.abs()).filter(|c| *c > 0.01).fold(f32::MAX, f32::min);
    sum.map(|c| (c / unit).round() as i32)
}

/// The orientation of a wedge, corner, tetra or hepta whose slanted side
/// looks toward the given directions.
///
/// - Wedge: two directions, such as up and back for a ramp that rises
///   toward the front.
/// - Tetra and hepta: three directions; the cut-off corner of the cube is
///   the one they share.
/// - Corner: three directions, the first being the one its point rises in,
///   away from its square base.
pub fn sloped(shape: Shape, toward: &[Dir]) -> Result<u8, String> {
    let needed = if shape == Shape::Wedge { 2 } else { 3 };
    if !is_sloped(shape) {
        return Err("\"slope\" is only for wedge, corner, tetra and hepta shapes. Set \"shape\" as well, or use \"facing\".".to_string());
    }
    if toward.len() != needed {
        return Err(format!("A {} needs {needed} slope directions, not {}. {}", name(shape), toward.len(), slope_help(shape)));
    }
    let mut wanted = [0i32; 3];
    for (i, dir) in toward.iter().enumerate() {
        if wanted[dir.axis()] != 0 {
            return Err(format!("The slope directions of a {} must lie on different axes; {} and its opposite or double cannot both be given. {}", name(shape), dir.name(), slope_help(shape)));
        }
        let weight = if shape == Shape::Corner && i == 0 { 2 } else { 1 };
        wanted[dir.axis()] = dir.vector()[dir.axis()] * weight;
    }
    (0..turns(shape)).find(|o| slant(shape, *o) == wanted).ok_or_else(|| format!("No {} is turned that way. {}", name(shape), slope_help(shape)))
}

/// The directions a sloped block's slanted side looks toward, in the order
/// [`sloped`] takes them.
pub fn slope_of(shape: Shape, orientation: u8) -> Option<Vec<Dir>> {
    if !is_sloped(shape) || orientation >= turns(shape) {
        return None;
    }
    let slant = slant(shape, orientation);
    let mut dirs: Vec<(i32, Dir)> = Vec::new();
    for axis in 0..3 {
        if slant[axis] != 0 {
            let mut v = [0; 3];
            v[axis] = slant[axis].signum();
            dirs.push((slant[axis].abs(), Dir::of_vector(v)?));
        }
    }
    // A corner's rising direction comes first.
    dirs.sort_by_key(|(weight, _)| -weight);
    Some(dirs.into_iter().map(|(_, d)| d).collect())
}

fn name(shape: Shape) -> &'static str {
    match shape {
        Shape::Wedge => "wedge",
        Shape::Corner => "corner",
        Shape::Tetra => "tetra",
        Shape::Hepta => "hepta",
        Shape::Sprite => "sprite",
        Shape::Rail => "rail",
        Shape::Cube => "cube",
    }
}

fn slope_help(shape: Shape) -> &'static str {
    match shape {
        Shape::Wedge => "Example: [\"up\", \"back\"] is a ramp whose slanted face looks up and back; its solid sides are bottom and front.",
        Shape::Corner => "Give the direction its point rises in first, then the two its slopes look toward. Example: [\"up\", \"left\", \"back\"].",
        _ => "Example: [\"up\", \"left\", \"front\"] cuts off the upper left front corner.",
    }
}

/// The orientation that turns a block's front, or a slab's open side,
/// toward a direction.
pub fn facing(toward: Dir) -> u8 {
    ShapeSet::facing(toward.vector())
}

/// The direction a facing block's orientation turns its front toward.
pub fn facing_of(orientation: u8) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| facing(*d) == orientation)
}

/// The orientation of a rail, docker or other block that turns 24 ways:
/// its arrow face on the `face` side, the arrow pointing along `pointing`.
/// Without `pointing` the arrow points to the front, or up where the face
/// itself looks front or back.
pub fn rail(face: Dir, pointing: Option<Dir>) -> Result<u8, String> {
    let pointing = pointing.unwrap_or(if face.axis() == 2 { Dir::Up } else { Dir::Front });
    if pointing.axis() == face.axis() {
        return Err(format!("A rail's arrow lies in its {} face, so it cannot point {}. Pick a direction across that face.", face.name(), pointing.name()));
    }
    (0..24).find(|o| turned_axes(*o) == (face.vector(), pointing.vector())).ok_or_else(|| "No rail is turned that way.".to_string())
}

/// (the side a 24-way block's arrow face is on, where the arrow points)
pub fn rail_of(orientation: u8) -> Option<(Dir, Dir)> {
    let (face, pointing) = turned_axes(orientation);
    Some((Dir::of_vector(face)?, Dir::of_vector(pointing)?))
}

/// How a block is turned, in the words the tools take; empty for blocks
/// that look the same every way.
pub fn describe(shape: Shape, faces_matter: bool, orientation: u8) -> String {
    let names = |dirs: &[Dir]| dirs.iter().map(|d| d.name()).collect::<Vec<_>>().join(", ");
    match shape {
        Shape::Rail => rail_of(orientation).map_or(String::new(), |(face, pointing)| format!("arrow face {}, pointing {}", face.name(), pointing.name())),
        Shape::Cube | Shape::Sprite if faces_matter || shape == Shape::Sprite => facing_of(orientation).map_or(String::new(), |d| format!("facing {}", d.name())),
        Shape::Cube | Shape::Sprite => String::new(),
        sloped => slope_of(sloped, orientation).map_or(String::new(), |dirs| format!("slope {}", names(&dirs))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every way of picking `n` directions on different axes, in order.
    fn picks(n: usize) -> Vec<Vec<Dir>> {
        let mut out: Vec<Vec<Dir>> = vec![vec![]];
        for _ in 0..n {
            let mut longer = Vec::new();
            for sofar in &out {
                for dir in Dir::ALL.into_iter().filter(|d| sofar.iter().all(|s| s.axis() != d.axis())) {
                    longer.push([sofar.clone(), vec![dir]].concat());
                }
            }
            out = longer;
        }
        out
    }

    #[test]
    fn left_is_plus_x_and_every_word_has_its_opposite() {
        assert_eq!(Dir::Left.vector(), [1, 0, 0]);
        assert_eq!(Dir::Front.vector(), [0, 0, 1]);
        assert_eq!(Dir::Up.vector(), [0, 1, 0]);
        for dir in Dir::ALL {
            assert_eq!(Dir::parse(dir.name()).unwrap(), dir);
            assert_eq!(dir.opposite().opposite(), dir);
            assert_ne!(dir.opposite(), dir);
            assert_eq!(yard_core::shape::SIDE_DIRS[dir.side()], dir.vector());
        }
        assert_eq!(Dir::parse(" Starboard ").unwrap(), Dir::Right);
        assert_eq!(Dir::parse("+X").unwrap(), Dir::Left);
        assert!(Dir::parse("sideways").unwrap_err().contains("left (+X)"));
    }

    #[test]
    fn a_wedge_is_solid_on_the_two_sides_its_slope_looks_away_from() {
        let mut seen = HashSet::new();
        for pick in picks(2) {
            let o = sloped(Shape::Wedge, &pick).unwrap();
            let geom = shapes().geom(Shape::Wedge, 0, o);
            for dir in &pick {
                assert!(geom.fills(dir.opposite().side()), "wedge sloping {pick:?} must be solid on its {} side", dir.opposite().name());
                assert!(!geom.faces.iter().any(|f| f.side == Some(dir.side())), "wedge sloping {pick:?} has no face on its {} side", dir.name());
            }
            let slope = geom.faces.iter().find(|f| f.side.is_none()).unwrap();
            for dir in &pick {
                assert!(slope.normal[dir.axis()] * dir.vector()[dir.axis()] as f32 > 0.5);
            }
            // The order of the two words does not matter.
            assert_eq!(sloped(Shape::Wedge, &[pick[1], pick[0]]).unwrap(), o);
            assert_eq!(slope_of(Shape::Wedge, o).map(|d| d.into_iter().collect::<HashSet<_>>()), Some(pick.iter().copied().collect()));
            seen.insert(o);
        }
        assert_eq!(seen.len(), 12, "all twelve wedges can be asked for");
    }

    #[test]
    fn a_ramp_rising_toward_the_front_is_the_games_wedge_zero() {
        // Checked against the game in yard-core: wedge 0 has a full bottom
        // and front, its slope looking up and back.
        assert_eq!(sloped(Shape::Wedge, &[Dir::Up, Dir::Back]).unwrap(), 0);
        assert_eq!(describe(Shape::Wedge, false, 0), "slope up, back");
    }

    #[test]
    fn tetras_and_heptas_are_cut_at_the_corner_their_slope_looks_toward() {
        for shape in [Shape::Tetra, Shape::Hepta] {
            let mut seen = HashSet::new();
            for pick in picks(3) {
                let o = sloped(shape, &pick).unwrap();
                let geom = shapes().geom(shape, 0, o);
                for dir in &pick {
                    let behind = dir.opposite().side();
                    if shape == Shape::Hepta {
                        assert!(geom.fills(behind), "hepta sloping {pick:?} is solid on its {} side", dir.opposite().name());
                        assert!(!geom.fills(dir.side()));
                    } else {
                        assert!(geom.faces.iter().any(|f| f.side == Some(behind)), "tetra sloping {pick:?} touches its {} side", dir.opposite().name());
                        assert!(!geom.faces.iter().any(|f| f.side == Some(dir.side())), "tetra sloping {pick:?} stays clear of its {} side", dir.name());
                    }
                }
                seen.insert(o);
            }
            assert_eq!(seen.len(), 8, "{shape:?}");
        }
    }

    #[test]
    fn a_corner_sits_on_the_side_opposite_to_where_its_point_rises() {
        let mut seen = HashSet::new();
        for pick in picks(3) {
            let o = sloped(Shape::Corner, &pick).unwrap();
            let geom = shapes().geom(Shape::Corner, 0, o);
            assert!(geom.fills(pick[0].opposite().side()), "corner rising {} has its base on the other side", pick[0].name());
            for dir in &pick[1..] {
                // Upright faces stand on the sides the slopes look away from.
                assert!(geom.faces.iter().any(|f| f.side == Some(dir.opposite().side())));
                assert!(!geom.faces.iter().any(|f| f.side == Some(dir.side())));
            }
            assert_eq!(slope_of(Shape::Corner, o).unwrap()[0], pick[0]);
            seen.insert(o);
        }
        assert_eq!(seen.len(), 24, "all twenty-four corners can be asked for");
    }

    #[test]
    fn wrong_slopes_are_explained() {
        assert!(sloped(Shape::Wedge, &[Dir::Up]).unwrap_err().contains("needs 2 slope directions"));
        assert!(sloped(Shape::Wedge, &[Dir::Up, Dir::Down]).unwrap_err().contains("different axes"));
        assert!(sloped(Shape::Tetra, &[Dir::Up, Dir::Back]).unwrap_err().contains("needs 3"));
        assert!(sloped(Shape::Cube, &[Dir::Up, Dir::Back]).unwrap_err().contains("only for wedge"));
    }

    #[test]
    fn a_slab_lies_against_the_side_opposite_to_its_facing() {
        for dir in Dir::ALL {
            let o = facing(dir);
            for slab in 1..=3 {
                let geom = shapes().geom(Shape::Cube, slab, o);
                assert!(geom.fills(dir.opposite().side()), "slab facing {} lies on the {} side", dir.name(), dir.opposite().name());
                assert!(!geom.fills(dir.side()));
            }
            assert_eq!(facing_of(o), Some(dir));
        }
    }

    #[test]
    fn a_rail_shows_its_arrow_on_the_face_and_in_the_direction_asked_for() {
        let mut seen = HashSet::new();
        for face in Dir::ALL {
            for pointing in Dir::ALL.into_iter().filter(|p| p.axis() != face.axis()) {
                let o = rail(face, Some(pointing)).unwrap();
                assert_eq!(turned_axes(o), (face.vector(), pointing.vector()));
                assert_eq!(rail_of(o), Some((face, pointing)));
                seen.insert(o);
            }
        }
        assert_eq!(seen.len(), 24);
        // The editor's own default: arrow face up, pointing forward.
        assert_eq!(rail(Dir::Up, None).unwrap(), 14);
        assert!(rail(Dir::Up, Some(Dir::Down)).is_err());
    }
}
