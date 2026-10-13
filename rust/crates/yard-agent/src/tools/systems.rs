//! Tools for systems: a computer with its modules, links between a
//! controller and what it runs, and taking back the agent's own last step.
//!
//! Links follow the game's rules (`yard_core::links::Rules`): only pairs
//! the game allows are made, and the model is told what was left out.

use super::{apply, blocks, check_volume, core_link, describe_change, entity_info, fingerprint, plural, pos_of, rules_for, show, span, Args, Ctx, Done, Tool};
use crate::schema;
use serde_json::Value;
use std::collections::BTreeMap;
use yard_core::catalog::Catalog;
use yard_core::edit::{Change, FillMode, Op, When, MAX_CELLS};
use yard_core::links::Rules;
use yard_core::{Entity, Pos};

/// Most positions one link or unlink call may list.
const MAX_TARGETS: usize = 500;
const CORE: Pos = Pos::new(0, 0, 0);

pub fn tools() -> Vec<Tool> {
    let targets = |what: &str| schema::array(what, schema::position("A block."), MAX_TARGETS);
    vec![
        Tool::on_entity(
            "place_system",
            "Places a computer and a box of its modules and links them, in one call: a weapon (Cannon Computer + Cannon Barrel), a salvager, any computer with the modules it controls. Modules of a weapon should form a line or bundle pointing forward (+Z) with nothing in front of it.",
            vec![
                ("computer", schema::string("The controlling block, e.g. \"Cannon Computer\".")),
                ("at", schema::position("Where the computer goes.")),
                ("facing", schema::choice("Which way the computer's front looks. Default front.", &super::orient::WORDS)),
                ("module", schema::string("The block it controls, e.g. \"Cannon Barrel\". find_blocks says what a computer controls.")),
                ("from", schema::position("One corner of the box of modules.")),
                ("to", schema::position("The opposite corner, included.")),
            ],
            &["computer", "at", "module", "from", "to"],
            place_system,
        ),
        Tool::on_entity(
            "link",
            "Links a controller (computer, logic block, the Ship Core...) to blocks that are already placed, as pressing V on them does in the game. Give positions, a box, or both; from a box every block the controller may control is linked and the rest is left out. Modules do nothing until linked.",
            vec![
                ("controller", schema::position("The controlling block.")),
                ("targets", targets("Blocks to link.")),
                ("from", schema::position("One corner of a box of blocks to link.")),
                ("to", schema::position("The opposite corner, included.")),
                ("block", schema::string("With a box: link only blocks of this type.")),
            ],
            &["controller"],
            link,
        ),
        Tool::on_entity(
            "unlink",
            "Removes links from a controller: to the listed positions, to the blocks in a box, or, with neither, to everything it controls.",
            vec![
                ("controller", schema::position("The controlling block.")),
                ("targets", targets("Blocks to unlink.")),
                ("from", schema::position("One corner of a box of blocks to unlink.")),
                ("to", schema::position("The opposite corner, included.")),
            ],
            &["controller"],
            unlink,
        ),
        Tool::on_entity(
            "undo",
            "Takes back your own latest change to the entity (one tool call's worth). Call it again to take back the one before. It never undoes the player's own edits.",
            vec![],
            &[],
            undo,
        ),
    ]
}

/// What a block type may control, for explaining a refusal.
fn controls_note(catalog: &Catalog, rules: &Rules<'_>, ty: u16) -> String {
    let Some(def) = catalog.get(ty) else { return String::new() };
    if !def.controlling.is_empty() {
        let names: Vec<String> = def.controlling.iter().take(8).map(|id| catalog.name(*id)).collect();
        format!("A {} controls: {}.", def.name, names.join(", "))
    } else if def.logic {
        format!("A {} is a logic block: it links to lights, doors, other logic blocks, rails and computers.", def.name)
    } else if rules.can_control(ty) {
        format!("A {} links only to a few kinds of block; read_guide \"linking\" has the rules.", def.name)
    } else {
        format!("A {} controls nothing; pick the computer or logic block as the controller.", def.name)
    }
}

