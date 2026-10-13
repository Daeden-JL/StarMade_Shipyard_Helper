//! The game's logic and rails, run on a design: what happens when a player
//! presses a button.
//!
//! This follows the game's own code (`SendableSegmentController` for
//! signals, `RailController` for rails), rule for rule:
//!
//! - A signal is "switch on" or "switch off", sent to one block. Signals
//!   that start a chain (a player's press, a rail event, a delay running
//!   out) wait for the next tick; everything a logic block passes on is
//!   handled within the same tick, first come first served.
//! - A logic block's state is its own active bit. Each kind turns the signal
//!   it gets into its new state by its own rule, and if that state changed
//!   it sends the new state to every block it controls.
//! - The one length of time in logic is half a second: the delay block's
//!   delay and the time a button stays in.
//! - A docked entity on rail track moves by itself, one block at a time,
//!   along the arrow of the block under it, for as long as the next block is
//!   track facing the same side. A rotator turns it once. Logic comes in
//!   through the blocks beside and linked to the rails.
//!
//! The simulation changes active bits (and, where logic swaps rails, rail
//! blocks) in the entities it is given, and remembers what they were, so
//! everything can be put back.

use crate::collide::Shapes;
use crate::dock::{about, times, Placement, ROTATORS};
use crate::entity::{Entity, Pos};
use crate::links::{flag, BlockLinks};
use crate::rig::Rig;
use crate::shape::{turned_axes, SIDE_DIRS};
use crate::Block;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// The game server's tick, in milliseconds.
pub const TICK_MS: f64 = 37.0;
/// The delay block's delay and the time a button stays in.
pub const DELAY_MS: f64 = 500.0;
/// Blocks a second along a rail at full speed; a rail nothing controls runs
/// at half of it.
pub const RAIL_SPEED: f32 = 3.0;
/// Degrees a second on a rotator at full speed.
pub const TURN_SPEED: f32 = 225.0;
/// The share of full speed without a speed controller.
pub const DEFAULT_SPEED: f32 = 0.5;
/// How long a blocked part waits before it tries again, in milliseconds.
const BLOCKED_WAIT_MS: f32 = 1500.0;
/// Blocks a second at which a shootout rail lets go of what reaches its end.
const LAUNCH_SPEED: f32 = 25.0;
/// Most door blocks one press switches.
const DOOR_GROUP_LIMIT: usize = 100_000;

const ACTIVATION: u16 = 405;
const DELAY_ONCE: u16 = 406;
const DELAY: u16 = 407;
const AND: u16 = 408;
const OR: u16 = 409;
const NOT: u16 = 410;
const DISPLAY: u16 = 479;
const DOCKER: u16 = 663;
const FLIP_FLOP: u16 = 667;
const WIRELESS: u16 = 668;
const REMOTE: u16 = 670;
const SPEED_CONTROLLER: u16 = 672;
const SHOOTOUT: u16 = 939;
const RANDOM: u16 = 979;
const SENSOR: u16 = 980;
/// Consoles: pressing one presses everything linked from it.
const CONSOLES: [u16; 12] = [975, 1028, 1029, 1030, 1038, 1039, 1040, 1041, 1051, 1052, 1053, 1054];
/// Blocks a press enters, opens or asks about in the game. Nothing to show here.
const DIALOGS: [u16; 33] =
    [1, 4, 6, 8, 38, 39, 56, 94, 113, 120, 121, 123, 134, 177, 211, 213, 215, 217, 259, 291, 344, 347, 445, 446, 447, 451, 542, 677, 683, 687, 1738, DISPLAY, SENSOR];

/// One "switch on" or "switch off" for one block.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Signal {
    pos: Pos,
    on: bool,
    /// Passed on by a logic block, as opposed to starting a chain.
    from_logic: bool,
}

/// A signal on its way through a tick, with the one that caused it.
struct Trace {
    signal: Signal,
    parent: Option<usize>,
}

/// The logic state of one entity besides its blocks.
#[derive(Default)]
struct Logic {
    /// Signals that start a chain, waiting for the next tick.
    waiting: VecDeque<Signal>,
    /// What each delay block and each pressed button holds, and since when.
    delayed: BTreeMap<Pos, (Signal, f64)>,
    delayed_once: BTreeMap<Pos, (Signal, f64)>,
    /// Counts the chains handled, for the delay that lets each through once.
    chain: u64,
    last_chain: HashMap<Pos, u64>,
    /// The logic blocks that control each AND and OR gate.
    inputs: HashMap<Pos, Vec<Pos>>,
    /// The entity's rail speed controllers.
    speed_controllers: Vec<Pos>,
}

/// A turn a rotator is giving.
#[derive(Clone, Copy, Debug)]
struct Turning {
    axis: [f32; 3],
    /// Degrees in all, signed by the right-hand rule about the axis.
    total: f32,
    /// How many of them are done.
    done: f32,
    /// Something was in the way: it is turning back to where it started.
    back: bool,
    /// Nine or more switched-on modules: when done, it starts again.
    forever: bool,
}

/// Where the rails have one docked part, and what it is doing.
#[derive(Clone, Debug)]
struct Rail {
    /// The cell its docker is in, in the coordinates of what it is docked to.
    contact: Pos,
    /// Its turn and shift when it last came to rest, and right now.
    at_rest: Placement,
    moved: Placement,
    /// Its rotator has turned it and will not again until armed anew.
    rotated: bool,
    /// The cell it is on its way to, and its shift when it gets there.
    going: Option<Pos>,
    target: [f32; 3],
    /// Share of full speed for the step under way.
    speed: f32,
    turning: Option<Turning>,
    /// Milliseconds still to wait after being blocked, and waited so far.
    move_wait: f32,
    move_waited: f32,
    turn_wait: f32,
    turn_waited: f32,
    /// Seconds it has been blocked for.
    move_blocked: f32,
    turn_blocked: f32,
    /// Seconds before it decides on its next step.
    pause: f32,
    /// At the end of this step a shootout rail lets go of it.
    shooting: bool,
    /// No longer docked: it stays where it is, or flies off.
    undocked: bool,
    launched: Option<([f32; 3], f32)>,
}

/// What a press of the activate key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// The block was switched; the simulation has work to do.
    Done,
    /// A console: what is linked from it was pressed.
    Console(usize),
    /// In the game this opens a window or seats the player. Nothing here.
    Dialog,
    /// The game ignores a press on this kind of block.
    Nothing,
}

/// Something worth telling whoever is watching.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// A part was undocked: by a signal to its docker, or to the rail under it.
    Undocked { part: usize },
    /// A part reached the end of a shootout rail and was let go.
    Launched { part: usize },
    /// A weapon or tool computer's trigger went down or up.
    Trigger { entity: usize, pos: Pos, on: bool },
}

/// The simulation of one design.
pub struct Sim {
    /// Milliseconds since it started.
    now: f64,
    logic: Vec<Logic>,
    /// By part of the rig; None for the main entity, turrets and what is held still.
    rails: Vec<Option<Rail>>,
    /// Which wireless logic module each one speaks to: (entity, position).
    wireless: HashMap<(usize, Pos), (usize, Pos)>,
    /// What every block it changed was before.
    originals: HashMap<(usize, Pos), Block>,
    /// Blocks changed since the caller last took this: (entity, position).
    pub changed: Vec<(usize, Pos)>,
    pub notes: Vec<Note>,
    /// Whether any part moved since the caller last cleared this.
    pub moved: bool,
    random: u64,
    /// Ticks to keep going after the last thing happened, so that rails get
    /// to decide on what logic has just changed.
    settle: u32,
}

