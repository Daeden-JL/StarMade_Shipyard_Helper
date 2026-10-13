//! Tools that place, change and remove blocks. Large shapes are one call
//! each: the model names a shape and its measures, never the blocks.

use super::shapes::{self, Grid, Section};
use super::{apply, blocks, check_volume, core_link, describe_change, entity_info, fills, plural, pos_of, rules_for, show, span, when_of, Args, Ctx, Done, Tool, ONLY_WHERE};
use crate::schema;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use yard_core::edit::{Change, FillMode, Op, When, MAX_CELLS};
use yard_core::shape::ShapeSet;
use yard_core::{Block, Pos};

/// Most blocks one place_blocks call may list.
const MAX_LISTED: usize = 200;
/// Most cross-sections of one hull.
const MAX_SECTIONS: usize = 32;
const MAX_WALL: i64 = 16;

/// The block arguments, followed by a tool's own.
fn with_block(mut own: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    let mut all = blocks::properties();
    all.append(&mut own);
    all
}

fn only_where() -> (&'static str, Value) {
    ("only_where", schema::string(ONLY_WHERE))
}

fn hollow() -> [(&'static str, Value); 2] {
    [
        ("hollow", schema::boolean("true builds only the outer shell, leaving the inside as it is. Default false (solid).")),
        ("thickness", schema::integer_in("Wall thickness of a hollow shape in blocks. Default 1.", 1, MAX_WALL)),
    ]
}

pub fn tools() -> Vec<Tool> {
    let section = schema::object(
        vec![
            ("z", schema::integer("Where along the ship this cross-section is (+Z is the front).")),
            ("half_width", schema::number("Blocks from the centre line to each side; the hull is 2 x half_width + 1 wide here. 0 is a single column.")),
            ("half_height", schema::number("Blocks from the centre up and down; the hull is 2 x half_height + 1 high here.")),
            ("roundness", schema::number("0 = box, 1 = ellipse (default), 2 = diamond; values between blend.")),
            ("y_offset", schema::number("Raises (or, negative, lowers) the centre of this section. Default 0.")),
        ],
        &["z", "half_width", "half_height"],
    );
    let listed = schema::object(with_block(vec![("pos", schema::position("Where the block goes."))]), &["pos"]);
    vec![
        Tool::on_entity(
            "fill",
            "Fills a box with one block: solid, hollow (only the six faces, one block thick) or frame (only the twelve edges). The main tool for decks, walls, rooms and blocky hulls. Use block \"air\" to cut openings.",
            with_block(vec![
                ("from", schema::position("One corner of the box.")),
                ("to", schema::position("The opposite corner, included.")),
                ("mode", schema::choice("Default solid.", &["solid", "hollow", "frame"])),
                only_where(),
            ]),
            &["block", "from", "to"],
            fill,
        ),
        Tool::on_entity(
            "line",
            "Places a straight line of blocks between two points, in any direction, optionally several blocks thick: struts, antennas, pipes, wing edges.",
            with_block(vec![
                ("from", schema::position("Where the line starts.")),
                ("to", schema::position("Where it ends, included.")),
                ("thickness", schema::integer_in("Width and height of the line in blocks. Default 1.", 1, 9)),
                only_where(),
            ]),
            &["block", "from", "to"],
            line,
        ),
        Tool::on_entity(
            "ellipsoid",
            "Places a sphere or ellipsoid, solid or as a hollow shell: domes, pods, rounded noses, tanks.",
            with_block({
                let mut own = vec![
                    ("center", schema::position("The centre block.")),
                    ("radii", json!({"type": "array", "description": "Radius along x, y and z: [rx, ry, rz]. A radius counts blocks from the centre block outward, so 3 spans 7 blocks.", "items": {"type": "number", "minimum": 0, "maximum": 400}, "minItems": 3, "maxItems": 3})),
                ];
                own.extend(hollow());
                own.push(only_where());
                own
            }),
            &["block", "center", "radii"],
            ellipsoid,
        ),
        Tool::on_entity(
            "cylinder",
            "Places a cylinder, cone or tapered tube along an axis, solid or hollow: engines, nacelles, gun barrels, towers, nose cones. Give end_radius 0 for a cone.",
            with_block({
                let mut own = vec![
                    ("from", schema::position("The centre of the end the shape starts at.")),
                    ("axis", schema::choice("The axis it runs along: z is front to back, y up and down, x left to right.", &["x", "y", "z"])),
                    ("length", schema::integer_in("How many blocks long. Positive runs toward the + end of the axis (front, up, left), negative toward the - end.", -1000, 1000)),
                    ("radius", json!({"type": "number", "description": "Radius at the start, counted from the centre block outward: 2 is 5 blocks across.", "minimum": 0, "maximum": 400})),
                    ("end_radius", json!({"type": "number", "description": "Radius at the far end, for a taper. Default: the same as radius. 0 makes a cone.", "minimum": 0, "maximum": 400})),
                    ("open_ends", schema::boolean("For a hollow shape: true leaves both ends open (a pipe), false closes them (default).")),
                ];
                own.extend(hollow());
                own.push(only_where());
                own
            }),
            &["block", "from", "axis", "length", "radius"],
            cylinder,
        ),
        Tool::on_entity(
            "hull",
            "Builds a ship-shaped hull in one call by lofting through cross-sections along Z: you give a few sections (stern, widest point, nose...), each with its z, half-width, half-height and roundness, and the hull blends smoothly between them. Hollow by default, closed at both ends. Always symmetric left to right. This is the tool for any hull that is not a plain box.",
            with_block(vec![
                ("sections", schema::array("The cross-sections, at least two, in any order. Example for a 30 long fighter: [{\"z\": -12, \"half_width\": 2, \"half_height\": 1.5}, {\"z\": -4, \"half_width\": 5, \"half_height\": 2.5}, {\"z\": 8, \"half_width\": 3, \"half_height\": 2}, {\"z\": 17, \"half_width\": 0, \"half_height\": 0}].", section, MAX_SECTIONS)),
                ("origin", schema::position("The point the sections are measured from. Default the Ship Core [0, 0, 0]. Shift x for a side pod.")),
                ("hollow", schema::boolean("true (default) builds a shell with room inside; false a solid body.")),
                ("thickness", schema::integer_in("Wall thickness of the shell in blocks. Default 1.", 1, MAX_WALL)),
                only_where(),
            ]),
            &["block", "sections"],
            hull,
        ),
        Tool::on_entity(
            "place_blocks",
            "Places individual blocks, each at its own position and optionally of its own type and turn: computers, lights, single wedges and other details. For anything larger use fill and the shape tools. Block arguments given outside the list apply to every listed block that does not set its own.",
            with_block(vec![("blocks", schema::array("The blocks to place.", listed, MAX_LISTED)), only_where()]),
            &["blocks"],
            place_blocks,
        ),
        Tool::on_entity(
            "replace",
            "Swaps every block of one type for another, inside a box or in the whole entity: recolouring a hull, changing armor grade. Wedges, corners and slabs of the old block become the same shapes of the new one, turned as they were.",
            vec![
                ("find", schema::string("The block to replace: exact name or id of the full block.")),
                ("with", schema::string("The block to put in its place, or \"air\" to remove them.")),
                ("from", schema::position("One corner of the box. Leave out both corners for the whole entity.")),
                ("to", schema::position("The opposite corner, included.")),
            ],
            &["find", "with"],
            replace,
        ),
        Tool::on_entity(
            "remove",
            "Removes the blocks inside a box, or only those of one type. The Ship Core cannot be removed.",
            vec![
                ("from", schema::position("One corner of the box.")),
                ("to", schema::position("The opposite corner, included.")),
                ("block", schema::string("Remove only blocks of this type. Default: all blocks.")),
            ],
            &["from", "to"],
            remove,
        ),
        Tool::on_entity(
            "mirror",
            "Copies one half of the entity onto the other across the plane through the Ship Core, turning sloped blocks to match and copying links. Build the left half (x > 0), then mirror it with axis x to get a symmetric ship.",
            vec![
                ("axis", schema::choice("x mirrors left and right, y up and down, z front and back.", &["x", "y", "z"])),
                ("keep", schema::choice("Which half is the original: \"positive\" (default; for x the pilot's left, x > 0) or \"negative\".", &["positive", "negative"])),
                ("from", schema::position("One corner of a box that limits what is copied. Leave out both corners to mirror the whole half.")),
                ("to", schema::position("The opposite corner, included.")),
                ("clear_other_half", schema::boolean("true (default) makes the other half an exact mirror image, removing what has no counterpart. false only adds.")),
            ],
            &["axis"],
            mirror,
        ),
        Tool::on_entity(
            "copy_region",
            "Copies the blocks of a box to another place, turned as they are, with the links among them: repeating a window row, an engine, a turret mount.",
            vec![
                ("from", schema::position("One corner of the box to copy.")),
                ("to", schema::position("The opposite corner, included.")),
                ("offset", schema::position("How far the copy is moved: every block lands at its position plus this.")),
                only_where(),
            ],
            &["from", "to", "offset"],
            copy_region,
        ),
    ]
}

/// The answer after an edit: what was done, what that changed, and how
/// large the entity is now.
fn report(ctx: &Ctx<'_>, entity: &str, what: String, change: &Change) -> Result<Done, String> {
    let blocks = entity_info(ctx, entity)?.blocks;
    let (placed, replaced, removed) = super::tally(change);
    Ok(Done { text: format!("{what} {} The entity now has {blocks} {}.", describe_change(change), plural(blocks as usize, "block")), changed: placed + replaced + removed })
}

fn block_name(ctx: &Ctx<'_>, block: Block) -> String {
    if block.is_air() { "air".to_string() } else { ctx.world.catalog().name(block.ty()) }
}

fn fill(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let (lo, hi, volume) = span(args.need_pos("from")?, args.need_pos("to")?);
    check_volume(volume)?;
    let block = blocks::resolve(ctx.world.catalog(), args, "block", true)?;
    let mode = match args.text("mode") {
        Some("hollow") => FillMode::Hollow,
        Some("frame") => FillMode::Frame,
        _ => FillMode::Solid,
    };
    let how = ["solid", "hollow", "as a frame"][mode as usize];
    if super::inside::only_inside(args) {
        let cells: BTreeSet<Pos> = box_cells(lo, hi, mode).collect();
        let (block, change) = place_cells(ctx, args, &cells, "a fill")?;
        return report(ctx, entity, format!("Filled {}..{} with {} ({how}), only the enclosed empty cells.", show(lo), show(hi), block_name(ctx, block)), &change);
    }
    let when = when_of(ctx, args)?;
    let link = core_link(&rules_for(ctx, &info), block.ty());
    let change = apply(ctx, entity, vec![Op::Fill { min: lo, max: hi, block, mode, when, link }], "a fill")?;
    report(ctx, entity, format!("Filled {}..{} with {} ({how}).", show(lo), show(hi), block_name(ctx, block)), &change)
}

/// A shape's report, saying so if it kept to the inside of the hull.
fn inside_note(args: &Args<'_>, text: String) -> String {
    if super::inside::only_inside(args) { format!("{text} Only the enclosed empty cells were filled.") } else { text }
}

/// The cells of a box a fill of this mode writes.
fn box_cells(lo: Pos, hi: Pos, mode: FillMode) -> impl Iterator<Item = Pos> {
    let faces = move |p: Pos| [p.x == lo.x || p.x == hi.x, p.y == lo.y || p.y == hi.y, p.z == lo.z || p.z == hi.z].into_iter().filter(|&on| on).count();
    (lo.z..=hi.z)
        .flat_map(move |z| (lo.y..=hi.y).flat_map(move |y| (lo.x..=hi.x).map(move |x| Pos::new(x, y, z))))
        .filter(move |&p| match mode {
            FillMode::Solid => true,
            FillMode::Hollow => faces(p) >= 1,
            FillMode::Frame => faces(p) >= 2,
        })
}

/// Writes a set of cells with one block, as one step.
fn place_cells(ctx: &mut Ctx<'_>, args: &Args<'_>, cells: &BTreeSet<Pos>, what: &str) -> Result<(Block, Change), String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    check_volume(cells.len() as u64)?;
    if let Some(far) = cells.iter().find(|p| p.x.abs().max(p.y.abs()).max(p.z.abs()) > yard_core::edit::MAX_COORD) {
        return Err(format!("The shape reaches {}, which is too far from the core; the limit is {} blocks in every direction.", show(*far), yard_core::edit::MAX_COORD));
    }
    let block = blocks::resolve(ctx.world.catalog(), args, "block", true)?;
    let link = core_link(&rules_for(ctx, &info), block.ty());
    if super::inside::only_inside(args) {
        let Some(inside) = super::inside::interior(ctx, entity, None)? else { return Ok((block, Change::default())) };
        let kept: BTreeSet<Pos> = cells.iter().copied().filter(|p| inside.contains(*p)).collect();
        let change = apply(ctx, entity, fills(&kept, block, When::Empty, link), what)?;
        return Ok((block, change));
    }
    let when = when_of(ctx, args)?;
    let change = apply(ctx, entity, fills(cells, block, when, link), what)?;
    Ok((block, change))
}

