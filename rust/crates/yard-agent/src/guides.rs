//! Short notes on game mechanics that a model reads when it needs them,
//! instead of carrying them in every prompt.

/// The guide topics, as the read_guide tool offers them.
pub const TOPICS: [&str; 8] = ["building", "shapes", "power", "weapons", "shields-thrust", "linking", "logic", "rails"];

/// The text of a guide, or None for an unknown topic.
pub fn read(topic: &str) -> Option<&'static str> {
    Some(match topic.trim().to_ascii_lowercase().as_str() {
        "building" => include_str!("../resources/guides/building.md"),
        "shapes" => include_str!("../resources/guides/shapes.md"),
        "power" => include_str!("../resources/guides/power.md"),
        "weapons" => include_str!("../resources/guides/weapons.md"),
        "shields-thrust" => include_str!("../resources/guides/shields-thrust.md"),
        "linking" => include_str!("../resources/guides/linking.md"),
        "logic" => include_str!("../resources/guides/logic.md"),
        "rails" => include_str!("../resources/guides/rails.md"),
        _ => return None,
    })
}