fn add(p: Pos, d: [i32; 3]) -> Pos {
    p.offset(d[0], d[1], d[2])
}

fn toward(from: f32, to: f32, most: f32) -> f32 {
    if (to - from).abs() <= most {
        to
    } else {
        from + most * (to - from).signum()
    }
}

/// Block kinds, by what the game's block list says of them.
struct Kinds<'a>(&'a dyn BlockLinks);

impl Kinds<'_> {
    fn is(&self, ty: u16, flags: u16) -> bool {
        self.0.link_traits(ty).is_some_and(|t| t.is(flags))
    }
    fn logic(&self, ty: u16) -> bool {
        self.is(ty, flag::SIGNAL)
    }
    fn track(&self, ty: u16) -> bool {
        self.is(ty, flag::RAIL_TRACK)
    }
    fn button(&self, ty: u16) -> bool {
        self.is(ty, flag::BUTTON)
    }
    fn door(&self, ty: u16) -> bool {
        self.is(ty, flag::DOOR)
    }
}

impl Sim {
    /// Starts from the design as it is: every active bit as saved, every
    /// docked part where the rig has it. `world` is every entity of the
    /// design in the order the rig counts them, and `wireless` the pairs of
    /// wireless logic modules ([`crate::Blueprint::wireless`]).
    pub fn new(rig: &mut Rig, world: &[&Entity], kinds: &dyn BlockLinks, wireless: &[((usize, Pos), (usize, Pos))]) -> Sim {
        let kinds = Kinds(kinds);
        let logic = world
            .iter()
            .map(|entity| {
                let mut logic = Logic::default();
                for (controller, by_type) in entity.links() {
                    let ty = entity.get(*controller).ty();
                    if ty == SPEED_CONTROLLER {
                        logic.speed_controllers.push(*controller);
                    }
                    if !kinds.logic(ty) {
                        continue;
                    }
                    for gate in [AND, OR] {
                        for target in by_type.get(&gate).into_iter().flatten() {
                            logic.inputs.entry(*target).or_default().push(*controller);
                        }
                    }
                }
                logic
            })
            .collect();
        let rails = rig
            .parts
            .iter_mut()
            .map(|part| {
                let dock = part.dock.as_ref()?;
                let parent_has_track = kinds.track(dock.rail.ty);
                if !parent_has_track {
                    return None;
                }
                // The rails take over from where the design was saved.
                part.value = 0.0;
                part.driven = Some(dock.moved);
                let facing = turned_axes(dock.rail.orientation).0;
                let shift = dock.moved.shift.map(|c| c.round() as i32);
                Some(Rail {
                    contact: add(add(dock.rail.pos, facing), shift),
                    at_rest: dock.moved,
                    moved: dock.moved,
                    rotated: dock.rotated,
                    going: None,
                    target: dock.moved.shift,
                    speed: DEFAULT_SPEED,
                    turning: None,
                    move_wait: 0.0,
                    move_waited: 0.0,
                    turn_wait: 0.0,
                    turn_waited: 0.0,
                    move_blocked: 0.0,
                    turn_blocked: 0.0,
                    pause: 0.0,
                    shooting: false,
                    undocked: false,
                    launched: None,
                })
            })
            .collect();
        rig.place();
        Sim { now: 0.0, logic, rails, wireless: wireless.iter().copied().collect(), originals: HashMap::new(), changed: Vec::new(), notes: Vec::new(), moved: true, random: 0x2545_F491_4F6C_DD1D, settle: 3 }
    }

    /// Whether anything is still under way: a signal waiting, a delay
    /// running, a part moving or about to decide.
    pub fn busy(&self) -> bool {
        self.settle > 0
            || self.logic.iter().any(|l| !l.waiting.is_empty() || !l.delayed.is_empty() || !l.delayed_once.is_empty())
            || self.rails.iter().flatten().any(|r| !r.undocked && (r.going.is_some() || r.turning.is_some() || r.pause > 0.0) || r.launched.is_some())
    }

    /// Puts every block the simulation changed back the way it was.
    pub fn undo(&mut self, world: &mut [&mut Entity]) {
        for ((entity, pos), block) in self.originals.drain() {
            if let Some(e) = world.get_mut(entity) {
                e.set(pos, block);
                self.changed.push((entity, pos));
            }
        }
    }

    /// Whether the simulation has changed any block.
    pub fn touched(&self) -> bool {
        !self.originals.is_empty()
    }