/// A solid, or its shell if the call asks for a hollow shape.
fn maybe_shell(args: &Args<'_>, solid: Grid, hollow_by_default: bool, open_axis: Option<usize>) -> Grid {
    let hollow = args.get("hollow").and_then(Value::as_bool).unwrap_or(hollow_by_default);
    if hollow { solid.shell(args.int("thickness").unwrap_or(1) as u32, open_axis) } else { solid }
}

fn bounds_of(cells: &BTreeSet<Pos>) -> String {
    let mut all = cells.iter();
    let Some(first) = all.next() else { return "nowhere".to_string() };
    let (lo, hi) = all.fold((*first, *first), |(lo, hi), p| (lo.lo(*p), hi.hi(*p)));
    format!("{}..{}", show(lo), show(hi))
}

fn line(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let (from, to) = (args.need_pos("from")?, args.need_pos("to")?);
    let cells = shapes::line(from, to, args.int("thickness").unwrap_or(1) as i32);
    let (block, change) = place_cells(ctx, args, &cells, "a line")?;
    report(ctx, args.entity(), inside_note(args, format!("Drew a line of {} from {} to {} ({} cells).", block_name(ctx, block), show(from), show(to), cells.len())), &change)
}

fn ellipsoid(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let center = args.need_pos("center")?;
    let radii: Vec<f64> = args.get("radii").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect();
    let [rx, ry, rz] = radii[..] else { return Err("\"radii\" must be three numbers: [rx, ry, rz].".to_string()) };
    let cells = maybe_shell(args, shapes::ellipsoid(center, [rx, ry, rz])?, false, None).cells();
    let (block, change) = place_cells(ctx, args, &cells, "an ellipsoid")?;
    report(ctx, args.entity(), inside_note(args, format!("Built an ellipsoid of {} around {}, spanning {} ({} cells).", block_name(ctx, block), show(center), bounds_of(&cells), cells.len())), &change)
}

