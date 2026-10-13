//! The editing session: the open design, the viewport, and everything a click
//! sets in motion. Edits are applied here at once and sent to the server in
//! order; the server's copy is the one that is saved and that holds the undo
//! history.

use crate::api::{self, StepFailure};
use crate::gl::{Overlay, Renderer};
use crate::looks::BlockLooks;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlCanvasElement;
use yard_core::api::{ClientBlock, GameYard, StepRequest, Summary, SyncInfo, SyncStatus, SyncVersion, YardKeep, YardLinkInfo, YardLinkRequest};
use yard_core::catalog::Shape;
use yard_core::shape::{ShapeSet, SIDE_DIRS};
use yard_core::edit::{self, Change, FillMode, Op, Symmetry, When};
use yard_core::dock::Motion;
use yard_core::rig::{Contact, Rig};
use yard_core::sim::{Note, Press, Sim};
use yard_core::entity::Bounds;
use yard_core::links::{self, Slot};
use yard_core::pick::{self, Hit};
use yard_core::{mesh, Block, Blueprint, Pos};

/// Above this many blocks the side panel's statistics are not recounted after
/// every edit; they catch up when the design is opened again.
const LIVE_STATS_LIMIT: u64 = 300_000;
/// Most blocks a fill counts while the pointer only hovers.
const FILL_HOVER_LIMIT: usize = 5_000;
/// Most blocks of a fill that are outlined as a hint.
const FILL_OUTLINED: usize = 300;
/// How far off a handle of the turret box a click may be and still grab it,
/// as the tangent of the angle from the eye.
const HANDLE_REACH: f32 = 0.03;
/// The cells of a turret box that always go with the turret: the rail
/// docker's, and the new core's beyond it.
fn turret_fixed_cells(lo: Pos, hi: Pos, base: u8, docker: Option<Pos>) -> (Pos, Pos) {
    let dock = docker_cell(lo, hi, base, docker);
    let d = yard_core::shape::SIDE_DIRS[(base as usize).min(5)];
    (dock, dock.offset(-d[0], -d[1], -d[2]))
}

/// The cells left out of a turret that count: those in the box, less the
/// docker's and the core's, wherever those are now.
fn turret_left_out(excluded: &BTreeSet<Pos>, lo: Pos, hi: Pos, base: u8, docker: Option<Pos>) -> BTreeSet<Pos> {
    let (dock, core) = turret_fixed_cells(lo, hi, base, docker);
    excluded.iter().copied().filter(|p| p.lo(lo) == lo && p.hi(hi) == hi && *p != dock && *p != core).collect()
}

/// The blocks of a turret box reached from `start` through blocks touching
/// at a face, an edge or a corner: the section of the box it is part of,
/// with air (or the edge of the box) all round it.
fn turret_section(entity: &yard_core::entity::Entity, start: Pos, lo: Pos, hi: Pos) -> Vec<Pos> {
    let in_box = |p: Pos| p.lo(lo) == lo && p.hi(hi) == hi;
    if !in_box(start) || entity.get(start).is_air() {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::from([start]);
    let mut waiting = vec![start];
    while let Some(p) = waiting.pop() {
        for d in 0..27 {
            let q = p.offset(d % 3 - 1, d / 3 % 3 - 1, d / 9 - 1);
            if in_box(q) && !entity.get(q).is_air() && seen.insert(q) {
                waiting.push(q);
            }
        }
    }
    seen.into_iter().collect()
}


/// The size of a design in blocks along X, Y and Z (wide, high, long).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dims {
    /// The whole design, docked entities at rest where they dock.
    pub whole: Option<[i32; 3]>,
    /// Each entity by path; an entity without blocks is left out.
    pub entities: std::collections::HashMap<String, [i32; 3]>,
}

/// The core's nose and top as they are (front +Z, top +Y), as sides of
/// [`yard_core::shape::SIDE_DIRS`].
pub const CORE_AS_IS: (usize, usize) = (0, 2);

/// Turret boxes up to this many cells have their blocks marked one by one.
const TURRET_MARKED_CELLS: i64 = 2_000_000;
/// The most block faces marked for a turret box.
const TURRET_MARKED_FACES: usize = 300_000;
/// Largest brush edge, in blocks.
pub const MAX_BRUSH: i32 = 32;

/// What the mouse does on the viewport. Chosen in the build panel or with a
/// key, and kept until another is chosen: nothing else switches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Click a block to make it the selected block. Changes nothing in the design.
    Pick,
    /// Click to give blocks the selected block's material.
    Paint,
    /// Click to place the selected block, right-click to remove, as in the game.
    Build,
    /// Click a block to see what it is linked to, right-click another to link
    /// or unlink the two: the game's C and V keys on the mouse.
    Link,
    /// Click a block to activate it as the game's R key does: press a
    /// button, flip a switch, open a door. Changes nothing in the design.
    Use,
    /// Click a block to choose a new cell for the core; the design moves round it.
    Core,
    /// Click to bevel the outer edges of steps with wedges, right-click to
    /// turn wedges and their kin back into whole blocks.
    Smooth,
    /// Click two blocks to mark the corners of a box, then cut it out of the
    /// entity as a turret docked back onto it.
    Turret,
    /// Copy the docked entity on screen in place of another, or onto a new
    /// rail at a cell clicked on its parent.
    Clone,
}

/// One thing a click can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Place,
    Remove,
    Paint,
    Pick,
    /// Make the block the one whose links are shown and changed (the game's C).
    Select,
    /// Link the block to the selected one, or unlink it (the game's V).
    Connect,
    /// Activate the block (the game's R).
    Use,
    /// Bevel the edges of steps in the brush with wedges.
    Smooth,
    /// Turn wedges, corners, tetras and heptas in the brush back into whole blocks.
    Unsmooth,
    /// Mark a corner of the box for a turret.
    Turret,
    /// Leave the blocks in the brush out of the turret, or take them back in.
    Exclude,
    /// Choose the core's new cell.
    Core,
}

/// What the movement keys move while a turret box is being chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Grab {
    /// Nothing: the keys fly the view.
    #[default]
    Nothing,
    /// A corner of the box: bit 0 set for its +X end, bit 1 its top, bit 2 its front (+Z).
    Corner(u8),
    /// The rail docker, which keeps to the face of the box that rests on the ship.
    Docker,
}

/// A corner of a box, in words.
fn corner_name(c: u8) -> String {
    let y = if c & 2 != 0 { "top" } else { "bottom" };
    let z = if c & 4 != 0 { "front" } else { "back" };
    let x = if c & 1 != 0 { "+X" } else { "-X" };
    format!("{y} {z} {x} corner")
}

/// The cell of a corner of the box `lo`..=`hi`.
fn corner_cell(lo: Pos, hi: Pos, c: u8) -> Pos {
    Pos::new(if c & 1 != 0 { hi.x } else { lo.x }, if c & 2 != 0 { hi.y } else { lo.y }, if c & 4 != 0 { hi.z } else { lo.z })
}

/// Where the rail docker of a turret cut from `lo`..=`hi` goes: the cell
/// chosen, kept on the base face of the box (moved onto it if the box has
/// changed), or the middle of that face.
pub fn docker_cell(lo: Pos, hi: Pos, base: u8, chosen: Option<Pos>) -> Pos {
    let middle = yard_core::turret::default_docker(lo, hi, base as usize).unwrap_or(lo);
    let Some(p) = chosen else { return middle };
    let (p, l, h, m) = (p.to_array(), lo.to_array(), hi.to_array(), middle.to_array());
    let d = yard_core::shape::SIDE_DIRS[(base as usize).min(5)];
    let cell: [i32; 3] = std::array::from_fn(|a| if d[a] != 0 { m[a] } else { p[a].clamp(l[a], h[a]) });
    Pos::new(cell[0], cell[1], cell[2])
}

/// How the Paint tool covers blocks. Chosen in the panel and kept until
/// another is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PaintMode {
    /// The one block that is clicked.
    Single,
    /// A box of blocks, as big as the brush size, dug inward from the face.
    #[default]
    Brush,
    /// Every touching block of the clicked block's material on the surface,
    /// as a paint bucket: the stripe, the wing, the hull plate.
    Fill,
    /// One material for another all over the entity: chosen in the list of
    /// blocks used, swapped for the block chosen in the block list. Each
    /// block keeps its shape and its turn.
    Swap,
}

impl PaintMode {
    pub fn help(self) -> &'static str {
        match self {
            PaintMode::Single => "Paints only the block you click.",
            PaintMode::Brush => "Paints the blocks in the brush (its size and shape are set in the brush panel over the view), from the face you click: the blocks in it with an open face, or every block in it with Inside blocks too ticked.",
            PaintMode::Fill => "Paints the touching blocks of the same material as the one you click, over the whole surface it reaches. Use brush size paints that many blocks deep (the brush size along the face you click); All touching paints every touching block of the material, all the way through.",
            PaintMode::Swap => "Click a block under Blocks in this entity (left) to choose the material to swap out, then choose the new block in the list below (right). Every block of that material in this entity changes (or in the whole design, with the box below ticked), each keeping its shape (cube, wedge, slab...) and its turn.",
        }
    }
}

/// The shape the brush covers inside its box. Chosen in the brush panel and
/// kept until another is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BrushShape {
    #[default]
    Cube,
    /// Round across the face clicked, as deep as the brush goes into it.
    Cylinder,
    /// Round every way: a ball, or an egg when the sizes differ.
    Sphere,
}

impl BrushShape {
    pub const ALL: [(BrushShape, &'static str); 3] = [(BrushShape::Cube, "Cube"), (BrushShape::Cylinder, "Cylinder"), (BrushShape::Sphere, "Sphere")];

    /// Whether cell `p` of the box `lo`..=`hi` is in the shape. A cylinder
    /// stands along `axis` (0 = X, 1 = Y, 2 = Z).
    pub fn holds(self, lo: Pos, hi: Pos, axis: usize, p: Pos) -> bool {
        let (lo, hi, p) = (lo.to_array(), hi.to_array(), p.to_array());
        // How far out the cell's centre is along one axis: 1 at the box's side.
        let out = |a: usize| {
            let (centre, radius) = ((lo[a] + hi[a]) as f32 / 2.0, (hi[a] - lo[a] + 1) as f32 / 2.0);
            ((p[a] as f32 - centre) / radius).powi(2)
        };
        match self {
            BrushShape::Cube => true,
            BrushShape::Cylinder => (0..3).filter(|a| *a != axis).map(out).sum::<f32>() <= 1.0,
            BrushShape::Sphere => (0..3).map(out).sum::<f32>() <= 1.0,
        }
    }

    /// The cells of the shape in the box, as rows along X: boxes to fill.
    pub fn rows(self, lo: Pos, hi: Pos, axis: usize) -> Vec<(Pos, Pos)> {
        if self == BrushShape::Cube {
            return vec![(lo, hi)];
        }
        let mut rows = Vec::new();
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                let mut start = None;
                for x in lo.x..=hi.x + 1 {
                    let inside = x <= hi.x && self.holds(lo, hi, axis, Pos::new(x, y, z));
                    match (inside, start) {
                        (true, None) => start = Some(x),
                        (false, Some(from)) => {
                            rows.push((Pos::new(from, y, z), Pos::new(x - 1, y, z)));
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
        }
        rows
    }
}

/// Whether an act covers the brush's shape rather than its whole box.
fn shaped(act: Act) -> bool {
    matches!(act, Act::Place | Act::Remove | Act::Paint | Act::Smooth | Act::Unsmooth)
}

/// The axis a brush goes into the block along: the one the clicked face looks along.
fn brush_axis(hit: &Hit) -> usize {
    (0..3).find(|a| hit.normal[*a] != 0).unwrap_or(1)
}

/// Draws the outline of the brush's shape in the box `a`..=`b`.
fn outline_brush(overlay: &mut Overlay, a: Pos, b: Pos, shape: BrushShape, axis: usize, colour: [u8; 4]) {
    // A single block is a block whatever the shape.
    if shape == BrushShape::Cube || a == b {
        overlay.wire_box(corner(a, -0.52), corner(b, 0.52), colour);
        return;
    }
    let (lo, hi) = (corner(a, -0.52), corner(b, 0.52));
    let centre = [0, 1, 2].map(|i| (lo[i] + hi[i]) / 2.0);
    let radius = [0, 1, 2].map(|i| (hi[i] - lo[i]) / 2.0);
    // A ring about `normal`, through `at` along it.
    let ring = |overlay: &mut Overlay, normal: usize, at: f32| {
        let (u, v) = ((normal + 1) % 3, (normal + 2) % 3);
        let point = |t: f32| {
            let mut p = centre;
            p[normal] = at;
            p[u] += radius[u] * t.cos();
            p[v] += radius[v] * t.sin();
            p
        };
        const STEPS: usize = 32;
        for k in 0..STEPS {
            let t = |k: usize| k as f32 / STEPS as f32 * std::f32::consts::TAU;
            overlay.line(point(t(k)), point(t(k + 1)), colour);
        }
    };
    match shape {
        BrushShape::Cylinder => {
            ring(overlay, axis, lo[axis]);
            ring(overlay, axis, hi[axis]);
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for (du, dv) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let (mut p, mut q) = (centre, centre);
                for r in [&mut p, &mut q] {
                    r[u] += radius[u] * du;
                    r[v] += radius[v] * dv;
                }
                (p[axis], q[axis]) = (lo[axis], hi[axis]);
                overlay.line(p, q, colour);
            }
        }
        _ => {
            for (normal, at) in centre.into_iter().enumerate() {
                ring(overlay, normal, at);
            }
        }
    }
}

/// Most blocks of a type that are marked at once.
const MARKED: usize = 3_000;
/// The colour of marks on the blocks chosen in the list of blocks used.
const HIGHLIGHT_COLOUR: [u8; 4] = [255, 60, 210, 255];
/// The colour of marks on blocks that should be linked and are not.
const UNLINKED_COLOUR: [u8; 4] = [255, 150, 20, 255];

/// Most blocks one fill paints: the region is cut off here.
pub const FILL_LIMIT: usize = 200_000;

/// The entity path that stands for the whole design: every entity where it
/// is docked. For looking; edits are made in the entities themselves.
pub const WHOLE: &str = "*";

/// What the panel shows about one docked entity.
#[derive(Clone, Debug, PartialEq)]
pub struct PartInfo {
    pub path: String,
    pub blocks: u64,
    pub motion: Motion,
    /// How many blocks it can slide back and ahead on its rail.
    pub track: (i32, i32),
    /// The least and greatest value its pose can take: degrees for what
    /// turns, blocks for what slides. Equal where it cannot move.
    pub range: (f32, f32),
    /// As saved it sits inside other blocks, so nothing stops it moving.
    pub overlapping: bool,
    /// It hangs from a turret axis.
    pub axis: bool,
}

/// A block a player can switch: a button, an activation module, a gate.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchInfo {
    /// Which entity of the design it is in, counted as the simulation counts them.
    pub entity: usize,
    pub path: String,
    pub pos: Pos,
    pub name: String,
    pub on: bool,
}

/// Most switches listed in the panel.
pub const LISTED_SWITCHES: usize = 150;

/// How long the mark of a collision stays on screen, in milliseconds.
const CONTACT_SHOWN_MS: f64 = 900.0;

/// Most blocks one click links: the group of touching blocks is cut off here.
pub const GROUP_LIMIT: usize = 200_000;
/// Most linked controllers listed one by one in the panel.
const LISTED_CONTROLLERS: usize = 40;

/// What the link panel shows about the selected block.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkInfo {
    pub pos: Pos,
    pub name: String,
    /// Whether the game lets this kind of block control anything.
    pub can_control: bool,
    /// What it controls, by block type: type, name, how many, and the colour
    /// those blocks are marked in.
    pub controls: Vec<(u16, String, usize, [u8; 3])>,
    /// The controlled blocks that control something themselves: slaved
    /// computers, logic blocks. Position and name; the first few only.
    pub controlled_controllers: Vec<(Pos, String)>,
    /// How many more of those there are than are listed.
    pub unlisted_controllers: usize,
    /// What controls it: position and name.
    pub controlled_by: Vec<(Pos, String)>,
}

/// What pressing V over a block would do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkStep {
    Link,
    Unlink,
}

impl Tool {
    /// What the mouse buttons do with this tool, as a sentence.
    pub fn help(self, swapped: bool) -> &'static str {
        match (self, swapped) {
            (Tool::Pick, _) => "Click a block to select it",
            (Tool::Paint, _) => "Click to paint with the selected block",
            (Tool::Build, false) => "Click to place the selected block, right-click to remove",
            (Tool::Build, true) => "Click to remove, right-click to place the selected block",
            (Tool::Link, _) => "Click a block to select it (C), right-click another to link or unlink it (V), with Shift for its whole group",
            (Tool::Core, _) => "Click a block (Shift-click the space in front of it) to choose the new place for the core, or use W A S D E Q; Enter confirms, Escape cancels",
            (Tool::Turret, _) => "Click two blocks to mark the corners of the part to cut out as a turret; right-click blocks in the box (as many as the brush takes) to leave them on the ship, or take them back, Shift+right-click for the whole section of touching blocks",
            (Tool::Smooth, _) => "Click to bevel the edges of steps in the brush with wedges, right-click to turn wedges back into whole blocks",
            (Tool::Use, _) => "Click a button, a switch, a door or a light to use it as in the game (R). Nothing in the design changes",
            (Tool::Clone, _) => "Copy the docked entity on screen: choose on the right what it replaces, or where its new rail goes, then click the cell for the rail (Shift-click the space in front of a block); Enter confirms, Escape cancels",
        }
    }

    /// What a click of the left or the right button does. Building follows
    /// the game: left places, right removes, unless the player has the two
    /// swapped as the game's own setting allows. The other tools use the left
    /// button only, so a right-click with them changes nothing.
    pub fn on_click(self, right: bool, swapped: bool) -> Option<Act> {
        match self {
            Tool::Pick => (!right).then_some(Act::Pick),
            Tool::Paint => (!right).then_some(Act::Paint),
            Tool::Build => Some(if right != swapped { Act::Remove } else { Act::Place }),
            Tool::Link => Some(if right { Act::Connect } else { Act::Select }),
            Tool::Use => (!right).then_some(Act::Use),
            Tool::Smooth => Some(if right { Act::Unsmooth } else { Act::Smooth }),
            Tool::Turret => Some(if right { Act::Exclude } else { Act::Turret }),
            Tool::Core => (!right).then_some(Act::Core),
            Tool::Clone => (!right).then_some(Act::Core),
        }
    }
}

/// Everything that is not shown through a signal.
#[derive(Default)]
pub struct Workspace {
    renderer: Option<Renderer>,
    blueprint: Option<Blueprint>,
    /// Which entity of the blueprint is on screen.
    path: String,
    /// A box around the blocks of that entity, for picking. Grows with edits.
    bounds: Option<Bounds>,
    looks: BlockLooks,
    design_id: String,
    rev: u64,
    /// Steps applied here but not yet sent: (entity path, ops).
    outbox: VecDeque<(String, Vec<Op>)>,
    sending: bool,
    /// An undo or redo is on its way; its result has to arrive before more edits.
    awaiting_history: bool,
    /// Bumped whenever a design is opened, so late replies for the previous one
    /// can be recognised and dropped.
    epoch: u32,
    /// Where the pointer is over the viewport, in page coordinates.
    pointer: Option<(i32, i32)>,
    /// Where the pointer last was over the viewport: where the brush is shown
    /// while its size is being changed in the panel.
    last_pointer: Option<(i32, i32)>,
    /// Counts the times the brush was shown, so that only the latest showing
    /// hides it again.
    brush_shows: u32,
    /// What a held mouse button will do when it is let go, if that is not the
    /// tool's left-button action; its outline is shown meanwhile.
    armed: Option<Act>,
    /// What to tell the visitor once the design being loaded is on screen.
    notice: Option<String>,
    /// The entity to show once the design has been loaded again, if it is there.
    show_after: Option<String>,
    /// The entities on screen in the whole-design view, each where it is
    /// docked; empty in any other view.
    rig: Rig,
    /// Where moving parts were last stopped by other parts, and until when
    /// (in milliseconds) each mark is shown.
    contacts: Vec<(Contact, f64)>,
    /// The game's logic and rails at work on the design, once something has
    /// been used; None while the design is as saved.
    sim: Option<Sim>,
    /// Whether the simulation is being run frame by frame right now, and
    /// the time not yet used up in whole ticks.
    sim_running: bool,
    sim_slack: f32,
    /// Where the design's switches are: (entity, position). Found once per
    /// design and again after an edit.
    switches: Option<Vec<(usize, Pos)>>,
    /// An old version from the sync history is on screen, not the design as
    /// it is now. Nothing may be edited: the server knows nothing of this copy.
    preview: bool,
    /// The design as it is, while a copy of it with the core moved is on
    /// screen (the Move core tool's preview); and whether the design changed
    /// elsewhere meanwhile, so that it must be loaded again afterwards.
    stashed: Option<Blueprint>,
    stale: bool,
    /// Where the view looked before the preview took it to the whole design.
    camera_kept: Option<crate::camera::Camera>,
    /// How many blocks of each type the entity on screen has (the whole
    /// design in the whole-design view), kept up to date with every edit.
    counts: std::collections::HashMap<u16, i64>,
    /// The cells each entity's blocks span (by path; `None` when it has no
    /// blocks), measured once and kept up to date with edits.
    sizes: std::collections::HashMap<String, Option<Bounds>>,
}

/// A rail being moved with the Move core tool instead of the core.
#[derive(Clone, Debug, PartialEq)]
pub struct RailMove {
    /// The docked entity whose rail it is.
    pub child: String,
    /// That entity is the one on screen: the rail is the one it rests on,
    /// a block of its parent, and the cell chosen is where its docker goes.
    pub own: bool,
    /// Whether the docked entity comes along with the rail.
    pub carried: bool,
}

/// A change of structure (a core move, a rail move, a copy) shown before it is made.
#[derive(Clone, Debug, PartialEq)]
pub struct CorePreview {
    /// The entity being worked on, to come back to.
    pub path: String,
    /// The part of the whole-design view the camera frames: the entity
    /// changed, or the copy made.
    pub part: String,
    /// What is shown, for the banner: "the core of docked 1 moved".
    pub about: String,
    /// What the change would do, in words.
    pub notes: Vec<String>,
}

/// A docked entity being copied with the Clone tool.
#[derive(Clone, Debug, PartialEq)]
pub struct Cloning {
    /// The docked entity copied.
    pub source: String,
    /// The entity the copy is docked to: the replaced entity's parent, or
    /// the entity on screen whose cell is being chosen for the new rail.
    pub parent: String,
    /// The docked entity the copy takes the place of; None for a new rail
    /// at the cell chosen ([`Ctx::relocating`]).
    pub replace: Option<String>,
    /// For a new rail: its arrow side, the way the copy rests.
    pub facing: [i32; 3],
}

/// The marker for a cell being chosen: the cell, an arrow from it (the
/// nose and top sides it points along, and whether to show the way the
/// nose points now beside it), and the cell left behind (an orange box).
pub struct Marker {
    pub at: Pos,
    pub arrow: Option<(usize, usize, bool)>,
    pub from: Option<Pos>,
}

/// What a rail block is called, by type.
pub fn rail_kind(ty: u16) -> &'static str {
    if ty == yard_core::dock::TURRET_AXIS {
        "turret axis"
    } else if yard_core::dock::ROTATORS.contains(&ty) {
        "rotator"
    } else {
        "rail"
    }
}

/// The name of a side, for a step along one axis.
pub fn side_name(d: [i32; 3]) -> &'static str {
    match d {
        [0, 0, 1] => "front (+Z)",
        [0, 0, -1] => "back (-Z)",
        [0, 1, 0] => "top (+Y)",
        [0, -1, 0] => "bottom (-Y)",
        [1, 0, 0] => "+X",
        [-1, 0, 0] => "-X",
        _ => "?",
    }
}