fn place_system(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let catalog = ctx.world.catalog();
    let rules = rules_for(ctx, &info);
    let at = args.need_pos("at")?;
    let (lo, hi, volume) = span(args.need_pos("from")?, args.need_pos("to")?);
    // Each module is written and then linked: two operations per cell.
    check_volume(volume * 2 + 2)?;
    if at == CORE {
        return Err("\"at\" is the Ship Core's own cell. Put the computer next to it.".to_string());
    }
    let computer = blocks::resolve(catalog, &Args::of(&serde_json::json!({"block": args.text("computer"), "facing": args.text("facing")})), "block", false)?;
    let module = blocks::lookup(catalog, args.text("module").unwrap_or(""))?;
    if !rules.can_link(computer.ty(), module.id) {
        return Err(format!("A {} cannot control {}. {} Nothing was placed.", catalog.name(computer.ty()), module.name, controls_note(catalog, &rules, computer.ty())));
    }
    let module_block = yard_core::Block::new(module.id, 0, module.active_when_placed());
    let mut cells = Vec::new();
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let p = Pos::new(x, y, z);
                if p != at && p != CORE {
                    cells.push(p);
                }
            }
        }
    }
    let mut ops = vec![
        Op::Fill { min: lo, max: hi, block: module_block, mode: FillMode::Solid, when: When::Always, link: None },
        Op::Fill { min: at, max: at, block: computer, mode: FillMode::Solid, when: When::Always, link: None },
        Op::Link { controller: at, targets: cells },
    ];
    let on_bar = core_link(&rules, computer.ty()).is_some();
    if on_bar {
        ops.push(Op::Link { controller: CORE, targets: vec![at] });
    }
    let change = apply(ctx, entity, ops, "placing a system")?;
    let linked = change.linked.iter().filter(|(c, _, _)| *c == at).count();
    let (placed, replaced, removed) = super::tally(&change);
    let mut text = format!("Placed {} at {} and {} in {}..{}. It controls {linked} of them now.", catalog.name(computer.ty()), show(at), module.name, show(lo), show(hi));
    if on_bar {
        text.push_str(" The Ship Core controls the computer, which puts it on the pilot's bar.");
    }
    text.push_str(&format!(" {}", describe_change(&change)));
    Ok(Done { text, changed: placed + replaced + removed })
}

/// The positions a link or unlink call names: its list, and the blocks in
/// its box. Returns them with whether a box was given.
fn named_targets(ctx: &Ctx<'_>, args: &Args<'_>, entity: &str, only: Option<u16>) -> Result<(Vec<Pos>, bool), String> {
    let mut targets = Vec::new();
    for item in args.get("targets").and_then(Value::as_array).into_iter().flatten() {
        targets.push(pos_of(item)?);
    }
    let region = match (args.pos("from")?, args.pos("to")?) {
        (Some(a), Some(b)) => Some((a.lo(b), a.hi(b))),
        (None, None) => None,
        _ => return Err("Give both \"from\" and \"to\" for a box, or neither.".to_string()),
    };
    if let Some((lo, hi)) = region {
        ctx.world.with_entity(entity, &mut |e| {
            e.for_each(|p, b| {
                let inside = p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && p.z >= lo.z && p.z <= hi.z;
                if inside && only.is_none_or(|ty| b.ty() == ty) && !targets.contains(&p) {
                    targets.push(p);
                }
            })
        })?;
    }
    if targets.len() as u64 > MAX_CELLS {
        return Err(format!("That names {} blocks; one step may cover at most {MAX_CELLS}.", targets.len()));
    }
    Ok((targets, region.is_some()))
}

/// Reads the block types at some positions.
fn types_at(ctx: &Ctx<'_>, entity: &str, positions: &[Pos]) -> Result<Vec<u16>, String> {
    let mut types = Vec::new();
    ctx.world.with_entity(entity, &mut |e: &Entity| types = positions.iter().map(|p| e.get(*p).ty()).collect())?;
    Ok(types)
}

/// "3 Grey Basic Armor, 1 Glass" from counts by type.
fn counted(catalog: &Catalog, counts: &BTreeMap<u16, usize>) -> String {
    counts.iter().map(|(ty, n)| format!("{n} {}", catalog.name(*ty))).collect::<Vec<_>>().join(", ")
}

