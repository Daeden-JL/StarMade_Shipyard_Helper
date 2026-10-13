//! The tools a model builds with: what each is called, what arguments it
//! takes, and what it does to the design.
//!
//! A tool checks its arguments, turns them into `yard_core` edit operations,
//! applies those as one undoable step, and answers in a few plain sentences:
//! what changed, how much, and what was refused and why. Errors are answers
//! too; the model reads them and tries again.

pub mod blocks;
mod build;
pub(crate) mod inspect;
mod inside;
pub mod orient;
pub mod shapes;
mod systems;

use crate::message::{Conversation, StepMark};
use crate::provider::ToolSpec;
use crate::schema;
use crate::world::{EntityInfo, World};
use serde_json::Value;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use yard_core::edit::{Change, FillMode, Op, When, MAX_CELLS, MAX_COORD};
use yard_core::links::Rules;
use yard_core::{Block, Pos};

pub use inspect::ascii_slices;

/// What a tool works with: the design, and the conversation's record of the
/// agent's own steps.
pub struct Ctx<'a> {
    pub world: &'a dyn World,
    pub conversation: &'a mut Conversation,
}

/// A tool's answer when it worked.
pub struct Done {
    pub text: String,
    /// Blocks placed, changed or removed.
    pub changed: usize,
}

impl Done {
    fn said(text: impl Into<String>) -> Self {
        Self { text: text.into(), changed: 0 }
    }
}

type Run = fn(&mut Ctx<'_>, &Args<'_>) -> Result<Done, String>;

pub struct Tool {
    pub name: &'static str,
    pub description: String,
    /// JSON Schema of the arguments.
    pub schema: Value,
    run: Run,
}

impl Tool {
    /// A tool that works on one entity: it takes an `entity` argument.
    fn on_entity(name: &'static str, description: &str, mut properties: Vec<(&str, Value)>, required: &[&str], run: Run) -> Self {
        properties.push(("entity", schema::string("Path of the entity to work on, from design_info: \"\" is the main entity. Leave out for the entity the player has open.")));
        Self { name, description: description.to_string(), schema: schema::object(properties, required), run }
    }

    fn plain(name: &'static str, description: &str, properties: Vec<(&str, Value)>, required: &[&str], run: Run) -> Self {
        Self { name, description: description.to_string(), schema: schema::object(properties, required), run }
    }
}

/// What a tool call came to, as the model and the player's screen are told.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    /// The answer for the model.
    pub text: String,
    pub ok: bool,
    /// Blocks placed, changed or removed.
    pub changed: usize,
}

/// Every tool, ready to be offered to a model and called.
pub struct Toolbox {
    tools: Vec<Tool>,
    /// The entity a call works on when it names none.
    focus: String,
}

impl Default for Toolbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbox {
    pub fn new() -> Self {
        let mut tools = inspect::tools();
        tools.extend(inside::tools());
        tools.extend(build::tools());
        tools.extend(systems::tools());
        Self { tools, focus: String::new() }
    }

    /// Calls that name no entity work on this one instead of the main one.
    pub fn focused_on(mut self, path: &str) -> Self {
        self.focus = path.to_string();
        self
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The tools as a model is told about them. The same on every call, in
    /// the same order, so that a provider's prompt cache stays valid.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| ToolSpec { name: t.name.to_string(), description: t.description.clone(), schema: t.schema.clone() }).collect()
    }

    /// Runs one tool call: checks the arguments against the tool's schema,
    /// runs the tool, and words the outcome for the model.
    pub fn call(&self, world: &dyn World, conversation: &mut Conversation, name: &str, arguments: &Value) -> ToolOutput {
        let failed = |text: String| ToolOutput { text: format!("Error: {text}"), ok: false, changed: 0 };
        let Some(tool) = self.tools.iter().find(|t| t.name == name.trim()) else {
            let names: Vec<&str> = self.tools.iter().map(|t| t.name).collect();
            return failed(format!("There is no tool \"{name}\". The tools are: {}.", names.join(", ")));
        };
        let mut arguments = arguments.clone();
        if let (false, Some(fields)) = (self.focus.is_empty(), arguments.as_object_mut()) {
            if tool.schema["properties"].get("entity").is_some() && !fields.contains_key("entity") {
                fields.insert("entity".to_string(), Value::String(self.focus.clone()));
            }
        }
        if let Err(problem) = schema::validate(&tool.schema, &mut arguments) {
            return failed(format!("{problem} Nothing was changed; correct the arguments and call {name} again."));
        }
        let args = Args { value: &arguments };
        match (tool.run)(&mut Ctx { world, conversation }, &args) {
            Ok(done) => ToolOutput { text: done.text, ok: true, changed: done.changed },
            Err(problem) => failed(problem),
        }
    }
}