/// Handles to all editor state. Cheap to copy into every closure.
#[derive(Clone, Copy)]
pub struct Ctx {
    ws: StoredValue<Workspace, LocalStorage>,
    pub designs: RwSignal<Vec<Summary>>,
    pub current: RwSignal<Option<Summary>>,
    pub entity_path: RwSignal<String>,
    /// (text, is an error)
    pub status: RwSignal<(String, bool)>,
    pub catalog: RwSignal<Vec<ClientBlock>>,
    pub tool: RwSignal<Tool>,
    pub block_id: RwSignal<u16>,
    /// Rotation of the block being placed. For wedges and other shapes it is the
    /// orientation value itself. For slabs and blocks with a front, 0 means
    /// "face away from the surface clicked" and 1..=6 pick a direction.
    pub turn: RwSignal<u8>,
    /// Brush size along X, Y and Z.
    pub brush: RwSignal<[i32; 3]>,
    /// The shape the brush covers in its box.
    pub brush_shape: RwSignal<BrushShape>,
    /// How the Paint tool covers blocks.
    pub paint_mode: RwSignal<PaintMode>,
    /// How tall a step the Smooth tool bevels the edge of, in blocks.
    pub smooth_step: RwSignal<u8>,
    /// Whether smoothing works on rooms sealed inside the hull and the inner
    /// face of thin hulls too.
    pub smooth_inside: RwSignal<bool>,
    /// A slope that ends against a whole block is carried on through it.
    pub smooth_sharpen: RwSignal<bool>,
    /// Whether the Paint tool's brush paints the blocks inside a solid part
    /// too, not only those on its surface (with an open face).
    pub paint_through: RwSignal<bool>,
    /// Whether the Fill can paints every touching block of the material, all
    /// the way through.
    pub fill_all: RwSignal<bool>,
    /// Whether the Fill can goes as deep below the surface as the brush size
    /// along the face clicked, rather than the surface only.
    pub fill_brush_depth: RwSignal<bool>,
    /// Whether the Paint tool's brush leaves every block that is not armour
    /// or a light alone: systems, logic, doors and the like.
    pub paint_armor_only: RwSignal<bool>,
    /// With the Paint tool's Swap mode: the block type whose material is to be swapped out.
    pub swap_from: RwSignal<Option<u16>>,
    /// Whether a swap goes through every entity of the design (the ship and
    /// everything docked to it), not only the one on screen.
    pub swap_whole: RwSignal<bool>,
    /// The block picked with the Pick tool: the rotation arrows turn it, and
    /// choosing another block puts that one in its place.
    pub picked: RwSignal<Option<Pos>>,
    /// The kind of block marked through the design (chosen in the list of
    /// blocks used), so that something hidden, the core above all, can
    /// be found.
    pub highlight: RwSignal<Option<u16>>,
    /// The cell the core would move to while the player is choosing it.
    pub relocating: RwSignal<Option<Pos>>,
    /// The sides (numbered as [`yard_core::shape::SIDE_DIRS`]) the core's
    /// nose and top are to point to: the entity turns round the core to
    /// match. Front and top, as it is, unless the player chooses otherwise.
    pub core_facing: RwSignal<(usize, usize)>,
    /// Whether, on a turret, the dock goes with the core: the rail docker,
    /// the axis it rests on and the axes of the turrets docked to it shift by
    /// as much, so the turret keeps its place and turns about a new line.
    pub dock_follows: RwSignal<bool>,
    /// While the core's new place is being chosen on a turret: how many
    /// entities hang from its turret axes. None on an entity that is not a
    /// turret, whose dock cannot follow.
    pub turret_parts: RwSignal<Option<usize>>,
    /// The entity on screen hangs from a turret axis but its docker faces
    /// front or back on it, so the game holds it still.
    pub turret_fixed: RwSignal<bool>,
    /// The core move being previewed: a copy of the design with the core
    /// moved is on screen, in the whole-design view.
    pub core_preview: RwSignal<Option<CorePreview>>,
    /// The core moves an auto-fit found, by entity, in order: Confirm makes
    /// them all. Empty until Auto-fit, and again once a cell is chosen by hand.
    pub fit_steps: RwSignal<Vec<(String, Pos)>>,
    /// With the Clone tool: the docked entity being copied and where to.
    pub cloning: RwSignal<Option<Cloning>>,
    /// With the Clone tool: the docked entity chosen in the panel for the
    /// copy to replace (its path, or empty).
    pub clone_target: RwSignal<String>,
    /// With the Move core tool: a rail being moved instead of the core.
    pub rail_move: RwSignal<Option<RailMove>>,
    /// The rails the Move core tool can move from the entity on screen:
    /// (docked entity, whether that is the entity on screen, label).
    pub rails_to_move: RwSignal<Vec<(String, bool, String)>>,
    /// The two corners of the box being chosen for a turret; the second
    /// once it has been clicked.
    pub turret_box: RwSignal<Option<(Pos, Option<Pos>)>>,
    /// Which side of that box rests on the entity (the game's numbering).
    pub turret_base: RwSignal<u8>,
    /// What the movement keys move while the box is being chosen.
    pub turret_grab: RwSignal<Grab>,
    /// The cell the player moved the rail docker to; the middle of the base face if none.
    pub turret_docker: RwSignal<Option<Pos>>,
    /// The block types the entity on screen uses most: (type, name, count).
    pub used_blocks: RwSignal<Vec<(u16, String, u64)>>,
    /// How big the open design is, in blocks along X, Y and Z: as a whole
    /// (docked entities at rest in their places) and each entity by path.
    pub dims: RwSignal<Dims>,
    /// Cells of the turret box whose blocks stay on the ship.
    pub turret_excluded: RwSignal<BTreeSet<Pos>>,
    /// What the last change of structure was, while it can still be put back.
    pub structure_undo: RwSignal<Option<String>>,
    /// Whether the dialog that asks to delete the open design is up.
    pub deleting: RwSignal<bool>,
    /// The new name being typed for the open design, while renaming it.
    pub renaming: RwSignal<Option<String>>,
    pub symmetry: RwSignal<Symmetry>,
    /// What is under the pointer, in words.
    pub hover: RwSignal<String>,
    /// Whether the brush is being shown in the view because its size or
    /// shape was just changed.
    pub brush_shown: RwSignal<bool>,
    /// (undo steps, redo steps) of the entity on screen, once the server has said.
    pub history: RwSignal<Option<(usize, usize)>>,
    /// Steps the server has not confirmed yet.
    pub pending: RwSignal<usize>,
    /// Left removes and right places, for players who have the game set that way.
    pub swap: RwSignal<bool>,
    /// Whether a double-click in the view turns on mouse look.
    pub mouse_look: RwSignal<bool>,
    /// Mouse look on: the pointer is captured and turns the view.
    pub looking: RwSignal<bool>,
    /// The game server blueprints are exchanged with, as last heard of.
    pub game: RwSignal<SyncInfo>,
    /// The block whose links are shown, chosen with the Link tool or the C key.
    pub selected: RwSignal<Option<Pos>>,
    /// Whether blocks that should be linked and are not are marked: modules
    /// no computer runs, computers that run none of their modules.
    pub show_unlinked: RwSignal<bool>,
    /// What that block is linked to, for the panel.
    pub link_info: RwSignal<Option<LinkInfo>>,
    /// The synced versions kept of the open design, newest first.
    pub versions: RwSignal<Vec<SyncVersion>>,
    /// The old version on screen, while one is being looked at.
    pub viewing: RwSignal<Option<SyncVersion>>,
    /// Whether the list of kept versions is unfolded.
    pub versions_open: RwSignal<bool>,
    /// The open design's link to a design in a game shipyard.
    pub yard: RwSignal<Option<YardLinkInfo>>,
    /// The game's shipyards, while one is being picked to link to.
    pub yards: RwSignal<Option<Vec<GameYard>>>,
    /// The docked entities of the whole-design view, for its panel.
    pub parts: RwSignal<Vec<PartInfo>>,
    /// How far each docked entity has been moved from how it was saved, by
    /// path: degrees for what turns, blocks for what slides.
    pub poses: RwSignal<std::collections::BTreeMap<String, f32>>,
    /// Whether the docked entities are being shown in motion.
    pub playing: RwSignal<bool>,
    /// Whether the motion starts over when every part has been through its
    /// own, or stops there.
    pub looping: RwSignal<bool>,
    /// Docked entities the whole-design view could not place.
    pub unplaced: RwSignal<usize>,
    /// Whether the game's logic and rails are being simulated: something
    /// has been used and the design is not as it was saved.
    pub simulating: RwSignal<bool>,
    /// The switches of what is on screen, for the panel, and how many
    /// there are in all.
    pub switches: RwSignal<(Vec<SwitchInfo>, usize)>,
    /// Where the movement keys are pushing the view: right, up and forward,
    /// each -1, 0 or 1, and whether the fast key is held.
    motion: StoredValue<([f32; 3], bool)>,
    /// Whether the view is being moved frame by frame right now.
    flying: StoredValue<bool>,
}

/// Where the choice of swapped mouse buttons is kept between visits.
const SWAP_KEY: &str = "yard.swap_buttons";

/// Where the choice of mouse look on double-click is kept.
const MOUSE_LOOK_KEY: &str = "yard.mouse_look";

fn stored_flag(key: &str) -> bool {
    let storage = window().local_storage().ok().flatten();
    storage.and_then(|s| s.get_item(key).ok().flatten()).is_some_and(|v| v == "1")
}

fn store_flag(key: &str, on: bool) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(key, if on { "1" } else { "0" });
    }
}

fn sentence(e: impl std::fmt::Display) -> String {
    let text = e.to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => text,
    }
}

/// The box round a part of the whole-design view where it is placed, as
/// the corners of its cells span it.
fn placed_box(part: &yard_core::rig::Part) -> Option<([f32; 3], [f32; 3])> {
    let b = part.bounds?;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for i in 0..8 {
        let c = [if i & 1 == 0 { b.min.x as f32 - 0.5 } else { b.max.x as f32 + 0.5 }, if i & 2 == 0 { b.min.y as f32 - 0.5 } else { b.max.y as f32 + 0.5 }, if i & 4 == 0 { b.min.z as f32 - 0.5 } else { b.max.z as f32 + 0.5 }];
        let at = part.placed.at(c);
        for a in 0..3 {
            lo[a] = lo[a].min(at[a]);
            hi[a] = hi[a].max(at[a]);
        }
    }
    Some((lo, hi))
}

fn corner(p: Pos, half: f32) -> [f32; 3] {
    [p.x as f32 + half, p.y as f32 + half, p.z as f32 + half]
}

/// "a Cannon Computer", "an Activation Module".
fn with_article(name: &str) -> String {
    let vowel = name.chars().next().is_some_and(|c| "aeiouAEIOU".contains(c));
    format!("{} {name}", if vowel { "an" } else { "a" })
}

/// Colours of the link marks. The first five are the game's own, a little
/// brighter to stand out against a dark view: the selected block, what it
/// controls, and among that a weapon computer's second weapon, effect and
/// light. The game does not show what controls the selected block; here that
/// is pink.
pub const SELECTED_COLOUR: [u8; 3] = [240, 160, 50];
pub const CONTROLS_COLOUR: [u8; 3] = [150, 95, 255];
pub const SLAVE_COLOUR: [u8; 3] = [50, 150, 235];
pub const EFFECT_COLOUR: [u8; 3] = [50, 230, 60];
pub const LIGHT_COLOUR: [u8; 3] = [170, 235, 70];
pub const CONTROLLED_BY_COLOUR: [u8; 3] = [255, 90, 200];

impl Ctx {
    pub fn new() -> Self {
        Self {
            ws: StoredValue::new_local(Workspace::default()),
            designs: RwSignal::new(Vec::new()),
            current: RwSignal::new(None),
            entity_path: RwSignal::new(String::new()),
            status: RwSignal::new((String::new(), false)),
            catalog: RwSignal::new(Vec::new()),
            tool: RwSignal::new(Tool::Pick),
            block_id: RwSignal::new(0),
            turn: RwSignal::new(0),
            brush: RwSignal::new([1; 3]),
            brush_shape: RwSignal::new(BrushShape::default()),
            paint_mode: RwSignal::new(PaintMode::default()),
            smooth_step: RwSignal::new(1),
            smooth_inside: RwSignal::new(true),
            smooth_sharpen: RwSignal::new(false),
            paint_through: RwSignal::new(false),
            fill_all: RwSignal::new(false),
            fill_brush_depth: RwSignal::new(false),
            paint_armor_only: RwSignal::new(false),
            swap_from: RwSignal::new(None),
            swap_whole: RwSignal::new(false),
            picked: RwSignal::new(None),
            highlight: RwSignal::new(None),
            relocating: RwSignal::new(None),
            core_facing: RwSignal::new(CORE_AS_IS),
            dock_follows: RwSignal::new(true),
            turret_parts: RwSignal::new(None),
            turret_fixed: RwSignal::new(false),
            core_preview: RwSignal::new(None),
            fit_steps: RwSignal::new(Vec::new()),
            cloning: RwSignal::new(None),
            clone_target: RwSignal::new(String::new()),
            rail_move: RwSignal::new(None),
            rails_to_move: RwSignal::new(Vec::new()),
            turret_box: RwSignal::new(None),
            turret_base: RwSignal::new(3),
            turret_grab: RwSignal::new(Grab::Nothing),
            turret_docker: RwSignal::new(None),
            turret_excluded: RwSignal::new(BTreeSet::new()),
            used_blocks: RwSignal::new(Vec::new()),
            dims: RwSignal::new(Dims::default()),
            structure_undo: RwSignal::new(None),
            deleting: RwSignal::new(false),
            renaming: RwSignal::new(None),
            symmetry: RwSignal::new(Symmetry::default()),
            hover: RwSignal::new(String::new()),
            brush_shown: RwSignal::new(false),
            history: RwSignal::new(None),
            pending: RwSignal::new(0),
            swap: RwSignal::new(stored_flag(SWAP_KEY)),
            mouse_look: RwSignal::new(stored_flag(MOUSE_LOOK_KEY)),
            looking: RwSignal::new(false),
            game: RwSignal::new(SyncInfo::default()),
            selected: RwSignal::new(None),
            show_unlinked: RwSignal::new(false),
            link_info: RwSignal::new(None),
            versions: RwSignal::new(Vec::new()),
            viewing: RwSignal::new(None),
            versions_open: RwSignal::new(false),
            yard: RwSignal::new(None),
            yards: RwSignal::new(None),
            parts: RwSignal::new(Vec::new()),
            poses: RwSignal::new(Default::default()),
            playing: RwSignal::new(false),
            looping: RwSignal::new(true),
            unplaced: RwSignal::new(0),
            simulating: RwSignal::new(false),
            switches: RwSignal::new((Vec::new(), 0)),
            motion: StoredValue::new(([0.0; 3], false)),
            flying: StoredValue::new(false),
        }
    }

    pub fn set_swap(self, swapped: bool) {
        store_flag(SWAP_KEY, swapped);
        self.swap.set(swapped);
    }

    pub fn set_mouse_look(self, on: bool) {
        store_flag(MOUSE_LOOK_KEY, on);
        self.mouse_look.set(on);
        if !on {
            self.stop_looking();
        }
    }

    /// Lets the pointer go, ending mouse look.
    pub fn stop_looking(self) {
        if self.looking.get_untracked() {
            document().exit_pointer_lock();
        }
    }

    /// What a click of the left or right button does right now.
    pub fn on_click(self, right: bool) -> Option<Act> {
        self.tool.get_untracked().on_click(right, self.swap.get_untracked())
    }

    /// Says which way the movement keys push the view, and sets it moving if
    /// it was still. All zeroes stops it.
    pub fn steer(self, along: [f32; 3], fast: bool) {
        self.motion.set_value((along, fast));
        if along != [0.0; 3] && !self.flying.get_value() {
            self.flying.set_value(true);
            self.fly_on(js_sys::Date::now());
        }
    }

    /// One frame of movement, then the next, for as long as a key is held.
    fn fly_on(self, last: f64) {
        request_animation_frame(move || {
            let (along, fast) = self.motion.get_value();
            if along == [0.0; 3] {
                self.flying.set_value(false);
                return;
            }
            let now = js_sys::Date::now();
            // A tab that was in the background does not leap when it returns.
            let seconds = ((now - last) / 1000.0).clamp(0.0, 0.1) as f32;
            self.ws.update_value(|w| {
                if let Some(r) = &mut w.renderer {
                    r.camera.fly(along, seconds, fast);
                }
            });
            self.refresh_overlay();
            self.redraw();
            self.fly_on(now);
        });
    }

    pub fn say(self, text: impl Into<String>) {
        self.status.set((text.into(), false));
    }

    pub fn fail(self, text: impl Into<String>) {
        self.status.set((text.into(), true));
    }

    pub fn start(self, canvas: HtmlCanvasElement) -> Result<(), String> {
        let renderer = Renderer::new(canvas.clone())?;
        renderer.draw();
        self.ws.update_value(|w| w.renderer = Some(renderer));
        // Panels opening and the window resizing change the canvas's size; the
        // picture has to be redrawn to fit or it is shown stretched.
        let on_resize = Closure::<dyn Fn()>::new(move || self.redraw());
        if let Ok(observer) = web_sys::ResizeObserver::new(on_resize.as_ref().unchecked_ref()) {
            observer.observe(&canvas);
            // Both live as long as the page does.
            std::mem::forget(observer);
        }
        on_resize.forget();
        Ok(())
    }

    /// Fetches the game's texture sheets; blocks switch from flat colours to
    /// textures once all of them are in. If one is missing they stay flat.
    fn load_textures(self, pages: u32) {
        let Some(loader) = self.ws.try_update_value(|w| w.renderer.as_mut().and_then(|r| r.start_atlas(pages))).flatten() else {
            return;
        };
        let arrived = Rc::new(Cell::new(0u32));
        for page in 0..pages {
            let Ok(image) = web_sys::HtmlImageElement::new() else { return };
            let (loader, arrived, loaded) = (loader.clone(), arrived.clone(), image.clone());
            let on_load = Closure::<dyn Fn()>::new(move || {
                if !loader.put(page, &loaded) {
                    return;
                }
                arrived.set(arrived.get() + 1);
                if arrived.get() == pages {
                    loader.finish();
                    self.ws.update_value(|w| w.renderer.iter_mut().for_each(|r| r.atlas_ready(pages)));
                    self.redraw();
                }
            });
            image.set_onload(Some(on_load.as_ref().unchecked_ref()));
            on_load.forget();
            image.set_src(&format!("/api/textures/{page}"));
        }
    }

    pub fn load_catalog(self, blocks: Vec<ClientBlock>, models: yard_core::model::ModelSet) {
        let pages = BlockLooks::texture_pages(&blocks, &models);
        self.ws.update_value(|w| {
            w.looks.load(&blocks);
            w.looks.load_models(models);
        });
        self.load_textures(pages);
        // Start with plain grey hull selected if the game has it, else anything placeable.
        let first = blocks
            .iter()
            .find(|b| b.name == "Grey Basic Armor" && b.placeable)
            .or_else(|| blocks.iter().find(|b| b.placeable && b.base == 0 && b.id != edit::CORE_TYPE))
            .map_or(0, |b| b.id);
        self.block_id.set(first);
        self.catalog.set(blocks);
    }

    pub fn redraw(self) {
        self.ws.with_value(|w| w.renderer.iter().for_each(Renderer::draw));
    }

    pub fn refresh_designs(self) {
        spawn_local(async move {
            match api::designs().await {
                Ok(list) => self.designs.set(list),
                Err(e) => self.fail(e),
            }
        });
    }

    /// Fetches the design list and the game server's state again, quietly:
    /// blueprints saved in the game turn up here by themselves, so the page
    /// asks every few seconds. A failed round just waits for the next.
    pub fn poll(self) {
        spawn_local(async move {
            if let Ok(info) = api::sync_info().await {
                if self.game.get_untracked() != info {
                    self.game.set(info);
                }
            }
            if let Ok(list) = api::designs().await {
                // A newer version from the game, or a new name: load it,
                // unless edits made here are still on their way.
                let replaced = self.current.with_untracked(|current| {
                    let current = current.as_ref()?;
                    let fresh = list.iter().find(|d| d.id == current.id)?;
                    (fresh.epoch != current.epoch).then(|| current.id.clone())
                });
                if self.designs.with_untracked(|old| *old != list) {
                    self.designs.set(list);
                }
                // Not while an old version is being looked at: going back to
                // the current one loads whatever is newest then.
                let viewing = self.viewing.with_untracked(Option::is_some);
                if self.core_preview.with_untracked(Option::is_some) {
                    // Nor while a core move is previewed: it is loaded once that is left.
                    if replaced.is_some() {
                        self.ws.update_value(|w| w.stale = true);
                    }
                } else if let (Some(id), 0, false) = (replaced, self.pending.get_untracked(), viewing) {
                    self.open_design_saying(id, "This design changed on the game server. The new version was loaded.");
                }
            }
            self.load_versions();
            self.load_yard();
        });
    }

    /// Where a design stands with the game server, from the design list.
    pub fn sync_of(self, id: &str) -> Option<SyncStatus> {
        self.designs.with(|list| list.iter().find(|d| d.id == id).and_then(|d| d.sync))
    }

    /// Puts a design into the exchange with the game server or takes it out.
    /// Gives the open design the name being typed, and loads it again: the
    /// name is part of its file.
    pub fn rename_design(self) {
        let (Some(summary), Some(name)) = (self.current.get_untracked(), self.renaming.get_untracked()) else { return };
        if name.trim() == summary.name {
            return self.renaming.set(None);
        }
        if self.pending.get_untracked() > 0 {
            return self.say("Still saving the last edit; try again in a moment.");
        }
        let in_game = matches!(summary.sync, Some(SyncStatus::Synced | SyncStatus::Waiting));
        self.say("Renaming...");
        spawn_local(async move {
            match api::rename_design(&summary.id, &name).await {
                Ok(renamed) => {
                    self.renaming.set(None);
                    self.designs.update(|list| {
                        if let Some(entry) = list.iter_mut().find(|d| d.id == renamed.id) {
                            *entry = renamed.clone();
                        }
                    });
                    let game = if in_game {
                        format!(" A blueprint the game has under the old name stays there; this design goes to the game as \"{}\".", renamed.name)
                    } else {
                        String::new()
                    };
                    self.open_design_saying(renamed.id.clone(), &format!("Renamed \"{}\".{game}", renamed.name));
                }
                Err(e) => self.fail(e),
            }
        });
    }

    pub fn switch_sync(self, id: String, on: bool) {
        spawn_local(async move {
            match api::switch_sync(&id, on).await {
                Ok(summary) => self.designs.update(|list| {
                    if let Some(entry) = list.iter_mut().find(|d| d.id == summary.id) {
                        entry.sync = summary.sync;
                    }
                }),
                Err(e) => self.fail(e),
            }
        });
    }

    /// Sends a design to the game server at its next call, without waiting
    /// for it to be left alone.
    pub fn sync_now(self, id: String) {
        let connected = self.game.with_untracked(|g| g.seconds_ago.is_some_and(|s| s <= 45));
        spawn_local(async move {
            match api::sync_now(&id).await {
                Ok(_) if connected => self.say("Sending to the game; it takes the design at its next call, within a few seconds."),
                Ok(_) => self.say("The design goes to the game as soon as the game server calls in; it is not connected now."),
                Err(e) => self.fail(e),
            }
        });
    }