fn link(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let catalog = ctx.world.catalog();
    let rules = rules_for(ctx, &info);
    let controller = args.need_pos("controller")?;
    let only = args.text("block").map(|name| blocks::lookup(catalog, name).map(|d| d.id)).transpose()?;
    let (targets, _) = named_targets(ctx, args, entity, only)?;
    if targets.is_empty() {
        return Err("Nothing to link: give \"targets\" (positions) or a box with \"from\" and \"to\" that holds blocks.".to_string());
    }
    let controller_type = types_at(ctx, entity, &[controller])?[0];
    if controller_type == 0 {
        return Err(format!("There is no block at {} to be the controller.", show(controller)));
    }
    // Sort the candidates: what may be linked, and what is left out and why.
    let types = types_at(ctx, entity, &targets)?;
    let mut already = 0;
    let (mut allowed, mut empty, mut refused) = (Vec::new(), Vec::new(), BTreeMap::<u16, usize>::new());
    let mut linked_now = Vec::new();
    ctx.world.with_entity(entity, &mut |e| linked_now = targets.iter().map(|t| yard_core::links::is_linked(e.links(), controller, *t)).collect())?;
    for ((target, ty), linked) in targets.iter().zip(&types).zip(&linked_now) {
        if *target == controller {
            continue;
        } else if *ty == 0 {
            empty.push(*target);
        } else if *linked {
            already += 1;
        } else if rules.can_link(controller_type, *ty) {
            allowed.push(*target);
        } else {
            *refused.entry(*ty).or_insert(0) += 1;
        }
    }
    let name = catalog.name(controller_type);
    let mut notes = Vec::new();
    if !refused.is_empty() {
        notes.push(format!("Left out, because the game does not let a {name} control them: {}. {}", counted(catalog, &refused), controls_note(catalog, &rules, controller_type)));
    }
    if !empty.is_empty() {
        let shown: Vec<String> = empty.iter().take(5).map(|p| show(*p)).collect();
        notes.push(format!("{} named {} empty: {}{}.", empty.len(), if empty.len() == 1 { "position is" } else { "positions are" }, shown.join(", "), if empty.len() > 5 { "..." } else { "" }));
    }
    if already > 0 {
        notes.push(format!("{already} {} already linked.", if already == 1 { "was" } else { "were" }));
    }
    if allowed.is_empty() {
        return Err(format!("Nothing was linked to the {name} at {}. {}", show(controller), notes.join(" ")));
    }
    let change = apply(ctx, entity, vec![Op::Link { controller, targets: allowed }], "linking")?;
    let made = change.linked.iter().filter(|(c, _, _)| *c == controller).count();
    let mut text = format!("Linked {made} {} to the {name} at {}.", plural(made, "block"), show(controller));
    text.push_str(&side_effects(catalog, &change, controller));
    for note in notes {
        text.push_str(&format!(" {note}"));
    }
    Ok(Done { text, changed: 0 })
}

/// What a linking step did besides the links asked for: the game moves a
/// block that can have one controller only, and hands computers back to
/// the core.
fn side_effects(catalog: &Catalog, change: &Change, controller: Pos) -> String {
    let mut text = String::new();
    let taken = change.unlinked.iter().filter(|(c, _, _)| *c != controller).count();
    if taken > 0 {
        text.push_str(&format!(" {taken} of them left the controller they had before: such a block has one controller only."));
    }
    let replaced = change.unlinked.iter().filter(|(c, _, _)| *c == controller).count();
    if replaced > 0 {
        text.push_str(&format!(" {replaced} earlier {} of this controller gave way: it has one place for a block of that kind.", plural(replaced, "link")));
    }
    let to_core: Vec<String> = change.linked.iter().filter(|(c, _, _)| *c == CORE && controller != CORE).map(|(_, ty, t)| format!("{} at {}", catalog.name(*ty), show(*t))).collect();
    if !to_core.is_empty() {
        text.push_str(&format!(" The Ship Core took back: {}.", to_core.join(", ")));
    }
    text
}

