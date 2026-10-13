//! The JSON the server sends and the browser client reads. Defined once here so
//! the two cannot drift apart.

use crate::catalog::{BlockDef, Catalog, Shape};
use crate::edit::{Change, Op};
use crate::Blueprint;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the client needs to know about a block type to draw and name it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlock {
    pub id: u16,
    pub name: String,
    pub category: String,
    pub shape: Shape,
    /// Average colour of the block's textures.
    pub color: [u8; 3],
    pub transparent: bool,
    pub light: bool,
    pub placeable: bool,
    /// 0 for a full block, 1..=3 for slabs (1 is three quarters thick, 3 a quarter).
    #[serde(default)]
    pub slab: u8,
    /// The full cube this is a wedge, corner or slab of; 0 if it is not a variant.
    #[serde(default)]
    pub base: u16,
    /// How many of its sides have their own texture: 1, 3 or 6. With 6 the
    /// block has a front, and its orientation says where that faces.
    #[serde(default)]
    pub sides: u8,
    /// Whether the game places it switched on (lights, doors, storage).
    #[serde(default)]
    pub active: bool,
    /// Texture tile per side of the block (front, back, top, bottom, right,
    /// left): page * 256 + row * 16 + column in the game's texture sheets,
    /// which the server hands out at `/api/textures/<page>`.
    #[serde(default)]
    pub textures: [i32; 6],
    /// Its side textures point where the block points.
    #[serde(default)]
    pub pointing: bool,
    /// What the game's linking rules need to know about it.
    #[serde(default)]
    pub links: crate::links::LinkTraits,
    /// The 3D model the game draws for it in place of a cube, as a number in
    /// the list `/api/models` hands out. Filled in by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<u16>,
    /// The model drawn while the block is switched on, where that differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_on: Option<u16>,
}