    /// Fetches the open design's link to a shipyard design, and the game's
    /// shipyards while one is being picked, quietly.
    pub fn load_yard(self) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else {
            return;
        };
        let picking = self.yards.with_untracked(Option::is_some);
        spawn_local(async move {
            let Ok(link) = api::yard_link(&id).await else { return };
            let still_open = self.current.with_untracked(|c| c.as_ref().is_some_and(|s| s.id == id));
            if !still_open {
                return;
            }
            if self.yard.with_untracked(|old| *old != link) {
                self.yard.set(link);
            }
            if picking {
                if let Ok(list) = api::game_yards().await {
                    if self.yards.with_untracked(|old| old.as_ref() != Some(&list)) && self.yards.with_untracked(Option::is_some) {
                        self.yards.set(Some(list));
                    }
                }
            }
        });
    }

    /// Opens or closes the list of the game's shipyards to link the open design to.
    pub fn pick_yard(self, open: bool) {
        if !open {
            return self.yards.set(None);
        }
        self.yards.set(Some(Vec::new()));
        self.load_yard();
    }

    /// Links the open design to a design in a shipyard, or to a new one.
    pub fn link_yard(self, yard: String, uid: Option<String>, keep: YardKeep) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        spawn_local(async move {
            match api::link_yard(&id, &YardLinkRequest { yard, uid, keep }).await {
                Ok(link) => {
                    self.yards.set(None);
                    self.yard.set(link);
                    self.say("Linked. Changes move while the design is loaded in its shipyard.");
                }
                Err(e) => self.fail(e),
            }
        });
    }

    pub fn unlink_yard(self) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        spawn_local(async move {
            match api::unlink_yard(&id).await {
                Ok(()) => {
                    self.yard.set(None);
                    self.say("No longer linked to the shipyard design. Both keep what they hold.");
                }
                Err(e) => self.fail(e),
            }
        });
    }

    /// Fetches the sync history of the open design, quietly.
    pub fn load_versions(self) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else {
            return;
        };
        spawn_local(async move {
            let Ok(list) = api::sync_history(&id).await else { return };
            // Another design may have been opened while the answer was on its way.
            let still_open = self.current.with_untracked(|c| c.as_ref().is_some_and(|s| s.id == id));
            if still_open && self.versions.with_untracked(|old| *old != list) {
                self.versions.set(list);
            }
        });
    }

    /// Puts an old version of the open design on screen, to look at. It
    /// cannot be edited; [`Ctx::open_design`] brings the current one back.
    pub fn view_version(self, version: SyncVersion) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        self.leave_core_preview();
        if self.pending.get_untracked() > 0 {
            self.say("Still saving the last edit; try again in a moment.");
            return;
        }
        self.say("Loading...");
        let epoch = self.ws.try_update_value(|w| {
            w.epoch += 1;
            w.epoch
        });
        spawn_local(async move {
            let loaded = match api::version_file(&id, version.seq).await {
                Ok(bytes) => Blueprint::from_sment(&bytes).map_err(|e| format!("Could not read that version: {e}")),
                Err(e) => Err(e),
            };
            if self.ws.with_value(|w| Some(w.epoch) != epoch) {
                return; // Something else was opened meanwhile.
            }
            match loaded {
                Ok(blueprint) => {
                    let path = self.ws.try_update_value(|w| {
                        let path = if w.path == WHOLE || blueprint.entity(&w.path).is_some() { w.path.clone() } else { String::new() };
                        w.blueprint = Some(blueprint);
                        (w.sim, w.rig, w.switches) = (None, Rig::default(), None);
                        w.sizes.clear();
                        w.preview = true;
                        path
                    });
                    self.viewing.set(Some(version));
                    self.simulating.set(false);
                    self.history.set(None);
                    self.say("");
                    self.show_entity(path.unwrap_or_default(), false);
                }
                Err(e) => self.fail(e),
            }
        });
    }

    /// Leaves the old version and shows the design as it is now.
    pub fn back_to_current(self) {
        if let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) {
            self.open_design(id, false);
        }
    }

    /// Makes an old version the design's contents again. From there it goes
    /// to the game like any other change.
    pub fn restore_version(self, seq: u64) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        spawn_local(async move {
            match api::restore_version(&id, seq).await {
                Ok(summary) => {
                    self.refresh_designs();
                    let text = match summary.sync {
                        Some(SyncStatus::Waiting) => "That version is the design again. It goes to the game server at the next exchange.",
                        _ => "That version is the design again.",
                    };
                    self.open_design_saying(id, text);
                }
                Err(e) => self.fail(e),
            }
        });
    }

    /// Stores an old version as a design of its own and opens it.
    pub fn copy_version(self, seq: u64) {
        let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        spawn_local(async move {
            match api::copy_version(&id, seq).await {
                Ok(summary) => {
                    self.refresh_designs();
                    self.open_design(summary.id, true);
                }
                Err(e) => self.fail(e),
            }
        });
    }

    /// Loads the open design again, keeping the view, and says `text` once it
    /// is on screen.
    /// Opens a design and says something once it is on screen.
    pub fn open_design_noting(self, id: String, text: String) {
        self.open_design(id, true);
        self.ws.update_value(|w| w.notice = Some(text));
    }

    fn open_design_saying(self, id: String, text: &str) {
        self.open_design(id, false);
        self.ws.update_value(|w| w.notice = Some(text.to_string()));
    }

    /// Loads a design from the server and shows it. `frame` also points the
    /// camera at it; a reload after falling out of step keeps the view.
    pub fn open_design(self, id: String, frame: bool) {
        self.say("Loading...");
        self.highlight.set(None);
        self.swap_from.set(None);
        if self.current.with_untracked(|c| c.as_ref().map(|s| s.id != id).unwrap_or(true)) {
            self.picked.set(None);
        }
        let epoch = self.ws.try_update_value(|w| {
            w.notice = None;
            w.epoch += 1;
            w.outbox.clear();
            w.sending = false;
            w.awaiting_history = false;
            // Whatever was previewed is left behind with the copy.
            w.stashed = None;
            w.stale = false;
            w.epoch
        });
        self.core_preview.set(None);
        self.pending.set(0);
        spawn_local(async move {
            let loaded = async {
                // The file first: asking for it makes the server save the design,
                // so the summary asked for after it describes the same state.
                let (bytes, rev) = api::design_file(&id).await?;
                let summary = api::design(&id).await?;
                let blueprint = Blueprint::from_sment(&bytes).map_err(|e| format!("Could not read the design: {e}"))?;
                Ok::<_, String>((summary, blueprint, rev))
            };
            let result = loaded.await;
            if self.ws.with_value(|w| Some(w.epoch) != epoch) {
                return; // Another design was opened meanwhile.
            }
            match result {
                Ok((summary, blueprint, rev)) => {
                    let other_design = self.ws.with_value(|w| w.design_id != summary.id);
                    if other_design {
                        self.versions.set(Vec::new());
                        self.yard.set(None);
                        self.yards.set(None);
                        // Poses are kept by entity path, which means another
                        // part in another design.
                        self.playing.set(false);
                        self.poses.set(Default::default());
                    }
                    self.viewing.set(None);
                    self.simulating.set(false);
                    let keep_path = self.ws.try_update_value(|w| {
                        if let Some(wanted) = w.show_after.take().filter(|p| blueprint.entity(p).is_some()) {
                            w.preview = false;
                            w.blueprint = Some(blueprint);
                            (w.sim, w.rig, w.switches) = (None, Rig::default(), None);
                        w.sizes.clear();
                            w.design_id = summary.id.clone();
                            w.rev = rev;
                            return wanted;
                        }
                        let same = w.design_id == summary.id && (w.path == WHOLE || blueprint.entity(&w.path).is_some());
                        w.preview = false;
                        w.blueprint = Some(blueprint);
                        (w.sim, w.rig, w.switches) = (None, Rig::default(), None);
                        w.sizes.clear();
                        w.design_id = summary.id.clone();
                        w.rev = rev;
                        if same { w.path.clone() } else { String::new() }
                    });
                    let address = format!("/d/{}", summary.id);
                    let _ = window().history().and_then(|h| h.replace_state_with_url(&JsValue::NULL, "", Some(&address)));
                    self.say(self.ws.try_update_value(|w| w.notice.take()).flatten().unwrap_or_default());
                    self.current.set(Some(summary));
                    self.show_entity(keep_path.unwrap_or_default(), frame);
                    // The block may be gone, or linked differently, in what was loaded.
                    self.refresh_links();
                    self.load_versions();
                    self.load_yard();
                }
                Err(e) => self.fail(e),
            }
        });
    }

    /// Puts the whole design on screen: every entity where it is docked.
    fn show_whole(self, frame: bool) {
        let poses = self.poses.get_untracked();
        let info = self.ws.try_update_value(|w| {
            let Workspace { renderer: Some(renderer), blueprint: Some(bp), looks, bounds, path: shown, rig, contacts, sim, .. } = w else { return (Vec::new(), 0) };
            let rules = looks.rules(0);
            renderer.clear_chunks();
            contacts.clear();
            // While the game's rails are being simulated the parts stay
            // where those have them.
            let fresh = sim.is_none();
            if fresh {
                *rig = Rig::new(bp, |ty| rules.is_track(ty), looks);
            }
            let mut info = Vec::new();
            for (group, part) in rig.parts.iter_mut().enumerate() {
                let Some(e) = bp.entity(&part.path) else { continue };
                for (key, _) in e.entity.segments() {
                    renderer.set_chunk(group, *key, &mesh::build_segment(&e.entity, *key, looks));
                }
                if part.dock.is_some() {
                    info.push(PartInfo {
                        path: part.path.clone(),
                        blocks: e.entity.block_count(),
                        motion: part.motion(),
                        track: (part.track.back.len() as i32, part.track.ahead.len() as i32),
                        range: part.range(),
                        overlapping: part.overlapping,
                        axis: part.dock.as_ref().is_some_and(|d| d.rail.ty == yard_core::dock::TURRET_AXIS),
                    });
                    // Where it was moved to before the view was left.
                    if fresh {
                        part.value = poses.get(&part.path).copied().unwrap_or(0.0);
                    }
                }
            }
            rig.place();
            // The box around everything, for the camera.
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for (group, part) in rig.parts.iter().enumerate() {
                renderer.place_group(group, part.placed.matrix());
                let Some((plo, phi)) = placed_box(part) else { continue };
                for a in 0..3 {
                    lo[a] = lo[a].min(plo[a]);
                    hi[a] = hi[a].max(phi[a]);
                }
            }
            *bounds = None;
            *shown = WHOLE.to_string();
            if frame && lo[0] <= hi[0] {
                renderer.camera.frame(lo, hi);
            }
            (info, rig.unplaced)
        });
        let (info, unplaced) = info.unwrap_or_default();
        self.parts.set(info);
        self.unplaced.set(unplaced);
        self.history.set(None);
        self.selected.set(None);
        self.entity_path.set(WHOLE.to_string());
        self.recount();
        self.measure();
        self.refresh_switches();
        self.refresh_links();
        self.refresh_overlay();
        self.redraw();
    }

    /// Shows the docked entities of the whole-design view where they have
    /// been moved to, and tells the panel.
    fn show_poses(self) {
        let poses = self.ws.try_update_value(|w| {
            let Workspace { renderer: Some(renderer), rig, .. } = w else { return Default::default() };
            for (group, part) in rig.parts.iter().enumerate() {
                renderer.place_group(group, part.placed.matrix());
            }
            rig.parts.iter().skip(1).map(|part| (part.path.clone(), part.value)).collect()
        });
        self.poses.set(poses.unwrap_or_default());
        self.refresh_overlay();
        self.redraw();
    }

    /// Moves one docked entity: degrees for what turns, blocks for what
    /// slides. It goes as far as other parts let it, as in the game.
    pub fn set_pose(self, path: String, value: f32) {
        let stopped = self.ws.try_update_value(|w| {
            let Workspace { blueprint: Some(bp), rig, contacts, looks, .. } = w else { return None };
            let part = rig.parts.iter().position(|p| p.path == path)?;
            if rig.parts[part].driven.is_some() {
                return Some("The game's rails are moving this part. Put the design back as saved to move it by hand.".to_string());
            }
            let entities = rig.entities(bp);
            let contact = rig.move_to(&entities, part, value, looks)?;
            *contacts = vec![(contact, js_sys::Date::now() + CONTACT_SHOWN_MS * 3.0)];
            let in_the_way = looks.name(entities[rig.parts[contact.still].entity].get(contact.still_cell).ty());
            Some(format!("Stopped there: it would run into {} of {}.", with_article(&in_the_way).to_lowercase(), crate::entity_label(&rig.parts[contact.still].path).to_lowercase()))
        });
        match stopped.flatten() {
            Some(text) => self.say(text),
            None => {
                self.ws.update_value(|w| w.contacts.clear());
                if !self.status.get_untracked().1 {
                    self.say("");
                }
            }
        }
        self.show_poses();
    }

    /// Puts every docked entity back the way the design was saved, and
    /// every block the simulation switched.
    pub fn reset_poses(self) {
        self.playing.set(false);
        self.stop_sim();
        self.ws.update_value(|w| {
            w.rig.reset();
            w.contacts.clear();
        });
        self.show_poses();
    }

    /// Starts or stops showing the docked entities in motion.
    pub fn play(self, on: bool) {
        if self.playing.get_untracked() == on {
            return;
        }
        if on {
            // The demonstration and the game's own rails do not mix.
            self.stop_sim();
        }
        self.playing.set(on);
        if on {
            self.ws.update_value(|w| w.rig.start_demo());
            self.play_on(js_sys::Date::now());
        }
    }

    /// One frame of the demonstration, then the next.
    fn play_on(self, last: f64) {
        request_animation_frame(move || {
            if !self.playing.get_untracked() || self.entity_path.get_untracked() != WHOLE {
                self.playing.set(false);
                return;
            }
            let now = js_sys::Date::now();
            // A tab that was in the background comes back with one long gap.
            let seconds = (((now - last) / 1000.0) as f32).clamp(0.0, 0.1);
            let looping = self.looping.get_untracked();
            let finished = self.ws.try_update_value(|w| {
                let Workspace { blueprint: Some(bp), rig, contacts, looks, .. } = w else { return true };
                let entities = rig.entities(bp);
                let played = rig.play(&entities, seconds, looping, looks);
                contacts.retain(|(_, until)| *until > now);
                contacts.extend(played.contacts.into_iter().map(|c| (c, now + CONTACT_SHOWN_MS)));
                played.finished
            });
            self.show_poses();
            if finished.unwrap_or(true) {
                self.playing.set(false);
                return;
            }
            self.play_on(now);
        });
    }

    /// Ends the simulation of the game's logic and rails: every block it
    /// switched and every part it moved goes back to how the design was saved.
    pub fn stop_sim(self) {
        let stopped = self.ws.try_update_value(|w| {
            let Workspace { blueprint: Some(bp), sim, rig, sim_running, .. } = w else { return false };
            let Some(mut ended) = sim.take() else { return false };
            *sim_running = false;
            ended.undo(&mut bp.blocks_mut());
            rig.reset();
            // What it put back has to be drawn again.
            *sim = Some(ended);
            true
        });
        if stopped == Some(true) {
            self.show_sim();
            self.ws.update_value(|w| w.sim = None);
            self.simulating.set(false);
            self.show_poses();
        }
    }

    /// Starts simulating the game's logic and rails on the design as it is
    /// saved, if that is not under way already. Docked entities on rails set
    /// off at once, as they do when the game loads the design.
    pub fn start_sim(self) -> bool {
        if self.ws.with_value(|w| w.sim.is_some()) {
            self.wake_sim();
            return true;
        }
        self.play(false);
        let started = self.ws.try_update_value(|w| {
            if w.preview {
                return false;
            }
            let Workspace { blueprint: Some(bp), looks, rig, sim, contacts, .. } = w else { return false };
            let rules = looks.rules(0);
            if rig.parts.is_empty() {
                *rig = Rig::new(bp, |ty| rules.is_track(ty), looks);
            }
            contacts.clear();
            *sim = Some(Sim::new(rig, &rig.entities(bp), looks, &bp.wireless()));
            true
        });
        if started != Some(true) {
            self.say("An old version is on screen. Go back to the current version to try the design out.");
            return false;
        }
        self.simulating.set(true);
        self.show_poses();
        self.wake_sim();
        true
    }

    /// Activates the block under a point of the page, as the game's R key does.
    pub fn use_at(self, x: i32, y: i32) {
        // Which entity of the design, counted as the simulation counts them, and which block.
        let found = self.ws.with_value(|w| {
            let bp = w.blueprint.as_ref()?;
            if w.path == WHOLE {
                let (part, hit) = Self::pick_whole(w, x, y)?;
                Some((w.rig.parts[part].entity, hit.pos))
            } else {
                Some((bp.entities().iter().position(|(path, _)| *path == w.path)?, Self::pick(w, x, y)?.pos))
            }
        });
        if let Some((entity, pos)) = found {
            self.use_block(entity, pos);
        }
    }

    /// Activates one block of the design, as the game's R key does.
    /// `entity` counts the design's entities as the simulation does.
    pub fn use_block(self, entity: usize, pos: Pos) {
        let found = self.ws.with_value(|w| {
            let bp = w.blueprint.as_ref()?;
            let entities: Vec<&yard_core::Entity> = bp.entities().into_iter().map(|(_, e)| &e.entity).collect();
            let name = w.looks.name(entities.get(entity)?.get(pos).ty());
            Some((name, Sim::would_press(&entities, &w.looks, entity, pos)))
        });
        let Some((name, would)) = found else { return };
        match would {
            Press::Nothing => return self.say(format!("The game does nothing when {} is activated.", with_article(&name).to_lowercase())),
            Press::Dialog => return self.say(format!("In the game, activating {} opens a window or seats the player. There is nothing to switch.", with_article(&name).to_lowercase())),
            Press::Console(0) => return self.say("Nothing that can be switched is linked from this console."),
            _ => {}
        }
        if !self.start_sim() {
            return;
        }
        self.ws.update_value(|w| {
            let Workspace { blueprint: Some(bp), looks, sim: Some(sim), .. } = w else { return };
            let entities: Vec<&yard_core::Entity> = bp.entities().into_iter().map(|(_, e)| &e.entity).collect();
            sim.press(&entities, looks, entity, pos);
        });
        self.say(match would {
            Press::Console(n) => format!("The console pressed the {n} blocks linked from it."),
            _ => format!("Used the {}.", name.to_lowercase()),
        });
        self.wake_sim();
    }

    /// Finds the switches of what is on screen and their states, for the panel.
    pub fn refresh_switches(self) {
        let listed = self.ws.try_update_value(|w| {
            let Workspace { blueprint: Some(bp), looks, switches, path, .. } = w else { return (Vec::new(), 0) };
            let entities = bp.entities();
            let found = switches.get_or_insert_with(|| {
                use yard_core::links::{flag, BlockLinks};
                let mut found = Vec::new();
                for (i, (_, e)) in entities.iter().enumerate() {
                    e.entity.for_each(|pos, block| {
                        let pressable = looks.link_traits(block.ty()).is_some_and(|t| t.is(flag::SIGNAL) && (t.is(flag::CAN_ACTIVATE) || block.ty() == 670));
                        if pressable {
                            found.push((i, pos));
                        }
                    });
                }
                found.sort();
                found
            });
            let shown: Vec<&(usize, Pos)> = found.iter().filter(|(i, _)| path == WHOLE || entities.get(*i).is_some_and(|(p, _)| p == path)).collect();
            let list = shown
                .iter()
                .take(LISTED_SWITCHES)
                .filter_map(|(i, pos)| {
                    let (entity_path, e) = entities.get(*i)?;
                    let block = e.entity.get(*pos);
                    Some(SwitchInfo { entity: *i, path: entity_path.clone(), pos: *pos, name: looks.name(block.ty()), on: block.active_bit() })
                })
                .collect();
            (list, shown.len())
        });
        let listed = listed.unwrap_or_default();
        if self.switches.with_untracked(|old| *old != listed) {
            self.switches.set(listed);
        }
    }

    /// The game's R key, for the block under the pointer.
    pub fn use_under_pointer(self) {
        if let Some((x, y)) = self.ws.with_value(|w| w.pointer) {
            self.use_at(x, y);
        }
    }

    /// Runs the simulation frame by frame until it has nothing left to do.
    fn wake_sim(self) {
        let asleep = self.ws.try_update_value(|w| w.sim.is_some() && !std::mem::replace(&mut w.sim_running, true));
        if asleep == Some(true) {
            self.sim_on(js_sys::Date::now());
        }
    }

    fn sim_on(self, last: f64) {
        request_animation_frame(move || {
            let now = js_sys::Date::now();
            // A tab that was in the background comes back with one long gap.
            let seconds = (((now - last) / 1000.0) as f32).clamp(0.0, 0.1);
            let busy = self.ws.try_update_value(|w| {
                let Workspace { blueprint: Some(bp), looks, rig, sim: Some(sim), sim_running: true, sim_slack, .. } = w else { return false };
                *sim_slack += seconds;
                *sim_slack -= sim.run(*sim_slack, rig, &mut bp.blocks_mut(), looks, looks);
                sim.busy()
            });
            self.show_sim();
            if busy == Some(true) {
                self.sim_on(now);
            } else {
                self.ws.update_value(|w| w.sim_running = false);
            }
        });
    }

    /// Draws what the simulation has changed since it was last drawn, and
    /// says what it has to say.
    fn show_sim(self) {
        let (moved, notes) = self
            .ws
            .try_update_value(|w| {
                let Workspace { renderer: Some(renderer), blueprint: Some(bp), looks, rig, sim: Some(sim), path, .. } = w else { return (false, Vec::new()) };
                let entities = bp.entities();
                // The segments whose looks changed: those of the switched
                // blocks and, for blocks on a border, the ones next door.
                let mut dirty = std::collections::BTreeSet::new();
                for (entity, pos) in sim.changed.drain(..) {
                    for d in [[0, 0, 0], [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]] {
                        dirty.insert((entity, yard_core::Entity::locate(pos.offset(d[0], d[1], d[2])).0));
                    }
                }
                for (entity, key) in dirty {
                    let Some((entity_path, e)) = entities.get(entity) else { continue };
                    let group = if path == WHOLE { rig.parts.iter().position(|p| p.entity == entity) } else { (entity_path == path).then_some(0) };
                    if let Some(group) = group {
                        renderer.set_chunk(group, key, &mesh::build_segment(&e.entity, key, looks));
                    }
                }
                let notes = sim
                    .notes
                    .drain(..)
                    .map(|note| match note {
                        Note::Undocked { part } => format!("{} was undocked. In the game it would drift free; here it stays where it was.", crate::entity_label(&rig.parts[part].path)),
                        Note::Launched { part } => format!("{} reached the end of its shootout rail and was launched.", crate::entity_label(&rig.parts[part].path)),
                        Note::Trigger { entity, pos, on } => {
                            let name = entities.get(entity).map(|(_, e)| looks.name(e.entity.get(pos).ty())).unwrap_or_default();
                            if on {
                                format!("The {} fires for as long as its signal stays on.", name.to_lowercase())
                            } else {
                                format!("The {} stopped firing.", name.to_lowercase())
                            }
                        }
                    })
                    .collect();
                (std::mem::take(&mut sim.moved), notes)
            })
            .unwrap_or_default();
        if let Some(note) = notes.into_iter().last() {
            self.say(note);
        }
        self.refresh_switches();
        if moved && self.entity_path.get_untracked() == WHOLE {
            self.show_poses();
        } else {
            self.refresh_overlay();
            self.redraw();
        }
    }

    /// The block under a point of the page in the whole-design view: which
    /// part it belongs to, and the hit in that part's own coordinates.
    fn pick_whole(w: &Workspace, x: i32, y: i32) -> Option<(usize, Hit)> {
        let (renderer, bp) = (w.renderer.as_ref()?, w.blueprint.as_ref()?);
        let (origin, dir) = renderer.pointer_ray(x, y);
        let mut best: Option<(usize, Hit)> = None;
        for (i, part) in w.rig.parts.iter().enumerate() {
            let (Some(bounds), Some(e)) = (part.bounds, bp.entity(&part.path)) else { continue };
            // The ray in the part's own coordinates. A turn keeps lengths, so
            // distances along it compare across parts.
            let back = part.placed.back();
            let local_dir = yard_core::dock::turned(&back.turn, dir);
            if let Some(hit) = pick::raycast(&e.entity, back.at(origin), local_dir, bounds) {
                if best.as_ref().is_none_or(|(_, b)| hit.distance < b.distance) {
                    best = Some((i, hit));
                }
            }
        }
        best
    }

    /// Puts one entity of the open design on screen, or with [`WHOLE`] all
    /// of them together.
    pub fn show_entity(self, path: String, frame: bool) {
        // Whatever is chosen, the design itself is what is shown.
        self.leave_core_preview();
        // Choosing another entity by hand calls off a cell being chosen on
        // this one (a core's new place, or a copy's rail).
        if frame && (self.relocating.get_untracked().is_some() || self.cloning.get_untracked().is_some()) {
            let chosen_on = self.cloning.get_untracked().map_or_else(|| self.entity_path.get_untracked(), |c| c.parent);
            if path != chosen_on {
                self.cancel_relocation();
            }
        }
        if path == WHOLE {
            return self.show_whole(frame);
        }
        self.playing.set(false);
        self.ws.update_value(|w| {
            if w.sim.is_none() {
                w.rig = Rig::default();
            }
            w.contacts.clear();
            let Workspace { renderer: Some(renderer), blueprint: Some(bp), looks, bounds, path: shown, .. } = w else { return };
            let Some(e) = bp.entity(&path) else { return };
            renderer.clear_chunks();
            for (key, _) in e.entity.segments() {
                renderer.set_chunk(0, *key, &mesh::build_segment(&e.entity, *key, looks));
            }
            *bounds = e.entity.bounds();
            *shown = path.clone();
            if let (true, Some(b)) = (frame, *bounds) {
                renderer.camera.frame(corner(b.min, -0.5), corner(b.max, 0.5));
            }
        });
        if self.entity_path.get_untracked() != path {
            self.history.set(None);
            // A position means another block in another entity.
            self.selected.set(None);
            self.picked.set(None);
        }
        self.entity_path.set(path);
        self.recount();
        self.measure();
        self.refresh_switches();
        self.refresh_links();
        self.refresh_overlay();
        self.redraw();
    }

    pub fn turn_camera(self, dx: f32, dy: f32, pan: bool) {
        self.ws.update_value(|w| {
            if let Some(r) = &mut w.renderer {
                if pan {
                    r.camera.pan(dx, dy);
                } else {
                    r.camera.orbit(dx, dy);
                }
                r.draw();
            }
        });
    }

    /// Mouse look: turns the view where the camera stands.
    pub fn look_camera(self, dx: f32, dy: f32) {
        self.ws.update_value(|w| {
            if let Some(r) = &mut w.renderer {
                r.camera.look(dx, dy);
            }
        });
    }

    /// Counts the blocks of the entity on screen afresh, by type (of the
    /// whole design in the whole-design view).
    /// Measures what has not been measured yet and publishes the sizes: each
    /// entity's, and the whole design's with every docked entity at rest
    /// where it docks.
    fn measure(self) {
        let dims = self.ws.try_update_value(|w| {
            let Workspace { blueprint: Some(bp), sizes, .. } = w else { return Dims::default() };
            for (path, e) in bp.entities() {
                sizes.entry(path).or_insert_with(|| e.entity.bounds());
            }
            let span = |b: &Bounds| [b.max.x - b.min.x + 1, b.max.y - b.min.y + 1, b.max.z - b.min.z + 1];
            let entities = sizes.iter().filter_map(|(path, b)| Some((path.clone(), span(b.as_ref()?)))).collect();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for (path, placement) in bp.layout(|_, d| d.at_rest()) {
                let Some(Some(b)) = sizes.get(&path) else { continue };
                for i in 0..8 {
                    let c = [if i & 1 == 0 { b.min.x as f32 - 0.5 } else { b.max.x as f32 + 0.5 }, if i & 2 == 0 { b.min.y as f32 - 0.5 } else { b.max.y as f32 + 0.5 }, if i & 4 == 0 { b.min.z as f32 - 0.5 } else { b.max.z as f32 + 0.5 }];
                    let at = placement.at(c);
                    for a in 0..3 {
                        lo[a] = lo[a].min(at[a]);
                        hi[a] = hi[a].max(at[a]);
                    }
                }
            }
            let whole = (lo[0] <= hi[0]).then(|| [0, 1, 2].map(|a| (hi[a] - lo[a]).round() as i32));
            Dims { whole, entities }
        });
        if let Some(dims) = dims {
            if self.dims.with_untracked(|d| *d != dims) {
                self.dims.set(dims);
            }
        }
    }

    fn recount(self) {
        self.ws.update_value(|w| {
            w.counts.clear();
            let Some(bp) = &w.blueprint else { return };
            for (path, e) in bp.entities() {
                if w.path == WHOLE || path == w.path {
                    for (ty, n) in e.entity.counts_by_type() {
                        *w.counts.entry(ty).or_default() += i64::from(n);
                    }
                }
            }
        });
        self.publish_counts();
    }

    /// Lists every block type on screen, most used first.
    fn publish_counts(self) {
        let list = self.ws.with_value(|w| {
            let mut list: Vec<(u16, u64)> = w.counts.iter().filter(|(_, n)| **n > 0).map(|(ty, n)| (*ty, *n as u64)).collect();
            list.sort_by_key(|(ty, n)| (std::cmp::Reverse(*n), *ty));
            list.into_iter().map(|(ty, n)| (ty, w.looks.name(ty), n)).collect::<Vec<_>>()
        });
        if self.used_blocks.get_untracked() != list {
            self.used_blocks.set(list);
        }
    }

    pub fn zoom_camera(self, wheel: f32) {
        self.ws.update_value(|w| {
            if let Some(r) = &mut w.renderer {
                r.camera.zoom(wheel);
            }
        });
        self.refresh_overlay();
        self.redraw();
    }

    pub fn pointer_moved(self, to: Option<(i32, i32)>) {
        self.ws.update_value(|w| {
            w.pointer = to;
            w.last_pointer = to.or(w.last_pointer);
        });
        self.refresh_overlay();
        self.redraw();
    }

    /// Shows the brush in the view for a moment, where the pointer last was
    /// over the design (or in the middle of the view), with its size and
    /// shape: called while they are changed in the panel.
    pub fn show_brush(self) {
        const SHOWN_FOR: std::time::Duration = std::time::Duration::from_millis(1500);
        let Some(show) = self.ws.try_update_value(|w| {
            w.brush_shows = w.brush_shows.wrapping_add(1);
            w.brush_shows
        }) else { return };
        self.brush_shown.set(true);
        set_timeout(
            move || {
                if self.ws.with_value(|w| w.brush_shows) == show {
                    self.brush_shown.set(false);
                }
            },
            SHOWN_FOR,
        );
    }

    /// Shows the outline for `act` while the button that will do it is held;
    /// `None` goes back to the tool's own outline.
    pub fn arm(self, act: Option<Act>) {
        let changed = self.ws.try_update_value(|w| std::mem::replace(&mut w.armed, act) != act);
        if changed == Some(true) {
            self.refresh_overlay();
            self.redraw();
        }
    }

    /// How deep below the surface the Fill can paints, for a click on `hit`:
    /// nothing for all the way through.
    fn fill_depth(self, hit: &Hit) -> Option<i32> {
        if self.fill_all.get_untracked() {
            None
        } else if self.fill_brush_depth.get_untracked() {
            Some(self.brush.get_untracked()[brush_axis(hit)].clamp(1, MAX_BRUSH))
        } else {
            Some(1)
        }
    }

    /// The block under a point of the page, if any.
    fn pick(w: &Workspace, x: i32, y: i32) -> Option<Hit> {
        let (renderer, bp, bounds) = (w.renderer.as_ref()?, w.blueprint.as_ref()?, w.bounds?);
        let (origin, dir) = renderer.pointer_ray(x, y);
        pick::raycast(&bp.entity(&w.path)?.entity, origin, dir, bounds)
    }

    /// The box of cells an action would cover for a hit: the brush, set against
    /// the face that was hit. Placing grows outward from the face; removing and
    /// painting dig inward from it.
    fn brush_box(act: Act, hit: &Hit, size: [i32; 3]) -> Option<(Pos, Pos)> {
        let outward = act == Act::Place;
        if outward && hit.normal == [0; 3] {
            return None;
        }
        let anchor = if outward { hit.outside() } else { hit.pos }.to_array();
        let size = if matches!(act, Act::Pick | Act::Select | Act::Connect | Act::Turret | Act::Core) { [1; 3] } else { size };
        let (mut lo, mut hi) = ([0; 3], [0; 3]);
        for a in 0..3 {
            let extra = size[a].clamp(1, MAX_BRUSH) - 1;
            if hit.normal[a] != 0 {
                let step = if outward { hit.normal[a] } else { -hit.normal[a] };
                let far = anchor[a] + step * extra;
                (lo[a], hi[a]) = (anchor[a].min(far), anchor[a].max(far));
            } else {
                lo[a] = anchor[a] - extra / 2;
                hi[a] = lo[a] + extra;
            }
        }
        Some((Pos::new(lo[0], lo[1], lo[2]), Pos::new(hi[0], hi[1], hi[2])))
    }

    /// Which sides of a block face open air: the empty cell beside it is
    /// part of space that reaches at least `OPEN_REACH` blocks away, or out
    /// of the design. A cavity inside a hull is not open air; with `inside`,
    /// a block no open air reaches has all its empty sides counted.
    fn air_sides(entity: &yard_core::entity::Entity, p: Pos, bounds: Bounds, inside: bool) -> u8 {
        const OPEN_REACH: i32 = 5;
        const SEARCH: usize = 600;
        let (mut bits, mut any) = (0, 0);
        for (side, d) in yard_core::shape::SIDE_DIRS.iter().enumerate() {
            let start = Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
            if !entity.get(start).is_air() {
                continue;
            }
            any |= 1 << side;
            let (mut seen, mut waiting) = (std::collections::HashSet::new(), std::collections::VecDeque::new());
            seen.insert(start);
            waiting.push_back(start);
            let mut open = false;
            while let Some(q) = waiting.pop_front() {
                let outside = q.x < bounds.min.x || q.x > bounds.max.x || q.y < bounds.min.y || q.y > bounds.max.y || q.z < bounds.min.z || q.z > bounds.max.z;
                if outside || (q.x - p.x).abs().max((q.y - p.y).abs()).max((q.z - p.z).abs()) >= OPEN_REACH || seen.len() >= SEARCH {
                    open = true;
                    break;
                }
                for step in yard_core::shape::SIDE_DIRS {
                    let next = Pos::new(q.x + step[0], q.y + step[1], q.z + step[2]);
                    if entity.get(next).is_air() && seen.insert(next) {
                        waiting.push_back(next);
                    }
                }
            }
            bits |= u8::from(open) << side;
        }
        yard_core::smooth::outer_first(bits, any, inside)
    }

    /// The block that bevels the edge of a step where `p` is one: a whole
    /// block of a hull material, with open air beside it and a lower tread
    /// within `step` blocks. None where there is nothing to bevel.
    fn bevelled(w: &Workspace, entity: &yard_core::entity::Entity, p: Pos, old: Block, step: u8, inside: bool) -> Option<Block> {
        let kind = w.looks.kind(old.ty());
        if kind.shape != Shape::Cube || kind.slab != 0 || kind.faces_matter {
            return None;
        }
        w.looks.material(old.ty())?;
        let air = Self::air_sides(entity, p, w.bounds?, inside);
        let at = |d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
        let mask = yard_core::smooth::bevel(air, |d| !entity.get(at(d)).is_air(), step)?;
        let form = yard_core::smooth::best_form(mask, |side| entity.get(at(yard_core::shape::SIDE_DIRS[side])).is_air())?;
        w.looks.reshape(old, form.shape, form.slab, form.orientation)
    }

    /// Whether a block is a wedge that smoothing would put in its cell were
    /// it empty: one filling a notch on the inner face of a thin hull.
    fn notch_filler(w: &Workspace, entity: &yard_core::entity::Entity, p: Pos, block: Block) -> bool {
        if w.looks.kind(block.ty()).shape != Shape::Wedge {
            return false;
        }
        let reshape = |b: Block, f: &yard_core::smooth::Form| w.looks.reshape(b, f.shape, f.slab, f.orientation);
        yard_core::smooth::notch_block(entity, p, |b| Self::smoothable(w, b), reshape).is_some_and(|b| b.ty() == block.ty() && b.orientation() == block.orientation())
    }

    /// Whether smoothing may reshape a block: a whole cube of a hull material.
    fn smoothable(w: &Workspace, b: Block) -> bool {
        let kind = w.looks.kind(b.ty());
        b.ty() != edit::CORE_TYPE && kind.shape == Shape::Cube && kind.slab == 0 && !kind.faces_matter && w.looks.material(b.ty()).is_some()
    }

    /// The sample points a block fills, by its shape and turn: nothing for air,
    /// everything for a whole block.
    fn samples_of_block(w: &Workspace, block: Block) -> Option<u64> {
        if block.is_air() {
            return None;
        }
        let kind = w.looks.kind(block.ty());
        Some(match kind.shape {
            Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta => yard_core::smooth::samples_of(kind.shape, 0, block.orientation()),
            Shape::Cube if kind.slab > 0 => yard_core::smooth::samples_of(kind.shape, kind.slab, block.orientation()),
            _ => u64::MAX,
        })
    }

    /// The block at the end of a row of wedges, beveled at `p` already
    /// (`planned`), cut flush with the row where its own bevel left an edge
    /// the row cuts. None where nothing changes.
    fn row_ended(w: &Workspace, entity: &yard_core::entity::Entity, p: Pos, planned: &std::collections::HashMap<Pos, Block>, step: u8, inside: bool) -> Option<Block> {
        let old = entity.get(p);
        let air = Self::air_sides(entity, p, w.bounds?, inside);
        let at = |d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
        let mask = yard_core::smooth::bevel(air, |d| !entity.get(at(d)).is_air(), step)?;
        let neighbour = |d: [i32; 3]| Self::samples_of_block(w, planned.get(&at(d)).copied().unwrap_or_else(|| entity.get(at(d))));
        let flush = yard_core::smooth::row_end(air, mask, neighbour);
        if flush == mask {
            return None;
        }
        let form = yard_core::smooth::best_form(flush, |side| entity.get(at(yard_core::shape::SIDE_DIRS[side])).is_air())?;
        w.looks.reshape(old, form.shape, form.slab, form.orientation)
    }

    /// The whole block at `p` that a slope ends against, cut so the slope
    /// carries on through it, judged by what its neighbours are or are about
    /// to become (`planned`). None where no slope ends here.
    fn sharpened(w: &Workspace, entity: &yard_core::entity::Entity, p: Pos, planned: &std::collections::HashMap<Pos, Block>, inside: bool) -> Option<Block> {
        let old = entity.get(p);
        if !Self::smoothable(w, old) {
            return None;
        }
        let air = Self::air_sides(entity, p, w.bounds?, inside);
        let at = |d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
        let neighbour = |d: [i32; 3]| Self::samples_of_block(w, planned.get(&at(d)).copied().unwrap_or_else(|| entity.get(at(d))));
        let mask = yard_core::smooth::continued(air, neighbour)?;
        let form = yard_core::smooth::best_form(mask, |side| entity.get(at(yard_core::shape::SIDE_DIRS[side])).is_air())?;
        w.looks.reshape(old, form.shape, form.slab, form.orientation).filter(|n| *n != old)
    }

    /// The block that takes off the corner standing up between two beveled
    /// rows at `p`, judged by what its neighbours are, or are about to become
    /// (`planned`). None where there is no such corner.
    fn inside_cornered(w: &Workspace, entity: &yard_core::entity::Entity, p: Pos, old: Block, planned: &std::collections::HashMap<Pos, Block>, inside: bool) -> Option<Block> {
        let kind = w.looks.kind(old.ty());
        if kind.shape != Shape::Cube || kind.slab != 0 || kind.faces_matter {
            return None;
        }
        w.looks.material(old.ty())?;
        let air = Self::air_sides(entity, p, w.bounds?, inside);
        let at = |d: [i32; 3]| Pos::new(p.x + d[0], p.y + d[1], p.z + d[2]);
        let neighbour = |d: [i32; 3]| Self::samples_of_block(w, planned.get(&at(d)).copied().unwrap_or_else(|| entity.get(at(d))));
        let mask = yard_core::smooth::inside_corner(air, neighbour)?;
        let form = yard_core::smooth::best_form(mask, |side| entity.get(at(yard_core::shape::SIDE_DIRS[side])).is_air())?;
        w.looks.reshape(old, form.shape, form.slab, form.orientation)
    }

    /// One pass of smoothing over the whole entity on screen: the edges of
    /// steps beveled, then the inside corners. One step, so Undo takes it back.
    pub fn smooth_whole(self) {
        if self.entity_path.get_untracked() == WHOLE || self.current.get_untracked().is_none() {
            return self.say("Choose an entity on the left first.");
        }
        let opts = yard_core::smooth::Smoothing { step: self.smooth_step.get_untracked(), inside: self.smooth_inside.get_untracked(), sharpen: self.smooth_sharpen.get_untracked() };
        self.say("Smoothing...");
        // Let the message show before the work, which takes a moment on a big ship.
        set_timeout(
            move || {
                let cells = self.ws.with_value(|w| {
                    let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity) else { return (Vec::new(), Vec::new()) };
                    let done = yard_core::smooth::smooth_entity_opts(
                        entity,
                        &opts,
                        |b| Self::smoothable(w, b),
                        |b, f| w.looks.reshape(b, f.shape, f.slab, f.orientation),
                        |b| Self::samples_of_block(w, b),
                    );
                    // Blocks reshaped, and wedges added in the notches of thin hulls.
                    done.into_iter().partition::<Vec<_>, _>(|(p, _)| !entity.get(*p).is_air())
                });
                let (cells, added) = cells;
                if cells.is_empty() && added.is_empty() {
                    return self.say("Nothing more to smooth at this step height.");
                }
                let count = cells.len() + added.len();
                if self.submit(Self::smoothing_ops(cells, added)).is_some() {
                    self.say(format!("Smoothed {count} blocks into wedges, corner pieces and heptas. Press again for another pass; Undo takes this one back."));
                }
            },
            std::time::Duration::from_millis(30),
        );
    }

    /// Smoothing done over: every wedge, corner, hepta and tetra of the hull
    /// materials turned back into a whole block and the notch wedges taken
    /// out, then one pass, all as one step. For a design smoothed before,
    /// whose shapes an ordinary pass leaves alone.
    pub fn resmooth_whole(self) {
        if self.entity_path.get_untracked() == WHOLE || self.current.get_untracked().is_none() {
            return self.say("Choose an entity on the left first.");
        }
        let opts = yard_core::smooth::Smoothing { step: self.smooth_step.get_untracked(), inside: self.smooth_inside.get_untracked(), sharpen: self.smooth_sharpen.get_untracked() };
        self.say("Smoothing again from scratch...");
        set_timeout(
            move || {
                let (cells, added, emptied) = self.ws.with_value(|w| {
                    let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity) else { return Default::default() };
                    let done = yard_core::smooth::resmooth_entity_opts(
                        entity,
                        &opts,
                        |b| Self::smoothable(w, b),
                        |b, f| w.looks.reshape(b, f.shape, f.slab, f.orientation),
                        |b| Self::samples_of_block(w, b),
                        |b| Self::unshaped(w, b),
                    );
                    let (emptied, done): (Vec<_>, Vec<_>) = done.into_iter().partition(|(_, b)| b.is_air());
                    let (cells, added): (Vec<_>, Vec<_>) = done.into_iter().partition(|(p, _)| !entity.get(*p).is_air());
                    (cells, added, emptied)
                });
                if cells.is_empty() && added.is_empty() && emptied.is_empty() {
                    return self.say("Smoothing it again from scratch changes nothing.");
                }
                let count = cells.len() + added.len() + emptied.len();
                let mut ops = Self::smoothing_ops(cells, added);
                if !emptied.is_empty() {
                    ops.push(Op::Set { cells: emptied, when: When::Occupied });
                }
                if self.submit(ops).is_some() {
                    self.say(format!("Smoothed again from scratch: {count} blocks changed. Undo takes it back."));
                }
            },
            std::time::Duration::from_millis(30),
        );
    }

    /// The whole block of a wedge, corner, hepta or tetra of a hull material.
    fn unshaped(w: &Workspace, b: Block) -> Option<Block> {
        let kind = w.looks.kind(b.ty());
        if !matches!(kind.shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta) || w.looks.material(b.ty()).is_none() {
            return None;
        }
        w.looks.reshape(b, Shape::Cube, 0, 0).filter(|c| Self::smoothable(w, *c))
    }

    /// The steps that write what smoothing planned: blocks reshaped where
    /// they are, wedges added where the cells are empty.
    fn smoothing_ops(cells: Vec<(Pos, Block)>, added: Vec<(Pos, Block)>) -> Vec<Op> {
        let mut ops = Vec::new();
        if !cells.is_empty() {
            ops.push(Op::Set { cells, when: When::Occupied });
        }
        if !added.is_empty() {
            ops.push(Op::Set { cells: added, when: When::Empty });
        }
        ops
    }

    /// The game's linking rules for the entity on screen.
    fn rules(w: &Workspace) -> yard_core::links::Rules<'_> {
        let entity_type = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map_or(0, |e| e.header.entity_type);
        w.looks.rules(entity_type)
    }

    /// What pressing V over the block at `to` would do while `from` is
    /// selected, or why it would do nothing.
    fn link_plan(w: &Workspace, from: Option<Pos>, to: Pos) -> Result<(Pos, LinkStep), String> {
        let from = from.ok_or("Select the block that should control this one first: click it with the Link tool, or press C over it.")?;
        let entity = &w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).ok_or("No design is open.")?.entity;
        if from == to {
            return Err("That is the selected block itself.".into());
        }
        if links::is_linked(entity.links(), from, to) {
            return Ok((from, LinkStep::Unlink));
        }
        let (a, b) = (entity.get(from).ty(), entity.get(to).ty());
        if !Self::rules(w).can_link(a, b) {
            let (a, b) = (with_article(&w.looks.name(a)), with_article(&w.looks.name(b)));
            return Err(format!("The game does not let {a} control {b}."));
        }
        Ok((from, LinkStep::Link))
    }

    /// Draws the marks for the selected block and says what it is linked to.
    /// None if there is no block there.
    fn describe_links(w: &Workspace, at: Pos, marks: &mut Overlay) -> Option<LinkInfo> {
        let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
        let block = entity.get(at);
        if block.is_air() {
            return None;
        }
        let tint = |c: [u8; 3], alpha: u8| [c[0], c[1], c[2], alpha];
        let centre = |p: Pos| [p.x as f32, p.y as f32, p.z as f32];
        let rules = Self::rules(w);
        let mut controls = Vec::new();
        let mut controlled_controllers = Vec::new();
        let mut unlisted_controllers = 0;
        if let Some(groups) = entity.links().get(&at) {
            for (ty, set) in groups {
                let colour = match rules.slot(block.ty(), *ty) {
                    Some(Slot::Slave) => SLAVE_COLOUR,
                    Some(Slot::Effect) => EFFECT_COLOUR,
                    Some(Slot::Light) => LIGHT_COLOUR,
                    None => CONTROLS_COLOUR,
                };
                controls.push((*ty, w.looks.name(*ty), set.len(), colour));
                for (lo, hi) in links::boxes(set) {
                    marks.wire_box(corner(lo, -0.54), corner(hi, 0.54), tint(colour, 255));
                    marks.solid_box(corner(lo, -0.54), corner(hi, 0.54), tint(colour, 34));
                }
                // A controlled block that controls something itself (a slaved
                // computer, the next logic block) gets a line and its own row.
                if rules.can_control(*ty) {
                    for &p in set {
                        marks.line(centre(at), centre(p), tint(colour, 220));
                        if controlled_controllers.len() < LISTED_CONTROLLERS {
                            controlled_controllers.push((p, w.looks.name(*ty)));
                        } else {
                            unlisted_controllers += 1;
                        }
                    }
                }
            }
        }
        controls.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        let controlled_by = links::controllers_of(entity.links(), at)
            .into_iter()
            .map(|c| {
                marks.wire_box(corner(c, -0.57), corner(c, 0.57), tint(CONTROLLED_BY_COLOUR, 255));
                marks.solid_box(corner(c, -0.57), corner(c, 0.57), tint(CONTROLLED_BY_COLOUR, 45));
                marks.line(centre(c), centre(at), tint(CONTROLLED_BY_COLOUR, 220));
                (c, w.looks.name(entity.get(c).ty()))
            })
            .collect();
        marks.wire_box(corner(at, -0.53), corner(at, 0.53), tint(SELECTED_COLOUR, 255));
        marks.wire_box(corner(at, -0.6), corner(at, 0.6), tint(SELECTED_COLOUR, 255));
        Some(LinkInfo {
            pos: at,
            name: w.looks.name(block.ty()),
            can_control: rules.can_control(block.ty()),
            controls,
            controlled_controllers,
            unlisted_controllers,
            controlled_by,
        })
    }

    /// Brings the link marks and the link panel up to date with the selection
    /// and the design. Lets go of a selection whose block is no longer there.
    pub fn refresh_links(self) {
        let selected = self.selected.get_untracked();
        let highlight = self.highlight.get_untracked();
        let show_unlinked = self.show_unlinked.get_untracked();
        let rail = self.rail_move.get_untracked();
        let cloning = self.cloning.get_untracked();
        let relocating = self.relocating.get_untracked().map(|at| match (&rail, &cloning) {
            (_, Some(job)) => {
                // The arrow is the rail's arrow side; the orange box is the original's rail.
                let side = SIDE_DIRS.iter().position(|d| *d == job.facing).unwrap_or(2);
                let top = if side / 2 == 1 { 4 } else { 2 };
                let from = self.ws.with_value(|w| w.blueprint.as_ref()?.docks().get(&job.source).map(|d| d.rail.pos));
                Marker { at, arrow: Some((side, top, false)), from }
            }
            (Some(rail), None) => Marker { at, arrow: None, from: Some(self.ws.with_value(|w| Self::rail_cell(w, rail)).unwrap_or(at)) },
            (None, None) => {
                let (nose, top) = self.core_facing.get_untracked();
                Marker { at, arrow: Some((nose, top, (nose, top) != CORE_AS_IS)), from: Some(Pos::new(0, 0, 0)) }
            }
        });
        let (turret, base) = (self.turret_box.get_untracked(), self.turret_base.get_untracked());
        let (grab, docker) = (self.turret_grab.get_untracked(), self.turret_docker.get_untracked());
        let excluded = self.turret_excluded.get_untracked();
        let info = self
            .ws
            .try_update_value(|w| {
                let mut marks = Overlay::default();
                let info = selected.and_then(|at| Self::describe_links(w, at, &mut marks));
                if let (Some(ty), true) = (highlight, w.path != WHOLE) {
                    Self::mark_blocks_of(w, ty, &mut marks);
                }
                if show_unlinked && w.path != WHOLE {
                    Self::mark_unlinked(w, &mut marks);
                }
                if w.path != WHOLE {
                    Self::mark_structure(w, relocating, turret, (base, grab, docker), &excluded, &mut marks);
                }
                if let Some(r) = &mut w.renderer {
                    r.set_marks(&marks);
                }
                info
            })
            .flatten();
        if selected.is_some() && info.is_none() {
            self.selected.set(None);
        }
        if self.link_info.with_untracked(|old| *old != info) {
            self.link_info.set(info);
        }
    }

    /// Draws marks through the design on the blocks of the entity on screen
    /// that should be linked and are not (the first `MARKED` of them), and
    /// returns them all.
    fn mark_unlinked(w: &Workspace, marks: &mut Overlay) -> Vec<(Pos, u16)> {
        let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity) else { return Vec::new() };
        let found = links::unlinked(entity, &Self::rules(w));
        for p in found.iter().take(MARKED) {
            marks.wire_box(corner(*p, -0.56), corner(*p, 0.56), UNLINKED_COLOUR);
            marks.solid_box(corner(*p, -0.56), corner(*p, 0.56), [UNLINKED_COLOUR[0], UNLINKED_COLOUR[1], UNLINKED_COLOUR[2], 70]);
        }
        found.into_iter().map(|p| (p, entity.get(p).ty())).collect()
    }

    /// Marks, or stops marking, the blocks that should be linked and are
    /// not, and says what they are.
    pub fn toggle_unlinked(self) {
        if self.show_unlinked.get_untracked() {
            self.show_unlinked.set(false);
            self.refresh_links();
            self.redraw();
            return;
        }
        if self.entity_path.get_untracked() == WHOLE {
            return self.say("Choose an entity on the left to check its links.");
        }
        self.show_unlinked.set(true);
        let found = self.ws.try_update_value(|w| {
            let found = Self::mark_unlinked(w, &mut Overlay::default());
            let mut by_type: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
            for (_, ty) in &found {
                *by_type.entry(*ty).or_default() += 1;
            }
            let mut kinds: Vec<(usize, String)> = by_type.into_iter().map(|(ty, n)| (n, w.looks.name(ty))).collect();
            kinds.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let (lo, hi) = found.iter().fold((Pos::new(i32::MAX, i32::MAX, i32::MAX), Pos::new(i32::MIN, i32::MIN, i32::MIN)), |(lo, hi), (p, _)| (lo.lo(*p), hi.hi(*p)));
            (found.len(), kinds, lo, hi)
        });
        self.refresh_links();
        let Some((count, kinds, lo, hi)) = found else { return };
        if count == 0 {
            return self.say("Every module in this entity is linked to its computer, and every computer runs modules.");
        }
        let listed: Vec<String> = kinds.iter().take(4).map(|(n, name)| format!("{n} {name}")).collect();
        let more = if kinds.len() > 4 { format!(" and {} more kinds", kinds.len() - 4) } else { String::new() };
        let marked = if count > MARKED { format!(" The first {MARKED} are marked.") } else { String::new() };
        self.say(format!("{} should be linked and {} not, marked in orange: {}{more}.{marked} Click Hide unlinked to stop.", crate::thousands(count as u64), if count == 1 { "is" } else { "are" }, listed.join(", ")));
        if count <= 200 {
            self.ws.update_value(|w| {
                if let Some(r) = &mut w.renderer {
                    r.camera.frame(corner(lo, -4.0), corner(hi, 4.0));
                }
            });
        }
        self.redraw();
    }

    /// Draws marks through the design on every block of one type (the first
    /// `MARKED` of them), and returns how many there are and the box around
    /// them.
    fn mark_blocks_of(w: &Workspace, ty: u16, marks: &mut Overlay) -> Option<(usize, Pos, Pos)> {
        let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
        let (mut count, mut lo, mut hi) = (0usize, Pos::new(i32::MAX, i32::MAX, i32::MAX), Pos::new(i32::MIN, i32::MIN, i32::MIN));
        entity.for_each(|p, b| {
            if b.ty() != ty {
                return;
            }
            if count < MARKED {
                marks.wire_box(corner(p, -0.56), corner(p, 0.56), HIGHLIGHT_COLOUR);
                marks.solid_box(corner(p, -0.56), corner(p, 0.56), [HIGHLIGHT_COLOUR[0], HIGHLIGHT_COLOUR[1], HIGHLIGHT_COLOUR[2], 60]);
            }
            count += 1;
            (lo, hi) = (lo.lo(p), hi.hi(p));
        });
        (count > 0).then_some((count, lo, hi))
    }

    /// Marks every block of a type through the design, turning the view to
    /// them if there are few; chosen again, lets go. For finding what is
    /// hidden inside a hull.
    pub fn toggle_highlight(self, ty: u16, name: String) {
        if self.highlight.get_untracked() == Some(ty) {
            self.highlight.set(None);
            self.refresh_links();
            self.redraw();
            return;
        }
        if self.entity_path.get_untracked() == WHOLE {
            self.say("Choose an entity on the left to find blocks in it.");
            return;
        }
        self.highlight.set(Some(ty));
        let found = self.ws.try_update_value(|w| Self::mark_blocks_of(w, ty, &mut Overlay::default())).flatten();
        self.refresh_links();
        match found {
            None => self.say(format!("This entity has no {name} blocks.")),
            Some((count, lo, hi)) => {
                self.say(if count > MARKED { format!("Marked the first {MARKED} of {count} {name} blocks.") } else { format!("Marked {count} {name} block{} through the design. Click the name again to stop.", if count == 1 { "" } else { "s" }) });
                // Turn the view to them when they are few.
                if count <= 200 {
                    self.ws.update_value(|w| {
                        if let Some(r) = &mut w.renderer {
                            r.camera.frame(corner(lo, -4.0), corner(hi, 4.0));
                        }
                    });
                    self.refresh_overlay();
                }
            }
        }
        self.redraw();
    }

    /// Draws, through the design, what the player is choosing: where the core
    /// would go, or the box of a turret with its base and the cell its axis
    /// would take.
    fn mark_structure(w: &Workspace, relocating: Option<Marker>, turret: Option<(Pos, Option<Pos>)>, (base, grab, docker): (u8, Grab, Option<Pos>), excluded: &BTreeSet<Pos>, marks: &mut Overlay) {
        const GO: [u8; 4] = [90, 255, 140, 255];
        const OLD: [u8; 4] = [255, 170, 60, 255];
        const BOX: [u8; 4] = [255, 160, 60, 255];
        const RAIL: [u8; 4] = [120, 190, 255, 255];
        const DOCKER: [u8; 4] = [255, 225, 70, 255];
        const CORE: [u8; 4] = [90, 230, 120, 255];
        const HANDLE: [u8; 4] = [255, 255, 255, 255];
        let with_alpha = |c: [u8; 4], a: u8| [c[0], c[1], c[2], a];
        if let Some(Marker { at, arrow, from }) = relocating {
            marks.wire_box(corner(at, -0.58), corner(at, 0.58), GO);
            marks.solid_box(corner(at, -0.5), corner(at, 0.5), with_alpha(GO, 70));
            if let Some(from) = from {
                marks.wire_box(corner(from, -0.6), corner(from, 0.6), OLD);
                marks.line([from.x as f32, from.y as f32, from.z as f32], [at.x as f32, at.y as f32, at.z as f32], with_alpha(GO, 160));
            }
            // A rail has no nose: the marker and the cell it leaves are all.
            let Some((nose, top, compare)) = arrow else { return };
            // The arrow shows the way the nose will point from the new place,
            // long enough to see on a big ship, with a short stalk for the top.
            let length = w.bounds.map_or(4.0, |b| {
                let (x, y, z) = b.size();
                (x.max(y).max(z) as f32 / 3.0).clamp(5.0, 120.0)
            });
            let c = [at.x as f32, at.y as f32, at.z as f32];
            let (f, u) = (SIDE_DIRS[nose].map(|v| v as f32), SIDE_DIRS[top].map(|v| v as f32));
            let side = [f[1] * u[2] - f[2] * u[1], f[2] * u[0] - f[0] * u[2], f[0] * u[1] - f[1] * u[0]];
            let along = |p: [f32; 3], d: [f32; 3], k: f32| [p[0] + d[0] * k, p[1] + d[1] * k, p[2] + d[2] * k];
            if compare {
                // The way the nose points now, faintly, for comparison.
                marks.line(c, along(c, [0.0, 0.0, 1.0], length * 0.6), with_alpha(OLD, 150));
            }
            let tip = along(c, f, length);
            let (head, wide) = (length * 0.3, length * 0.15);
            marks.line(c, tip, GO);
            marks.line(c, along(c, u, length * 0.3), with_alpha(GO, 200));
            let back = along(c, f, length - head);
            let notch = along(c, f, length - head * 0.75);
            // Two crossed fins, so the head shows from any side.
            for d in [side, u] {
                marks.quad([tip, along(back, d, -wide), notch, along(back, d, wide)], with_alpha(GO, 170));
            }
        }
        if let Some((first, second)) = turret {
            let (lo, hi) = match second {
                Some(b) => (first.lo(b), first.hi(b)),
                None => (first, first),
            };
            marks.wire_box(corner(lo, -0.57), corner(hi, 0.57), BOX);
            if second.is_none() {
                marks.solid_box(corner(lo, -0.55), corner(hi, 0.55), with_alpha(BOX, 38));
                // The one corner so far, which the movement keys move.
                marks.wire_box(corner(first, -0.7), corner(first, 0.7), HANDLE);
                return;
            }
            let side = (base as usize).min(5);
            let d = yard_core::shape::SIDE_DIRS[side];
            Self::mark_turret_blocks(w, lo, hi, &turret_left_out(excluded, lo, hi, base, docker), marks);
            let (low, high) = (lo.to_array(), hi.to_array());
            let axis = (0..3).find(|&a| d[a] != 0).unwrap_or(1);
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let plane = if d[axis] > 0 { high[axis] as f32 + 0.5 } else { low[axis] as f32 - 0.5 };
            let point = |pu: f32, pv: f32| {
                let mut p = [0.0; 3];
                (p[axis], p[u], p[v]) = (plane, pu, pv);
                p
            };
            let (u0, u1, v0, v1) = (low[u] as f32 - 0.5, high[u] as f32 + 0.5, low[v] as f32 - 0.5, high[v] as f32 + 0.5);
            marks.quad([point(u0, v0), point(u1, v0), point(u1, v1), point(u0, v1)], with_alpha(RAIL, 60));
            // The corners, which can be grabbed and moved; the grabbed one stands out.
            for c in 0..8u8 {
                let at = corner_cell(lo, hi, c);
                if grab == Grab::Corner(c) {
                    marks.wire_box(corner(at, -0.75), corner(at, 0.75), HANDLE);
                    marks.solid_box(corner(at, -0.6), corner(at, 0.6), with_alpha(HANDLE, 110));
                } else {
                    marks.wire_box(corner(at, -0.62), corner(at, 0.62), with_alpha(BOX, 220));
                }
            }
            // The rail docker on the base face, the turret axis outside it on
            // the ship, and the new core inside, beyond the docker.
            let dock = docker_cell(lo, hi, base, docker);
            let rail = dock.offset(d[0], d[1], d[2]);
            let core = dock.offset(-d[0], -d[1], -d[2]);
            marks.wire_box(corner(dock, -0.53), corner(dock, 0.53), DOCKER);
            marks.solid_box(corner(dock, -0.45), corner(dock, 0.45), with_alpha(DOCKER, 150));
            if grab == Grab::Docker {
                marks.wire_box(corner(dock, -0.75), corner(dock, 0.75), HANDLE);
            }
            marks.wire_box(corner(rail, -0.5), corner(rail, 0.5), RAIL);
            marks.solid_box(corner(rail, -0.4), corner(rail, 0.4), with_alpha(RAIL, 120));
            marks.wire_box(corner(core, -0.4), corner(core, 0.4), CORE);
            marks.solid_box(corner(core, -0.3), corner(core, 0.3), with_alpha(CORE, 120));
        }
    }

    /// The blocks the turret takes, their outer faces tinted, and those left
    /// out in red. Boxes too big to go through cell by cell keep only their outline.
    fn mark_turret_blocks(w: &Workspace, lo: Pos, hi: Pos, excluded: &BTreeSet<Pos>, marks: &mut Overlay) {
        const TAKEN: [u8; 4] = [255, 160, 60, 80];
        const LEFT: [u8; 4] = [255, 70, 70, 110];
        let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity) else { return };
        let cells = (hi.x - lo.x + 1) as i64 * (hi.y - lo.y + 1) as i64 * (hi.z - lo.z + 1) as i64;
        if cells > TURRET_MARKED_CELLS {
            return;
        }
        let in_box = |p: Pos| p.lo(lo) == lo && p.hi(hi) == hi;
        // Which group a cell is in: none, taken, or left out.
        let group = |p: Pos| -> u8 {
            if !in_box(p) || entity.get(p).is_air() {
                0
            } else if excluded.contains(&p) {
                2
            } else {
                1
            }
        };
        let mut faces = 0;
        for z in lo.z..=hi.z {
            for y in lo.y..=hi.y {
                for x in lo.x..=hi.x {
                    let p = Pos::new(x, y, z);
                    let g = group(p);
                    if g == 0 {
                        continue;
                    }
                    for d in yard_core::shape::SIDE_DIRS {
                        if group(p.offset(d[0], d[1], d[2])) == g {
                            continue;
                        }
                        // The face on side d, a hair outside the block.
                        let axis = (0..3).find(|&a| d[a] != 0).unwrap_or(0);
                        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                        let c = [p.x as f32, p.y as f32, p.z as f32];
                        let point = |du: f32, dv: f32| {
                            let mut q = c;
                            q[axis] += d[axis] as f32 * 0.51;
                            q[u] += du;
                            q[v] += dv;
                            q
                        };
                        marks.quad([point(-0.5, -0.5), point(0.5, -0.5), point(0.5, 0.5), point(-0.5, 0.5)], if g == 1 { TAKEN } else { LEFT });
                        faces += 1;
                        if faces >= TURRET_MARKED_FACES {
                            return;
                        }
                    }
                }
            }
        }
    }

    // ---- moving the core ----

    /// Starts choosing the cell the core is to move to.
    pub fn start_relocation(self) {
        if self.entity_path.get_untracked() == WHOLE || self.current.get_untracked().is_none() {
            return self.say("Choose an entity on the left first: the core of one entity moves at a time.");
        }
        self.stop_sim();
        self.turret_box.set(None);
        self.relocating.set(Some(Pos::new(0, 0, 0)));
        self.core_facing.set(CORE_AS_IS);
        let (turret, fixed) = self.ws.with_value(|w| {
            let Some(bp) = w.blueprint.as_ref() else { return (None, false) };
            let (own, children) = yard_core::relocate::turret_docks(bp, &w.path);
            let fixed = own.as_ref().is_some_and(|d| d.motion() == Motion::Fixed);
            (own.map(|_| children.len()), fixed)
        });
        self.turret_parts.set(turret);
        self.turret_fixed.set(fixed);
        let rails = self.ws.with_value(|w| {
            let Some(bp) = w.blueprint.as_ref() else { return Vec::new() };
            let single = |ty: u16| ty == yard_core::dock::TURRET_AXIS || yard_core::dock::ROTATORS.contains(&ty);
            let mut out = Vec::new();
            for (child, dock) in bp.docks() {
                if !single(dock.rail.ty) {
                    continue;
                }
                let kind = if dock.rail.ty == yard_core::dock::TURRET_AXIS { "turret axis" } else { "rotator" };
                if child == w.path {
                    out.push((child.clone(), true, format!("The {kind} this entity rests on")));
                } else if child.rsplit_once('/').map_or("", |(p, _)| p) == w.path {
                    out.push((child.clone(), false, format!("The {kind} {} hangs from", crate::entity_label(&child).to_lowercase())));
                }
            }
            out
        });
        self.rails_to_move.set(rails);
        self.rail_move.set(None);
        self.say(format!(
            "Click a block, or Shift-click the space in front of it, to choose the new place for the core, and choose on the right which way its nose and top point. W A S D E Q move it, Enter confirms, Escape cancels.{}",
            if turret.is_some() { " This is a turret: its dock goes with the core unless unticked on the right, and Preview shows it turning about the new line." } else { "" }
        ));
        self.refresh_links();
        self.redraw();
    }

    pub fn cancel_relocation(self) {
        let back = self.leave_core_preview();
        self.cloning.set(None);
        self.relocating.set(None);
        self.core_facing.set(CORE_AS_IS);
        self.turret_parts.set(None);
        self.rail_move.set(None);
        self.rails_to_move.set(Vec::new());
        self.fit_steps.set(Vec::new());
        self.say("");
        match back {
            Some(path) => self.show_entity(path, false),
            None => {
                self.refresh_links();
                self.redraw();
            }
        }
    }

    /// The cell a rail being moved has now, in the entity on screen's
    /// coordinates: the docker's cell for the rail the entity rests on, the
    /// rail's own cell for one a docked entity hangs from.
    fn rail_cell(w: &Workspace, rail: &RailMove) -> Option<Pos> {
        let docks = w.blueprint.as_ref()?.docks();
        Some(if rail.own { docks.get(&w.path)?.docker.pos } else { docks.get(&rail.child)?.rail.pos })
    }

    /// Chooses what the Move core tool moves: the core (None), or one of
    /// the rails on offer, by its place in [`Ctx::rails_to_move`].
    pub fn choose_rail(self, which: Option<usize>) {
        if let Some(path) = self.leave_core_preview() {
            self.show_entity(path, false);
        }
        let rail = which.and_then(|i| self.rails_to_move.with_untracked(|r| r.get(i).cloned())).map(|(child, own, _)| RailMove {
            child,
            own,
            carried: self.rail_move.get_untracked().is_none_or(|r| r.carried),
        });
        let at = match &rail {
            Some(rail) => self.ws.with_value(|w| Self::rail_cell(w, rail)).unwrap_or(Pos::new(0, 0, 0)),
            None => Pos::new(0, 0, 0),
        };
        self.rail_move.set(rail);
        self.set_relocation(at);
    }

    /// Chooses whether the docked entity comes along with its rail.
    pub fn set_rail_carried(self, on: bool) {
        self.rail_move.update(|r| {
            if let Some(r) = r {
                r.carried = on;
            }
        });
        if let Some(at) = self.relocating.get_untracked() {
            self.set_relocation(at);
        }
    }

    /// Chooses whether, on a turret, the dock goes with the core.
    pub fn set_dock_follows(self, on: bool) {
        self.dock_follows.set(on);
        if let Some(at) = self.relocating.get_untracked() {
            self.set_relocation(at);
        }
    }

    /// Shows a copy of the design with the core moved as chosen, in the
    /// whole-design view with its parts in motion, so a turret can be seen
    /// turning about its new line. Nothing is saved until it is confirmed.
    pub fn preview_relocation(self) {
        if self.core_preview.get_untracked().is_some() {
            return;
        }
        if let Some(job) = self.cloning.get_untracked() {
            return self.preview_clone(job);
        }
        let Some(at) = self.relocating.get_untracked() else { return };
        let (nose, top) = self.core_facing.get_untracked();
        let turning = (nose, top) != CORE_AS_IS;
        let rail = self.rail_move.get_untracked();
        if let Some(rail) = &rail {
            if self.ws.with_value(|w| Self::rail_cell(w, rail)) == Some(at) {
                return self.say("The rail is there now. Choose another cell for it to preview.");
            }
        } else if at == Pos::new(0, 0, 0) && !turning {
            return self.say("The core is there now. Choose another cell, or another way for it to face, to preview.");
        }
        let facing = turning.then(|| (SIDE_DIRS[nose], SIDE_DIRS[top]));
        let mv = yard_core::relocate::Move { to: at, facing, dock_follows: self.dock_follows.get_untracked() };
        let done = self.enter_preview(|w, copy| match &rail {
            Some(rail) => {
                let by = yard_core::relocate::rail_shift(copy, &w.path, &rail.child, at).map_err(sentence)?;
                Ok(yard_core::relocate::move_rail(copy, &rail.child, by, rail.carried, &w.looks).map_err(sentence)?.notes)
            }
            None => Ok(yard_core::relocate::relocate(copy, &w.path, mv, &w.looks).map_err(sentence)?.notes),
        });
        match done {
            Ok((path, notes)) => {
                let about = if rail.is_some() { format!("a rail of {} moved", crate::entity_label(&path).to_lowercase()) } else { format!("the core of {} moved", crate::entity_label(&path).to_lowercase()) };
                self.show_preview(&path, &path, about, &notes);
                self.say(format!("Previewing; nothing is saved. {}", notes.join(" ")));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Shows a copy of the design with the docked entity copied as `job`
    /// says, framed on the copy. Nothing is saved until it is confirmed.
    fn preview_clone(self, job: Cloning) {
        let place = match &job.replace {
            Some(entity) => yard_core::clone::Place::Replace { entity: entity.clone() },
            None => {
                let Some(at) = self.relocating.get_untracked() else { return self.say("Choose the cell for the copy's rail first.") };
                yard_core::clone::Place::New { parent: job.parent.clone(), at, facing: job.facing }
            }
        };
        let mut made_path = String::new();
        let done = self.enter_preview(|w, copy| {
            let made = yard_core::clone::clone_docked(copy, &w.looks, &job.source, &place).map_err(sentence)?;
            made_path = made.child;
            Ok(made.notes)
        });
        match done {
            Ok((path, notes)) => {
                let about = format!("{} copied as {}", crate::entity_label(&job.source).to_lowercase(), crate::entity_label(&made_path).to_lowercase());
                self.show_preview(&path, &made_path, about, &notes);
                self.say(format!("Previewing; nothing is saved. {}", notes.join(" ")));
            }
            Err(e) => self.fail(e),
        }
    }

    /// Puts a copy of the design, changed by `change`, on screen in place
    /// of the design, which is kept to come back to (see
    /// [`Ctx::leave_core_preview`]). Returns the entity being worked on and
    /// what the change said, or what went wrong, in which case nothing
    /// changed on screen.
    fn enter_preview(self, change: impl FnOnce(&Workspace, &mut yard_core::Blueprint) -> Result<Vec<String>, String>) -> Result<(String, Vec<String>), String> {
        self.stop_sim();
        self.playing.set(false);
        let done = self.ws.try_update_value(|w| {
            let path = w.path.clone();
            let camera = w.renderer.as_ref().map(|r| r.camera);
            let mut copy = w.blueprint.as_ref().ok_or_else(|| "Nothing is open.".to_string())?.clone();
            let notes = change(w, &mut copy)?;
            let Some(bp) = w.blueprint.as_mut() else { return Err("Nothing is open.".to_string()) };
            w.stashed = Some(std::mem::replace(bp, copy));
            w.camera_kept = camera;
            w.stale = false;
            (w.sim, w.rig, w.switches) = (None, Rig::default(), None);
            w.sizes.clear();
            Ok((path, notes))
        });
        done.unwrap_or_else(|| Err("Nothing is open.".to_string()))
    }

    /// Shows the copy [`Ctx::enter_preview`] put on screen: the whole-design
    /// view framed on the part at `part`, its parts sweeping so that a
    /// turret's new line shows. `path` is the entity to come back to.
    fn show_preview(self, path: &str, part: &str, about: String, notes: &[String]) {
        self.core_preview.set(Some(CorePreview { path: path.to_string(), part: part.to_string(), about, notes: notes.to_vec() }));
        self.simulating.set(false);
        self.show_whole(false);
        self.frame_part(part);
        if self.parts.with_untracked(|parts| parts.iter().any(|p| p.range.1 > p.range.0)) {
            self.play(true);
        }
    }

    /// Points the camera at one part of the whole-design view, with what is
    /// docked to it and some of its surroundings in view.
    fn frame_part(self, path: &str) {
        self.ws.update_value(|w| {
            let Workspace { renderer: Some(renderer), rig, .. } = w else { return };
            let below = format!("{path}/");
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for part in rig.parts.iter().filter(|p| p.path == path || p.path.starts_with(&below)) {
                let Some((plo, phi)) = placed_box(part) else { continue };
                for a in 0..3 {
                    lo[a] = lo[a].min(plo[a]);
                    hi[a] = hi[a].max(phi[a]);
                }
            }
            if lo[0] <= hi[0] {
                let room = (0..3).map(|a| hi[a] - lo[a]).fold(0.0f32, f32::max).max(6.0);
                renderer.camera.frame(lo.map(|v| v - room), hi.map(|v| v + room));
            }
        });
        self.redraw();
    }

    /// Takes the preview of a core move off the screen: the design as it is
    /// comes back, with the cell still chosen so that it can be changed or
    /// confirmed. Returns the entity whose core was being moved, for the
    /// caller to show; None when nothing was being previewed.
    fn leave_core_preview(self) -> Option<String> {
        let preview = self.core_preview.get_untracked()?;
        self.playing.set(false);
        self.stop_sim();
        let stale = self.ws.try_update_value(|w| {
            if let Some(original) = w.stashed.take() {
                w.blueprint = Some(original);
            }
            if let (Some(renderer), Some(camera)) = (w.renderer.as_mut(), w.camera_kept.take()) {
                renderer.camera = camera;
            }
            (w.sim, w.rig, w.switches) = (None, Rig::default(), None);
            w.sizes.clear();
            std::mem::take(&mut w.stale)
        });
        self.core_preview.set(None);
        self.simulating.set(false);
        if stale == Some(true) {
            if let Some(id) = self.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) {
                self.ws.update_value(|w| w.show_after = Some(preview.path.clone()));
                self.open_design_saying(id, "The design changed meanwhile and was loaded again. Check the cell chosen for the core.");
            }
        }
        Some(preview.path)
    }

    /// Finds, for the turret and each section docked to it down the chain,
    /// the cell for its core where it turns furthest before meeting the
    /// ship or another part (the dock following the core, and no block
    /// left floating outside the structure), makes the moves on a copy and
    /// previews them. Confirm makes them all; Back to choosing keeps them
    /// until another cell is chosen.
    pub fn fit_relocation(self) {
        if self.relocating.get_untracked().is_none() || self.turret_parts.get_untracked().is_none() {
            return self.say("Auto-fit is for a turret whose core is being moved.");
        }
        if self.rail_move.get_untracked().is_some() {
            return self.say("Auto-fit places the core; choose \"The core\" above first.");
        }
        if let Some(path) = self.leave_core_preview() {
            self.show_entity(path, false);
        }
        self.say("Trying every cell of each section's row...");
        let origin = Pos::new(0, 0, 0);
        let mut found: Option<yard_core::pivot::Chain> = None;
        let done = self.enter_preview(|w, copy| {
            let rules = w.looks.rules(0);
            let chain = yard_core::pivot::fit_chain(copy, &w.path, |ty| rules.is_track(ty), &w.looks, &w.looks).map_err(sentence)?;
            let nothing = chain.steps.is_empty();
            // The notes name each section by its path; the names on the left are clearer.
            let notes = chain
                .notes
                .iter()
                .map(|n| {
                    chain
                        .steps
                        .iter()
                        .find_map(|s| n.strip_prefix(&format!("{}: ", if s.path.is_empty() { "the main entity" } else { s.path.as_str() })).map(|rest| format!("{}: {rest}", crate::entity_label(&s.path))))
                        .unwrap_or_else(|| n.clone())
                })
                .collect();
            found = Some(chain);
            // Nothing to move: no preview, and nothing wrong either.
            if nothing { Err(String::new()) } else { Ok(notes) }
        });
        let degrees = |range: f32| format!("{range:.0}°");
        let label = |path: &str| crate::entity_label(path);
        let mut said: Vec<String> = Vec::new();
        let chain = found.unwrap_or_default();
        let (path, notes) = match done {
            Ok(ok) => ok,
            Err(e) if !e.is_empty() => return self.fail(e),
            Err(_) => {
                // Every section is at its best already, or none could be fitted.
                self.set_relocation(origin);
                for (path, now) in &chain.kept {
                    said.push(format!("{} turns {} where its core is, as far as any sound cell of its row allows.", label(path), degrees(*now)));
                }
                for (path, why) in &chain.skipped {
                    said.push(format!("{} was not fitted: {why}.", label(path)));
                }
                if said.is_empty() {
                    said.push("There is no section to fit.".to_string());
                }
                return self.say(format!("Nothing to move. {}", said.join(" ")));
            }
        };
        for step in &chain.steps {
            said.push(format!(
                "{}: core to {}, {}, {}, where it turns {} ({:.0}° one way, {:.0}° the other) against {} now.",
                label(&step.path),
                step.fit.to.x,
                step.fit.to.y,
                step.fit.to.z,
                degrees(step.fit.range()),
                -step.fit.back,
                step.fit.ahead,
                degrees(step.now)
            ));
        }
        for (path, now) in &chain.kept {
            said.push(format!("{} stays: it turns {} where its core is.", label(path), degrees(*now)));
        }
        for (path, why) in &chain.skipped {
            said.push(format!("{} was not fitted: {why}.", label(path)));
        }
        let own = chain.steps.iter().find(|s| s.path == path).map_or(origin, |s| s.fit.to);
        self.dock_follows.set(true);
        self.relocating.set(Some(own));
        self.fit_steps.set(chain.steps.iter().map(|s| (s.path.clone(), s.fit.to)).collect());
        let about = format!("the core of {} moved", crate::entity_label(&path).to_lowercase());
        self.show_preview(&path, &path, about, &notes);
        self.refresh_links();
        self.say(format!(
            "Auto-fit, no block left floating outside the structure: {} Previewing {}; Confirm makes {}, Back to choosing to change it.",
            said.join(" "),
            if chain.steps.len() == 1 { "it" } else { "them" },
            if chain.steps.len() == 1 { "it" } else { "them all" }
        ));
    }

    /// Back from the preview to choosing the core's place.
    pub fn end_relocation_preview(self) {
        if let Some(path) = self.leave_core_preview() {
            self.show_entity(path, false);
            let steps = self.fit_steps.with_untracked(Vec::len);
            self.say(if steps > 1 {
                format!("Back to choosing. The auto-fit's {steps} moves are still set: Enter confirms them all, choosing a cell keeps only this core's move, Escape cancels.")
            } else {
                "Back to choosing. Enter confirms, Escape cancels.".to_string()
            });
        }
    }

    /// Chooses which way the core's nose and top are to point. A top along
    /// the nose's own axis gives way to the first side across it.
    pub fn set_core_facing(self, nose: usize, top: usize) {
        let axis = |side: usize| side / 2;
        let top = if axis(top) == axis(nose) { [2, 0, 4, 3, 1, 5].into_iter().find(|s| axis(*s) != axis(nose)).unwrap_or(2) } else { top };
        self.core_facing.set((nose.min(5), top));
        if let Some(at) = self.relocating.get_untracked() {
            self.set_relocation(at);
        }
    }

    fn set_relocation(self, at: Pos) {
        if let Some(job) = self.cloning.get_untracked() {
            let text = self.ws.with_value(|w| {
                let bp = w.blueprint.as_ref();
                let block = bp.and_then(|bp| bp.entity(&w.path)).map_or(Block::AIR, |e| e.entity.get(at));
                let kind = bp.and_then(|bp| bp.docks().get(&job.source).map(|d| d.rail.ty)).map_or("rail", rail_kind);
                let in_place = if block.is_air() { String::new() } else { format!(", in place of {}", w.looks.name(block.ty())) };
                format!(
                    "The copy's {kind} would go to {}, {}, {}{in_place}, the copy resting on its {} side (its rail docker in the cell that way, which must be empty). Enter confirms, Escape cancels.",
                    at.x,
                    at.y,
                    at.z,
                    side_name(job.facing)
                )
            });
            self.relocating.set(Some(at));
            self.fit_steps.set(Vec::new());
            self.say(text);
            self.refresh_links();
            self.redraw();
            return;
        }
        let with_dock = self.turret_parts.get_untracked().is_some() && self.dock_follows.get_untracked();
        let rail = self.rail_move.get_untracked();
        let text = self.ws.with_value(|w| {
            let block = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map_or(Block::AIR, |e| e.entity.get(at));
            if let Some(rail) = &rail {
                let now = Self::rail_cell(w, rail).unwrap_or(Pos::new(0, 0, 0));
                let who = if rail.own { "this entity".to_string() } else { crate::entity_label(&rail.child).to_lowercase() };
                if at == now {
                    return "The rail is where it is now; choose another cell for it. Enter confirms, Escape cancels.".to_string();
                }
                let in_place = if block.is_air() { String::new() } else { format!(", in place of {}", w.looks.name(block.ty())) };
                return match (rail.own, rail.carried) {
                    (true, true) => format!("This entity and the rail it rests on would move by {}, {}, {}: its rail docker comes to rest where cell {}, {}, {} is now{}. Enter confirms, Escape cancels.", at.x - now.x, at.y - now.y, at.z - now.z, at.x, at.y, at.z, in_place),
                    (true, false) => format!("The rail docker would go to {}, {}, {}{}, and the rail under it on the parent moves to match; this entity stays where it is. Enter confirms, Escape cancels.", at.x, at.y, at.z, in_place),
                    (false, true) => format!("The rail would go to {}, {}, {}{}; {who} comes along with it, docked as it is. Enter confirms, Escape cancels.", at.x, at.y, at.z, in_place),
                    (false, false) => format!("The rail would go to {}, {}, {}{}; {who} stays where it is, its rail docker moving to meet the rail. Enter confirms, Escape cancels.", at.x, at.y, at.z, in_place),
                };
            }
            let dock = if with_dock { " The rail docker and the turret axis go with it, so the turret stays where it is." } else { "" };
            let dock = if self.turret_fixed.get_untracked() { format!("{dock} Its docker faces front or back, so the game holds it still wherever the core goes.") } else { dock.to_string() };
            if at == Pos::new(0, 0, 0) {
                "The core stays here. The arrow points the way its nose will face; choose another cell, or turn it on the right.".to_string()
            } else if block.is_air() {
                format!("The core would go to {}, {}, {}, an empty space.{dock} The arrow points the way its nose will face. Enter confirms, Escape cancels.", at.x, at.y, at.z)
            } else if with_dock {
                format!("The core would go to {}, {}, {}, in place of {}.{dock} The arrow points the way its nose will face. Enter confirms, Escape cancels.", at.x, at.y, at.z, w.looks.name(block.ty()))
            } else {
                format!("The core would go to {}, {}, {}, in place of {} (which goes where the core is now). The arrow points the way its nose will face. Enter confirms, Escape cancels.", at.x, at.y, at.z, w.looks.name(block.ty()))
            }
        });
        self.relocating.set(Some(at));
        self.fit_steps.set(Vec::new());
        self.say(text);
        self.refresh_links();
        self.redraw();
    }

    /// A click while choosing: the block clicked, or with Shift the space in front of it.
    fn relocation_click(self, x: i32, y: i32, in_front: bool) {
        let hit = self.ws.with_value(|w| Self::pick(w, x, y).map(|hit| (if in_front && hit.normal != [0; 3] { hit.outside() } else { hit.pos }, hit.normal)));
        if let Some((at, normal)) = hit {
            // A copy's rail faces the way the face clicked does.
            if normal != [0; 3] && self.cloning.get_untracked().is_some_and(|c| c.replace.is_none()) {
                self.cloning.update(|c| {
                    if let Some(c) = c {
                        c.facing = normal;
                    }
                });
            }
            self.set_relocation(at);
        }
    }

    /// A movement key while choosing: one cell, along the grid axis nearest the way the view looks.
    pub fn nudge_relocation(self, code: &str) {
        let Some(at) = self.relocating.get_untracked() else { return };
        if self.core_preview.get_untracked().is_some() {
            return;
        }
        let (forward, right) = self.ws.with_value(|w| w.renderer.as_ref().map(|r| r.camera.heading())).unwrap_or(([0, 0, -1], [1, 0, 0]));
        let step = match code {
            "KeyW" | "ArrowUp" => forward,
            "KeyS" | "ArrowDown" => forward.map(|v| -v),
            "KeyD" | "ArrowRight" => right,
            "KeyA" | "ArrowLeft" => right.map(|v| -v),
            "KeyE" | "PageUp" => [0, 1, 0],
            "KeyQ" | "PageDown" => [0, -1, 0],
            _ => return,
        };
        self.set_relocation(at.offset(step[0], step[1], step[2]));
    }

    pub fn confirm_relocation(self) {
        if let Some(job) = self.cloning.get_untracked() {
            return self.confirm_clone(job);
        }
        let Some(at) = self.relocating.get_untracked() else { return };
        // From the preview, the design as it is comes back first (the entity
        // being worked on is the one the preview was of, not the whole
        // design on screen); what the server makes of it is loaded afresh.
        let entity = match self.leave_core_preview() {
            Some(path) => {
                self.show_entity(path.clone(), false);
                path
            }
            None => self.entity_path.get_untracked(),
        };
        let (nose, top) = self.core_facing.get_untracked();
        let turning = (nose, top) != CORE_AS_IS;
        let rail = self.rail_move.get_untracked();
        let steps = self.fit_steps.get_untracked();
        if let Some(rail) = &rail {
            if self.ws.with_value(|w| Self::rail_cell(w, rail)) == Some(at) {
                return self.say("The rail is there now. Choose another cell for it.");
            }
        } else if at == Pos::new(0, 0, 0) && !turning && steps.is_empty() {
            return self.say("The core is there now. Choose another cell, or another way for it to face.");
        }
        let Some(summary) = self.current.get_untracked() else { return };
        if self.pending.get_untracked() > 0 {
            return self.say("Still saving the last edit; try again in a moment.");
        }
        let rev = self.ws.with_value(|w| w.rev);
        if rail.is_none() && !turning && !steps.is_empty() {
            self.say(if steps.len() == 1 { "Moving the core...".to_string() } else { format!("Moving the cores of {} sections...", steps.len()) });
            let what = if steps.len() == 1 { "Fitted the pivot".to_string() } else { format!("Fitted the pivots of {} sections", steps.len()) };
            spawn_local(async move {
                match api::fit_pivots(&summary.id, rev, steps.into_iter().map(|(e, to)| (e, to.to_array())).collect()).await {
                    Ok(done) => self.structure_done(summary.id, done, &what),
                    Err(e) => self.structure_failed(summary.id, e),
                }
            });
            return;
        }
        if let Some(rail) = rail {
            self.say("Moving the rail...");
            spawn_local(async move {
                match api::move_rail(&summary.id, rev, &entity, &rail.child, at.to_array(), rail.carried).await {
                    Ok(done) => self.structure_done(summary.id, done, "Moved the rail"),
                    Err(e) => self.structure_failed(summary.id, e),
                }
            });
            return;
        }
        let dock_follows = self.dock_follows.get_untracked();
        self.say(if turning { "Moving and turning the core..." } else { "Moving the core..." });
        let facing = turning.then(|| (SIDE_DIRS[nose], SIDE_DIRS[top]));
        spawn_local(async move {
            match api::relocate_core(&summary.id, rev, &entity, at.to_array(), facing, dock_follows).await {
                Ok(done) => self.structure_done(summary.id, done, if turning { "Moved and turned the core" } else { "Moved the core" }),
                Err(e) => self.structure_failed(summary.id, e),
            }
        });
    }

    /// Makes the copy `job` describes on the server.
    fn confirm_clone(self, job: Cloning) {
        let new = match &job.replace {
            Some(_) => None,
            None => {
                let Some(at) = self.relocating.get_untracked() else { return self.say("Choose the cell for the copy's rail first.") };
                Some((job.parent.clone(), at.to_array(), job.facing))
            }
        };
        if let Some(path) = self.leave_core_preview() {
            self.show_entity(path, false);
        }
        let Some(summary) = self.current.get_untracked() else { return };
        if self.pending.get_untracked() > 0 {
            return self.say("Still saving the last edit; try again in a moment.");
        }
        let rev = self.ws.with_value(|w| w.rev);
        self.say("Copying...");
        spawn_local(async move {
            match api::clone_docked(&summary.id, rev, &job.source, job.replace.clone(), new).await {
                Ok(done) => self.structure_done(summary.id, done, "Copied the entity"),
                Err(e) => self.structure_failed(summary.id, e),
            }
        });
    }

    // ---- copying a docked entity ----

    /// The docked entities a copy of the one on screen could take the place
    /// of: every other one that does not hold it, by path and name.
    pub fn clone_targets(self) -> Vec<(String, String)> {
        let source = self.entity_path.get_untracked();
        self.ws.with_value(|w| {
            let Some(bp) = w.blueprint.as_ref() else { return Vec::new() };
            bp.docks().keys().filter(|p| **p != source && !source.starts_with(&format!("{p}/"))).map(|p| (p.clone(), crate::entity_label(p))).collect()
        })
    }

    /// The entity the docked entity at `path` hangs from.
    fn parent_path(path: &str) -> String {
        path.rsplit_once('/').map_or(String::new(), |(p, _)| p.to_string())
    }

    /// Previews a copy of the docked entity on screen in place of the one at `target`.
    pub fn clone_in_place_of(self, target: String) {
        let source = self.entity_path.get_untracked();
        if source == WHOLE || source.is_empty() {
            return self.say("Choose a docked entity on the left first: that is what gets copied.");
        }
        if target.is_empty() {
            return self.say("Choose on the right which docked entity the copy takes the place of.");
        }
        self.stop_sim();
        self.turret_box.set(None);
        self.relocating.set(None);
        let job = Cloning { source, parent: Self::parent_path(&target), replace: Some(target), facing: [0, 0, 0] };
        self.cloning.set(Some(job.clone()));
        self.preview_clone(job);
    }

    /// Starts choosing the cell on the parent for the rail a copy of the
    /// docked entity on screen is to hang from: the parent comes on screen,
    /// with the cell of the original's rail chosen to begin with.
    pub fn start_cloning(self) {
        let source = self.entity_path.get_untracked();
        if source == WHOLE || source.is_empty() {
            return self.say("Choose a docked entity on the left first: that is what gets copied.");
        }
        let rail = self.ws.with_value(|w| w.blueprint.as_ref()?.docks().get(&source).map(|d| d.rail));
        let Some(rail) = rail else { return self.say("This entity has no dock on record, so a copy of it has nothing to hang from.") };
        self.stop_sim();
        self.turret_box.set(None);
        let (up, _) = yard_core::shape::turned_axes(rail.orientation);
        let parent = Self::parent_path(&source);
        self.cloning.set(Some(Cloning { source: source.clone(), parent: parent.clone(), replace: None, facing: up }));
        self.show_entity(parent.clone(), true);
        self.set_relocation(rail.pos);
        self.say(format!(
            "Choosing where the copy of {} goes on {}: click a block to make it the rail (the copy rests on the face clicked), or Shift-click the space in front of it; W A S D E Q move it, Enter confirms, Escape cancels. The orange box is the original's rail. Preview shows the copy in place.",
            crate::entity_label(&source).to_lowercase(),
            crate::entity_label(&parent).to_lowercase()
        ));
    }

    /// Chooses the side the copy rests on: the way its rail faces.
    pub fn set_clone_facing(self, side: usize) {
        self.cloning.update(|c| {
            if let Some(c) = c {
                c.facing = SIDE_DIRS[side.min(5)];
            }
        });
        if let Some(at) = self.relocating.get_untracked() {
            self.set_relocation(at);
        }
    }

    /// The design has changed shape on the server: load it again and show what was made.
    fn structure_done(self, id: String, done: yard_core::api::Restructured, what: &str) {
        self.cloning.set(None);
        self.relocating.set(None);
        self.core_facing.set(CORE_AS_IS);
        self.turret_parts.set(None);
        self.rail_move.set(None);
        self.rails_to_move.set(Vec::new());
        self.fit_steps.set(Vec::new());
        self.turret_box.set(None);
        self.structure_undo.set(Some(what.to_string()));
        self.ws.update_value(|w| w.show_after = Some(done.entity.clone()));
        self.open_design(id, true);
        self.ws.update_value(|w| w.notice = Some(format!("{what}. {}", done.notes.join(" "))));
    }

    /// A change of structure was turned down. If this page was behind the
    /// server, the design is loaded again; the box or cell chosen stays, so
    /// the change can simply be asked for once more.
    fn structure_failed(self, id: String, failure: StepFailure) {
        match failure {
            StepFailure::Refused(why) => self.fail(why),
            StepFailure::OutOfStep(why) => self.open_design_saying(id, &format!("{why} It was loaded again; try once more.")),
        }
    }

    /// Deletes the open design, once the player has said yes, and goes back to the start page.
    pub fn delete_design(self) {
        let Some(summary) = self.current.get_untracked() else { return };
        self.say("Deleting...");
        spawn_local(async move {
            match api::delete_design(&summary.id).await {
                Ok(()) => {
                    self.deleting.set(false);
                    // Nothing of the design is left to show: start afresh.
                    let _ = window().location().set_href("/");
                }
                Err(e) => {
                    self.deleting.set(false);
                    self.fail(e);
                }
            }
        });
    }

    /// Puts the design back as it was before the last relocation or turret.
    pub fn restore_structure(self) {
        let Some(summary) = self.current.get_untracked() else { return };
        let (rev, path) = (self.ws.with_value(|w| w.rev), self.entity_path.get_untracked());
        spawn_local(async move {
            match api::restore_structure(&summary.id, rev).await {
                Ok(done) => {
                    self.structure_undo.set(None);
                    self.ws.update_value(|w| w.show_after = Some(path));
                    self.open_design(summary.id, true);
                    self.ws.update_value(|w| w.notice = Some(done.notes.join(" ")));
                }
                Err(e) => {
                    self.structure_undo.set(None);
                    self.structure_failed(summary.id, e);
                }
            }
        });
    }

    // ---- making a turret ----

    /// The box chosen for a turret, once both corners are.
    fn turret_bounds(self) -> Option<(Pos, Pos)> {
        match self.turret_box.get_untracked() {
            Some((a, Some(b))) => Some((a.lo(b), a.hi(b))),
            _ => None,
        }
    }

    /// The handle of the turret box nearest the ray under the pointer, if
    /// one is close enough to have been meant.
    fn turret_handle_at(self, x: i32, y: i32) -> Option<Grab> {
        let (lo, hi) = self.turret_bounds()?;
        let dock = docker_cell(lo, hi, self.turret_base.get_untracked(), self.turret_docker.get_untracked());
        let (origin, dir) = self.ws.with_value(|w| w.renderer.as_ref().map(|r| r.pointer_ray(x, y)))?;
        let handles = (0..8u8).map(|c| (Grab::Corner(c), corner_cell(lo, hi, c))).chain([(Grab::Docker, dock)]);
        let mut best: Option<(f32, Grab)> = None;
        for (grab, at) in handles {
            let to = [at.x as f32 - origin[0], at.y as f32 - origin[1], at.z as f32 - origin[2]];
            let along: f32 = (0..3).map(|i| to[i] * dir[i]).sum();
            if along <= 0.0 {
                continue;
            }
            let off = ((0..3).map(|i| (to[i] - dir[i] * along).powi(2)).sum::<f32>()).sqrt();
            // Within the cell itself, or a little way off it on screen.
            let angle = off / along;
            if (off <= 0.9 || angle <= HANDLE_REACH) && best.is_none_or(|(a, _)| angle < a) {
                best = Some((angle, grab));
            }
        }
        best.map(|(_, g)| g)
    }

    /// Says what the turret box is and what is grabbed, and redraws it.
    fn turret_changed(self) {
        let text = match (self.turret_box.get_untracked(), self.turret_grab.get_untracked()) {
            (None, _) => String::new(),
            (Some((_, None)), _) => "Corner chosen. W A S D E Q move it; click the opposite corner of the part to cut out.".to_string(),
            (Some((a, Some(b))), grab) => {
                let (lo, hi) = (a.lo(b), a.hi(b));
                let size = format!("{} by {} by {} blocks.", hi.x - lo.x + 1, hi.y - lo.y + 1, hi.z - lo.z + 1);
                match grab {
                    Grab::Nothing => format!("{size} Click a corner or the yellow rail docker (or press Tab) to move it; Make turret when it is right."),
                    Grab::Corner(c) => format!("{size} Moving the {}: W A S D E Q. Tab moves to the next corner or the docker, Enter makes the turret, Escape lets go.", corner_name(c)),
                    Grab::Docker => {
                        let at = docker_cell(lo, hi, self.turret_base.get_untracked(), self.turret_docker.get_untracked());
                        format!("Rail docker at {}, {}, {}. W A S D E Q move it along the face that rests on the ship; Tab moves to the next corner, Enter makes the turret, Escape lets go.", at.x, at.y, at.z)
                    }
                }
            }
        };
        self.say(text);
        self.refresh_links();
        self.redraw();
    }

    /// A click with the Turret tool: a handle of the box grabs it; otherwise
    /// the first corner of a box, then the second, then a new box.
    fn turret_click(self, x: i32, y: i32) {
        if let Some(grab) = self.turret_handle_at(x, y) {
            self.turret_grab.set(grab);
            return self.turret_changed();
        }
        let Some(at) = self.ws.with_value(|w| Self::pick(w, x, y).map(|hit| hit.pos)) else { return };
        match self.turret_box.get_untracked() {
            Some((first, None)) => {
                let (lo, hi) = (first.lo(at), first.hi(at));
                self.turret_box.set(Some((lo, Some(hi))));
                // The corner just clicked stays in hand.
                let c = u8::from(at.x == hi.x && at.x != lo.x) | u8::from(at.y == hi.y && at.y != lo.y) << 1 | u8::from(at.z == hi.z && at.z != lo.z) << 2;
                self.turret_grab.set(Grab::Corner(c));
                self.turret_docker.set(None);
            }
            _ => {
                self.turret_box.set(Some((at, None)));
                self.turret_grab.set(Grab::Corner(0));
                self.turret_docker.set(None);
                self.turret_excluded.set(BTreeSet::new());
            }
        }
        self.turret_changed();
    }

    /// Whether the movement keys are moving part of the turret box.
    pub fn turret_grabbed(self) -> bool {
        self.turret_box.get_untracked().is_some() && self.turret_grab.get_untracked() != Grab::Nothing
    }

    /// Takes a handle of the turret box in hand, for the movement keys.
    pub fn grab_turret(self, grab: Grab) {
        self.turret_grab.set(grab);
        self.turret_changed();
    }

    /// A right-click with the Turret tool: the blocks of the box in the brush
    /// are left out of the turret, or, where the block clicked is left out
    /// already, taken back in. With Shift (`section`), the brush is the whole
    /// section of the box the block is part of instead: every block reached
    /// from it through blocks touching at a face, an edge or a corner, with
    /// air all round it.
    fn turret_exclude(self, x: i32, y: i32, section: bool) {
        let Some((lo, hi)) = self.turret_bounds() else {
            return self.say("Mark the box first: click two blocks at opposite corners. Then right-click blocks in it to leave them on the ship.");
        };
        let size = self.brush.get_untracked();
        let Some((hit, a, b)) = self.ws.with_value(|w| {
            let hit = Self::pick(w, x, y)?;
            let (a, b) = Self::brush_box(Act::Exclude, &hit, size)?;
            Some((hit.pos, a, b))
        }) else {
            return;
        };
        let in_box = |p: Pos| p.lo(lo) == lo && p.hi(hi) == hi;
        if !in_box(hit) {
            return self.say("That block is outside the turret box.");
        }
        let back_in = self.turret_excluded.get_untracked().contains(&hit);
        let cells: Vec<Pos> = self.ws.with_value(|w| {
            let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity) else { return Vec::new() };
            if section {
                return turret_section(entity, hit, lo, hi);
            }
            let (a, b) = (a.hi(lo), b.lo(hi));
            let mut cells = Vec::new();
            for z in a.z..=b.z {
                for y in a.y..=b.y {
                    for x in a.x..=b.x {
                        let p = Pos::new(x, y, z);
                        if !entity.get(p).is_air() {
                            cells.push(p);
                        }
                    }
                }
            }
            cells
        });
        let (base, docker) = (self.turret_base.get_untracked(), self.turret_docker.get_untracked());
        let (dock, core) = turret_fixed_cells(lo, hi, base, docker);
        let kept_in = !back_in && cells.iter().any(|p| *p == dock || *p == core);
        self.turret_excluded.update(|set| {
            for p in cells {
                if back_in {
                    set.remove(&p);
                } else {
                    set.insert(p);
                }
            }
        });
        let count = self.turret_excluded.with_untracked(|set| turret_left_out(set, lo, hi, base, docker).len());
        let mut text = match count {
            0 => "Every block in the box goes with the turret.".to_string(),
            1 => "1 block in the box stays on the ship (red). Right-click it to take it back.".to_string(),
            n => format!("{n} blocks in the box stay on the ship (red). Right-click one to take those in the brush back."),
        };
        if kept_in {
            text.push_str(" The rail docker's cell and the core's above it go with the turret.");
        }
        self.say(text);
        self.refresh_links();
        self.redraw();
    }

    /// How many blocks of the box are left out of the turret now.
    pub fn turret_left_out_count(self) -> usize {
        let (base, docker) = (self.turret_base.get(), self.turret_docker.get());
        match self.turret_box.get() {
            Some((a, Some(b))) => self.turret_excluded.with(|set| turret_left_out(set, a.lo(b), a.hi(b), base, docker).len()),
            _ => 0,
        }
    }

    /// Takes every left-out block of the box back into the turret.
    pub fn include_all_turret(self) {
        self.turret_excluded.set(BTreeSet::new());
        self.turret_changed();
    }

    /// Lets go of the grabbed handle, so the keys fly the view again.
    pub fn release_turret(self) {
        self.turret_grab.set(Grab::Nothing);
        self.turret_changed();
    }

    /// Grabs the next handle of the box: the corners in turn, then the docker.
    pub fn next_turret_handle(self) {
        if self.turret_bounds().is_none() {
            return;
        }
        self.turret_grab.update(|g| {
            *g = match *g {
                Grab::Nothing => Grab::Corner(0),
                Grab::Corner(7) => Grab::Docker,
                Grab::Corner(c) => Grab::Corner(c + 1),
                Grab::Docker => Grab::Corner(0),
            }
        });
        self.turret_changed();
    }

    /// A movement key while a handle is grabbed: one cell, along the grid
    /// axis nearest the way the view looks.
    pub fn nudge_turret(self, code: &str) {
        let (forward, right) = self.ws.with_value(|w| w.renderer.as_ref().map(|r| r.camera.heading())).unwrap_or(([0, 0, -1], [1, 0, 0]));
        let step = match code {
            "KeyW" | "ArrowUp" => forward,
            "KeyS" | "ArrowDown" => forward.map(|v| -v),
            "KeyD" | "ArrowRight" => right,
            "KeyA" | "ArrowLeft" => right.map(|v| -v),
            "KeyE" | "PageUp" => [0, 1, 0],
            "KeyQ" | "PageDown" => [0, -1, 0],
            _ => return,
        };
        let Some((first, second)) = self.turret_box.get_untracked() else { return };
        let Some(second) = second else {
            self.turret_box.set(Some((first.offset(step[0], step[1], step[2]), None)));
            return self.turret_changed();
        };
        let (lo, hi) = (first.lo(second), first.hi(second));
        let base = self.turret_base.get_untracked();
        match self.turret_grab.get_untracked() {
            Grab::Nothing => return,
            Grab::Corner(c) => {
                let (mut l, mut h, mut c) = (lo.to_array(), hi.to_array(), c);
                for a in 0..3 {
                    if c >> a & 1 == 1 {
                        h[a] += step[a];
                    } else {
                        l[a] += step[a];
                    }
                    // Pushed past the opposite corner: it becomes that side's corner.
                    if l[a] > h[a] {
                        std::mem::swap(&mut l[a], &mut h[a]);
                        c ^= 1 << a;
                    }
                }
                let (lo, hi) = (Pos::new(l[0], l[1], l[2]), Pos::new(h[0], h[1], h[2]));
                self.turret_box.set(Some((lo, Some(hi))));
                self.turret_grab.set(Grab::Corner(c));
                // The docker keeps its place on the base face, wherever that now is.
                if let Some(d) = self.turret_docker.get_untracked() {
                    self.turret_docker.set(Some(docker_cell(lo, hi, base, Some(d))));
                }
            }
            Grab::Docker => {
                let now = docker_cell(lo, hi, base, self.turret_docker.get_untracked());
                let moved = docker_cell(lo, hi, base, Some(now.offset(step[0], step[1], step[2])));
                if moved == now {
                    self.say("The rail docker keeps to the face of the box that rests on the ship, inside the box. Move a corner to make the box bigger, or choose another side.");
                    return;
                }
                self.turret_docker.set(Some(moved));
            }
        }
        self.turret_changed();
    }

    pub fn clear_turret(self) {
        if self.turret_box.get_untracked().is_some() {
            self.turret_box.set(None);
            self.turret_grab.set(Grab::Nothing);
            self.turret_docker.set(None);
            self.turret_excluded.set(BTreeSet::new());
            self.say("");
            self.refresh_links();
            self.redraw();
        }
    }

    /// The chosen side changed: the docker goes to the middle of the new face.
    pub fn turret_base_changed(self) {
        self.turret_docker.set(None);
        self.turret_changed();
    }

    pub fn make_turret(self) {
        let Some((lo, hi)) = self.turret_bounds() else {
            return self.say("Choose the two corners of the part first: click one block, then the opposite one.");
        };
        let (Some(summary), entity) = (self.current.get_untracked(), self.entity_path.get_untracked()) else { return };
        if entity == WHOLE {
            return self.say("Choose an entity on the left first.");
        }
        if self.pending.get_untracked() > 0 {
            return self.say("Still saving the last edit; try again in a moment.");
        }
        let (rev, base) = (self.ws.with_value(|w| w.rev), self.turret_base.get_untracked());
        let docker = docker_cell(lo, hi, base, self.turret_docker.get_untracked());
        let exclude: Vec<[i32; 3]> = turret_left_out(&self.turret_excluded.get_untracked(), lo, hi, base, self.turret_docker.get_untracked()).iter().map(|p| p.to_array()).collect();
        self.say("Cutting out the turret...");
        spawn_local(async move {
            match api::make_turret(&summary.id, rev, &entity, lo.to_array(), hi.to_array(), base, docker.to_array(), exclude).await {
                Ok(done) => self.structure_done(summary.id, done, "Made a turret"),
                Err(e) => self.structure_failed(summary.id, e),
            }
        });
    }

    /// Makes a block the one whose links are shown; `None` shows none.
    pub fn select(self, at: Option<Pos>) {
        self.selected.set(at);
        self.refresh_links();
        self.refresh_overlay();
        self.redraw();
    }

    /// Selects a block, or lets go of it if it is the selected one already.
    fn toggle_selection(self, at: Pos) {
        self.select((self.selected.get_untracked() != Some(at)).then_some(at));
    }

    /// The game's C key: selects the block under the pointer. Pressed over
    /// the selected block or over nothing, it lets go of the selection.
    pub fn select_under_pointer(self) {
        let at = self.ws.with_value(|w| w.pointer.and_then(|(x, y)| Self::pick(w, x, y)).map(|hit| hit.pos));
        match at {
            Some(at) => self.toggle_selection(at),
            None => self.select(None),
        }
    }

    /// The game's V key, for the block under the pointer.
    pub fn connect_under_pointer(self, group: bool) {
        if let Some((x, y)) = self.ws.with_value(|w| w.pointer) {
            self.connect_at(x, y, group);
        }
    }

    /// Looks at a block: turns the view so it is in the middle.
    pub fn look_at(self, at: Pos) {
        self.ws.update_value(|w| {
            if let Some(r) = &mut w.renderer {
                r.camera.frame(corner(at, -4.0), corner(at, 4.0));
            }
        });
        self.refresh_overlay();
        self.redraw();
    }

    /// Unlinks every block of one type from the selected block.
    pub fn unlink_type(self, ty: u16) {
        let Some(controller) = self.selected.get_untracked() else { return };
        let targets: Vec<Pos> = self.ws.with_value(|w| {
            let entity = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map(|e| &e.entity);
            entity.and_then(|e| e.links().get(&controller)).and_then(|groups| groups.get(&ty)).map(|set| set.iter().copied().collect()).unwrap_or_default()
        });
        if !targets.is_empty() {
            self.submit(vec![Op::Unlink { controller, targets }]);
        }
    }

    /// Links the block at a point of the page to the selected block, or
    /// unlinks it if it is linked: the game's V key. With `group`, every block
    /// of the same type that hangs together with it goes along.
    pub fn connect_at(self, x: i32, y: i32, group: bool) {
        let from = self.selected.get_untracked();
        let planned = self.ws.with_value(|w| {
            let hit = Self::pick(w, x, y)?;
            let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
            Some(Self::link_plan(w, from, hit.pos).map(|(controller, step)| {
                // Rail track hangs together only where it points the same way.
                let track = Self::rules(w).is_track(entity.get(hit.pos).ty());
                let targets = if group { links::touching_group(entity, hit.pos, GROUP_LIMIT, track) } else { vec![hit.pos] };
                let cut_off = group && targets.len() >= GROUP_LIMIT;
                let names = (w.looks.name(entity.get(hit.pos).ty()), w.looks.name(entity.get(controller).ty()));
                // The step applies the game's rules: a block that can have
                // one controller leaves the one it had, and so on.
                let op = match step {
                    LinkStep::Unlink => Op::Unlink { controller, targets },
                    LinkStep::Link => Op::Link { controller, targets },
                };
                (vec![op], names, cut_off)
            }))
        });
        match planned {
            Some(Ok((ops, (target, controller), cut_off))) => {
                let Some(change) = self.submit(ops) else { return };
                let mine = |list: &[(Pos, u16, Pos)]| list.iter().filter(|(c, _, _)| Some(*c) == from).count();
                let (linked, unlinked) = (mine(&change.linked), mine(&change.unlinked));
                let mut text = if linked > 0 {
                    format!("Linked {linked} x {target} to the {controller}.")
                } else {
                    format!("Unlinked {unlinked} x {target} from the {controller}.")
                };
                // What else the game's rules did: a module left its old
                // computer, a computer went back to the ship core.
                let (taken, given) = (change.unlinked.len() - unlinked, change.linked.len() - linked);
                if linked > 0 && taken > 0 {
                    text.push_str(&format!(" {taken} other link{} gave way, as in the game.", if taken == 1 { "" } else { "s" }));
                }
                if given > 0 {
                    text.push_str(" What was let go answers to the ship core again.");
                }
                if cut_off {
                    text.push_str(&format!(" The group is larger than {GROUP_LIMIT} blocks; the rest was left."));
                }
                self.say(text);
            }
            Some(Err(why)) => self.fail(why),
            None => {}
        }
    }

    /// How many settings the rotation control has for a block type.
    pub fn turns(self, ty: u16) -> u8 {
        let kind = self.ws.with_value(|w| w.looks.kind(ty));
        match (kind.shape, kind.faces_matter) {
            (Shape::Cube, true) => 7, // automatic, then the six directions
            (shape, faces) => ShapeSet::choices(shape, faces),
        }
    }

    /// Steps the rotation of the selected block forward or back.
    pub fn rotate(self, by: i32) {
        if let (Tool::Pick, Some(at)) = (self.tool.get_untracked(), self.picked.get_untracked()) {
            return self.rotate_picked(at, by);
        }
        let n = self.turns(self.block_id.get_untracked()) as i32;
        if n > 1 {
            self.turn.update(|t| *t = (*t as i32 + by).rem_euclid(n) as u8);
        }
    }

    /// Selects the block to place and resets the rotation to that block's default.
    pub fn choose_block(self, ty: u16) {
        let kind = self.ws.with_value(|w| w.looks.kind(ty));
        // Rails and their kind start pointing up with their front forward.
        self.turn.set(if kind.shape == Shape::Rail { 14 } else { 0 });
        self.block_id.set(ty);
        match (self.tool.get_untracked(), self.paint_mode.get_untracked(), self.swap_from.get_untracked(), self.picked.get_untracked()) {
            (Tool::Paint, PaintMode::Swap, Some(from), _) => self.swap_material(from, ty),
            (Tool::Pick, _, _, Some(at)) => self.replace_picked(at, ty),
            _ => {}
        }
    }

    /// The turn a picked block gives the rotation shown in the panel.
    fn turn_of(kind: crate::looks::Kind, block: Block) -> u8 {
        match (kind.shape, kind.faces_matter) {
            (Shape::Cube, false) => 0,
            (Shape::Cube, true) => block.orientation() % 6 + 1,
            _ => block.orientation(),
        }
    }

    /// Turns the picked block by one step of its own turns.
    fn rotate_picked(self, at: Pos, by: i32) {
        let planned = self.ws.with_value(|w| {
            let old = w.blueprint.as_ref()?.entity(&w.path)?.entity.get(at);
            if old.is_air() {
                return None;
            }
            let kind = w.looks.kind(old.ty());
            let n = ShapeSet::choices(kind.shape, kind.faces_matter) as i32;
            if n <= 1 {
                return Some(Err(format!("{} has no rotation.", w.looks.name(old.ty()))));
            }
            let mut new = old;
            new.set_orientation((old.orientation() as i32 % n + by).rem_euclid(n) as u8);
            Some(Ok((new, Self::turn_of(kind, new))))
        });
        match planned {
            None => self.picked.set(None),
            Some(Err(text)) => self.say(text),
            Some(Ok((new, turn))) => {
                if self.submit(vec![Op::Set { cells: vec![(at, new)], when: When::Occupied }]).is_some() {
                    self.turn.set(turn);
                }
            }
        }
    }

    /// Turns every block of the picked block's type in the entity on screen
    /// the way the picked block faces, as one step that Undo takes back.
    pub fn turn_all_like_picked(self) {
        let Some(at) = self.picked.get_untracked() else {
            return self.say("Click a block with the Pick tool first.");
        };
        let planned = self.ws.with_value(|w| {
            let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
            let model = entity.get(at);
            if model.is_air() {
                return None;
            }
            let (ty, name, kind) = (model.ty(), w.looks.name(model.ty()), w.looks.kind(model.ty()));
            if ShapeSet::choices(kind.shape, kind.faces_matter) <= 1 && !(kind.shape == Shape::Cube && kind.faces_matter) {
                return Some(Err(format!("{name} has no rotation.")));
            }
            let mut cells = Vec::new();
            entity.for_each(|p, b| {
                if b.ty() == ty && b.orientation() != model.orientation() {
                    let mut turned = b;
                    turned.set_orientation(model.orientation());
                    cells.push((p, turned));
                }
            });
            Some(Ok((cells, name)))
        });
        match planned {
            None => self.picked.set(None),
            Some(Err(text)) => self.say(text),
            Some(Ok((cells, name))) if cells.is_empty() => self.say(format!("Every {name} in this entity faces this way already.")),
            Some(Ok((cells, name))) => {
                let count = cells.len();
                if self.submit(vec![Op::Set { cells, when: When::Occupied }]).is_some() {
                    self.say(format!("Turned {} {name} to face the way the picked one does. Undo puts them back.", crate::thousands(count as u64)));
                }
            }
        }
    }

    /// Puts a block of type `ty` in place of the picked block, turned as it was where it can be.
    fn replace_picked(self, at: Pos, ty: u16) {
        if ty == edit::CORE_TYPE {
            return self.say("There is one core; it cannot be placed. Relocate core moves it.");
        }
        let planned = self.ws.with_value(|w| {
            let old = w.blueprint.as_ref()?.entity(&w.path)?.entity.get(at);
            if old.is_air() || old.ty() == ty {
                return None;
            }
            let (was, kind) = (w.looks.kind(old.ty()), w.looks.kind(ty));
            let n = ShapeSet::choices(kind.shape, kind.faces_matter);
            let orientation = if (was.shape, was.faces_matter) == (kind.shape, kind.faces_matter) { old.orientation() } else if n <= 1 { 0 } else { old.orientation() % n };
            let new = Block::new(ty, orientation, kind.active);
            Some((new, Self::turn_of(kind, new)))
        });
        if let Some((new, turn)) = planned {
            if self.submit(vec![Op::Set { cells: vec![(at, new)], when: When::Occupied }]).is_some() {
                self.turn.set(turn);
            }
        }
    }

    /// A block type's name as the player sees it.
    pub fn block_name(self, ty: u16) -> String {
        self.ws.with_value(|w| w.looks.name(ty))
    }

    /// Removes every block of the type marked in the list on the left from
    /// the entity on screen, as one step that Undo takes back.
    pub fn remove_marked_type(self) {
        let Some(ty) = self.highlight.get_untracked() else {
            return self.say("Click a block in the list on the left first.");
        };
        if self.entity_path.get_untracked() == WHOLE {
            return self.say("Choose an entity on the left first.");
        }
        let name = self.ws.with_value(|w| w.looks.name(ty));
        if ty == edit::CORE_TYPE {
            return self.say("The core cannot be removed.");
        }
        if self.submit(vec![Op::Retype { pairs: vec![(ty, 0)] }]).is_some() {
            let count = self.ws.with_value(|w| w.counts.get(&ty).copied().unwrap_or(0));
            if count <= 0 {
                self.highlight.set(None);
            }
            self.refresh_links();
            self.redraw();
            self.say(format!("Removed every {name} from this entity. Undo puts them back."));
        }
    }

    /// A click in the list of blocks used: with the Paint tool's Swap
    /// mode, the material to swap out; otherwise marks those blocks.
    pub fn used_block_clicked(self, ty: u16, name: String) {
        if (self.tool.get_untracked(), self.paint_mode.get_untracked()) != (Tool::Paint, PaintMode::Swap) {
            return self.toggle_highlight(ty, name);
        }
        if self.ws.with_value(|w| w.looks.material(ty)).is_none() {
            return self.say(format!("{name} does not come in other colours, so it cannot be swapped."));
        }
        self.swap_from.set(Some(ty));
        if self.highlight.get_untracked() != Some(ty) {
            self.toggle_highlight(ty, name.clone());
        }
        self.say(format!("Swapping out {name}: now choose the new block in the block list on the right."));
    }

    /// Swaps the material of block type `from` for that of `to` all over the entity on screen.
    fn swap_material(self, from: u16, to: u16) {
        if self.entity_path.get_untracked() == WHOLE {
            return self.say("Choose an entity on the left first.");
        }
        let plan = self.ws.with_value(|w| {
            let (a, b) = (w.looks.material(from)?, w.looks.material(to)?);
            let (pairs, missing) = w.looks.swap_pairs(a, b);
            Some((a == b, pairs, missing.iter().map(|ty| w.looks.name(*ty)).collect::<Vec<_>>(), w.looks.name(a), w.looks.name(b)))
        });
        let Some((same, pairs, missing, from_name, to_name)) = plan else {
            return self.say("Only blocks that come in colours (armor, hull and the like) can be swapped. Choose one of those on the right.");
        };
        if same {
            return self.say(format!("That is {from_name} already. Choose another block on the right."));
        }
        let whole = self.swap_whole.get_untracked();
        // The entities that hold the material: the one on screen, or with
        // the whole design, every one, the one on screen first.
        let targets: Vec<String> = self
            .ws
            .with_value(|w| {
                let here = w.path.clone();
                if !whole {
                    return pairs.iter().any(|(ty, _)| w.counts.get(ty).is_some_and(|n| *n > 0)).then_some(vec![here]);
                }
                let bp = w.blueprint.as_ref()?;
                let mut all: Vec<String> = bp
                    .entities()
                    .into_iter()
                    .filter(|(_, e)| {
                        let counts = e.entity.counts_by_type();
                        pairs.iter().any(|(ty, _)| counts.get(ty).is_some_and(|n| *n > 0))
                    })
                    .map(|(path, _)| path)
                    .collect();
                all.sort_by_key(|p| *p != here);
                Some(all)
            })
            .unwrap_or_default();
        if targets.is_empty() {
            self.swap_from.set(None);
            return self.say(if whole {
                format!("The design has no {from_name} anywhere, so there is nothing to swap.")
            } else {
                format!("This entity has no {from_name}, so there is nothing to swap. The list of blocks on the left is for the entity on screen; tick \"Whole design\" to swap in every docked entity too.")
            });
        }
        let (mut count, mut entities) = (0, 0);
        for path in &targets {
            match self.submit_to(path.clone(), vec![Op::Retype { pairs: pairs.clone() }]) {
                Some(change) => {
                    count += change.cells.len();
                    entities += 1;
                }
                None => {
                    // Refused: submit has said why. What went before stands.
                    self.swap_from.set(None);
                    return;
                }
            }
        }
        self.swap_from.set(None);
        if self.highlight.get_untracked().is_some() {
            self.highlight.set(None);
            self.refresh_links();
            self.redraw();
        }
        let mut text = if entities > 1 {
            format!("Swapped {count} blocks of {from_name} for {to_name} across {entities} entities of the design, each keeping its shape and turn. Undo puts back those of the entity on screen; the others are undone from their own view.")
        } else if whole {
            format!("Swapped {count} blocks of {from_name} for {to_name}, each keeping its shape and turn; no other entity of the design has {from_name}. Undo puts them back.")
        } else {
            format!("Swapped {count} blocks of {from_name} for {to_name}, each keeping its shape and turn. Undo puts them back.")
        };
        if !missing.is_empty() {
            text.push_str(&format!(" {to_name} has no {} shape, so those were left as they are.", missing.join(", ")));
        }
        self.say(text);
    }

    /// The block the game would link a newly placed block of type `ty` to:
    /// the ship core for what the core controls (computers above all), else
    /// the block selected with C where the two may be linked.
    fn link_for_placed(w: &Workspace, ty: u16, selected: Option<Pos>) -> Option<Pos> {
        let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
        let rules = Self::rules(w);
        let core = Pos::new(0, 0, 0);
        if rules.kind == links::Kind::Ship && entity.get(core).ty() == edit::CORE_TYPE && rules.core_takes(ty) {
            return Some(core);
        }
        selected.filter(|at| rules.links_when_placed(entity.get(*at).ty(), ty))
    }

    /// The boxes a placing click would fill, with the block for each: the brush
    /// and its mirror images, each image's block turned to match.
    fn placements(w: &Workspace, hit: &Hit, ty: u16, turn: u8, symmetry: Symmetry, size: [i32; 3]) -> Vec<(Pos, Pos, Block)> {
        let Some((lo, hi)) = Self::brush_box(Act::Place, hit, size) else { return Vec::new() };
        let kind = w.looks.kind(ty);
        let orientation = match (kind.shape, kind.faces_matter) {
            (Shape::Cube, false) => 0,
            (Shape::Cube, true) if turn == 0 => ShapeSet::facing(hit.normal),
            (Shape::Cube, true) => (turn - 1) % 6,
            (shape, faces) => turn % ShapeSet::choices(shape, faces),
        };
        symmetry
            .boxes(lo, hi)
            .into_iter()
            .map(|(a, b, flip)| {
                let turned = ShapeSet::mirrored(kind.shape, orientation, flip, kind.faces_matter);
                (a, b, Block::new(ty, turned, kind.active))
            })
            .collect()
    }

    /// Redraws the brush outline, the mirror planes and the hover text.
    pub fn refresh_overlay(self) {
        let (hovering, mut size, symmetry) = (self.on_click(false), self.brush.get_untracked(), self.symmetry.get_untracked());
        let (brush_shape, brush_shown) = (self.brush_shape.get_untracked(), self.brush_shown.get_untracked());
        let paint_mode = self.paint_mode.get_untracked();
        let (selected, turn) = (self.block_id.get_untracked(), self.turn.get_untracked());
        let (linking, link_from) = (self.tool.get_untracked() == Tool::Link, self.selected.get_untracked());
        let (picked, linking_tool) = (self.picked.get_untracked(), self.tool.get_untracked());
        let text = self.ws.try_update_value(|w| {
            if w.path == WHOLE {
                // Looking only: name the block under the pointer and the
                // entity it belongs to.
                let mut overlay = Overlay::default();
                let mut text = String::new();
                if let Some((i, hit)) = w.pointer.and_then(|(x, y)| Self::pick_whole(w, x, y)) {
                    let part = &w.rig.parts[i];
                    let block = w.blueprint.as_ref().and_then(|bp| bp.entity(&part.path)).map_or(Block::AIR, |e| e.entity.get(hit.pos));
                    text = format!("{} at {}, {}, {} of {}", w.looks.name(block.ty()), hit.pos.x, hit.pos.y, hit.pos.z, crate::entity_label(&part.path).to_lowercase());
                    overlay.wire_box_placed(corner(hit.pos, -0.52), corner(hit.pos, 0.52), &part.placed, [255, 255, 255, 255]);
                    if let (Some(b), true) = (part.bounds, i > 0) {
                        overlay.wire_box_placed(corner(b.min, -0.6), corner(b.max, 0.6), &part.placed, [240, 160, 50, 150]);
                    }
                }
                // Where a moving part was stopped: its block and the one in its way.
                for (contact, _) in &w.contacts {
                    for (part, cell) in [(contact.mover, contact.mover_cell), (contact.still, contact.still_cell)] {
                        let Some(part) = w.rig.parts.get(part) else { continue };
                        overlay.wire_box_placed(corner(cell, -0.54), corner(cell, 0.54), &part.placed, [255, 70, 60, 255]);
                    }
                }
                if let Some(r) = &mut w.renderer {
                    r.set_overlay(&overlay);
                }
                return text;
            }
            // The outline is for the button being held, else for what a left
            // click would do.
            let tool = w.armed.or(hovering).unwrap_or(Act::Pick);
            if tool == Act::Paint && paint_mode != PaintMode::Brush {
                size = [1; 3];
            }
            let mut overlay = Overlay::default();
            let mut hit = w.pointer.and_then(|(x, y)| Self::pick(w, x, y));
            if hit.is_none() && brush_shown {
                // The brush is being changed in the panel: show it where the
                // pointer last was over the design, or in the middle of the view.
                let middle = w.renderer.as_ref().map(|r| r.middle());
                hit = [w.last_pointer, middle].into_iter().flatten().find_map(|(x, y)| Self::pick(w, x, y));
            }
            let mut text = String::new();
            if let Some(hit) = &hit {
                let block = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map_or(Block::AIR, |e| e.entity.get(hit.pos));
                text = format!("{} at {}, {}, {}", w.looks.name(block.ty()), hit.pos.x, hit.pos.y, hit.pos.z);
                // With the Link tool, or while V's button is held: what linking would do here.
                let plan = (linking || tool == Act::Connect).then(|| Self::link_plan(w, link_from, hit.pos));
                if let (Some(plan), Some(from)) = (&plan, link_from) {
                    let controller = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)).map_or(Block::AIR, |e| e.entity.get(from));
                    let controller = w.looks.name(controller.ty());
                    match plan {
                        Ok((_, LinkStep::Link)) => text.push_str(&format!(". Right-click or V links it to the {controller}; Shift+right-click links every one touching it")),
                        Ok((_, LinkStep::Unlink)) => text.push_str(&format!(". Linked to the {controller}; right-click or V unlinks it, Shift+right-click every one touching it")),
                        Err(_) if from == hit.pos => text.push_str(". Selected"),
                        Err(_) => text.push_str(&format!(". Cannot be linked to the {controller}")),
                    }
                }
                // A fill: say how many blocks it would take.
                if tool == Act::Paint && paint_mode == PaintMode::Fill {
                    if let Some(entity) = w.blueprint.as_ref().and_then(|bp| bp.entity(&w.path)) {
                        let (region, more) = edit::paint_region(&entity.entity, hit.pos, |ty| w.looks.material(ty), self.fill_depth(hit), FILL_HOVER_LIMIT);
                        match (region.len(), more) {
                            (0, _) => text.push_str(". Nothing here can be painted"),
                            (n, true) => text.push_str(&format!(". Fill paints {n}+ blocks")),
                            (1, false) => text.push_str(". Fill paints this block"),
                            (n, false) => text.push_str(&format!(". Fill paints {n} blocks")),
                        }
                        // The first blocks of it, as a hint of its shape.
                        for p in region.iter().take(FILL_OUTLINED) {
                            overlay.wire_box(corner(*p, -0.5), corner(*p, 0.5), [255, 215, 110, 90]);
                        }
                    }
                }
                let colour = match (tool, &plan) {
                    (Act::Place, _) => [120, 255, 150],
                    (Act::Remove, _) => [255, 110, 110],
                    (Act::Paint, _) => [255, 215, 110],
                    (Act::Turret, _) => [255, 160, 60],
                    (Act::Exclude, _) => [255, 90, 90],
                    (Act::Core, _) => [90, 255, 140],
                    (Act::Smooth, _) => [150, 220, 255],
                    (Act::Unsmooth, _) => [255, 170, 110],
                    (Act::Connect, Some(Ok((_, LinkStep::Link)))) => [120, 255, 150],
                    (Act::Connect, Some(Ok((_, LinkStep::Unlink)))) => [255, 110, 110],
                    (Act::Connect, _) => [150, 150, 150],
                    (Act::Pick | Act::Select, _) => [255, 255, 255],
                    (Act::Use, _) => [110, 200, 255],
                };
                let tint = |i: usize| [colour[0], colour[1], colour[2], if i == 0 { 255 } else { 130 }];
                let (shape, axis) = (if shaped(tool) { brush_shape } else { BrushShape::Cube }, brush_axis(hit));
                if tool == Act::Place {
                    let kind = w.looks.kind(selected);
                    for (i, (a, b, block)) in Self::placements(w, hit, selected, turn, symmetry, size).into_iter().enumerate() {
                        outline_brush(&mut overlay, a, b, shape, axis, tint(i));
                        // Show the actual shape, cell by cell, unless the brush is large.
                        let cells = (b.x - a.x + 1) as i64 * (b.y - a.y + 1) as i64 * (b.z - a.z + 1) as i64;
                        let plain = kind.shape == Shape::Cube && kind.slab == 0;
                        if cells > 32 || (plain && !kind.faces_matter) {
                            continue;
                        }
                        let geom = w.looks.shapes().geom(kind.shape, kind.slab, block.orientation());
                        for z in a.z..=b.z {
                            for y in a.y..=b.y {
                                for x in a.x..=b.x {
                                    if !shape.holds(a, b, axis, Pos::new(x, y, z)) {
                                        continue;
                                    }
                                    let origin = [x as f32 - 0.5, y as f32 - 0.5, z as f32 - 0.5];
                                    let at = |c: [f32; 3]| [origin[0] + c[0], origin[1] + c[1], origin[2] + c[2]];
                                    if kind.shape == Shape::Rail {
                                        // A block that turns 24 ways: an arrow on the side its
                                        // first texture will face, pointing where that texture's
                                        // top edge will point (a docker's arrow, a rail's travel).
                                        let (up, forward) = yard_core::shape::turned_axes(block.orientation());
                                        let side = [(up[1] * forward[2] - up[2] * forward[1]) as f32, (up[2] * forward[0] - up[0] * forward[2]) as f32, (up[0] * forward[1] - up[1] * forward[0]) as f32];
                                        let point = |along: f32, across: f32| {
                                            at([0, 1, 2].map(|a| 0.5 + up[a] as f32 * 0.56 + forward[a] as f32 * along + side[a] * across))
                                        };
                                        overlay.line(point(-0.4, 0.0), point(0.4, 0.0), tint(i));
                                        overlay.line(point(0.4, 0.0), point(0.1, 0.25), tint(i));
                                        overlay.line(point(0.4, 0.0), point(0.1, -0.25), tint(i));
                                    } else if !plain {
                                        for face in &geom.faces {
                                            for k in 0..face.corners.len() {
                                                overlay.line(at(face.corners[k]), at(face.corners[(k + 1) % face.corners.len()]), tint(i));
                                            }
                                        }
                                    } else {
                                        // A block with a front: a tick from its centre out through that face.
                                        let d = [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]]
                                            [block.orientation() as usize % 6];
                                        overlay.line(at([0.5; 3]), at([0.5 + d[0] * 0.9, 0.5 + d[1] * 0.9, 0.5 + d[2] * 0.9]), tint(i));
                                    }
                                }
                            }
                        }
                    }
                } else if let Some((lo, hi)) = Self::brush_box(tool, hit, size) {
                    for (i, (a, b, _)) in symmetry.boxes(lo, hi).into_iter().enumerate() {
                        outline_brush(&mut overlay, a, b, shape, axis, tint(i));
                    }
                }
            }
            // The block picked with the Pick tool, which the arrows turn and a new block replaces.
            if let (Some(at), true) = (picked, linking_tool == Tool::Pick) {
                overlay.wire_box(corner(at, -0.56), corner(at, 0.56), [255, 230, 120, 255]);
                overlay.wire_box(corner(at, -0.6), corner(at, 0.6), [255, 230, 120, 255]);
            }
            if let Some(b) = w.bounds {
                let (lo, hi) = (corner(b.min, -2.5), corner(b.max, 2.5));
                let planes = [(symmetry.x, 0, [255, 90, 90]), (symmetry.y, 1, [90, 255, 120]), (symmetry.z, 2, [90, 150, 255])];
                for (plane, axis, c) in planes {
                    let Some(twice) = plane else { continue };
                    let at = twice as f32 / 2.0;
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    let point = |pu: f32, pv: f32| {
                        let mut p = [0.0; 3];
                        p[axis] = at;
                        p[u] = pu;
                        p[v] = pv;
                        p
                    };
                    let corners = [point(lo[u], lo[v]), point(hi[u], lo[v]), point(hi[u], hi[v]), point(lo[u], hi[v])];
                    overlay.quad(corners, [c[0], c[1], c[2], 28]);
                    for i in 0..4 {
                        overlay.line(corners[i], corners[(i + 1) % 4], [c[0], c[1], c[2], 150]);
                    }
                }
            }
            if let Some(r) = &mut w.renderer {
                r.set_overlay(&overlay);
            }
            text
        });
        let text = text.unwrap_or_default();
        if self.hover.get_untracked() != text {
            self.hover.set(text);
        }
    }

    /// A click on the viewport: do `tool` to what is under it. `group` is the
    /// Shift key, which makes linking take a whole group of touching blocks.
    pub fn act(self, tool: Act, x: i32, y: i32, group: bool) {
        if self.relocating.get_untracked().is_some() {
            return self.relocation_click(x, y, group);
        }
        if tool == Act::Use {
            return self.use_at(x, y);
        }
        if self.entity_path.get_untracked() == WHOLE {
            match tool {
                Act::Place | Act::Remove | Act::Paint | Act::Connect | Act::Smooth | Act::Unsmooth | Act::Turret | Act::Exclude | Act::Core => self.say("The whole design is for looking at and trying out. Choose an entity on the left to edit it."),
                // Nothing else to do with a click here: it uses the block.
                _ => self.use_at(x, y),
            }
            return;
        }
        if tool == Act::Turret {
            return self.turret_click(x, y);
        }
        if tool == Act::Exclude {
            return self.turret_exclude(x, y, group);
        }
        if tool == Act::Core && self.tool.get_untracked() == Tool::Clone {
            return self.say("Choose on the right what the copy replaces, or where its new rail goes.");
        }
        if tool == Act::Core {
            // The relocation was confirmed or called off: a click starts another.
            self.start_relocation();
            if self.relocating.get_untracked().is_some() {
                self.relocation_click(x, y, group);
            }
            return;
        }
        match tool {
            Act::Select => {
                // A click past the design keeps the selection: it was most
                // likely the start of turning the view.
                if let Some(at) = self.ws.with_value(|w| Self::pick(w, x, y).map(|hit| hit.pos)) {
                    self.toggle_selection(at);
                }
                return;
            }
            Act::Connect => return self.connect_at(x, y, group),
            _ => {}
        }
        let (mut size, symmetry, selected) = (self.brush.get_untracked(), self.symmetry.get_untracked(), self.block_id.get_untracked());
        let shape = if shaped(tool) { self.brush_shape.get_untracked() } else { BrushShape::Cube };
        let paint_mode = self.paint_mode.get_untracked();
        let (smooth_step, smooth_inside, smooth_sharpen) = (self.smooth_step.get_untracked(), self.smooth_inside.get_untracked(), self.smooth_sharpen.get_untracked());
        if tool == Act::Paint && paint_mode != PaintMode::Brush {
            size = [1; 3];
        }
        let (turn, link_from) = (self.turn.get_untracked(), self.selected.get_untracked());
        let planned = self.ws.with_value(|w| {
            let hit = Self::pick(w, x, y)?;
            let entity = &w.blueprint.as_ref()?.entity(&w.path)?.entity;
            if tool == Act::Pick {
                let block = entity.get(hit.pos);
                // The core is picked for its type, but cannot be turned or replaced.
                self.picked.set((!block.is_air() && block.ty() != edit::CORE_TYPE).then_some(hit.pos));
                return Some(Err(block));
            }
            if tool == Act::Place {
                let link = Self::link_for_placed(w, selected, link_from);
                let axis = brush_axis(&hit);
                let ops = Self::placements(w, &hit, selected, turn, symmetry, size)
                    .into_iter()
                    .flat_map(|(lo, hi, block)| shape.rows(lo, hi, axis).into_iter().map(move |(min, max)| (min, max, block)))
                    .map(|(min, max, block)| Op::Fill { min, max, block, mode: FillMode::Solid, when: When::Empty, link })
                    .collect();
                return Some(Ok(ops));
            }
            if tool == Act::Paint && paint_mode == PaintMode::Fill {
                if w.looks.material(entity.get(hit.pos).ty()).is_none() {
                    return Some(Ok(Vec::new()));
                }
                // The bucket, and its mirror images where the mirror planes are set.
                let depth = self.fill_depth(&hit);
                let mut cells = Vec::new();
                let mut done = std::collections::HashSet::new();
                for (start, _) in symmetry.points(hit.pos) {
                    let (region, _) = edit::paint_region(entity, start, |ty| w.looks.material(ty), depth, FILL_LIMIT);
                    for p in region {
                        if !done.insert(p) {
                            continue;
                        }
                        if let Some(new) = w.looks.repaint(entity.get(p), selected).filter(|n| *n != entity.get(p)) {
                            cells.push((p, new));
                        }
                    }
                }
                return Some(Ok(if cells.is_empty() { Vec::new() } else { vec![Op::Set { cells, when: When::Occupied }] }));
            }
            if matches!(tool, Act::Smooth | Act::Unsmooth) {
                let (lo, hi) = Self::brush_box(tool, &hit, size)?;
                let axis = brush_axis(&hit);
                let mut cells = Vec::new();
                let mut done = std::collections::HashSet::new();
                for (a, b, _) in symmetry.boxes(lo, hi) {
                    for z in a.z..=b.z {
                        for y in a.y..=b.y {
                            for x in a.x..=b.x {
                                let p = Pos::new(x, y, z);
                                let old = entity.get(p);
                                if old.is_air() || !shape.holds(a, b, axis, p) || !done.insert(p) {
                                    continue;
                                }
                                let new = if tool == Act::Smooth {
                                    Self::bevelled(w, entity, p, old, smooth_step, smooth_inside)
                                } else if Self::notch_filler(w, entity, p, old) {
                                    // A wedge smoothing put in a notch goes, rather than filling it whole.
                                    Some(Block::AIR)
                                } else if matches!(w.looks.kind(old.ty()).shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta) {
                                    w.looks.reshape(old, Shape::Cube, 0, 0)
                                } else {
                                    None
                                };
                                if let Some(new) = new.filter(|n| *n != old) {
                                    cells.push((p, new));
                                }
                            }
                        }
                    }
                }
                // Then the ends of rows cut flush, and the inside corners, judged by what the edges round them became.
                if tool == Act::Smooth {
                    let mut planned: std::collections::HashMap<Pos, Block> = cells.iter().copied().collect();
                    // A round at a time: a block cut flush can be what the next one along is to be flush with.
                    loop {
                        let ends: Vec<(Pos, Block)> = planned.keys().filter_map(|&p| Self::row_ended(w, entity, p, &planned, smooth_step, smooth_inside).filter(|b| planned.get(&p) != Some(b)).map(|b| (p, b))).collect();
                        if ends.is_empty() {
                            break;
                        }
                        planned.extend(ends);
                    }
                    // Slopes carried on through the whole blocks in the brush they end against, a round at a time.
                    while smooth_sharpen {
                        let more: Vec<(Pos, Block)> = done.iter().filter(|p| !planned.contains_key(p)).filter_map(|&p| Self::sharpened(w, entity, p, &planned, smooth_inside).map(|b| (p, b))).collect();
                        if more.is_empty() {
                            break;
                        }
                        planned.extend(more);
                    }
                    cells = planned.iter().map(|(p, b)| (*p, *b)).collect();
                    let corners: Vec<(Pos, Block)> = done
                        .iter()
                        .filter(|p| !planned.contains_key(p))
                        .filter_map(|&p| {
                            let old = entity.get(p);
                            Self::inside_cornered(w, entity, p, old, &planned, smooth_inside).filter(|n| *n != old).map(|n| (p, n))
                        })
                        .collect();
                    cells.extend(corners);
                }
                // And the notches on the inner face of thin hulls beside the brush, filled.
                let mut added = Vec::new();
                if tool == Act::Smooth && smooth_inside {
                    let mut seen = std::collections::HashSet::new();
                    for p in &done {
                        for d in yard_core::shape::SIDE_DIRS {
                            let q = p.offset(d[0], d[1], d[2]);
                            if !entity.get(q).is_air() || !seen.insert(q) {
                                continue;
                            }
                            let reshape = |b: Block, f: &yard_core::smooth::Form| w.looks.reshape(b, f.shape, f.slab, f.orientation);
                            if let Some(new) = yard_core::smooth::notch_block(entity, q, |b| Self::smoothable(w, b), reshape) {
                                added.push((q, new));
                            }
                        }
                    }
                }
                // Unsmoothing a notch's wedge empties the cell.
                let (emptied, cells): (Vec<_>, Vec<_>) = cells.into_iter().partition(|(_, b)| b.is_air());
                let mut ops = Self::smoothing_ops(cells, added);
                if !emptied.is_empty() {
                    ops.push(Op::Set { cells: emptied, when: When::Occupied });
                }
                return Some(Ok(ops));
            }
            let (lo, hi) = Self::brush_box(tool, &hit, size)?;
            let (through, armor_only) = (self.paint_through.get_untracked(), self.paint_armor_only.get_untracked());
            let axis = brush_axis(&hit);
            let mut ops = Vec::new();
            for (a, b, _) in symmetry.boxes(lo, hi) {
                match tool {
                    Act::Remove => {
                        for (min, max) in shape.rows(a, b, axis) {
                            ops.push(Op::Fill { min, max, block: Block::AIR, mode: FillMode::Solid, when: When::Always, link: None })
                        }
                    }
                    Act::Paint => {
                        // Each block keeps its shape and takes the new material.
                        let mut cells = Vec::new();
                        for z in a.z..=b.z {
                            for y in a.y..=b.y {
                                for x in a.x..=b.x {
                                    let p = Pos::new(x, y, z);
                                    let old = entity.get(p);
                                    if old.is_air() || !shape.holds(a, b, axis, p) || (!through && !edit::on_surface(entity, p)) || (armor_only && !w.looks.is_armor_or_light(old.ty())) {
                                        continue;
                                    }
                                    if let Some(new) = w.looks.repaint(old, selected).filter(|n| *n != old) {
                                        cells.push((p, new));
                                    }
                                }
                            }
                        }
                        if !cells.is_empty() {
                            ops.push(Op::Set { cells, when: When::Occupied });
                        }
                    }
                    Act::Place | Act::Pick | Act::Select | Act::Connect | Act::Use | Act::Smooth | Act::Unsmooth | Act::Turret | Act::Exclude | Act::Core => {}
                }
            }
            Some(Ok(ops))
        });
        match planned {
            Some(Err(picked)) if picked.ty() != edit::CORE_TYPE && !picked.is_air() => {
                // Take the block's rotation along with its type.
                let kind = self.ws.with_value(|w| w.looks.kind(picked.ty()));
                self.block_id.set(picked.ty());
                self.turn.set(match (kind.shape, kind.faces_matter) {
                    (Shape::Cube, false) => 0,
                    (Shape::Cube, true) => picked.orientation() % 6 + 1,
                    _ => picked.orientation(),
                });
            }
            Some(Ok(ops)) if ops.is_empty() && tool == Act::Smooth => {
                self.say("No edges of steps to bevel here. Raise the step height, or choose a bigger brush.");
            }
            Some(Ok(ops)) if !ops.is_empty() => {
                if tool == Act::Place && selected == 0 {
                    self.fail("Choose a block to place first.");
                    return;
                }
                let _ = self.submit(ops);
            }
            _ => {}
        }
    }

    /// Applies one step to the copy on screen and queues it for the server.
    /// Returns what it changed, or None if it changed nothing or was refused.
    pub fn submit(self, ops: Vec<Op>) -> Option<Change> {
        let path = self.ws.with_value(|w| w.path.clone());
        self.submit_to(path, ops)
    }

    /// The same for the entity at `path`, which need not be the one on screen.
    pub fn submit_to(self, path: String, ops: Vec<Op>) -> Option<Change> {
        // Edits go into the design as saved, not as the simulation left it.
        self.stop_sim();
        let applied = self.ws.try_update_value(|w| {
            if w.preview {
                return Err("This is an old version, which cannot be edited. Go back to the current version, or restore this one.".to_string());
            }
            if w.stashed.is_some() {
                return Err("A core move is being previewed. Confirm it or go back to choosing before editing.".to_string());
            }
            if w.awaiting_history {
                return Err("Still finishing the last undo; try again in a moment.".to_string());
            }
            let on_screen = w.path == path;
            let Workspace { blueprint, looks, .. } = w;
            let entity = blueprint.as_mut().and_then(|bp| bp.entity_mut(&path)).ok_or("No design is open.")?;
            // Links are made by the game's rules, as the server will make them.
            let rules = looks.rules(entity.header.entity_type);
            let change = edit::apply_with(&mut entity.entity, &ops, &rules).map_err(sentence)?;
            if change.is_empty() {
                return Ok(None);
            }
            if !on_screen {
                // Nothing to redraw, but the parts were put together from the blocks as they were.
                w.rig = Rig::default();
                w.switches = None;
            }
            w.outbox.push_back((path, ops));
            Ok(Some((change, on_screen)))
        });
        match applied {
            Some(Ok(Some((change, on_screen)))) => {
                // Any edit makes the design as it was before a relocation or a turret no longer the one to go back to.
                self.structure_undo.set(None);
                self.pending.update(|n| *n += 1);
                self.say("");
                if on_screen {
                    self.show_change(&change);
                }
                self.pump();
                Some(change)
            }
            Some(Err(e)) => {
                self.fail(e);
                None
            }
            _ => None,
        }
    }

    /// Follows a step somebody else made in the open design: the chat agent
    /// building. `before` and `after` are the design's revisions around the
    /// step. If this copy was in step, the same ops (or, for an undo, the
    /// change the server worked out) are applied here; if it was not, or has
    /// edits of its own under way, the design is loaded again.
    pub fn apply_remote(self, entity: String, ops: Vec<Op>, change: Option<Change>, before: u64, after: u64) {
        self.stop_sim();
        let applied = self.ws.try_update_value(|w| {
            if w.preview {
                // An old version is on screen; the current one is loaded
                // afresh when the visitor goes back to it.
                return Some(None);
            }
            if w.stashed.is_some() {
                // A core move is previewed on a copy; the design is loaded
                // afresh once the preview is left.
                w.stale = true;
                return Some(None);
            }
            if w.rev != before || w.sending || !w.outbox.is_empty() || w.awaiting_history {
                return None;
            }
            let Workspace { blueprint, looks, .. } = w;
            let e = blueprint.as_mut()?.entity_mut(&entity)?;
            let change = match change {
                Some(change) => {
                    change.apply_to(&mut e.entity);
                    change
                }
                None => edit::apply_with(&mut e.entity, &ops, &looks.rules(e.header.entity_type)).ok()?,
            };
            w.rev = after;
            Some(Some(change))
        });
        match applied.flatten() {
            Some(Some(change)) => {
                let shown = self.entity_path.get_untracked();
                if shown == entity {
                    self.history.update(|h| {
                        if let Some((undo, redo)) = h {
                            (*undo, *redo) = (*undo + 1, 0);
                        }
                    });
                    self.show_change(&change);
                } else if shown == WHOLE {
                    self.show_whole(false);
                }
            }
            Some(None) => {}
            None => {
                let id = self.ws.with_value(|w| w.design_id.clone());
                self.open_design_saying(id, "The design changed while it was open here. It was loaded again.");
            }
        }
    }

    /// Asks the server to undo or redo; the result is applied when it arrives.
    pub fn history_step(self, op: Op) {
        self.stop_sim();
        let queued = self.ws.try_update_value(|w| {
            if w.blueprint.is_none() || w.awaiting_history || w.preview || w.stashed.is_some() || w.path == WHOLE {
                return false;
            }
            w.awaiting_history = true;
            w.outbox.push_back((w.path.clone(), vec![op]));
            true
        });
        if queued == Some(true) {
            self.pending.update(|n| *n += 1);
            self.pump();
        }
    }

    /// Sends the next queued step, one at a time and in order.
    fn pump(self) {
        let next = self
            .ws
            .try_update_value(|w| {
                if w.sending {
                    return None;
                }
                let (entity, ops) = w.outbox.pop_front()?;
                w.sending = true;
                Some((w.epoch, w.design_id.clone(), StepRequest { rev: w.rev, entity, ops }))
            })
            .flatten();
        let Some((epoch, id, request)) = next else { return };
        spawn_local(async move {
            let result = api::step(&id, &request).await;
            if self.ws.with_value(|w| w.epoch != epoch) {
                return; // The design was reloaded or another one opened meanwhile.
            }
            let from_history = matches!(request.ops.as_slice(), [Op::Undo] | [Op::Redo]);
            match result {
                Ok(done) => {
                    self.ws.update_value(|w| {
                        w.rev = done.rev;
                        w.sending = false;
                        if from_history {
                            w.awaiting_history = false;
                        }
                        if let (Some(change), Some(bp)) = (&done.change, &mut w.blueprint) {
                            if let Some(e) = bp.entity_mut(&request.entity) {
                                change.apply_to(&mut e.entity);
                            }
                        }
                    });
                    if self.entity_path.get_untracked() == request.entity {
                        self.history.set(Some((done.undo, done.redo)));
                        if let Some(change) = &done.change {
                            self.show_change(change);
                        }
                    }
                    self.pending.update(|n| *n = n.saturating_sub(1));
                    self.pump();
                }
                Err(StepFailure::Refused(why)) if from_history => {
                    // "Nothing to undo": the two copies still agree.
                    self.ws.update_value(|w| {
                        w.sending = false;
                        w.awaiting_history = false;
                    });
                    self.pending.update(|n| *n = n.saturating_sub(1));
                    self.say(why);
                    self.pump();
                }
                Err(StepFailure::Refused(why)) | Err(StepFailure::OutOfStep(why)) => {
                    // This copy has edits the server does not; take the server's.
                    self.open_design_saying(id, &format!("{why} The design was loaded again."));
                }
            }
        });
    }

    /// Brings the viewport and the side panel up to date after blocks changed.
    fn show_change(self, change: &Change) {
        let stats = self.ws.try_update_value(|w| {
            // The parts were put together from the blocks as they were.
            if w.path != WHOLE {
                w.rig = Rig::default();
            }
            w.switches = None;
            for &(_, before, after) in &change.cells {
                if before.ty() != after.ty() {
                    *w.counts.entry(before.ty()).or_default() -= i64::from(!before.is_air());
                    *w.counts.entry(after.ty()).or_default() += i64::from(!after.is_air());
                }
            }
            let Workspace { renderer: Some(renderer), blueprint: Some(bp), looks, bounds, path, design_id, .. } = w else {
                return None;
            };
            let e = bp.entity(path)?;
            for key in change.dirty_segments() {
                renderer.set_chunk(0, key, &mesh::build_segment(&e.entity, key, looks));
            }
            for &(p, _, after) in &change.cells {
                if !after.is_air() {
                    let b = bounds.get_or_insert(Bounds { min: p, max: p });
                    b.min = b.min.lo(p);
                    b.max = b.max.hi(p);
                }
            }
            // The entity's size grows with what was placed; a block taken off
            // its outer edge means measuring it again.
            if let Some(size) = w.sizes.get_mut(path.as_str()) {
                let mut again = false;
                for &(p, before, after) in &change.cells {
                    match (size.as_mut(), after.is_air()) {
                        (Some(b), false) => (b.min, b.max) = (b.min.lo(p), b.max.hi(p)),
                        (None, false) => *size = Some(Bounds { min: p, max: p }),
                        (Some(b), true) if !before.is_air() => {
                            again |= [p.x == b.min.x, p.x == b.max.x, p.y == b.min.y, p.y == b.max.y, p.z == b.min.z, p.z == b.max.z].contains(&true)
                        }
                        _ => {}
                    }
                }
                if again {
                    w.sizes.remove(path.as_str());
                }
            }
            let total: u64 = bp.entities().iter().map(|(_, e)| e.entity.block_count()).sum();
            (total <= LIVE_STATS_LIMIT).then(|| Summary::describe(design_id, bp, 0, |ty| looks.known_name(ty)))
        });
        self.publish_counts();
        self.measure();
        if let Some(Some(mut fresh)) = stats {
            self.current.update(|current| {
                if let Some(old) = current {
                    fresh.created = old.created;
                    fresh.epoch = old.epoch;
                    *old = fresh.clone();
                }
            });
            // Keep the design list's block count in step too. An edit also
            // means the game no longer has the design as it is here.
            self.designs.update(|list| {
                if let Some(entry) = list.iter_mut().find(|d| d.id == fresh.id) {
                    fresh.created = entry.created;
                    fresh.epoch = entry.epoch;
                    fresh.sync = entry.sync.map(|s| if s == SyncStatus::Synced { SyncStatus::Waiting } else { s });
                    *entry = fresh;
                }
            });
        }
        // An edited design is at none of its kept versions any more; the
        // server says so too once it is asked again.
        if !change.is_empty() && self.versions.with_untracked(|list| list.iter().any(|v| v.current)) {
            self.versions.update(|list| list.iter_mut().for_each(|v| v.current = false));
        }
        self.refresh_switches();
        // Blocks and links may have changed under the selection.
        if self.selected.get_untracked().is_some() {
            self.refresh_links();
        }
        self.refresh_overlay();
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(pos: (i32, i32, i32), normal: [i32; 3]) -> Hit {
        Hit { pos: Pos::new(pos.0, pos.1, pos.2), normal, distance: 1.0 }
    }

    #[test]
    fn the_buttons_do_what_they_do_in_the_game_and_only_the_build_tool_edits_on_the_right() {
        // Building: left places, right removes; the game's swap setting turns that around.
        assert_eq!(Tool::Build.on_click(false, false), Some(Act::Place));
        assert_eq!(Tool::Build.on_click(true, false), Some(Act::Remove));
        assert_eq!(Tool::Build.on_click(false, true), Some(Act::Remove));
        assert_eq!(Tool::Build.on_click(true, true), Some(Act::Place));
        // The other tools use the left button whatever the setting, and a
        // right-click with them leaves the design alone.
        for swapped in [false, true] {
            assert_eq!(Tool::Pick.on_click(false, swapped), Some(Act::Pick));
            assert_eq!(Tool::Paint.on_click(false, swapped), Some(Act::Paint));
            assert_eq!(Tool::Smooth.on_click(false, swapped), Some(Act::Smooth));
            assert_eq!(Tool::Turret.on_click(true, swapped), Some(Act::Exclude));
            assert_eq!(Tool::Core.on_click(false, swapped), Some(Act::Core));
            assert_eq!(Tool::Core.on_click(true, swapped), None);
            assert_eq!(Tool::Smooth.on_click(true, swapped), Some(Act::Unsmooth));
            assert_eq!(Tool::Pick.on_click(true, swapped), None);
            assert_eq!(Tool::Paint.on_click(true, swapped), None);
            // Linking: left selects like the game's C, right connects like its V.
            assert_eq!(Tool::Link.on_click(false, swapped), Some(Act::Select));
            assert_eq!(Tool::Link.on_click(true, swapped), Some(Act::Connect));
        }
    }

    #[test]
    fn placing_grows_outward_from_the_clicked_face() {
        // Clicking the top of a block: one block goes in the cell above.
        let single = Ctx::brush_box(Act::Place, &hit((0, 0, 0), [0, 1, 0]), [1, 1, 1]).unwrap();
        assert_eq!(single, (Pos::new(0, 1, 0), Pos::new(0, 1, 0)));
        // A 3 x 2 x 5 brush on the same face: centred sideways, two layers upward.
        let big = Ctx::brush_box(Act::Place, &hit((0, 0, 0), [0, 1, 0]), [3, 2, 5]).unwrap();
        assert_eq!(big, (Pos::new(-1, 1, -2), Pos::new(1, 2, 2)));
        // On a face pointing toward -X the brush extends further toward -X.
        let side = Ctx::brush_box(Act::Place, &hit((4, 0, 0), [-1, 0, 0]), [3, 1, 1]).unwrap();
        assert_eq!(side, (Pos::new(1, 0, 0), Pos::new(3, 0, 0)));
        // With the camera inside a block there is no face to build on.
        assert_eq!(Ctx::brush_box(Act::Place, &hit((0, 0, 0), [0, 0, 0]), [1, 1, 1]), None);
    }

    #[test]
    fn removing_and_painting_dig_inward_from_the_clicked_face() {
        let h = hit((0, 5, 0), [0, 1, 0]);
        assert_eq!(Ctx::brush_box(Act::Remove, &h, [1, 3, 1]).unwrap(), (Pos::new(0, 3, 0), Pos::new(0, 5, 0)));
        assert_eq!(Ctx::brush_box(Act::Paint, &h, [2, 1, 2]).unwrap(), (Pos::new(0, 5, 0), Pos::new(1, 5, 1)));
        // Picking always takes exactly the block under the pointer.
        assert_eq!(Ctx::brush_box(Act::Pick, &h, [9, 9, 9]).unwrap(), (Pos::new(0, 5, 0), Pos::new(0, 5, 0)));
        // Oversized brushes are capped.
        let (lo, hi) = Ctx::brush_box(Act::Remove, &h, [1, 1000, 1]).unwrap();
        assert_eq!(hi.y - lo.y + 1, MAX_BRUSH);
    }
    #[test]
    fn a_section_of_the_turret_box_ends_at_air_and_at_the_box() {
        let mut e = yard_core::entity::Entity::new();
        let b = Block::new(5, 0, false);
        // A turret body, and a ridge one cell of air away; a block touching the ridge only at an edge.
        for x in 0..=2 {
            for y in 0..=2 {
                e.set(Pos::new(x, y, 0), b);
            }
            e.set(Pos::new(x, 0, 2), b);
        }
        e.set(Pos::new(0, 1, 3), b);
        // And a block outside the box, touching the ridge.
        e.set(Pos::new(3, 0, 2), b);
        let (lo, hi) = (Pos::new(0, 0, 0), Pos::new(2, 2, 3));
        let mut ridge = turret_section(&e, Pos::new(1, 0, 2), lo, hi);
        ridge.sort_by_key(|p| (p.x, p.y, p.z));
        assert_eq!(ridge, vec![Pos::new(0, 0, 2), Pos::new(0, 1, 3), Pos::new(1, 0, 2), Pos::new(2, 0, 2)]);
        assert_eq!(turret_section(&e, Pos::new(1, 1, 0), lo, hi).len(), 9);
        assert!(turret_section(&e, Pos::new(1, 1, 1), lo, hi).is_empty(), "air is no section");
    }

    #[test]
    fn brush_shapes_round_off_the_box() {
        let (lo, hi) = (Pos::new(0, 0, 0), Pos::new(4, 4, 4));
        let count = |shape: BrushShape, axis: usize| {
            let mut n = 0;
            for z in 0..5 {
                for y in 0..5 {
                    for x in 0..5 {
                        n += shape.holds(lo, hi, axis, Pos::new(x, y, z)) as usize;
                    }
                }
            }
            n
        };
        assert_eq!(count(BrushShape::Cube, 1), 125);
        // A disc of 21 cells (a 5 by 5 square less its 4 corners), 5 deep.
        assert_eq!(count(BrushShape::Cylinder, 1), 21 * 5);
        assert!(BrushShape::Cylinder.holds(lo, hi, 1, Pos::new(0, 0, 2)) && !BrushShape::Cylinder.holds(lo, hi, 1, Pos::new(0, 2, 0)));
        assert!(BrushShape::Sphere.holds(lo, hi, 0, Pos::new(2, 2, 0)) && !BrushShape::Sphere.holds(lo, hi, 0, Pos::new(0, 0, 2)));
        // One block is one block whatever the shape.
        for (shape, _) in BrushShape::ALL {
            assert!(shape.holds(lo, lo, 1, lo));
        }
        // A cube is filled as one box; the round shapes as rows along X that
        // cover their cells and nothing else.
        assert_eq!(BrushShape::Cube.rows(lo, hi, 2), vec![(lo, hi)]);
        for shape in [BrushShape::Cylinder, BrushShape::Sphere] {
            let rows = shape.rows(lo, hi, 2);
            let cells: usize = rows.iter().map(|(a, b)| (b.x - a.x + 1) as usize).sum();
            assert_eq!(cells, count(shape, 2));
            assert!(rows.iter().all(|(a, b)| a.y == b.y && a.z == b.z && (a.x..=b.x).all(|x| shape.holds(lo, hi, 2, Pos::new(x, a.y, a.z)))));
        }
    }

}