fn axis_arg(args: &Args<'_>) -> usize {
    match args.text("axis") {
        Some("x") => 0,
        Some("y") => 1,
        _ => 2,
    }
}

fn cylinder(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let from = args.need_pos("from")?;
    let axis = axis_arg(args);
    let length = args.int("length").unwrap_or(1) as i32;
    if length == 0 {
        return Err("\"length\" must not be 0. Positive runs toward the + end of the axis, negative toward the - end.".to_string());
    }
    let radius = args.num("radius").unwrap_or(0.0);
    let end_radius = args.num("end_radius").unwrap_or(radius);
    let open = args.flag("open_ends").then_some(axis);
    let cells = maybe_shell(args, shapes::cylinder(from, axis, length, radius, end_radius)?, false, open).cells();
    let (block, change) = place_cells(ctx, args, &cells, "a cylinder")?;
    let shape = if end_radius == radius { "cylinder" } else if end_radius == 0.0 || radius == 0.0 { "cone" } else { "tapered cylinder" };
    report(ctx, args.entity(), inside_note(args, format!("Built a {shape} of {} spanning {} ({} cells).", block_name(ctx, block), bounds_of(&cells), cells.len())), &change)
}

fn hull(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let origin = args.pos("origin")?.unwrap_or(Pos::new(0, 0, 0));
    let listed = args.get("sections").and_then(Value::as_array).cloned().unwrap_or_default();
    if listed.len() < 2 {
        return Err("A hull needs at least two sections: one where it starts and one where it ends. Add more between them to shape it.".to_string());
    }
    let mut sections = Vec::new();
    for item in &listed {
        let number = |name: &str, default: f64| item.get(name).and_then(Value::as_f64).unwrap_or(default);
        let section = Section { z: number("z", 0.0) as i32, half_width: number("half_width", 0.0), half_height: number("half_height", 0.0), roundness: number("roundness", 1.0), y_offset: number("y_offset", 0.0) };
        if section.half_width < 0.0 || section.half_height < 0.0 || section.half_width > 2000.0 || section.half_height > 2000.0 || section.z.abs() > yard_core::edit::MAX_COORD {
            return Err(format!("The section at z {} has measures out of range: half_width and half_height must be 0 or more and the hull must stay within {} blocks of the core.", section.z, yard_core::edit::MAX_COORD));
        }
        sections.push(section);
    }
    let cells = maybe_shell(args, shapes::hull(origin, &sections)?, true, None).cells();
    let (block, change) = place_cells(ctx, args, &cells, "a hull")?;
    report(ctx, args.entity(), inside_note(args, format!("Built a hull of {} through {} sections, spanning {} ({} cells).", block_name(ctx, block), sections.len(), bounds_of(&cells), cells.len())), &change)
}

fn place_blocks(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let rules = rules_for(ctx, &info);
    let when = when_of(ctx, args)?;
    let listed = args.get("blocks").and_then(Value::as_array).cloned().unwrap_or_default();
    if listed.is_empty() {
        return Err("\"blocks\" is empty. List at least one block: {\"pos\": [x, y, z], \"block\": \"name\"}.".to_string());
    }
    let mut ops = Vec::new();
    for (i, item) in listed.iter().enumerate() {
        // What an item leaves out it takes from the call itself.
        let mut merged = item.clone();
        for (name, _) in blocks::properties() {
            if let (None, Some(shared)) = (item.get(name), args.get(name)) {
                merged[name] = shared.clone();
            }
        }
        let at = pos_of(&item["pos"])?;
        let block = blocks::resolve(ctx.world.catalog(), &Args::of(&merged), "block", true).map_err(|e| format!("blocks[{i}] at {}: {e}", show(at)))?;
        // One fill per block: a fill can link what it places to the core.
        ops.push(Op::Fill { min: at, max: at, block, mode: FillMode::Solid, when, link: core_link(&rules, block.ty()) });
    }
    let change = apply(ctx, entity, ops, "placing blocks")?;
    let skipped = listed.len().saturating_sub(change.cells.len());
    let note = if skipped > 0 { format!(" {skipped} of them changed nothing (already there, excluded by only_where, or the Ship Core's cell).") } else { String::new() };
    report(ctx, entity, format!("Placed {} listed {}.{note}", listed.len(), plural(listed.len(), "block")), &change)
}