fn unlink(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let catalog = ctx.world.catalog();
    let controller = args.need_pos("controller")?;
    let (mut targets, boxed) = named_targets(ctx, args, entity, None)?;
    let mut held = Vec::new();
    let mut controller_type = 0;
    ctx.world.with_entity(entity, &mut |e| {
        controller_type = e.get(controller).ty();
        held = e.links().get(&controller).into_iter().flat_map(|groups| groups.values().flatten().copied()).collect();
    })?;
    if controller_type == 0 {
        return Err(format!("There is no block at {}.", show(controller)));
    }
    let name = catalog.name(controller_type);
    if held.is_empty() {
        return Ok(Done::said(format!("The {name} at {} controls nothing; there is nothing to unlink.", show(controller))));
    }
    if targets.is_empty() && !boxed && !args.has("targets") {
        targets = held.clone();
    }
    let named = targets.len();
    targets.retain(|t| held.contains(t));
    if targets.is_empty() {
        return Ok(Done::said(format!("None of the {named} named {} is linked to the {name} at {}; nothing changed. It controls {} {}.", plural(named, "block"), show(controller), held.len(), plural(held.len(), "block"))));
    }
    let change = apply(ctx, entity, vec![Op::Unlink { controller, targets }], "unlinking")?;
    let gone = change.unlinked.iter().filter(|(c, _, _)| *c == controller).count();
    let mut text = format!("Unlinked {gone} {} from the {name} at {}; it still controls {}.", plural(gone, "block"), show(controller), held.len() - gone);
    let to_core: Vec<String> = change.linked.iter().filter(|(c, _, _)| *c == CORE).map(|(_, ty, t)| format!("{} at {}", catalog.name(*ty), show(*t))).collect();
    if !to_core.is_empty() {
        text.push_str(&format!(" The Ship Core took back: {}.", to_core.join(", ")));
    }
    Ok(Done { text, changed: 0 })
}

fn undo(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    entity_info(ctx, entity)?;
    let Some(mark) = ctx.conversation.last_step(entity).cloned() else {
        return Err("You have made no change to this entity in this conversation that could be taken back.".to_string());
    };
    let reversal = ctx.world.step(entity, vec![Op::Undo])?;
    // The history is shared with the player. If its latest step is not the
    // one remembered here, it is put back and left alone.
    if fingerprint(&reversal.inverse()) != mark.print {
        ctx.world.step(entity, vec![Op::Redo])?;
        ctx.conversation.forget_step(entity);
        return Err("The latest change to this entity is not yours: it was edited by hand since. Nothing was undone. To change it back, build the difference with the other tools.".to_string());
    }
    ctx.conversation.forget_step(entity);
    let (placed, replaced, removed) = super::tally(&reversal);
    let more = if ctx.conversation.last_step(entity).is_some() { " Call undo again to take back the change before it." } else { "" };
    Ok(Done { text: format!("Took back {}. {}{more}", mark.what, describe_change(&reversal)), changed: placed + replaced + removed })
}

#[cfg(test)]
mod tests {
    use super::super::testing::Bench;
    use super::CORE;
    use serde_json::json;
    use yard_core::edit::Op;
    use yard_core::Pos;

    fn cannon(bench: &mut Bench) -> String {
        bench.ok("place_system", json!({"computer": "Cannon Computer", "at": [1, 0, 0], "module": "Cannon Barrel", "from": [1, 0, 1], "to": [1, 0, 6]}))
    }

    #[test]
    fn place_system_places_and_links_a_computer_and_its_modules() {
        let mut bench = Bench::new();
        let said = cannon(&mut bench);
        assert_eq!(
            said,
            "Placed Cannon Computer at (1, 0, 0) and Cannon Barrel in (1, 0, 1)..(1, 0, 6). It controls 6 of them now. The Ship Core controls the computer, which puts it on the pilot's bar. 7 blocks placed, 7 links made."
        );
        let computer = Pos::new(1, 0, 0);
        assert!((1..=6).all(|z| bench.linked(computer, Pos::new(1, 0, z))) && bench.linked(CORE, computer));
        assert_eq!((bench.count(6), bench.count(16)), (1, 6));
    }