/// A call in a few words for the player's screen: the tool and its plainest
/// arguments.
pub fn summary(name: &str, arguments: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["block", "find", "with", "computer", "module", "controller", "query", "topic", "from", "to", "center", "pos", "axis", "start_from", "side"] {
        match arguments.get(key) {
            Some(Value::String(s)) => parts.push(format!("{key} {s}")),
            Some(Value::Array(a)) if a.len() == 3 && a.iter().all(Value::is_number) => parts.push(format!("{key} ({}, {}, {})", a[0], a[1], a[2])),
            _ => {}
        }
    }
    let text = if parts.is_empty() { name.to_string() } else { format!("{name}: {}", parts.join(", ")) };
    crate::provider::clip(&text, 120)
}

/// The checked arguments of one call.
pub struct Args<'a> {
    value: &'a Value,
}

impl<'a> Args<'a> {
    pub fn of(value: &'a Value) -> Self {
        Self { value }
    }

    pub fn has(&self, name: &str) -> bool {
        self.value.get(name).is_some()
    }

    pub fn get(&self, name: &str) -> Option<&'a Value> {
        self.value.get(name)
    }

    pub fn text(&self, name: &str) -> Option<&'a str> {
        self.value.get(name).and_then(Value::as_str)
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        self.value.get(name).and_then(Value::as_i64)
    }

    pub fn num(&self, name: &str) -> Option<f64> {
        self.value.get(name).and_then(Value::as_f64)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.value.get(name).and_then(Value::as_bool).unwrap_or(false)
    }

    /// A position, checked to lie within reach of the core.
    pub fn pos(&self, name: &str) -> Result<Option<Pos>, String> {
        match self.value.get(name) {
            None => Ok(None),
            Some(value) => pos_of(value).map(Some),
        }
    }

    /// A position that the schema requires.
    pub fn need_pos(&self, name: &str) -> Result<Pos, String> {
        self.pos(name)?.ok_or_else(|| format!("Missing argument \"{name}\"."))
    }

    /// The entity the call works on: the main one unless it names another.
    pub fn entity(&self) -> &'a str {
        self.text("entity").unwrap_or("").trim().trim_matches(|c| c == '/' || c == '"' || c == '\'')
    }
}

/// Reads `[x, y, z]`.
pub fn pos_of(value: &Value) -> Result<Pos, String> {
    let coords: Vec<i64> = value.as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    let [x, y, z] = coords[..] else { return Err("A position is three whole numbers: [x, y, z].".to_string()) };
    let limit = MAX_COORD as i64;
    if x.abs() > limit || y.abs() > limit || z.abs() > limit {
        return Err(format!("Position ({x}, {y}, {z}) is too far from the core; the limit is {MAX_COORD} blocks in every direction."));
    }
    Ok(Pos::new(x as i32, y as i32, z as i32))
}

pub fn show(p: Pos) -> String {
    format!("({}, {}, {})", p.x, p.y, p.z)
}

/// The box two corners span: its low and high corner, and its size.
pub fn span(a: Pos, b: Pos) -> (Pos, Pos, u64) {
    let (lo, hi) = (a.lo(b), a.hi(b));
    let volume = (hi.x - lo.x + 1) as u64 * (hi.y - lo.y + 1) as u64 * (hi.z - lo.z + 1) as u64;
    (lo, hi, volume)
}

/// Refuses work on more cells than one step may cover.
pub fn check_volume(cells: u64) -> Result<(), String> {
    if cells > MAX_CELLS {
        Err(format!("That covers {cells} blocks; one step may cover at most {MAX_CELLS}. Do it in several smaller parts."))
    } else {
        Ok(())
    }
}

/// The facts of the entity a call works on.
pub fn entity_info(ctx: &Ctx<'_>, path: &str) -> Result<EntityInfo, String> {
    ctx.world.entities().into_iter().find(|e| e.path == path).ok_or_else(|| crate::world::no_such_entity(path))
}

/// The game's linking rules for the entity a call works on.
pub fn rules_for<'a>(ctx: &Ctx<'a>, info: &EntityInfo) -> Rules<'a> {
    Rules::new(ctx.world.catalog(), info.kind.rules_kind())
}

/// The block a ship's core takes the moment it is placed: computers, which
/// is how they come to be on the pilot's bar. The game links them so, and
/// so do the tools.
pub fn core_link(rules: &Rules<'_>, ty: u16) -> Option<Pos> {
    (rules.kind == yard_core::links::Kind::Ship && rules.core_takes(ty)).then_some(Pos::new(0, 0, 0))
}