impl ClientBlock {
    /// What the browser is told about a block of the game's block list.
    /// `color` is the average colour of its textures.
    pub fn of(def: &BlockDef, color: [u8; 3]) -> Self {
        Self {
            id: def.id,
            name: def.name.clone(),
            category: def.category.clone(),
            shape: def.shape,
            color,
            transparent: def.transparent,
            light: def.light,
            placeable: def.placeable && !def.deprecated,
            slab: def.slab,
            base: def.base,
            sides: def.sides,
            active: def.active_when_placed(),
            textures: def.textures,
            pointing: def.pointing,
            links: crate::links::LinkTraits::of(def),
            model: None,
            model_on: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntitySummary {
    /// "" for the main entity, "ATTACHED_0", "ATTACHED_0/ATTACHED_1"... for docked ones.
    pub path: String,
    pub blocks: u64,
    pub links: usize,
    /// Lowest and highest block position, or None for an empty entity.
    pub min: Option<[i32; 3]>,
    pub max: Option<[i32; 3]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeCount {
    pub id: u16,
    pub name: String,
    pub count: u32,
}

/// Where a design stands with the game server's blueprint catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    /// The game server has this design exactly as it is here.
    Synced,
    /// New or changed here; it goes to the game server at the next exchange.
    Waiting,
    /// Kept out of the exchange, in both directions.
    Off,
    /// The game had this design and no longer does: it was deleted there. It
    /// is not sent back unless someone switches its syncing on again.
    Gone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub blocks: u64,
    pub entities: Vec<EntitySummary>,
    /// Most common block types across all entities.
    pub top_types: Vec<TypeCount>,
    pub warnings: Vec<String>,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// When the stored file last changed, in seconds since the Unix epoch. Set
    /// by the server when it saves.
    #[serde(default)]
    pub modified: u64,
    /// Fingerprint of the stored file's contents. Set by the server when it saves.
    #[serde(default)]
    pub print: String,
    /// Goes up each time the design is replaced from outside the editor: a
    /// newer version from the game, a rename. An editor that has the design
    /// open sees the number change and loads it again. Kept by the server.
    #[serde(default)]
    pub epoch: u64,
    /// Filled in by the server when it answers, once a game server has ever
    /// connected; not part of what is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncStatus>,
}

impl Summary {
    pub fn of(id: &str, bp: &Blueprint, catalog: &Catalog, created: u64) -> Self {
        Self::describe(id, bp, created, |ty| catalog.get(ty).map(|d| d.name.clone()))
    }

    /// Like [`Summary::of`] for callers that have block names but no full
    /// catalog. `name_of` returns None for a block type the game does not have.
    pub fn describe(id: &str, bp: &Blueprint, created: u64, name_of: impl Fn(u16) -> Option<String>) -> Self {
        let mut by_type: BTreeMap<u16, u32> = BTreeMap::new();
        let mut entities = Vec::new();
        for (path, e) in bp.entities() {
            for (ty, n) in e.entity.counts_by_type() {
                *by_type.entry(ty).or_insert(0) += n;
            }
            let bounds = e.entity.bounds();
            entities.push(EntitySummary {
                path,
                blocks: e.entity.block_count(),
                links: e.entity.link_count(),
                min: bounds.map(|b| [b.min.x, b.min.y, b.min.z]),
                max: bounds.map(|b| [b.max.x, b.max.y, b.max.z]),
            });
        }
        let mut warnings = bp.warnings.clone();
        let unknown: u64 = by_type.iter().filter(|(ty, _)| name_of(**ty).is_none()).map(|(_, n)| *n as u64).sum();
        if unknown > 0 {
            warnings.push(format!(
                "{unknown} blocks are of types this version of the game does not have. The game removes such blocks when it loads the design."
            ));
        }
        let mut top: Vec<_> = by_type.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Summary {
            id: id.to_string(),
            name: bp.name.clone(),
            blocks: entities.iter().map(|e| e.blocks).sum(),
            entities,
            top_types: top
                .into_iter()
                .take(12)
                .map(|(id, count)| TypeCount { id, name: name_of(id).unwrap_or_else(|| format!("Unknown block {id}")), count })
                .collect(),
            warnings,
            created,
            modified: 0,
            print: String::new(),
            epoch: 0,
            sync: None,
        }
    }
}

/// One undoable step from the editor: a click, a drag, an agent tool call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRequest {
    /// The revision the sender's copy of the design is at. The server refuses
    /// the step if its own copy has moved on, and the sender then reloads.
    pub rev: u64,
    /// Which entity: "" for the main one, "ATTACHED_0"... for docked ones.
    pub entity: String,
    pub ops: Vec<Op>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepResponse {
    pub rev: u64,
    /// Steps of this entity that can now be undone and redone.
    pub undo: usize,
    pub redo: usize,
    /// What the step changed. Sent for undo and redo only: for those the server
    /// holds the history, so the client cannot work the result out by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

/// Moving the core of one entity to another cell of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelocateRequest {
    pub rev: u64,
    /// Which entity: "" for the main one, "ATTACHED_0"... for docked ones.
    pub entity: String,
    /// The cell the core is to move to, in that entity's design coordinates.
    pub to: [i32; 3],
    /// The way, in the entity's coordinates as they are, that is to become
    /// the core's forward (+Z), and the way that is to become its up (+Y):
    /// the entity turns round the core to match. Left out, nothing turns.
    #[serde(default)]
    pub forward: Option<[i32; 3]>,
    #[serde(default)]
    pub up: Option<[i32; 3]>,
    /// On a turret, whether the dock goes with the core: the rail docker,
    /// the turret axis it rests on and the axes of the turrets docked to
    /// this one shift by as much, so the turret keeps its place and only
    /// the line it turns about moves. On by default; nothing else on an
    /// entity that is not a turret.
    #[serde(default = "yes")]
    pub dock_follows: bool,
}

fn yes() -> bool {
    true
}

/// Moving the rail block (a turret axis or a rotator) a docked entity hangs from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoveRailRequest {
    pub rev: u64,
    /// The entity the cell is given in: the docked entity itself (`to` is
    /// then where its rail docker is to be, the rail going in front of it
    /// on the parent) or its parent (`to` is the rail's own new cell).
    pub entity: String,
    /// The docked entity whose rail moves: "ATTACHED_0", "ATTACHED_0/ATTACHED_1"...
    pub child: String,
    pub to: [i32; 3],
    /// Whether the docked entity comes along with its rail, docked as it
    /// was (it shifts by as much in the world), or stays where it is with
    /// its rail docker moved to meet the rail.
    #[serde(default = "yes")]
    pub carried: bool,
}

/// Cutting a box of blocks out of one entity to dock it as a turret.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurretRequest {
    pub rev: u64,
    pub entity: String,
    /// The corners of the box, in design coordinates, either way round.
    pub min: [i32; 3],
    pub max: [i32; 3],
    /// The side of the box that rests on the entity (0 front +Z, 1 back -Z,
    /// 2 top +Y, 3 bottom -Y, 4 right +X, 5 left -X).
    pub base: u8,
    /// The cell of the base face the rail docker takes; the middle of it if none.
    #[serde(default)]
    pub docker: Option<[i32; 3]>,
    /// Cells of the box whose blocks stay on the entity.
    #[serde(default)]
    pub exclude: Vec<[i32; 3]>,
}

/// Putting a design back as it was before its last change of structure.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub rev: u64,
}