/// The entity's blocks inside an optional box.
fn blocks_in(ctx: &Ctx<'_>, entity: &str, region: Option<(Pos, Pos)>) -> Result<Vec<(Pos, Block)>, String> {
    let mut found = Vec::new();
    ctx.world.with_entity(entity, &mut |e| {
        e.for_each(|p, b| {
            if region.is_none_or(|(lo, hi)| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && p.z >= lo.z && p.z <= hi.z) {
                found.push((p, b));
            }
        })
    })?;
    Ok(found)
}

/// The box a call names with `from` and `to`, if it names one.
fn optional_box(args: &Args<'_>) -> Result<Option<(Pos, Pos)>, String> {
    match (args.pos("from")?, args.pos("to")?) {
        (Some(a), Some(b)) => Ok(Some((a.lo(b), a.hi(b)))),
        (None, None) => Ok(None),
        _ => Err("Give both \"from\" and \"to\", or neither for the whole entity.".to_string()),
    }
}

fn replace(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let catalog = ctx.world.catalog();
    let find = blocks::lookup(catalog, args.text("find").unwrap_or(""))?;
    let with_text = args.text("with").unwrap_or("");
    let with = if blocks::is_air_word(with_text) { None } else { Some(blocks::lookup(catalog, with_text)?) };
    // Old type -> new type, shape by shape; 0 removes.
    let mut swaps: BTreeMap<u16, u16> = BTreeMap::from([(find.id, with.map_or(0, |d| d.id))]);
    let mut unmatched = Vec::new();
    if find.base == 0 {
        for shape in blocks::SHAPES {
            let Ok(old) = blocks::variant(catalog, find, shape) else { continue };
            match with.map(|w| blocks::variant(catalog, w, shape)) {
                None => drop(swaps.insert(old.id, 0)),
                Some(Ok(new)) => drop(swaps.entry(old.id).or_insert(new.id)),
                Some(Err(_)) => unmatched.push((old.id, shape)),
            }
        }
    }
    let region = optional_box(args)?;
    let found = blocks_in(ctx, entity, region)?;
    let cells: Vec<(Pos, Block)> = found
        .iter()
        .filter_map(|(p, old)| {
            let new_type = *swaps.get(&old.ty())?;
            let mut new = *old;
            new.set_type(new_type);
            Some((*p, if new_type == 0 { Block::AIR } else { new }))
        })
        .collect();
    check_volume(cells.len() as u64)?;
    let left: usize = found.iter().filter(|(_, b)| unmatched.iter().any(|(id, _)| *id == b.ty())).count();
    let change = apply(ctx, entity, vec![Op::Set { cells, when: When::Always }], "a replacement")?;
    let target = with.map_or("air", |d| d.name.as_str());
    let scope = region.map_or("in the whole entity".to_string(), |(lo, hi)| format!("in {}..{}", show(lo), show(hi)));
    let mut what = format!("Replaced {} with {target} {scope}.", find.name);
    if left > 0 {
        let shapes: Vec<&str> = unmatched.iter().filter(|(id, _)| found.iter().any(|(_, b)| b.ty() == *id)).map(|(_, s)| *s).collect();
        what.push_str(&format!(" {left} shaped {} kept their type, because {target} has no {} variant.", plural(left, "block"), shapes.join(" or ")));
    }
    if change.is_empty() && left == 0 {
        what.push_str(&format!(" There was no {} there.", find.name));
    }
    report(ctx, entity, what, &change)
}

fn remove(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let (lo, hi, _) = span(args.need_pos("from")?, args.need_pos("to")?);
    let when = match args.text("block") {
        Some(name) => When::Type(blocks::lookup(ctx.world.catalog(), name)?.id),
        None => When::Always,
    };
    // Only where the entity has blocks: a generous box costs nothing.
    let Some((min, max)) = entity_info(ctx, entity)?.bounds else { return Ok(Done::said("The entity is empty; there is nothing to remove.")) };
    let (clipped_lo, clipped_hi) = (lo.hi(min), hi.lo(max));
    if clipped_lo.x > clipped_hi.x || clipped_lo.y > clipped_hi.y || clipped_lo.z > clipped_hi.z {
        return Ok(Done::said(format!("There are no blocks in {}..{}; nothing was removed.", show(lo), show(hi))));
    }
    check_volume(span(clipped_lo, clipped_hi).2)?;
    let change = apply(ctx, entity, vec![Op::Fill { min: clipped_lo, max: clipped_hi, block: Block::AIR, mode: FillMode::Solid, when, link: None }], "a removal")?;
    let mut what = format!("Cleared {}..{}.", show(lo), show(hi));
    if lo.x <= 0 && lo.y <= 0 && lo.z <= 0 && hi.x >= 0 && hi.y >= 0 && hi.z >= 0 && when == When::Always {
        what.push_str(" The Ship Core at (0, 0, 0) stays; it cannot be removed.");
    }
    report(ctx, entity, what, &change)
}