/// A set of cells as few fill operations: rows, sheets and blocks of cells
/// become one box each, which keeps a step small to send and to replay.
pub fn fills(cells: &BTreeSet<Pos>, block: Block, when: When, link: Option<Pos>) -> Vec<Op> {
    yard_core::links::boxes(cells).into_iter().map(|(min, max)| Op::Fill { min, max, block, mode: FillMode::Solid, when, link }).collect()
}

/// Applies ops as one step and remembers it as the agent's own, so that
/// `undo` can take back exactly this step.
pub fn apply(ctx: &mut Ctx<'_>, entity: &str, ops: Vec<Op>, what: &str) -> Result<Change, String> {
    if ops.is_empty() {
        return Ok(Change::default());
    }
    let change = ctx.world.step(entity, ops)?;
    if !change.is_empty() {
        ctx.conversation.note_step(StepMark { entity: entity.to_string(), print: fingerprint(&change), what: what.to_string() });
    }
    Ok(change)
}

/// A number that two changes share only if they did the same thing to the
/// same cells and links, whatever order they list them in.
pub fn fingerprint(change: &Change) -> u64 {
    fn hashed(item: impl Hash, salt: u8) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (salt, item).hash(&mut hasher);
        hasher.finish()
    }
    let cells = change.cells.iter().map(|c| hashed(c, 1));
    let linked = change.linked.iter().map(|l| hashed(l, 2));
    let unlinked = change.unlinked.iter().map(|l| hashed(l, 3));
    cells.chain(linked).chain(unlinked).fold(hashed(change.cells.len(), 0), u64::wrapping_add)
}

/// Blocks a change placed, replaced and removed.
pub fn tally(change: &Change) -> (usize, usize, usize) {
    let placed = change.cells.iter().filter(|(_, before, after)| before.is_air() && !after.is_air()).count();
    let removed = change.cells.iter().filter(|(_, before, after)| !before.is_air() && after.is_air()).count();
    (placed, change.cells.len() - placed - removed, removed)
}

/// What a change did, in a sentence or two.
pub fn describe_change(change: &Change) -> String {
    let (placed, replaced, removed) = tally(change);
    let mut parts = Vec::new();
    for (count, word) in [(placed, "placed"), (replaced, "changed"), (removed, "removed")] {
        if count > 0 {
            parts.push(format!("{count} {} {word}", plural(count, "block")));
        }
    }
    for (count, word) in [(change.linked.len(), "made"), (change.unlinked.len(), "removed")] {
        if count > 0 {
            parts.push(format!("{count} {} {word}", plural(count, "link")));
        }
    }
    if parts.is_empty() { "Nothing changed.".to_string() } else { format!("{}.", crate::world::sentence(&parts.join(", ")).trim_end_matches('.')) }
}

pub fn plural(count: usize, word: &str) -> String {
    if count == 1 { word.to_string() } else { format!("{word}s") }
}

/// Which cells an edit may write, from the `only_where` argument.
pub fn when_of(ctx: &Ctx<'_>, args: &Args<'_>) -> Result<When, String> {
    match args.text("only_where").map(str::trim) {
        None | Some("") | Some("anywhere") => Ok(When::Always),
        Some("empty") => Ok(When::Empty),
        Some("occupied") => Ok(When::Occupied),
        Some("inside") => Err("only_where \"inside\" works with fill, line, cylinder, ellipsoid and hull; to place systems inside the hull use fill_inside.".to_string()),
        Some(block) => Ok(When::Type(blocks::lookup(ctx.world.catalog(), block)?.id)),
    }
}