/// Copying a docked entity, with everything docked to it, to another dock
/// (see `clone::clone_docked`): in place of another docked entity
/// (`replace`), or onto a new rail block at `at` in `parent`, facing
/// `facing` (one of `parent`/`at`/`facing` or `replace` must be given).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloneRequest {
    pub rev: u64,
    /// The docked entity to copy: "ATTACHED_0", "ATTACHED_0/ATTACHED_1"...
    pub source: String,
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub at: Option<[i32; 3]>,
    #[serde(default)]
    pub facing: Option<[i32; 3]>,
}

/// Moving the cores of the sections of a dock chain, each with its dock
/// following, in order (see `relocate::relocate`): what an auto-fit found.
/// All are made, or none.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PivotsRequest {
    pub rev: u64,
    pub steps: Vec<PivotStep>,
}

/// One section's core move: to `to`, in the entity's coordinates as they
/// are when its turn comes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PivotStep {
    pub entity: String,
    pub to: [i32; 3],
}

/// What a change of structure did. The sender loads the design again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restructured {
    pub rev: u64,
    /// The entity to look at afterwards: the same one, or the one just made.
    pub entity: String,
    /// What the player should know, in words.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewDesign {
    pub name: String,
}

/// A new name for a design.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rename {
    pub name: String,
}

/// Name of the response header that carries a design file's revision.
pub const REV_HEADER: &str = "x-yard-rev";

/// Turns the exchange with the game server on or off for one design.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncSwitch {
    pub on: bool,
}

/// What the editor shows about the game server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncInfo {
    /// Whether a game server has ever exchanged blueprints with this server.
    #[serde(default)]
    pub known: bool,
    /// The name the game server's agent gave, if it has reported since this
    /// server started.
    pub server: Option<String>,
    /// Seconds since its last report; None if it has not reported since this
    /// server started.
    pub seconds_ago: Option<u64>,
    /// Blueprints in the game's catalog at that report.
    pub blueprints: usize,
}

/// One blueprint in the game's catalog, as its agent reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameBlueprint {
    /// The blueprint's name, which is also its folder name. Exact spelling.
    pub name: String,
    /// Fingerprint of the blueprint's files; see the server's `sync` module.
    /// Empty when `busy`.
    #[serde(default)]
    pub print: String,
    /// The blueprint is there but cannot be read right now: the game is in
    /// the middle of saving it. Nothing is decided about it this time. An
    /// agent must report such a blueprint this way and never leave it out,
    /// because a blueprint missing from the report counts as deleted.
    #[serde(default)]
    pub busy: bool,
}