fn mirror(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let catalog = ctx.world.catalog();
    let axis = axis_arg(args);
    let sign = if args.text("keep") == Some("negative") { -1 } else { 1 };
    let clear = args.get("clear_other_half").and_then(Value::as_bool).unwrap_or(true);
    let region = optional_box(args)?;
    let coord = |p: Pos| [p.x, p.y, p.z][axis];
    let image = |p: Pos| match axis {
        0 => Pos::new(-p.x, p.y, p.z),
        1 => Pos::new(p.x, -p.y, p.z),
        _ => Pos::new(p.x, p.y, -p.z),
    };
    let mut flip = [false; 3];
    flip[axis] = true;
    let turned = |block: Block| {
        let Some(def) = catalog.get(block.ty()) else { return block };
        let mut out = block;
        out.set_orientation(ShapeSet::mirrored(def.shape, block.orientation(), flip, blocks::faces_matter(def)));
        out
    };
    // The original half, and what stands on the other side where its image goes.
    let originals: BTreeMap<Pos, Block> = blocks_in(ctx, entity, region)?.into_iter().filter(|(p, _)| coord(*p) * sign > 0).collect();
    let image_region = region.map(|(lo, hi)| (image(lo).lo(image(hi)), image(lo).hi(image(hi))));
    let mut cells: Vec<(Pos, Block)> = originals.iter().map(|(p, b)| (image(*p), turned(*b))).collect();
    if clear {
        let others = blocks_in(ctx, entity, image_region)?;
        cells.extend(others.into_iter().filter(|(p, _)| coord(*p) * sign < 0 && !originals.contains_key(&image(*p))).map(|(p, _)| (p, Block::AIR)));
    }
    check_volume(cells.len() as u64 * 2)?;
    // Links of the original half, mirrored: a controller on the plane keeps
    // its place and takes the images as well.
    let mut links: BTreeMap<Pos, Vec<Pos>> = BTreeMap::new();
    ctx.world.with_entity(entity, &mut |e| {
        for (controller, groups) in e.links() {
            if coord(*controller) * sign < 0 {
                continue;
            }
            for target in groups.values().flatten().filter(|t| originals.contains_key(t)) {
                links.entry(image(*controller)).or_default().push(image(*target));
            }
        }
    })?;
    let mut ops = vec![Op::Set { cells, when: When::Always }];
    ops.extend(links.into_iter().map(|(controller, targets)| Op::Link { controller, targets }));
    let change = apply(ctx, entity, ops, "a mirroring")?;
    let names = [["right (x < 0)", "left (x > 0)"], ["lower (y < 0)", "upper (y > 0)"], ["rear (z < 0)", "front (z > 0)"]][axis];
    let (kept, made) = if sign > 0 { (names[1], names[0]) } else { (names[0], names[1]) };
    report(ctx, entity, format!("Mirrored the {kept} half onto the {made} half: {} {} copied.", originals.len(), plural(originals.len(), "block")), &change)
}

fn copy_region(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let rules = rules_for(ctx, &info);
    let (lo, hi, _) = span(args.need_pos("from")?, args.need_pos("to")?);
    let offset = args.need_pos("offset")?;
    if offset == Pos::new(0, 0, 0) {
        return Err("\"offset\" is [0, 0, 0], which would copy the blocks onto themselves. Give the distance to move the copy by.".to_string());
    }
    let moved = |p: Pos| p.offset(offset.x, offset.y, offset.z);
    pos_of(&json!([moved(lo).x, moved(lo).y, moved(lo).z]))?;
    pos_of(&json!([moved(hi).x, moved(hi).y, moved(hi).z]))?;
    let when = when_of(ctx, args)?;
    let originals: BTreeMap<Pos, Block> = blocks_in(ctx, entity, Some((lo, hi)))?.into_iter().filter(|(_, b)| b.ty() != yard_core::edit::CORE_TYPE).collect();
    if originals.is_empty() {
        return Ok(Done::said(format!("There are no blocks in {}..{} to copy.", show(lo), show(hi))));
    }
    check_volume(originals.len() as u64 * 2)?;
    let cells: Vec<(Pos, Block)> = originals.iter().map(|(p, b)| (moved(*p), *b)).collect();
    let mut links: BTreeMap<Pos, Vec<Pos>> = BTreeMap::new();
    ctx.world.with_entity(entity, &mut |e| {
        for (controller, groups) in e.links().iter().filter(|(c, _)| originals.contains_key(c)) {
            let targets: Vec<Pos> = groups.values().flatten().filter(|t| originals.contains_key(t)).map(|t| moved(*t)).collect();
            if !targets.is_empty() {
                links.insert(moved(*controller), targets);
            }
        }
    })?;
    let mut ops = vec![Op::Set { cells, when }];
    ops.extend(links.into_iter().map(|(controller, targets)| Op::Link { controller, targets }));
    // The core takes copied computers as it takes placed ones.
    let computers: Vec<Pos> = originals.iter().filter(|(_, b)| core_link(&rules, b.ty()).is_some()).map(|(p, _)| moved(*p)).collect();
    if !computers.is_empty() {
        ops.push(Op::Link { controller: Pos::new(0, 0, 0), targets: computers });
    }
    let change = apply(ctx, entity, ops, "a copy")?;
    report(ctx, entity, format!("Copied {} {} from {}..{} to {}..{}.", originals.len(), plural(originals.len(), "block"), show(lo), show(hi), show(moved(lo)), show(moved(hi))), &change)
}

const _: () = assert!(MAX_CELLS >= 1_000_000);

#[cfg(test)]
mod tests {
    use super::super::orient::{self, Dir};
    use super::super::testing::Bench;
    use serde_json::json;
    use yard_core::catalog::Shape;
    use yard_core::Pos;

    const GREY: u16 = 598;
    const RED: u16 = 620;

    #[test]
    fn fill_makes_solid_hollow_and_framed_boxes() {
        let mut bench = Bench::new();
        let said = bench.ok("fill", json!({"from": [-1, 1, 0], "to": [1, 3, 3], "block": "Grey Basic Armor"}));
        assert_eq!(said, "Filled (-1, 1, 0)..(1, 3, 3) with Grey Basic Armor (solid). 36 blocks placed. The entity now has 37 blocks.");
        assert_eq!(bench.count(GREY), 36);
        bench.ok("fill", json!({"from": [10, 0, 0], "to": [14, 4, 4], "block": "598", "mode": "hollow"}));
        assert_eq!(bench.count(GREY), 36 + 125 - 27);
        assert_eq!((bench.ty(12, 2, 2), bench.ty(10, 2, 2)), (0, GREY));
        bench.ok("fill", json!({"from": [20, 0, 0], "to": [24, 4, 4], "block": "Red Basic Armor", "mode": "frame"}));
        assert_eq!(bench.count(RED), 12 * 3 + 8);
    }