    #[test]
    fn place_system_works_inside_a_solid_hull_and_around_its_own_computer() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-3, -1, -3], "to": [3, 1, 8], "block": "Grey Basic Armor"}));
        // The box of modules includes the computer's own cell.
        let said = bench.ok("place_system", json!({"computer": "Damage Beam Computer", "at": [2, 0, 0], "facing": "up", "module": "Damage Beam Module", "from": [2, 0, 0], "to": [2, 0, 5]}));
        assert!(said.contains("It controls 5 of them now.") && said.contains("6 blocks changed"), "{said}");
        assert_eq!((bench.ty(2, 0, 0), bench.count(415)), (414, 5));
        assert!(bench.linked(Pos::new(2, 0, 0), Pos::new(2, 0, 5)));
    }

    #[test]
    fn place_system_refuses_pairs_the_game_does_not_link() {
        let mut bench = Bench::new();
        let wrong = bench.err("place_system", json!({"computer": "Cannon Computer", "at": [1, 0, 0], "module": "Missile Tube", "from": [1, 0, 1], "to": [1, 0, 3]}));
        assert!(wrong.contains("A Cannon Computer cannot control Missile Tube. A Cannon Computer controls: Cannon Barrel. Nothing was placed."), "{wrong}");
        assert!(bench.err("place_system", json!({"computer": "Cannon Computer", "at": [0, 0, 0], "module": "Cannon Barrel", "from": [1, 0, 1], "to": [1, 0, 3]})).contains("Ship Core's own cell"));
        assert!(bench.err("place_system", json!({"computer": "Cannon Computer", "at": [1, 0, 0], "module": "Cannon Barrel", "from": [1, 0, 1]})).contains("Missing argument \"to\""));
        assert_eq!(bench.blocks(), 1);
    }

    #[test]
    fn link_takes_what_the_game_allows_from_a_box_and_names_the_rest() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [2, 0, 0], "to": [2, 0, 0], "block": "Cannon Computer"}));
        bench.ok("fill", json!({"from": [2, 0, 1], "to": [2, 0, 4], "block": "Cannon Barrel"}));
        bench.ok("fill", json!({"from": [2, 1, 1], "to": [2, 1, 3], "block": "Grey Basic Armor"}));
        let computer = Pos::new(2, 0, 0);
        assert!(!bench.linked(computer, Pos::new(2, 0, 1)), "blocks placed apart are not linked by themselves");
        let said = bench.ok("link", json!({"controller": [2, 0, 0], "from": [2, 0, 0], "to": [2, 1, 4]}));
        assert!(said.starts_with("Linked 4 blocks to the Cannon Computer at (2, 0, 0)."), "{said}");
        assert!(said.contains("Left out, because the game does not let a Cannon Computer control them: 3 Grey Basic Armor. A Cannon Computer controls: Cannon Barrel."), "{said}");
        assert!((1..=4).all(|z| bench.linked(computer, Pos::new(2, 0, z))) && !bench.linked(computer, Pos::new(2, 1, 1)));
        let again = bench.err("link", json!({"controller": [2, 0, 0], "targets": [[2, 0, 1], [2, 0, 2], [9, 9, 9]]}));
        assert!(again.contains("Nothing was linked") && again.contains("1 named position is empty: (9, 9, 9).") && again.contains("2 were already linked."), "{again}");
    }

    #[test]
    fn link_reports_what_the_games_rules_did_besides() {
        let mut bench = Bench::new();
        cannon(&mut bench);
        // A second computer takes two of the first one's barrels: a module has one computer.
        bench.ok("fill", json!({"from": [2, 0, 0], "to": [2, 0, 0], "block": "Cannon Computer"}));
        let said = bench.ok("link", json!({"controller": [2, 0, 0], "targets": [[1, 0, 5], [1, 0, 6]]}));
        assert!(said.contains("Linked 2 blocks") && said.contains("2 of them left the controller they had before"), "{said}");
        assert!(!bench.linked(Pos::new(1, 0, 0), Pos::new(1, 0, 6)) && bench.linked(Pos::new(2, 0, 0), Pos::new(1, 0, 6)));
        // Slaving the second computer to the first takes it from the core.
        let slaved = bench.ok("link", json!({"controller": [1, 0, 0], "targets": [[2, 0, 0]]}));
        assert!(slaved.contains("Linked 1 block to the Cannon Computer at (1, 0, 0)."), "{slaved}");
        assert!(!bench.linked(CORE, Pos::new(2, 0, 0)));
        let freed = bench.ok("unlink", json!({"controller": [1, 0, 0], "targets": [[2, 0, 0]]}));
        assert!(freed.contains("Unlinked 1 block") && freed.contains("The Ship Core took back: Cannon Computer at (2, 0, 0)."), "{freed}");
    }

    #[test]
    fn link_checks_its_controller_and_filter() {
        let mut bench = Bench::new();
        cannon(&mut bench);
        assert!(bench.err("link", json!({"controller": [7, 7, 7], "targets": [[1, 0, 1]]})).contains("no block at (7, 7, 7)"));
        assert!(bench.err("link", json!({"controller": [1, 0, 0]})).contains("Nothing to link"));
        assert!(bench.err("link", json!({"controller": [1, 0, 0], "from": [0, 0, 0]})).contains("both \"from\" and \"to\""));
        assert!(bench.err("link", json!({"targets": [[1, 0, 1]]})).contains("Missing argument \"controller\""));
        let hull = bench.err("link", json!({"controller": [1, 0, 3], "targets": [[1, 0, 4]]}));
        assert!(hull.contains("A Cannon Barrel controls nothing; pick the computer or logic block as the controller."), "{hull}");
        bench.ok("fill", json!({"from": [3, 0, 1], "to": [3, 0, 2], "block": "White Light"}));
        bench.ok("fill", json!({"from": [3, 0, 3], "to": [3, 0, 3], "block": "Activation Module"}));
        let lights = bench.ok("link", json!({"controller": [3, 0, 3], "from": [1, 0, 0], "to": [3, 0, 9], "block": "White Light"}));
        assert!(lights.starts_with("Linked 2 blocks to the Activation Module at (3, 0, 3).") && !lights.contains("Left out"), "{lights}");
    }

    #[test]
    fn unlink_lets_go_of_named_blocks_a_box_or_everything() {
        let mut bench = Bench::new();
        cannon(&mut bench);
        let computer = Pos::new(1, 0, 0);
        let one = bench.ok("unlink", json!({"controller": [1, 0, 0], "targets": [[1, 0, 6], [5, 5, 5]]}));
        assert_eq!(one, "Unlinked 1 block from the Cannon Computer at (1, 0, 0); it still controls 5.");
        bench.ok("unlink", json!({"controller": [1, 0, 0], "from": [1, 0, 4], "to": [1, 0, 9]}));
        assert!(bench.linked(computer, Pos::new(1, 0, 3)) && !bench.linked(computer, Pos::new(1, 0, 4)));
        let none = bench.ok("unlink", json!({"controller": [1, 0, 0], "targets": [[1, 0, 6]]}));
        assert!(none.starts_with("None of the 1 named block is linked"), "{none}");
        let all = bench.ok("unlink", json!({"controller": [1, 0, 0]}));
        assert!(all.starts_with("Unlinked 3 blocks") && all.ends_with("it still controls 0."), "{all}");
        assert!(bench.ok("unlink", json!({"controller": [1, 0, 0]})).contains("controls nothing"));
        assert!(bench.err("unlink", json!({"controller": [8, 8, 8]})).contains("no block at (8, 8, 8)"));
    }

    #[test]
    fn undo_takes_back_the_agents_own_steps_latest_first() {
        let mut bench = Bench::new();
        assert!(bench.err("undo", json!({})).contains("made no change"));
        bench.ok("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}));
        cannon(&mut bench);
        let first = bench.ok("undo", json!({}));
        assert_eq!(first, "Took back placing a system. 7 blocks removed, 7 links removed. Call undo again to take back the change before it.");
        assert_eq!((bench.count(16), bench.count(598)), (0, 5));
        let second = bench.ok("undo", json!({}));
        assert_eq!(second, "Took back a fill. 5 blocks removed.");
        assert_eq!(bench.blocks(), 1);
        assert!(bench.err("undo", json!({})).contains("made no change"));
    }

    #[test]
    fn undo_leaves_the_players_own_edits_alone() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}));
        // The player places a block by hand: a step the agent did not make.
        let by_hand = Op::Set { cells: vec![(Pos::new(0, 5, 0), yard_core::Block::new(63, 0, false))], when: Default::default() };
        crate::World::step(&bench.world, "", vec![by_hand]).unwrap();
        let refused = bench.err("undo", json!({}));
        assert!(refused.contains("is not yours") && refused.contains("Nothing was undone"), "{refused}");
        assert_eq!((bench.ty(0, 5, 0), bench.count(598)), (63, 5), "the player's block and the agent's fill both stand");
        // The player can still undo their own step afterwards.
        crate::World::step(&bench.world, "", vec![Op::Undo]).unwrap();
        assert_eq!(bench.ty(0, 5, 0), 0);
    }

    #[test]
    fn steps_that_change_nothing_are_not_remembered_for_undo() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}));
        bench.ok("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}));
        bench.ok("undo", json!({}));
        assert_eq!(bench.blocks(), 1);
    }
}