/// A game server's agent saying what the game has. Sent every few seconds,
/// and only when the whole list could be read: every blueprint the report
/// leaves out is taken to have been deleted in the game.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameReport {
    /// A label for the game server, for people to read.
    pub server: String,
    pub blueprints: Vec<GameBlueprint>,
    /// The shipyards on stations the game has loaded right now. None from an
    /// agent that does not know about shipyards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yards: Option<Vec<GameYard>>,
}

/// One shipyard in the game, as its agent reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameYard {
    /// Stays the same for as long as the shipyard exists: the station's
    /// unique name and the position of the shipyard's computer.
    pub id: String,
    /// The station's name, for people to read.
    pub station: String,
    /// Where the shipyard's computer is on the station.
    pub at: [i32; 3],
    pub state: YardState,
    /// A player is aboard the loaded design, so it must not be swapped out.
    #[serde(default)]
    pub in_use: bool,
    /// The shipyard's inventory has room for one more design.
    #[serde(default)]
    pub room: bool,
    /// The designs the shipyard holds.
    #[serde(default)]
    pub designs: Vec<YardDesign>,
    /// The design that is loaded, by its unique name.
    #[serde(default)]
    pub loaded: Option<String>,
    /// Fingerprint of the latest complete copy taken of the loaded design,
    /// for a design the plan asked to watch.
    #[serde(default)]
    pub loaded_print: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardState {
    /// Waiting for an order, with no design loaded.
    #[default]
    Idle,
    /// A design is loaded and can be edited.
    Loaded,
    /// Working on an order, or holding a real ship.
    Busy,
}

/// A design held in a shipyard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct YardDesign {
    /// The design's unique name in the game.
    pub uid: String,
    /// The name the shipyard shows.
    pub name: String,
}

/// The answer to a report: what the agent should hand over and take in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncPlan {
    /// Game blueprints to upload here, by name.
    pub send: Vec<String>,
    /// Designs to download and put into the game's catalog.
    pub fetch: Vec<SyncFetch>,
    /// What to do with designs held in shipyards.
    #[serde(default)]
    pub yards: YardPlan,
}

/// What an agent should do with shipyard designs that are linked to designs here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct YardPlan {
    /// Loaded designs, by unique name, to keep a fresh copy of and report
    /// the fingerprint of as `loaded_print`.
    pub watch: Vec<String>,
    /// Loaded designs to upload: they changed in the shipyard.
    pub take: Vec<YardTake>,
    /// Blueprints from the game's catalog to put into a shipyard as a design.
    pub give: Vec<YardGive>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YardTake {
    /// The design here it belongs to.
    pub design: String,
    /// The shipyard design's unique name.
    pub uid: String,
    /// The fingerprint of the copy to upload, as reported.
    pub print: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YardGive {
    /// The design here it belongs to.
    pub design: String,
    /// The shipyard, by [`GameYard::id`].
    pub yard: String,
    /// The shipyard design it replaces, by unique name. None for a new one.
    pub replace: Option<String>,
    /// The name the shipyard should show for it.
    pub name: String,
    /// The blueprint in the game's catalog to make it from.
    pub blueprint: String,
    /// The fingerprint that blueprint must have.
    pub print: String,
    /// The fingerprint the replaced design's latest copy must have: one
    /// that changed since is not swapped out. None to replace it regardless.
    pub expect: Option<String>,
}

/// The agent confirming that a shipyard holds a design made from a blueprint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YardGiven {
    pub design: String,
    /// The new shipyard design's unique name.
    pub uid: String,
    /// The blueprint's fingerprint, as it was given.
    pub print: String,
}

/// The agent saying why it could not do something with a shipyard design.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YardTrouble {
    pub design: String,
    pub why: String,
}

/// Which side wins when a design here is linked to a design in a shipyard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YardKeep {
    /// The design here goes into the shipyard.
    Here,
    /// The shipyard's design comes here.
    Shipyard,
}

/// A request to link a design to a shipyard.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YardLinkRequest {
    /// By [`GameYard::id`].
    pub yard: String,
    /// The shipyard design to link to, by unique name; None to make a new one.
    pub uid: Option<String>,
    pub keep: YardKeep,
}