    #[test]
    fn fill_respects_only_where_and_never_touches_the_core() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [0, 0, 0], "to": [0, 0, 3], "block": "Grey Basic Armor"}));
        assert_eq!((bench.ty(0, 0, 0), bench.count(GREY)), (1, 3), "the core stays");
        bench.ok("fill", json!({"from": [0, 0, 0], "to": [0, 0, 5], "block": "Red Basic Armor", "only_where": "empty"}));
        assert_eq!((bench.count(GREY), bench.count(RED)), (3, 2));
        bench.ok("fill", json!({"from": [0, 0, 0], "to": [0, 0, 9], "block": "Glass", "only_where": "Red Basic Armor"}));
        assert_eq!((bench.count(RED), bench.count(63), bench.blocks()), (0, 2, 6));
        bench.ok("fill", json!({"from": [0, 0, 0], "to": [0, 0, 9], "block": "Red Basic Armor", "only_where": "occupied"}));
        assert_eq!((bench.count(RED), bench.blocks()), (5, 6));
        let nothing = bench.ok("fill", json!({"from": [0, 0, 1], "to": [0, 0, 5], "block": "Red Basic Armor"}));
        assert!(nothing.contains("Nothing changed."), "{nothing}");
        bench.ok("fill", json!({"from": [0, 0, 0], "to": [0, 0, 9], "block": "air"}));
        assert_eq!(bench.blocks(), 1);
    }

    #[test]
    fn fill_refuses_bad_blocks_and_boxes_that_are_too_large() {
        let mut bench = Bench::new();
        assert!(bench.err("fill", json!({"from": [0, 0, 1], "to": [0, 0, 2], "block": "Unobtainium"})).contains("no block called"));
        assert!(bench.err("fill", json!({"from": [-100, -100, -100], "to": [100, 100, 100], "block": "Glass"})).contains("at most 2000000"));
        assert!(bench.err("fill", json!({"from": [0, 0, 1], "to": [0, 0, 2], "block": "Glass", "mode": "dotted"})).contains("must be one of"));
        assert!(bench.err("fill", json!({"from": [0, 0, 1], "to": [0, 0, 2], "block": "Glass", "only_where": "Unobtainium"})).contains("no block called"));
        assert_eq!(bench.blocks(), 1);
    }

    #[test]
    fn a_computer_placed_by_any_tool_is_linked_to_the_core_as_in_the_game() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [2, 0, 0], "to": [2, 0, 0], "block": "Cannon Computer"}));
        bench.ok("place_blocks", json!({"blocks": [{"pos": [3, 0, 0], "block": "Missile Computer"}]}));
        let core = Pos::new(0, 0, 0);
        assert!(bench.linked(core, Pos::new(2, 0, 0)) && bench.linked(core, Pos::new(3, 0, 0)));
    }

    #[test]
    fn line_joins_two_points_at_any_thickness() {
        let mut bench = Bench::new();
        let said = bench.ok("line", json!({"from": [3, 0, 0], "to": [9, 3, 0], "block": "Grey Basic Armor"}));
        assert!(said.starts_with("Drew a line of Grey Basic Armor from (3, 0, 0) to (9, 3, 0) (7 cells). 7 blocks placed."), "{said}");
        assert_eq!((bench.ty(3, 0, 0), bench.ty(9, 3, 0), bench.ty(6, 2, 0)), (GREY, GREY, GREY));
        bench.ok("line", json!({"from": [0, 10, 0], "to": [0, 10, 4], "block": "Red Basic Armor", "thickness": 3}));
        assert_eq!(bench.count(RED), 9 * 7);
        assert!(bench.err("line", json!({"from": [0, 10, 0], "to": [0, 10, 4], "block": "Glass", "thickness": 40})).contains("at most 9"));
    }

    #[test]
    fn ellipsoid_builds_solid_bodies_and_closed_shells() {
        let mut bench = Bench::new();
        bench.ok("ellipsoid", json!({"center": [0, 20, 0], "radii": [4, 4, 4], "block": "Grey Basic Armor"}));
        let solid = bench.count(GREY);
        assert_eq!(solid, 389, "the cells within 4.5 blocks of the centre");
        assert_eq!((bench.ty(0, 20, 0), bench.ty(4, 20, 0), bench.ty(5, 20, 0), bench.ty(4, 24, 4)), (GREY, GREY, 0, 0));
        let said = bench.ok("ellipsoid", json!({"center": [0, -20, 0], "radii": [6, 3, 9], "block": "Red Basic Armor", "hollow": true}));
        assert!(said.contains("spanning (-6, -23, -9)..(6, -17, 9)"), "{said}");
        assert_eq!((bench.ty(0, -20, 0), bench.ty(6, -20, 0), bench.ty(0, -17, 0), bench.ty(0, -20, -9)), (0, RED, RED, RED));
        assert!(bench.err("ellipsoid", json!({"center": [0, 0, 0], "radii": [4, 4], "block": "Glass"})).contains("exactly 3 values"));
        assert!(bench.err("ellipsoid", json!({"center": [0, 0, 0], "radii": [150, 150, 150], "block": "Glass"})).contains("too large"));
    }

    #[test]
    fn cylinder_builds_tubes_and_cones_along_any_axis() {
        let mut bench = Bench::new();
        let said = bench.ok("cylinder", json!({"from": [10, 0, -5], "axis": "z", "length": -6, "radius": 2, "block": "Grey Basic Armor", "hollow": true, "open_ends": true}));
        assert!(said.starts_with("Built a cylinder of Grey Basic Armor spanning (8, -2, -10)..(12, 2, -5)"), "{said}");
        assert_eq!((bench.ty(10, 0, -5), bench.ty(10, 0, -10), bench.ty(12, 0, -7), bench.ty(10, 0, -11)), (0, 0, GREY, 0), "an open pipe running backward");
        let cone = bench.ok("cylinder", json!({"from": [-10, 0, 0], "axis": "y", "length": 5, "radius": 2, "end_radius": 0, "block": "Red Basic Armor"}));
        assert!(cone.starts_with("Built a cone"), "{cone}");
        assert_eq!((bench.ty(-8, 0, 0), bench.ty(-10, 4, 0), bench.ty(-9, 4, 0), bench.ty(-10, 5, 0)), (RED, RED, 0, 0));
        bench.ok("cylinder", json!({"from": [0, 30, 0], "axis": "x", "length": 3, "radius": 1, "block": "Glass"}));
        assert_eq!(bench.count(63), 27);
        assert!(bench.err("cylinder", json!({"from": [0, 0, 0], "axis": "z", "length": 0, "radius": 1, "block": "Glass"})).contains("must not be 0"));
        assert!(bench.err("cylinder", json!({"from": [0, 0, 0], "axis": "w", "length": 3, "radius": 1, "block": "Glass"})).contains("must be one of"));
    }

    #[test]
    fn hull_lofts_a_closed_symmetric_shell_through_its_sections() {
        let mut bench = Bench::new();
        let sections = json!([
            {"z": -12, "half_width": 2, "half_height": 1.5, "roundness": 0.5},
            {"z": -4, "half_width": 5, "half_height": 3},
            {"z": 8, "half_width": 3, "half_height": 2},
            {"z": 17, "half_width": 0, "half_height": 0},
        ]);
        let said = bench.ok("hull", json!({"sections": sections, "block": "Red Basic Armor"}));
        assert!(said.starts_with("Built a hull of Red Basic Armor through 4 sections, spanning (-5, -3, -12)..(5, 3, 17)"), "{said}");
        // Hollow: the skin is there, the inside is not; the core keeps its place.
        assert_eq!((bench.ty(0, 0, 0), bench.ty(5, 0, -4), bench.ty(0, 3, -4), bench.ty(2, 0, -4), bench.ty(0, 1, 2)), (1, RED, RED, 0, 0));
        // Closed at the stern and pointed at the nose.
        assert_eq!((bench.ty(0, 0, -12), bench.ty(1, 1, -12), bench.ty(0, 0, 17), bench.ty(1, 0, 17), bench.ty(0, 0, 18)), (RED, RED, RED, 0, 0));
        let cells = bench.world.with_blueprint(|b| {
            let mut cells = Vec::new();
            b.root.entity.for_each(|p, _| cells.push(p));
            cells
        });
        assert!(cells.iter().all(|p| bench.ty(-p.x, p.y, p.z) != 0), "left and right halves match");
        let shell = bench.count(RED);
        bench.ok("hull", json!({"sections": sections, "block": "Red Basic Armor", "hollow": false}));
        assert!(bench.count(RED) > shell && bench.ty(2, 0, -4) == RED, "the solid body is filled");
    }

    #[test]
    fn hull_checks_its_sections() {
        let mut bench = Bench::new();
        assert!(bench.err("hull", json!({"sections": [{"z": 0, "half_width": 2, "half_height": 2}], "block": "Glass"})).contains("at least two sections"));
        assert!(bench.err("hull", json!({"sections": [{"z": 0, "half_width": 2}, {"z": 5, "half_width": 2, "half_height": 2}], "block": "Glass"})).contains("Missing argument \"half_height\" in \"sections[0]\""));
        assert!(bench.err("hull", json!({"sections": [{"z": 0, "half_width": 2, "half_height": 2}, {"z": 0, "half_width": 1, "half_height": 1}], "block": "Glass"})).contains("same z"));
        assert!(bench.err("hull", json!({"sections": [{"z": 0, "half_width": -2, "half_height": 2}, {"z": 9, "half_width": 1, "half_height": 1}], "block": "Glass"})).contains("out of range"));
        assert!(bench.err("hull", json!({"sections": "a frigate", "block": "Glass"})).contains("must be a list"));
        assert_eq!(bench.blocks(), 1);
    }

    #[test]
    fn place_blocks_places_each_block_with_its_own_type_and_turn() {
        let mut bench = Bench::new();
        let said = bench.ok(
            "place_blocks",
            json!({"block": "Grey Basic Armor", "shape": "wedge", "blocks": [
                {"pos": [1, 0, 5], "slope": ["up", "front"]},
                {"pos": [-1, 0, 5], "slope": ["up", "back"]},
                {"pos": [0, 1, 5], "block": "Glass", "shape": "cube"},
                {"pos": [0, 0, 0], "block": "Glass", "shape": "cube"},
            ]}),
        );
        assert!(said.starts_with("Placed 4 listed blocks. 1 of them changed nothing"), "{said}");
        let (front, back) = (bench.world.block("", Pos::new(1, 0, 5)), bench.world.block("", Pos::new(-1, 0, 5)));
        assert_eq!((front.ty(), back.ty(), bench.ty(0, 1, 5), bench.ty(0, 0, 0)), (599, 599, 63, 1));
        assert_eq!(orient::slope_of(Shape::Wedge, front.orientation()).unwrap(), [Dir::Up, Dir::Front]);
        assert_eq!(orient::slope_of(Shape::Wedge, back.orientation()).unwrap(), [Dir::Up, Dir::Back]);
        let bad = bench.err("place_blocks", json!({"blocks": [{"pos": [5, 5, 5], "block": "Glass"}, {"pos": [6, 5, 5], "block": "Glass", "slope": ["up", "back"]}]}));
        assert!(bad.contains("blocks[1] at (6, 5, 5): Glass has no slanted face"), "{bad}");
        assert_eq!(bench.ty(5, 5, 5), 0, "a call with a bad block places none of its blocks");
        assert!(bench.err("place_blocks", json!({"blocks": []})).contains("is empty"));
        assert!(bench.err("place_blocks", json!({"blocks": [{"block": "Glass"}]})).contains("Missing argument \"pos\" in \"blocks[0]\""));
        let many: Vec<_> = (0..201).map(|i| json!({"pos": [i, 9, 9], "block": "Glass"})).collect();
        assert!(bench.err("place_blocks", json!({"blocks": many})).contains("at most 200"));
    }

    #[test]
    fn replace_recolours_a_hull_and_keeps_every_shape_and_turn() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-2, 1, 0], "to": [2, 1, 4], "block": "Grey Basic Armor"}));
        bench.ok("place_blocks", json!({"blocks": [{"pos": [0, 2, 0], "block": "Grey Basic Armor", "shape": "wedge", "slope": ["up", "back"]}, {"pos": [0, 2, 1], "block": "Grey Basic Armor", "shape": "slab_1/2", "facing": "up"}]}));
        let wedge_before = bench.world.block("", Pos::new(0, 2, 0));
        let said = bench.ok("replace", json!({"find": "Grey Basic Armor", "with": "Red Basic Armor"}));
        assert!(said.starts_with("Replaced Grey Basic Armor with Red Basic Armor in the whole entity. 27 blocks changed."), "{said}");
        let wedge = bench.world.block("", Pos::new(0, 2, 0));
        assert_eq!((wedge.ty(), wedge.orientation()), (621, wedge_before.orientation()), "the wedge is red now and turned as before");
        assert_eq!(bench.world.block("", Pos::new(0, 2, 1)).ty(), 626, "a half slab stays a half slab");
        assert_eq!((bench.count(GREY), bench.count(RED)), (0, 25));
        // In a box only.
        bench.ok("replace", json!({"find": "Red Basic Armor", "with": "Glass", "from": [-2, 1, 0], "to": [-2, 1, 4]}));
        assert_eq!((bench.count(63), bench.count(RED)), (5, 20));
        // Glass has no wedge: shaped blocks keep their type and the model is told.
        let partly = bench.ok("replace", json!({"find": "Red Basic Armor", "with": "Glass"}));
        assert!(partly.contains("2 shaped blocks kept their type, because Glass has no wedge or slab_1/2 variant."), "{partly}");
        assert_eq!((bench.count(RED), bench.count(621)), (0, 1));
        let none = bench.ok("replace", json!({"find": "Cannon Barrel", "with": "air"}));
        assert!(none.contains("There was no Cannon Barrel there."), "{none}");
        assert!(bench.err("replace", json!({"find": "Ship Core", "with": "Glass"})).contains("exactly one"));
        assert!(bench.err("replace", json!({"find": "Glass", "with": "Glass", "from": [0, 0, 0]})).contains("both \"from\" and \"to\""));
    }

    #[test]
    fn remove_clears_a_box_or_one_type_and_keeps_the_core() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-2, -2, -2], "to": [2, 2, 2], "block": "Grey Basic Armor"}));
        bench.ok("fill", json!({"from": [-2, 2, -2], "to": [2, 2, 2], "block": "Glass"}));
        bench.ok("remove", json!({"from": [-9, -9, -9], "to": [9, 9, 9], "block": "Glass"}));
        assert_eq!((bench.count(63), bench.count(GREY)), (0, 99));
        let said = bench.ok("remove", json!({"from": [-4000, -4000, -4000], "to": [4000, 4000, 4000]}));
        assert!(said.contains("The Ship Core at (0, 0, 0) stays") && said.contains("99 blocks removed. The entity now has 1 block."), "{said}");
        assert!(bench.ok("remove", json!({"from": [50, 50, 50], "to": [60, 60, 60]})).contains("nothing was removed"));
        assert!(bench.err("remove", json!({"from": [0, 0, 0]})).contains("Missing argument \"to\""));
    }

    #[test]
    fn mirror_copies_one_half_onto_the_other_with_turns_and_links() {
        let mut bench = Bench::new();
        // The pilot's left half: a wedge sloping outward, and a cannon.
        bench.ok("place_blocks", json!({"blocks": [{"pos": [3, 0, 0], "block": "Grey Basic Armor", "shape": "wedge", "slope": ["up", "left"]}]}));
        bench.ok("place_system", json!({"computer": "Cannon Computer", "at": [2, 0, 0], "module": "Cannon Barrel", "from": [2, 0, 1], "to": [2, 0, 3]}));
        // Something on the right that has no counterpart.
        bench.ok("fill", json!({"from": [-5, 4, 0], "to": [-5, 4, 0], "block": "Glass"}));
        let said = bench.ok("mirror", json!({"axis": "x"}));
        assert!(said.starts_with("Mirrored the left (x > 0) half onto the right (x < 0) half: 5 blocks copied."), "{said}");
        let image = bench.world.block("", Pos::new(-3, 0, 0));
        assert_eq!(image.ty(), 599);
        assert_eq!(orient::slope_of(Shape::Wedge, image.orientation()).unwrap(), [Dir::Right, Dir::Up], "the mirrored wedge slopes outward on its own side");
        assert_eq!((bench.ty(-2, 0, 0), bench.ty(-2, 0, 3), bench.ty(-5, 4, 0)), (6, 16, 0));
        let (core, computer) = (Pos::new(0, 0, 0), Pos::new(-2, 0, 0));
        assert!(bench.linked(computer, Pos::new(-2, 0, 1)) && bench.linked(computer, Pos::new(-2, 0, 3)) && bench.linked(core, computer));
        assert!(bench.linked(Pos::new(2, 0, 0), Pos::new(2, 0, 1)), "the original keeps its links");
        assert_eq!(bench.blocks(), 11);
    }

    #[test]
    fn mirror_can_add_without_clearing_and_work_on_a_box_and_other_axes() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [0, 1, 1], "to": [0, 2, 3], "block": "Grey Basic Armor"}));
        bench.ok("fill", json!({"from": [0, -5, 9], "to": [0, -5, 9], "block": "Glass"}));
        bench.ok("mirror", json!({"axis": "y", "clear_other_half": false}));
        assert_eq!((bench.ty(0, -1, 1), bench.ty(0, -2, 3), bench.ty(0, -5, 9)), (GREY, GREY, 63), "what was below stays");
        bench.ok("mirror", json!({"axis": "z", "keep": "positive", "from": [0, 1, 1], "to": [0, 1, 9]}));
        assert_eq!((bench.ty(0, 1, -1), bench.ty(0, 1, -3), bench.ty(0, 2, -1)), (GREY, GREY, 0), "only the box was mirrored");
        bench.ok("mirror", json!({"axis": "z", "keep": "negative"}));
        assert_eq!((bench.ty(0, 2, 1), bench.ty(0, 1, 1), bench.ty(0, -5, 9)), (0, GREY, 0), "the front is now the image of the rear");
        assert!(bench.err("mirror", json!({"axis": "diagonal"})).contains("must be one of"));
    }

    #[test]
    fn copy_region_moves_a_copy_with_its_links() {
        let mut bench = Bench::new();
        bench.ok("place_system", json!({"computer": "Cannon Computer", "at": [2, 0, 0], "module": "Cannon Barrel", "from": [2, 0, 1], "to": [2, 0, 3]}));
        let said = bench.ok("copy_region", json!({"from": [2, 0, 0], "to": [2, 0, 3], "offset": [0, 5, 0]}));
        assert!(said.starts_with("Copied 4 blocks from (2, 0, 0)..(2, 0, 3) to (2, 5, 0)..(2, 5, 3). 4 blocks placed, 4 links made."), "{said}");
        let copy = Pos::new(2, 5, 0);
        assert!(bench.linked(copy, Pos::new(2, 5, 2)) && bench.linked(Pos::new(0, 0, 0), copy));
        assert!(!bench.linked(Pos::new(2, 0, 0), Pos::new(2, 5, 2)));
        assert!(bench.err("copy_region", json!({"from": [2, 0, 0], "to": [2, 0, 3], "offset": [0, 0, 0]})).contains("onto themselves"));
        assert!(bench.err("copy_region", json!({"from": [2, 0, 0], "to": [2, 5, 3], "offset": [0, 3999, 0]})).contains("too far"));
        assert!(bench.ok("copy_region", json!({"from": [40, 0, 0], "to": [41, 0, 3], "offset": [0, 1, 0]})).contains("no blocks"));
    }
}