pub const ONLY_WHERE: &str = "Which cells may be written: \"anywhere\" (default; existing blocks are overwritten), \"empty\" (existing blocks are kept), \"inside\" (only empty cells enclosed by the hull: for systems inside a ship), \"occupied\" (only existing blocks are replaced), or a block name (only blocks of that type are replaced).";

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::world::{sample_catalog, MemoryWorld};
    use std::sync::Arc;

    /// A new ship, a toolbox and a conversation to call tools with.
    pub struct Bench {
        pub world: MemoryWorld,
        pub toolbox: Toolbox,
        pub conversation: Conversation,
    }

    impl Bench {
        pub fn new() -> Self {
            Self { world: MemoryWorld::new_ship(Arc::new(sample_catalog()), "Bench"), toolbox: Toolbox::new(), conversation: Conversation::new() }
        }

        pub fn call(&mut self, name: &str, arguments: Value) -> ToolOutput {
            self.toolbox.call(&self.world, &mut self.conversation, name, &arguments)
        }

        /// Calls a tool that must work, and returns what it said.
        pub fn ok(&mut self, name: &str, arguments: Value) -> String {
            let out = self.call(name, arguments.clone());
            assert!(out.ok, "{name} {arguments} failed: {}", out.text);
            out.text
        }

        /// Calls a tool that must refuse, and returns why.
        pub fn err(&mut self, name: &str, arguments: Value) -> String {
            let out = self.call(name, arguments.clone());
            assert!(!out.ok, "{name} {arguments} should have failed but said: {}", out.text);
            assert!(out.text.starts_with("Error: "));
            out.text
        }

        pub fn ty(&self, x: i32, y: i32, z: i32) -> u16 {
            self.world.block("", Pos::new(x, y, z)).ty()
        }

        pub fn blocks(&self) -> u64 {
            self.world.entities()[0].blocks
        }

        pub fn count(&self, ty: u16) -> u32 {
            self.world.with_blueprint(|b| b.root.entity.counts_by_type().get(&ty).copied().unwrap_or(0))
        }

        pub fn linked(&self, controller: Pos, target: Pos) -> bool {
            self.world.with_blueprint(|b| yard_core::links::is_linked(b.root.entity.links(), controller, target))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Bench;
    use super::*;
    use serde_json::json;

    #[test]
    fn every_tool_has_a_distinct_name_a_description_and_an_object_schema() {
        let toolbox = Toolbox::new();
        let mut names: Vec<&str> = toolbox.tools().iter().map(|t| t.name).collect();
        assert!(names.len() >= 20, "{names:?}");
        for tool in toolbox.tools() {
            assert!(tool.description.len() > 40, "{} needs a description a model can choose by", tool.name);
            assert_eq!(tool.schema["type"], "object");
            for (name, property) in tool.schema["properties"].as_object().unwrap() {
                assert!(property["description"].as_str().is_some_and(|d| d.len() > 8), "{}.{name} needs a description", tool.name);
            }
            for required in tool.schema["required"].as_array().unwrap() {
                assert!(tool.schema["properties"].get(required.as_str().unwrap()).is_some(), "{} requires an argument it does not have", tool.name);
            }
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), toolbox.tools().len());
        // The definitions are the same on every call: the prompt cache depends on it.
        assert_eq!(Toolbox::new().specs(), toolbox.specs());
    }

    #[test]
    fn an_unknown_tool_and_bad_arguments_come_back_as_errors_to_correct() {
        let mut bench = Bench::new();
        let unknown = bench.err("build_ship", json!({}));
        assert!(unknown.contains("There is no tool \"build_ship\"") && unknown.contains("fill"));
        let missing = bench.err("fill", json!({"from": [0, 0, 1]}));
        assert!(missing.contains("Missing argument") && missing.contains("call fill again"), "{missing}");
        let far = bench.err("fill", json!({"from": [0, 0, 1], "to": [0, 0, 5000], "block": "Grey Basic Armor"}));
        assert!(far.contains("too far from the core"), "{far}");
        assert!(bench.ok("count_blocks", json!({"entity": "\"\""})).starts_with("1 block"), "a quoted empty path is the main entity");
        let nowhere = bench.err("fill", json!({"from": [0, 0, 1], "to": [0, 0, 2], "block": "Grey Basic Armor", "entity": "ATTACHED_3"}));
        assert!(nowhere.contains("no entity \"ATTACHED_3\""), "{nowhere}");
        assert_eq!(bench.blocks(), 1);
    }

    #[test]
    fn a_change_is_recognised_by_its_fingerprint_whatever_its_order() {
        let cell = |x: i32, ty: u16| (Pos::new(x, 0, 0), Block::AIR, Block::new(ty, 0, false));
        let a = Change { cells: vec![cell(1, 5), cell(2, 5)], ..Default::default() };
        let b = Change { cells: vec![cell(2, 5), cell(1, 5)], ..Default::default() };
        let c = Change { cells: vec![cell(1, 5), cell(2, 6)], ..Default::default() };
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&c));
        assert_ne!(fingerprint(&a), fingerprint(&a.inverse()));
    }

    #[test]
    fn calls_are_summarised_for_the_players_screen() {
        assert_eq!(summary("fill", &json!({"block": "Red Basic Armor", "from": [0, 0, 1], "to": [4, 2, 9], "mode": "hollow"})), "fill: block Red Basic Armor, from (0, 0, 1), to (4, 2, 9)");
        assert_eq!(summary("design_info", &json!({})), "design_info");
    }
}