/// Where a design stands with the shipyard design it is linked to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct YardLinkInfo {
    pub station: String,
    pub at: [i32; 3],
    /// The name the shipyard shows for the design, once there is one.
    pub name: Option<String>,
    /// Both sides hold the same thing.
    pub in_step: bool,
    /// What is happening or what it waits for, for people to read.
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncFetch {
    pub design: String,
    /// The blueprint name in the game; also the folder inside the file.
    pub name: String,
    /// The design's fingerprint right now. An agent whose game refused this
    /// very version can tell without downloading it again.
    pub print: String,
}

/// The agent confirming that a fetched design is now in the game's catalog.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncDone {
    pub design: String,
    pub name: String,
    /// The fingerprint that came with the downloaded file.
    pub design_print: String,
    /// The fingerprint of the game's copy after it was imported.
    pub game_print: String,
}

/// Name of the response header that carries a design file's fingerprint.
pub const PRINT_HEADER: &str = "x-yard-print";
/// Name of the request header for the secret an agent shares with the server.
pub const TOKEN_HEADER: &str = "x-yard-sync-token";

/// Which way a version of a design travelled when it was synced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    /// The game's blueprint came here.
    FromGame,
    /// The design went to the game.
    ToGame,
    /// Both sides already held the same thing when they were paired up.
    Matched,
    /// Not synced at all: edits made here that the game never saw, kept
    /// because an earlier version was put back over them.
    Unsynced,
    /// The design came from a shipyard in the game it is linked to.
    FromShipyard,
}

/// One version of a design that was in step with the game server. The server
/// keeps the file of each of the last [`SYNC_VERSIONS_KEPT`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncVersion {
    /// Counts up per design; never reused.
    pub seq: u64,
    /// When it was synced, in seconds since the Unix epoch.
    pub time: u64,
    pub direction: SyncDirection,
    /// The design's name at that time.
    pub name: String,
    pub blocks: u64,
    pub entities: usize,
    /// Size of the version's `.sment` file.
    pub bytes: u64,
    /// Fingerprint of the version's contents.
    pub print: String,
    /// Filled in when the versions are listed: the design is at this version
    /// right now.
    #[serde(default)]
    pub current: bool,
}

/// How many synced versions of each design are kept.
pub const SYNC_VERSIONS_KEPT: usize = 20;

/// The body of every error response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Pos};

    #[test]
    fn a_summary_counts_blocks_and_flags_types_the_game_does_not_have() {
        let mut bp = Blueprint::new_ship("Scout", "test");
        for x in 1..=4 {
            bp.root.entity.set(Pos::new(x, 0, 0), Block::new(5, 0, false));
        }
        bp.root.entity.set(Pos::new(0, 1, 0), Block::new(1999, 0, false));
        let names = |ty: u16| match ty {
            1 => Some("Ship Core".to_string()),
            5 => Some("Grey Armor".to_string()),
            _ => None,
        };
        let s = Summary::describe("abc", &bp, 7, names);
        assert_eq!((s.name.as_str(), s.blocks, s.created), ("Scout", 6, 7));
        assert_eq!(s.entities[0].min, Some([0, 0, 0]));
        assert_eq!(s.entities[0].max, Some([4, 1, 0]));
        assert_eq!(s.top_types[0], TypeCount { id: 5, name: "Grey Armor".into(), count: 4 });
        assert!(s.top_types.iter().any(|t| t.name == "Unknown block 1999"));
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].starts_with("1 blocks are of types"));
        // Without unknown blocks there is nothing to warn about.
        bp.root.entity.set(Pos::new(0, 1, 0), Block::AIR);
        assert!(Summary::describe("abc", &bp, 7, names).warnings.is_empty());
    }
}

/// What importing a 3D model made.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportResult {
    pub summary: Summary,
    /// Things the player should know about how the model was read.
    pub notes: Vec<String>,
}