    fn roll(&mut self) -> f32 {
        // Xorshift: the same run every time, which a demonstration wants.
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random >> 40) as f32 / (1u64 << 24) as f32
    }

    fn write(&mut self, world: &mut [&mut Entity], entity: usize, pos: Pos, block: Block) {
        let before = world[entity].get(pos);
        if before == block {
            return;
        }
        self.originals.entry((entity, pos)).or_insert(before);
        world[entity].set(pos, block);
        self.changed.push((entity, pos));
        self.settle = 3;
    }

    fn switch(&mut self, world: &mut [&mut Entity], entity: usize, pos: Pos, on: bool) -> bool {
        let mut block = world[entity].get(pos);
        if block.is_air() || block.active_bit() == on {
            return false;
        }
        block.set_active(on);
        self.write(world, entity, pos, block);
        true
    }

    fn send(&mut self, entity: usize, pos: Pos, on: bool) {
        self.logic[entity].waiting.push_back(Signal { pos, on, from_logic: false });
        self.settle = 3;
    }

    /// What the player's activate key would do on a block, without doing it.
    pub fn would_press(world: &[&Entity], kinds: &dyn BlockLinks, entity: usize, pos: Pos) -> Press {
        let kinds = Kinds(kinds);
        let Some(e) = world.get(entity) else { return Press::Nothing };
        let ty = e.get(pos).ty();
        if CONSOLES.contains(&ty) {
            let linked = e.links().get(&pos).into_iter().flat_map(|by_type| by_type.values().flatten());
            let usable = |target: &&Pos| {
                let there = e.get(**target).ty();
                kinds.is(there, flag::CAN_ACTIVATE) && !DIALOGS.contains(&there) && !CONSOLES.contains(&there)
            };
            return Press::Console(linked.filter(usable).count());
        }
        if DIALOGS.contains(&ty) {
            return Press::Dialog;
        }
        if kinds.is(ty, flag::CAN_ACTIVATE) || ty == REMOTE {
            Press::Done
        } else {
            Press::Nothing
        }
    }

    /// The player's activate key on a block.
    pub fn press(&mut self, world: &[&Entity], kinds: &dyn BlockLinks, entity: usize, pos: Pos) -> Press {
        let kinds = Kinds(kinds);
        let Some(e) = world.get(entity) else { return Press::Nothing };
        let block = e.get(pos);
        let ty = block.ty();
        if CONSOLES.contains(&ty) {
            // The console's own bit is not touched: every block linked from
            // it that can be switched gets the press.
            let linked: Vec<Pos> = e.links().get(&pos).into_iter().flat_map(|by_type| by_type.values().flatten().copied()).collect();
            let mut pressed = 0;
            for target in linked {
                let there = e.get(target);
                if kinds.is(there.ty(), flag::CAN_ACTIVATE) && !DIALOGS.contains(&there.ty()) && !CONSOLES.contains(&there.ty()) {
                    self.send(entity, target, !there.active_bit());
                    pressed += 1;
                }
            }
            return Press::Console(pressed);
        }
        if DIALOGS.contains(&ty) {
            return Press::Dialog;
        }
        // The inner ship remote opens a window to name it; its switch is on
        // the hotbar. Here the press is that switch.
        if !kinds.is(ty, flag::CAN_ACTIVATE) && ty != REMOTE {
            return Press::Nothing;
        }
        // A flip-flop that is on is sent "on" again, which turns it off.
        let on = if ty == FLIP_FLOP && block.active_bit() { true } else { !block.active_bit() };
        self.send(entity, pos, on);
        Press::Done
    }

    /// Runs the simulation on by a stretch of time, a tick at a time.
    /// Returns how many seconds of it were used up in whole ticks.
    pub fn run(&mut self, seconds: f32, rig: &mut Rig, world: &mut [&mut Entity], kinds: &dyn BlockLinks, shapes: &dyn Shapes) -> f32 {
        let ticks = (seconds as f64 * 1000.0 / TICK_MS).floor() as usize;
        for _ in 0..ticks {
            self.tick(rig, world, kinds, shapes);
        }
        (ticks as f64 * TICK_MS / 1000.0) as f32
    }

    /// One tick of the game server.
    pub fn tick(&mut self, rig: &mut Rig, world: &mut [&mut Entity], kinds: &dyn BlockLinks, shapes: &dyn Shapes) {
        let kinds = Kinds(kinds);
        self.settle = self.settle.saturating_sub(1);
        let seconds = (TICK_MS / 1000.0) as f32;
        for part in 1..rig.parts.len() {
            if self.rails.get(part).is_some_and(Option::is_some) {
                self.decide(part, rig, world, &kinds, seconds);
                self.advance(part, rig, world, &kinds, shapes, seconds);
            }
        }
        for entity in 0..world.len().min(self.logic.len()) {
            self.logic_pass(entity, rig, world, &kinds);
        }
        self.now += TICK_MS;
    }

    // ---- logic ----

    fn logic_pass(&mut self, entity: usize, rig: &mut Rig, world: &mut [&mut Entity], kinds: &Kinds) {
        // Signals handled here may put new chain starts in the queue (a rail
        // swap, an undocking): those are taken in the same tick too.
        for _ in 0..64 {
            if self.logic[entity].waiting.is_empty() {
                break;
            }
            let mut traces: Vec<Trace> = self.logic[entity].waiting.drain(..).map(|signal| Trace { signal, parent: None }).collect();
            let mut next = 0;
            while next < traces.len() && traces.len() < 250_000 {
                self.handle(entity, next, &mut traces, rig, world, kinds);
                next += 1;
            }
        }
        // Delays that have run out start a chain in the next tick.
        let now = self.now;
        let logic = &mut self.logic[entity];
        let due: Vec<Pos> = logic.delayed.iter().filter(|(_, (_, since))| now - since > DELAY_MS).map(|(p, _)| *p).collect();
        for pos in due {
            let (signal, _) = logic.delayed.remove(&pos).expect("just listed");
            // A button lets go, whatever it was holding.
            let on = if kinds.button(world[entity].get(pos).ty()) { false } else { signal.on };
            logic.waiting.push_back(Signal { pos, on, from_logic: false });
        }
        let due: Vec<Pos> = logic.delayed_once.iter().filter(|(_, (_, since))| now - since > DELAY_MS).map(|(p, _)| *p).collect();
        for pos in due {
            let (signal, _) = logic.delayed_once.remove(&pos).expect("just listed");
            logic.waiting.push_back(Signal { from_logic: false, ..signal });
        }
    }

    /// Handles one signal; what the block passes on is added to `traces`.
    fn handle(&mut self, entity: usize, index: usize, traces: &mut Vec<Trace>, rig: &mut Rig, world: &mut [&mut Entity], kinds: &Kinds) {
        let Signal { pos, on, from_logic } = traces[index].signal;
        let block = world[entity].get(pos);
        if block.is_air() {
            return;
        }
        let (ty, was) = (block.ty(), block.active_bit());
        let now = self.now;
        // A button that goes in comes out again half a second later.
        if kinds.button(ty) && !was && on {
            self.logic[entity].delayed.insert(pos, (Signal { pos, on, from_logic: false }, now));
        }
        // A sensor reads live ship values; warp gates and the AI switch
        // things a design at rest does not have.
        if matches!(ty, SENSOR | 542 | 121) {
            return;
        }
        // A wireless module tells the one it speaks to, on whatever entity
        // that is, unless that one is there already. Reached through other
        // logic it tells it at once; pressed itself, a second later.
        if ty == WIRELESS {
            if let Some(&(there, other)) = self.wireless.get(&(entity, pos)) {
                let partner = world.get(there).map_or(Block::AIR, |e| e.get(other));
                if partner.ty() == WIRELESS && partner.active_bit() == was && there < self.logic.len() {
                    let told = Signal { pos: other, on, from_logic: false };
                    if traces[index].parent.is_none() {
                        self.logic[there].delayed.insert(other, (told, now + DELAY_MS));
                    } else {
                        self.logic[there].waiting.push_back(told);
                    }
                    self.settle = 3;
                }
            }
        }
        if from_logic {
            // A delay holds what a logic block sends it. One signal at a
            // time: a newer one takes the older one's place and starts the
            // half second over.
            if ty == DELAY {
                self.logic[entity].delayed.insert(pos, (Signal { pos, on, from_logic: false }, now));
                return;
            }
            if ty == DELAY_ONCE {
                let logic = &mut self.logic[entity];
                if logic.chain > logic.last_chain.get(&pos).copied().unwrap_or(0) {
                    logic.last_chain.insert(pos, logic.chain);
                    logic.delayed_once.insert(pos, (Signal { pos, on, from_logic: false }, now));
                }
                return;
            }
        } else {
            let logic = &mut self.logic[entity];
            match (ty, logic.last_chain.get(&pos)) {
                (DELAY_ONCE, Some(seen)) => logic.chain = *seen,
                _ => logic.chain += 1,
            }
        }
        if kinds.logic(ty) {
            // The same block twice on one path: the game cuts the loop here.
            let mut ancestor = traces[index].parent;
            while let Some(a) = ancestor {
                if traces[a].signal.pos == pos {
                    return;
                }
                ancestor = traces[a].parent;
            }
            let now_on = match ty {
                AND => on && self.inputs_of(entity, pos).iter().all(|input| world[entity].get(*input).active_bit()),
                OR => on || self.inputs_of(entity, pos).iter().any(|input| world[entity].get(*input).active_bit()),
                NOT => !on,
                RANDOM => self.roll() > 0.5,
                // On every "on" it changes over; "off" leaves it be.
                FLIP_FLOP => {
                    if on {
                        !was
                    } else {
                        was
                    }
                }
                _ => on,
            };
            // What the block does to rails beside it is decided by the
            // signal that came in (for a NOT, by what it puts out).
            let swaps = (on != was || (ty == NOT && on == was)) && if ty == NOT { !on } else { on };
            let chain = self.logic[entity].chain;
            self.logic[entity].last_chain.insert(pos, chain);
            if now_on == was {
                return;
            }
            let targets: Vec<Pos> = world[entity].links().get(&pos).into_iter().flat_map(|by_type| by_type.values().flatten().copied()).collect();
            for target in &targets {
                traces.push(Trace { signal: Signal { pos: *target, on: now_on, from_logic: true }, parent: Some(index) });
            }
            self.switch(world, entity, pos, now_on);
            if swaps && !targets.is_empty() {
                self.rail_logic(entity, pos, &targets, rig, world, kinds);
            }
            return;
        }
        if ty == DOCKER {
            // "On" to the docker of a docked entity lets go of the rail.
            if on {
                if let Some(part) = rig.parts.iter().position(|p| p.entity == entity && p.dock.as_ref().is_some_and(|d| d.docker.pos == pos)) {
                    self.undock(part, rig, world, kinds);
                }
            }
            return;
        }
        if kinds.track(ty) {
            // A signal to a rail block does nothing to the rail block.
            return;
        }
        if !from_logic && kinds.door(ty) {
            // A press on a door switches the whole door: every door block
            // that touches it, of whatever kind.
            let mut group = vec![pos];
            let mut seen: HashSet<Pos> = HashSet::from([pos]);
            let mut at = 0;
            while at < group.len() && group.len() < DOOR_GROUP_LIMIT {
                for d in SIDE_DIRS {
                    let beside = add(group[at], d);
                    if kinds.door(world[entity].get(beside).ty()) && seen.insert(beside) {
                        group.push(beside);
                    }
                }
                at += 1;
            }
            for cell in group {
                self.switch(world, entity, cell, on);
            }
            return;
        }
        // Anything else is simply switched: a light, one block of a door, a computer.
        if self.switch(world, entity, pos, on) && kinds.is(ty, flag::MAIN | flag::SUPPORT) {
            self.notes.push(Note::Trigger { entity, pos, on });
        }
    }

    fn inputs_of(&self, entity: usize, gate: Pos) -> Vec<Pos> {
        self.logic[entity].inputs.get(&gate).cloned().unwrap_or_default()
    }

    /// What a logic block that has just been switched on does to the rails
    /// it controls. With a rail block beside it, every rail it controls
    /// becomes a copy of that one, and a rotator among them is armed again.
    /// With none beside it, whatever is docked over a rail it controls is
    /// undocked.
    fn rail_logic(&mut self, entity: usize, pos: Pos, targets: &[Pos], rig: &mut Rig, world: &mut [&mut Entity], kinds: &Kinds) {
        let source = SIDE_DIRS.iter().map(|d| world[entity].get(add(pos, *d))).find(|b| kinds.track(b.ty()));
        let docked_over = |sim: &Sim, world: &[&mut Entity], target: Pos| -> Option<usize> {
            (1..rig.parts.len()).find(|&part| {
                rig.parts[rig.parts[part].parent].entity == entity
                    && sim.rails[part].as_ref().is_some_and(|rail| !rail.undocked && current_rails(rail.contact, world[entity], kinds).iter().any(|(at, _)| *at == target))
            })
        };
        match source {
            Some(source) => {
                for &target in targets {
                    let there = world[entity].get(target);
                    let both_rotators = ROTATORS.contains(&source.ty()) && ROTATORS.contains(&there.ty());
                    if !kinds.track(there.ty()) && !both_rotators {
                        continue;
                    }
                    if (there.ty(), there.orientation()) != (source.ty(), source.orientation()) {
                        self.write(world, entity, target, source);
                    }
                    if let Some(part) = docked_over(self, world, target) {
                        if let Some(rail) = &mut self.rails[part] {
                            rail.rotated = false;
                        }
                    }
                    self.settle = 3;
                }
            }
            None => {
                if let Some(part) = targets.iter().find_map(|target| docked_over(self, world, *target)) {
                    self.undock(part, rig, world, kinds);
                }
            }
        }
    }

    // ---- rails ----

    /// Sends a value to the logic a rail block speaks to: the logic blocks
    /// linked from it or, if there are none, the activation modules and
    /// buttons beside it. `except` are blocks that get nothing this time.
    fn rail_says(&mut self, entity: usize, rail: Pos, on: bool, except: &[Pos], world: &[&mut Entity], kinds: &Kinds) -> Vec<Pos> {
        let linked: Vec<Pos> = world[entity]
            .links()
            .get(&rail)
            .into_iter()
            .flat_map(|by_type| by_type.iter().filter(|(ty, _)| kinds.logic(**ty)).flat_map(|(_, cells)| cells.iter().copied()))
            .collect();
        if linked.is_empty() {
            self.says_beside(entity, rail, on, world, kinds);
        } else {
            for pos in linked.iter().filter(|p| !except.contains(p)) {
                self.send(entity, *pos, on);
            }
        }
        linked
    }

    /// Sends a value to the activation modules and buttons beside a block.
    fn says_beside(&mut self, entity: usize, pos: Pos, on: bool, world: &[&mut Entity], kinds: &Kinds) {
        for d in SIDE_DIRS {
            let beside = add(pos, d);
            if kinds.is(world[entity].get(beside).ty(), flag::BY_RAIL) {
                self.send(entity, beside, on);
            }
        }
    }

    fn undock(&mut self, part: usize, rig: &Rig, world: &[&mut Entity], kinds: &Kinds) {
        let Some(rail) = self.rails.get(part).cloned().flatten().filter(|r| !r.undocked) else { return };
        let (on, own) = (rig.parts[rig.parts[part].parent].entity, rig.parts[part].entity);
        for (at, block) in current_rails(rail.contact, world[on], kinds) {
            if !ROTATORS.contains(&block.ty()) {
                self.rail_says(on, at, false, &[], world, kinds);
            }
        }
        if let Some(dock) = &rig.parts[part].dock {
            self.says_beside(own, dock.docker.pos, false, world, kinds);
        }
        if let Some(rail) = &mut self.rails[part] {
            rail.undocked = true;
            rail.going = None;
            rail.turning = None;
        }
        self.notes.push(Note::Undocked { part });
        self.settle = 3;
    }

    /// The share of full speed for a step that starts on a rail block: the
    /// part of the activation modules linked from its speed controller that
    /// are on, or a half where there is no controller or it has no modules.
    fn speed_on(&self, entity: usize, rail: Pos, world: &[&mut Entity]) -> f32 {
        let e = &world[entity];
        for controller in &self.logic[entity].speed_controllers {
            let Some(by_type) = e.links().get(controller) else { continue };
            if !by_type.values().any(|cells| cells.contains(&rail)) {
                continue;
            }
            return match by_type.get(&ACTIVATION).filter(|modules| !modules.is_empty()) {
                Some(modules) => modules.iter().filter(|m| e.get(**m).active_bit()).count() as f32 / modules.len() as f32,
                None => DEFAULT_SPEED,
            };
        }
        DEFAULT_SPEED
    }

    /// Plans the turn a rotator gives the part over it, if it gives one.
    fn plan_turn(&mut self, part: usize, entity: usize, at: Pos, block: Block, world: &[&mut Entity]) {
        let e = &world[entity];
        let modules: Vec<Pos> = e.links().get(&at).and_then(|by_type| by_type.get(&ACTIVATION)).into_iter().flatten().copied().collect();
        let on = modules.iter().filter(|m| e.get(**m).active_bit()).count();
        let Some(rail) = &mut self.rails[part] else { return };
        if !modules.is_empty() && on == 0 {
            rail.rotated = true;
            return;
        }
        // A quarter turn by itself; with modules linked from it, an eighth
        // of a turn for each that is on.
        let eighths = if modules.is_empty() { 2 } else { on.min(8) };
        // The clockwise one turns against the right-hand rule about the
        // side it faces.
        let sign = if block.ty() == ROTATORS[0] { -1.0 } else { 1.0 };
        rail.turning = Some(Turning { axis: turned_axes(block.orientation()).0.map(|c| c as f32), total: sign * eighths as f32 * 45.0, done: 0.0, back: false, forever: on > 8 });
        rail.turn_blocked = 0.0;
        self.settle = 3;
    }

    /// What an idle part does next: the game asks this every tick.
    fn decide(&mut self, part: usize, rig: &Rig, world: &[&mut Entity], kinds: &Kinds, seconds: f32) {
        let on = rig.parts[rig.parts[part].parent].entity;
        let Some(rail) = &mut self.rails[part] else { return };
        if rail.undocked || rail.going.is_some() || rail.turning.is_some() {
            return;
        }
        if rail.pause > 0.0 {
            rail.pause -= seconds;
            return;
        }
        let (contact, rotated) = (rail.contact, rail.rotated);
        for (at, block) in current_rails(contact, world[on], kinds) {
            let (facing, arrow) = turned_axes(block.orientation());
            let next = world[on].get(add(at, arrow));
            let rotator = ROTATORS.contains(&block.ty());
            let onward = kinds.track(next.ty()) && turned_axes(next.orientation()).0 == facing;
            let lets_go = block.ty() == SHOOTOUT && !(next.ty() == SHOOTOUT && next.orientation() == block.orientation());
            if lets_go {
                let Some(rail) = &mut self.rails[part] else { return };
                rail.speed = 5.0;
                rail.going = Some(add(contact, arrow));
                rail.target = [0, 1, 2].map(|i| rail.at_rest.shift[i] + arrow[i] as f32);
                rail.shooting = true;
                self.settle = 3;
            } else if onward {
                let speed = if next.ty() == SHOOTOUT { 5.0 } else { self.speed_on(on, at, world) };
                if speed == 0.0 {
                    return;
                }
                if !rotated && rotator {
                    self.plan_turn(part, on, at, block, world);
                }
                let Some(rail) = &mut self.rails[part] else { return };
                rail.speed = speed;
                rail.going = Some(add(contact, arrow));
                rail.target = [0, 1, 2].map(|i| rail.at_rest.shift[i] + arrow[i] as f32);
                self.settle = 3;
            } else if !rotated && rotator {
                let speed = self.speed_on(on, at, world);
                if speed == 0.0 {
                    continue;
                }
                if let Some(rail) = &mut self.rails[part] {
                    rail.speed = speed;
                }
                self.plan_turn(part, on, at, block, world);
            }
        }
    }

    /// Whether the part, where the rails now have it, is in something's way.
    fn blocked(&mut self, part: usize, moved: &Placement, rig: &mut Rig, world: &[&mut Entity], shapes: &dyn Shapes) -> bool {
        rig.parts[part].driven = Some(*moved);
        rig.place();
        let entities: Vec<&Entity> = world.iter().map(|e| &**e).collect();
        rig.hit(&entities, part, shapes).is_some()
    }

    /// Moves a part on by one tick of the step or turn it is making.
    fn advance(&mut self, part: usize, rig: &mut Rig, world: &mut [&mut Entity], kinds: &Kinds, shapes: &dyn Shapes, seconds: f32) {
        let Some(mut rail) = self.rails[part].clone() else { return };
        let on = rig.parts[rig.parts[part].parent].entity;
        if let Some((direction, since)) = rail.launched {
            // Let go: it flies on for a little while, then is left there.
            rail.moved.shift = [0, 1, 2].map(|i| rail.moved.shift[i] + direction[i] * LAUNCH_SPEED * seconds);
            rail.launched = (since + seconds < 2.0).then_some((direction, since + seconds));
            rig.parts[part].driven = Some(rail.moved);
            rig.place();
            self.moved = true;
            self.rails[part] = Some(rail);
            return;
        }
        if rail.undocked {
            return;
        }
        // A turn comes before a step.
        if let Some(mut turning) = rail.turning {
            if rail.turn_wait > 0.0 {
                rail.turn_wait -= seconds * 1000.0;
                rail.turn_waited += seconds * 1000.0;
                self.rails[part] = Some(rail);
                return;
            }
            let goal = if turning.back { 0.0 } else { turning.total.abs() };
            if turning.done == goal {
                // There: the turn becomes part of where it rests.
                if !turning.back {
                    rail.at_rest.turn = times(&about(turning.axis, turning.total.to_radians()), &rail.at_rest.turn);
                }
                rail.moved.turn = rail.at_rest.turn;
                rail.turning = None;
                rail.rotated = !turning.forever || turning.back;
                if rail.going.is_none() {
                    for (at, _) in current_rails(rail.contact, world[on], kinds) {
                        self.says_beside(on, at, true, world, kinds);
                    }
                }
                rig.parts[part].driven = Some(rail.moved);
                rig.place();
                self.moved = true;
                self.settle = 3;
                self.rails[part] = Some(rail);
                return;
            }
            let next = toward(turning.done, goal, TURN_SPEED * rail.speed * seconds);
            let before = rail.moved;
            let turn = |degrees: f32| times(&about(turning.axis, (turning.total.signum() * degrees).to_radians()), &rail.at_rest.turn);
            let mut moved = rail.moved;
            moved.turn = turn(next);
            if self.blocked(part, &moved, rig, world, shapes) {
                rig.parts[part].driven = Some(before);
                rig.place();
                rail.turn_wait = BLOCKED_WAIT_MS;
                rail.turn_blocked += seconds + rail.turn_waited / 1000.0;
                rail.turn_waited = 0.0;
                if rail.turn_blocked > 1.0 {
                    // Still blocked after the wait: back to where it started.
                    turning.back = true;
                    rail.turn_blocked = 0.0;
                }
            } else {
                rail.moved = moved;
                turning.done = next;
                self.moved = true;
            }
            rail.turning = Some(turning);
            self.rails[part] = Some(rail);
            return;
        }
        let Some(going) = rail.going else { return };
        if rail.move_wait > 0.0 {
            rail.move_wait -= seconds * 1000.0;
            rail.move_waited += seconds * 1000.0;
            self.rails[part] = Some(rail);
            return;
        }
        let step = RAIL_SPEED * rail.speed * seconds;
        let before = rail.moved;
        let mut moved = rail.moved;
        moved.shift = [0, 1, 2].map(|i| toward(moved.shift[i], rail.target[i], step));
        if self.blocked(part, &moved, rig, world, shapes) {
            rig.parts[part].driven = Some(before);
            rig.place();
            rail.move_wait = BLOCKED_WAIT_MS;
            rail.move_blocked += seconds + rail.move_waited / 1000.0;
            rail.move_waited = 0.0;
            if rail.move_blocked >= 1.0 {
                // Still blocked after the wait: back to the cell it came
                // from, and a pause before it tries again.
                rail.going = Some(rail.contact);
                rail.target = rail.at_rest.shift;
                rail.shooting = false;
                rail.pause = self.roll() * 5.0;
            }
            self.rails[part] = Some(rail);
            return;
        }
        rail.moved = moved;
        self.moved = true;
        if moved.shift != rail.target {
            rail.move_blocked = 0.0;
            self.rails[part] = Some(rail);
            return;
        }
        // Arrived. The rail it is over now says "on" to its logic, then the
        // one it left says "off".
        let left: Vec<Pos> = current_rails(rail.contact, world[on], kinds).into_iter().filter(|(_, b)| !ROTATORS.contains(&b.ty())).map(|(at, _)| at).collect();
        let direction = [0, 1, 2].map(|i| rail.target[i] - rail.at_rest.shift[i]);
        rail.contact = going;
        rail.at_rest.shift = rail.moved.shift;
        rail.going = None;
        rail.move_blocked = 0.0;
        let mut reached = Vec::new();
        for (at, block) in current_rails(rail.contact, world[on], kinds) {
            if ROTATORS.contains(&block.ty()) {
                continue;
            }
            rail.rotated = false;
            reached.extend(self.rail_says(on, at, true, &[], world, kinds));
        }
        for at in left {
            self.rail_says(on, at, false, &reached, world, kinds);
        }
        if rail.shooting {
            rail.shooting = false;
            rail.undocked = true;
            rail.launched = Some((direction, 0.0));
            self.notes.push(Note::Launched { part });
        }
        self.settle = 3;
        self.rails[part] = Some(rail);
    }
}

/// The rail blocks a docker in a cell is on: every track block beside the
/// cell that faces it. Nearly always one.
fn current_rails(contact: Pos, on: &Entity, kinds: &Kinds) -> Vec<(Pos, Block)> {
    let mut out = Vec::new();
    for d in SIDE_DIRS {
        let at = add(contact, d);
        let block = on.get(at);
        let facing = turned_axes(block.orientation()).0;
        if kinds.track(block.ty()) && facing == d.map(|c| -c) {
            out.push((at, block));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Dock, Piece, NO_TURN};
    use crate::links::LinkTraits;
    use crate::shape::Geom;

    const LIGHT: u16 = 55;
    const DOOR: u16 = 122;
    const HULL: u16 = 5;
    const RAIL: u16 = 662;
    const BUTTON: u16 = 666;
    const CANNON: u16 = 6;

    struct TestKinds(HashMap<u16, LinkTraits>);

    impl BlockLinks for TestKinds {
        fn link_traits(&self, ty: u16) -> Option<&LinkTraits> {
            self.0.get(&ty)
        }
    }

    fn kinds() -> TestKinds {
        let logic = flag::SIGNAL | flag::CAN_ACTIVATE;
        let table = [
            (ACTIVATION, logic | flag::BY_RAIL),
            (BUTTON, logic | flag::BUTTON | flag::BY_RAIL),
            (AND, logic),
            (OR, logic),
            (NOT, logic),
            (DELAY, logic),
            (FLIP_FLOP, logic),
            (RANDOM, logic),
            (WIRELESS, logic),
            (975, flag::SIGNAL | flag::BUTTON | flag::CAN_ACTIVATE),
            (LIGHT, flag::LIGHT | flag::CAN_ACTIVATE),
            (DOOR, flag::DOOR | flag::CAN_ACTIVATE),
            (589, flag::DOOR | flag::CAN_ACTIVATE),
            (CANNON, flag::MAIN | flag::CAN_ACTIVATE),
            (RAIL, flag::RAIL_TRACK),
            (ROTATORS[0], flag::RAIL_TRACK),
            (ROTATORS[1], flag::RAIL_TRACK),
            (SHOOTOUT, flag::RAIL_TRACK | flag::GHOST),
        ];
        TestKinds(table.into_iter().map(|(ty, flags)| (ty, LinkTraits { flags, ..LinkTraits::default() })).collect())
    }

    struct Cubes(Geom);
    impl Shapes for Cubes {
        fn geom(&self, _: Block) -> Option<&Geom> {
            Some(&self.0)
        }
    }

    fn put(e: &mut Entity, at: (i32, i32, i32), ty: u16, on: bool) -> Pos {
        let p = Pos::new(at.0, at.1, at.2);
        e.set(p, Block::new(ty, 14, on));
        p
    }

    fn link(e: &mut Entity, from: Pos, to: Pos) {
        let ty = e.get(to).ty();
        e.link(from, to, ty);
    }

    /// A simulation of one entity with nothing docked to it.
    struct Bench {
        ship: Entity,
        rig: Rig,
        sim: Sim,
        kinds: TestKinds,
        shapes: Cubes,
    }

    impl Bench {
        fn new(ship: Entity) -> Bench {
            let (kinds, shapes) = (kinds(), Cubes(Geom::cube()));
            let mut rig = Rig::assemble(&[(String::new(), None, &ship)], |ty| ty == RAIL, &shapes);
            let sim = Sim::new(&mut rig, &[&ship], &kinds, &[]);
            Bench { ship, rig, sim, kinds, shapes }
        }

        fn press(&mut self, at: Pos) -> Press {
            self.sim.press(&[&self.ship], &self.kinds, 0, at)
        }

        fn run(&mut self, seconds: f32) {
            self.sim.run(seconds, &mut self.rig, &mut [&mut self.ship], &self.kinds, &self.shapes);
        }

        fn on(&self, at: Pos) -> bool {
            self.ship.get(at).active_bit()
        }
    }

    #[test]
    fn an_activation_module_switches_what_it_controls_and_a_button_lets_go_again() {
        let mut ship = Entity::new();
        let module = put(&mut ship, (0, 0, 0), ACTIVATION, false);
        let button = put(&mut ship, (2, 0, 0), BUTTON, false);
        let (lamp, other) = (put(&mut ship, (0, 2, 0), LIGHT, false), put(&mut ship, (2, 2, 0), LIGHT, false));
        link(&mut ship, module, lamp);
        link(&mut ship, button, other);
        let mut b = Bench::new(ship);
        assert_eq!(b.press(module), Press::Done);
        assert!(!b.on(lamp), "nothing happens before the next tick");
        b.run(0.04);
        assert!(b.on(module) && b.on(lamp));
        b.press(module);
        b.run(0.04);
        assert!(!b.on(module) && !b.on(lamp));
        // The button: on at once, off by itself a good half second later.
        b.press(button);
        b.run(0.04);
        assert!(b.on(button) && b.on(other));
        b.run(0.4);
        assert!(b.on(button) && b.on(other), "still in after 0.44 s");
        b.run(0.2);
        assert!(!b.on(button) && !b.on(other), "out after 0.64 s");
        b.run(0.2);
        assert!(!b.sim.busy(), "and then nothing is left to do");
        // A light pressed directly is switched by itself; hull is not a switch.
        b.press(lamp);
        b.run(0.04);
        assert!(b.on(lamp) && !b.on(module));
        let hull = put(&mut b.ship, (5, 5, 5), HULL, false);
        assert_eq!(b.press(hull), Press::Nothing);
        assert_eq!(b.press(Pos::new(9, 9, 9)), Press::Nothing);
        // Everything can be put back.
        assert!(b.sim.touched());
        b.sim.undo(&mut [&mut b.ship]);
        assert!(!b.on(lamp) && !b.on(other) && !b.on(module));
    }

    #[test]
    fn gates_follow_the_games_rules() {
        let mut ship = Entity::new();
        let (a, b_in) = (put(&mut ship, (0, 0, 0), ACTIVATION, false), put(&mut ship, (2, 0, 0), ACTIVATION, false));
        let (and, or, not) = (put(&mut ship, (0, 2, 0), AND, false), put(&mut ship, (2, 2, 0), OR, false), put(&mut ship, (4, 2, 0), NOT, false));
        let flip = put(&mut ship, (6, 2, 0), FLIP_FLOP, false);
        for gate in [and, or] {
            link(&mut ship, a, gate);
            link(&mut ship, b_in, gate);
        }
        link(&mut ship, a, not);
        link(&mut ship, a, flip);
        let mut b = Bench::new(ship);
        let state = |b: &Bench| (b.on(and), b.on(or), b.on(not), b.on(flip));
        b.press(a);
        b.run(0.04);
        assert_eq!(state(&b), (false, true, false, true), "a on: OR on, NOT stays off, the flip-flop changes over");
        b.press(b_in);
        b.run(0.04);
        assert_eq!(state(&b), (true, true, false, true), "both on: AND on");
        b.press(a);
        b.run(0.04);
        assert_eq!(state(&b), (false, true, true, true), "a off: AND off, NOT on, the flip-flop ignores off");
        b.press(a);
        b.run(0.04);
        assert_eq!(state(&b), (true, true, false, false), "a on again: the flip-flop changes back");
        // A press on a NOT does nothing: it is sent the opposite of what it
        // is, and puts out the opposite of that.
        b.press(not);
        b.run(0.04);
        assert!(!b.on(not));
        // A press forces an OR on and an AND off until an input changes.
        b.press(b_in);
        b.press(a);
        b.run(0.04);
        assert_eq!((b.on(and), b.on(or)), (false, false));
        b.press(or);
        b.run(0.04);
        assert!(b.on(or));
    }

    #[test]
    fn a_delay_passes_things_on_half_a_second_late_and_makes_a_clock_with_a_not() {
        let mut ship = Entity::new();
        let module = put(&mut ship, (0, 0, 0), ACTIVATION, false);
        let delay = put(&mut ship, (2, 0, 0), DELAY, false);
        let lamp = put(&mut ship, (4, 0, 0), LIGHT, false);
        link(&mut ship, module, delay);
        link(&mut ship, delay, lamp);
        // A NOT and a delay that feed each other.
        let not = put(&mut ship, (0, 4, 0), NOT, false);
        let round = put(&mut ship, (2, 4, 0), DELAY, false);
        link(&mut ship, not, round);
        link(&mut ship, round, not);
        // And a NOT that feeds itself.
        let alone = put(&mut ship, (0, 8, 0), NOT, false);
        let kick = put(&mut ship, (2, 8, 0), ACTIVATION, true);
        link(&mut ship, alone, alone);
        link(&mut ship, kick, alone);
        let mut b = Bench::new(ship);
        b.press(module);
        b.run(0.3);
        assert!(b.on(module) && !b.on(delay) && !b.on(lamp));
        b.run(0.3);
        assert!(b.on(delay) && b.on(lamp));
        // The clock: started by switching the delay on and off by hand, it
        // then runs by itself, changing over every half second or so.
        b.press(round);
        b.run(0.04);
        b.press(round);
        b.run(0.08);
        assert!(b.on(not), "the delay going off turns the NOT on");
        let mut changes = 0;
        let mut last = b.on(round);
        for _ in 0..100 {
            b.run(0.04);
            if b.on(round) != last {
                (last, changes) = (b.on(round), changes + 1);
            }
        }
        assert!((6..=8).contains(&changes), "{changes} changes in four seconds");
        assert!(b.sim.busy(), "a clock never stops");
        // The NOT that feeds itself is switched once and does not flicker.
        b.press(kick);
        b.run(0.04);
        assert!(b.on(alone));
        b.run(0.2);
        assert!(b.on(alone));
    }

    #[test]
    fn a_press_switches_a_whole_door_and_logic_one_block_of_it() {
        let mut ship = Entity::new();
        let door: Vec<Pos> = (0..3).map(|x| put(&mut ship, (x, 0, 0), DOOR, true)).collect();
        // Another kind of door touching it belongs to it; one apart does not.
        let glass = put(&mut ship, (3, 0, 0), 589, true);
        let apart = put(&mut ship, (5, 0, 0), DOOR, true);
        let module = put(&mut ship, (0, 3, 0), ACTIVATION, true);
        link(&mut ship, module, door[0]);
        let console = put(&mut ship, (0, 5, 0), 975, false);
        link(&mut ship, console, apart);
        link(&mut ship, console, module);
        let mut b = Bench::new(ship);
        b.press(door[1]);
        b.run(0.04);
        assert!(door.iter().all(|p| !b.on(*p)) && !b.on(glass), "open, all of it");
        assert!(b.on(apart));
        b.press(glass);
        b.run(0.04);
        assert!(door.iter().all(|p| b.on(*p)) && b.on(glass), "and shut again");
        // Through logic only the linked block follows.
        b.press(module);
        b.run(0.04);
        assert!(!b.on(door[0]) && b.on(door[1]));
        // A console presses what is linked from it and stays as it is.
        assert_eq!(b.press(console), Press::Console(2));
        b.run(0.04);
        assert!(!b.on(apart) && b.on(module) && b.on(door[0]) && !b.on(console));
        // In the game a core is entered, not switched.
        let core = put(&mut b.ship, (9, 9, 9), 1, false);
        assert_eq!(b.press(core), Press::Dialog);
    }

    #[test]
    fn a_computer_reports_its_trigger() {
        let mut ship = Entity::new();
        let module = put(&mut ship, (0, 0, 0), ACTIVATION, false);
        let cannon = put(&mut ship, (2, 0, 0), CANNON, false);
        link(&mut ship, module, cannon);
        let mut b = Bench::new(ship);
        b.press(module);
        b.run(0.04);
        assert_eq!(b.sim.notes, vec![Note::Trigger { entity: 0, pos: cannon, on: true }]);
    }

    /// A ship with rail track along Z and a rider docked on the block at z = 0.
    struct Yard {
        ship: Entity,
        rider: Entity,
        rig: Rig,
        sim: Sim,
        kinds: TestKinds,
        shapes: Cubes,
    }

    impl Yard {
        fn new(ship: Entity, rail_type: u16) -> Yard {
            Yard::with(ship, rail_type, Entity::new(), &[])
        }

        fn with(ship: Entity, rail_type: u16, mut rider: Entity, wireless: &[((usize, Pos), (usize, Pos))]) -> Yard {
            rider.set(Pos::new(0, -1, 0), Block::new(DOCKER, 10, false));
            rider.set(Pos::new(0, 0, 0), Block::new(1, 0, false));
            let dock = Dock {
                rail: Piece { pos: Pos::new(0, 0, 0), ty: rail_type, orientation: 14 },
                docker: Piece { pos: Pos::new(0, -1, 0), ty: DOCKER, orientation: 10 },
                turret: NO_TURN,
                moved: Placement::SAME,
                rotated: false,
            };
            let (kinds, shapes) = (kinds(), Cubes(Geom::cube()));
            let mut rig = Rig::assemble(&[(String::new(), None, &ship), ("ATTACHED_0".to_string(), Some(dock), &rider)], |ty| ty == RAIL, &shapes);
            let sim = Sim::new(&mut rig, &[&ship, &rider], &kinds, wireless);
            Yard { ship, rider, rig, sim, kinds, shapes }
        }

        fn run(&mut self, seconds: f32) {
            self.sim.run(seconds, &mut self.rig, &mut [&mut self.ship, &mut self.rider], &self.kinds, &self.shapes);
        }

        fn press(&mut self, at: Pos) {
            self.sim.press(&[&self.ship, &self.rider], &self.kinds, 0, at);
        }

        /// Where the rider's core is, in the ship's coordinates.
        fn core(&self) -> [f32; 3] {
            self.rig.parts[1].placed.at([0.0; 3])
        }
    }

    fn track(ship: &mut Entity, from: i32, to: i32) {
        for z in from..=to {
            ship.set(Pos::new(0, 0, z), Block::new(RAIL, 14, false));
        }
    }

    #[test]
    fn a_rider_travels_its_track_by_itself_and_the_rails_tell_the_logic_beside_them() {
        let mut ship = Entity::new();
        track(&mut ship, 0, 4);
        let start = put(&mut ship, (1, 0, 0), ACTIVATION, true);
        let end = put(&mut ship, (1, 0, 4), ACTIVATION, false);
        let lamp = put(&mut ship, (3, 0, 4), LIGHT, false);
        link(&mut ship, end, lamp);
        let mut y = Yard::new(ship, RAIL);
        assert_eq!(y.core(), [0.0, 2.0, 0.0]);
        // A block and a half a second without a speed controller.
        y.run(1.0);
        let z = y.core()[2];
        assert!((1.3..=1.6).contains(&z), "after a second it is at {z}");
        assert!(!y.ship.get(start).active_bit(), "the rail it left said off");
        y.run(3.0);
        assert_eq!(y.core(), [0.0, 2.0, 4.0], "it stops at the end of the track");
        assert!(y.ship.get(end).active_bit() && y.ship.get(lamp).active_bit(), "and the rail there said on");
        y.run(1.0);
        assert!(!y.sim.busy());
        assert_eq!(y.core(), [0.0, 2.0, 4.0]);
    }

    #[test]
    fn a_speed_controller_holds_a_rider_until_a_module_is_on() {
        let mut ship = Entity::new();
        track(&mut ship, 0, 3);
        let controller = put(&mut ship, (3, 0, 0), SPEED_CONTROLLER, false);
        let (slow, fast) = (put(&mut ship, (3, 2, 0), ACTIVATION, false), put(&mut ship, (3, 4, 0), ACTIVATION, false));
        for z in 0..=3 {
            link(&mut ship, controller, Pos::new(0, 0, z));
        }
        link(&mut ship, controller, slow);
        link(&mut ship, controller, fast);
        let mut y = Yard::new(ship, RAIL);
        y.run(1.0);
        assert_eq!(y.core()[2], 0.0, "no module on: no speed");
        y.press(slow);
        y.press(fast);
        y.run(0.6);
        let z = y.core()[2];
        assert!((1.3..=1.8).contains(&z), "both on: three blocks a second, at {z} after half a second");
    }

    #[test]
    fn logic_beside_a_rail_turns_the_track_round() {
        let mut ship = Entity::new();
        track(&mut ship, 0, 3);
        // A module with a rail beside it that points the other way, linked
        // to the whole track: switched on, it makes the track a copy of that.
        let module = put(&mut ship, (5, 0, 0), ACTIVATION, false);
        ship.set(Pos::new(5, 0, 1), Block::new(RAIL, 12, false));
        for z in 0..=3 {
            link(&mut ship, module, Pos::new(0, 0, z));
        }
        let mut y = Yard::new(ship, RAIL);
        y.run(3.0);
        assert_eq!(y.core()[2], 3.0);
        y.press(module);
        y.run(0.1);
        assert_eq!(y.ship.get(Pos::new(0, 0, 2)).orientation(), 12);
        y.run(3.0);
        assert_eq!(y.core()[2], 0.0, "it came all the way back");
        // The swapped rails are put back with everything else.
        y.sim.undo(&mut [&mut y.ship, &mut y.rider]);
        assert_eq!(y.ship.get(Pos::new(0, 0, 2)).orientation(), 14);
    }

    #[test]
    fn a_rotator_turns_once_and_again_when_logic_arms_it() {
        let mut ship = Entity::new();
        ship.set(Pos::new(0, 0, 0), Block::new(ROTATORS[0], 14, false));
        // A button away from the rotator, linked to it, with a rotator of
        // the same kind beside it: pressed, it arms the linked one again.
        let button = put(&mut ship, (3, 0, 0), BUTTON, false);
        ship.set(Pos::new(4, 0, 0), Block::new(ROTATORS[0], 14, false));
        link(&mut ship, button, Pos::new(0, 0, 0));
        let mut y = Yard::new(ship, ROTATORS[0]);
        y.rider.set(Pos::new(0, 0, 2), Block::new(HULL, 0, false));
        let nose = |y: &Yard| y.rig.parts[1].placed.at([0.0, 0.0, 2.0]).map(|c| (c * 100.0).round() / 100.0);
        assert_eq!(nose(&y), [0.0, 2.0, 2.0]);
        y.run(0.4);
        let halfway = nose(&y);
        assert!(halfway[0] < -0.5 && halfway[2] > 0.5, "halfway round after 0.4 s: {halfway:?}");
        y.run(0.6);
        assert_eq!(nose(&y), [-2.0, 2.0, 0.0], "a quarter turn clockwise, seen from above");
        y.run(2.0);
        assert_eq!(nose(&y), [-2.0, 2.0, 0.0], "and then it rests");
        assert!(!y.sim.busy());
        // The button, linked to the rotator, arms it again.
        y.press(button);
        y.run(1.2);
        assert_eq!(nose(&y), [0.0, 2.0, -2.0]);
    }

    #[test]
    fn a_blocked_rider_waits_goes_back_and_a_signal_to_its_docker_lets_go() {
        let mut ship = Entity::new();
        track(&mut ship, 0, 3);
        // Something standing over the track where the rider's core would pass.
        ship.set(Pos::new(0, 2, 2), Block::new(HULL, 0, false));
        let mut y = Yard::new(ship, RAIL);
        y.run(1.0);
        let z = y.core()[2];
        assert!(z > 0.9 && z < 1.1, "stopped short of the block, at {z}");
        y.run(12.0);
        let z = y.core()[2];
        assert!((0.0..=1.1).contains(&z), "it never gets past: {z}");
        // On the rider, a module linked to its docker.
        let mut ship = Entity::new();
        track(&mut ship, 0, 0);
        let mut y = Yard::new(ship, RAIL);
        let module = Pos::new(0, 1, 0);
        y.rider.set(module, Block::new(ACTIVATION, 0, false));
        y.rider.link(module, Pos::new(0, -1, 0), DOCKER);
        y.sim.press(&[&y.ship, &y.rider], &y.kinds, 1, module);
        y.run(0.1);
        assert_eq!(y.sim.notes, vec![Note::Undocked { part: 1 }]);
    }

    #[test]
    fn a_shootout_rail_lets_go_at_its_end() {
        let mut ship = Entity::new();
        for z in 0..=2 {
            ship.set(Pos::new(0, 0, z), Block::new(SHOOTOUT, 14, false));
        }
        let mut y = Yard::new(ship, SHOOTOUT);
        y.run(1.5);
        assert_eq!(y.sim.notes, vec![Note::Launched { part: 1 }]);
        assert!(y.core()[2] > 5.0, "it flew off the end: {:?}", y.core());
        y.run(3.0);
        assert!(!y.sim.busy());
    }

    #[test]
    fn wireless_modules_carry_a_signal_from_one_entity_to_another() {
        // On the ship a button linked to a wireless module; on the rider the
        // module it speaks to, linked to a lamp.
        let mut ship = Entity::new();
        track(&mut ship, 0, 0);
        let button = put(&mut ship, (4, 0, 0), BUTTON, false);
        let sender = put(&mut ship, (4, 2, 0), WIRELESS, false);
        link(&mut ship, button, sender);
        let mut rider = Entity::new();
        let receiver = put(&mut rider, (0, 2, 0), WIRELESS, false);
        let lamp = put(&mut rider, (0, 4, 0), LIGHT, false);
        link(&mut rider, receiver, lamp);
        let pairs = [((0, sender), (1, receiver)), ((1, receiver), (0, sender))];
        let mut y = Yard::with(ship, RAIL, rider, &pairs);
        y.press(button);
        y.run(0.12);
        assert!(y.ship.get(sender).active_bit());
        assert!(y.rider.get(receiver).active_bit() && y.rider.get(lamp).active_bit(), "reached through logic, it is told at once");
        y.run(0.7);
        assert!(!y.rider.get(lamp).active_bit(), "and the button letting go follows");
        // Pressed itself, a wireless module tells the other a second later.
        y.press(sender);
        y.run(0.5);
        assert!(y.ship.get(sender).active_bit() && !y.rider.get(receiver).active_bit());
        y.run(0.7);
        assert!(y.rider.get(receiver).active_bit() && y.rider.get(lamp).active_bit());
        assert!(y.ship.get(sender).active_bit(), "and the other does not answer back");
    }
}
