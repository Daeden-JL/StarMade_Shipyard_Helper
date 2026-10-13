//! Tools that look: what the design holds, what a region looks like, what a
//! block is and what it is linked to, which blocks exist, and the guides.

use super::{blocks, entity_info, plural, show, span, Args, Ctx, Done, Tool};
use crate::guides;
use crate::schema;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use yard_core::catalog::{Catalog, Shape};
use yard_core::entity::Links;
use yard_core::interior::Interior;
use yard_core::{Entity, Pos};

/// Block types listed by design_info and count_blocks.
const MAX_TYPES: usize = 30;
/// Controllers listed by design_info and list_links.
const MAX_CONTROLLERS: usize = 30;
/// Boxes listed per group of linked blocks.
const MAX_BOXES: usize = 6;
/// Characters across and down one map of scan_region.
const MAX_MAP: i32 = 64;
/// Characters of maps in one scan_region answer.
const MAX_SCAN_CHARS: usize = 6000;
/// Groups listed by find_groups.
const MAX_GROUPS: usize = 20;
const SYMBOLS: &str = "#abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::on_entity(
            "design_info",
            "Summary of the design: its entities with size, bounds and block counts, the blocks of one entity by type, and its links. Call this first, and again after large changes to check the result.",
            vec![],
            &[],
            design_info,
        ),
        Tool::plain(
            "find_blocks",
            "Search the game's block list by keywords, e.g. \"grey armor\", \"cannon\", \"light\", \"rail\". Returns exact names and ids, which shapes each block comes in (wedge, corner, slabs...), whether it has a front to turn, and what it controls. Use it before using a block for the first time; never guess block names.",
            vec![("query", schema::string("Words that must all appear in the block's name, key or category.")), ("limit", schema::integer_in("Most blocks to list. Default 15.", 1, 40))],
            &["query"],
            find_blocks,
        ),
        Tool::on_entity(
            "count_blocks",
            "Counts the blocks inside a box by type, or in the whole entity when no box is given. Cheap; use it to verify what a build step did.",
            vec![("from", schema::position("One corner of the box.")), ("to", schema::position("The opposite corner, included."))],
            &[],
            count_blocks,
        ),
        Tool::on_entity(
            "scan_region",
            "Draws part of the design as character maps, one per layer along an axis, with a legend. Empty cells enclosed by the hull are drawn ':', empty cells outside it '.'. Use it to see shapes, find holes, see the room inside a hull and check symmetry. Large regions are drawn smaller and with fewer layers; scan a smaller box for detail.",
            vec![
                ("from", schema::position("One corner of the box. Default: the entity's whole bounds.")),
                ("to", schema::position("The opposite corner, included.")),
                ("axis", schema::choice("The axis the layers are stacked along: \"y\" gives deck plans seen from above (default), \"z\" cross-sections seen from behind, \"x\" side profiles.", &["x", "y", "z"])),
                ("max_layers", schema::integer_in("Most layers to draw, spread evenly over the box. Default 6.", 1, 24)),
            ],
            &[],
            scan_region,
        ),
        Tool::on_entity(
            "find_groups",
            "Finds where a block is used: its connected groups (blocks touching face to face, its wedges, corners and slabs included), each with its size, bounds and centre, largest first. Use it to locate marks the player made or names, such as outlines in a colour, and to find parts like engines or turrets. For outlines drawn on a side of the ship, find_outlines is easier.",
            vec![
                ("block", schema::string("The block to look for: exact name or id of the full block.")),
                ("from", schema::position("One corner of a box to search. Leave out both corners for the whole entity.")),
                ("to", schema::position("The opposite corner, included.")),
                ("min_size", schema::integer_in("Leave out groups smaller than this many blocks. Default 1.", 1, 1_000_000)),
            ],
            &["block"],
            find_groups,
        ),
        Tool::on_entity(
            "get_block",
            "What is at one position: the block, how it is turned, what controls it, what it controls, and its six neighbours. An empty position is said to be inside or outside the hull.",
            vec![("pos", schema::position("The position to look at."))],
            &["pos"],
            get_block,
        ),
        Tool::on_entity(
            "list_links",
            "Lists links. With a position: what the block there controls and what controls it. Without: every controller in the entity and what it controls.",
            vec![("pos", schema::position("A controller or a controlled block. Leave out to list all controllers."))],
            &[],
            list_links,
        ),
        Tool::plain(
            "read_guide",
            "Short notes on how the game works. Read the matching guide before building a kind of system for the first time in a conversation.",
            vec![("topic", schema::choice("Which guide.", &guides::TOPICS))],
            &["topic"],
            read_guide,
        ),
    ]
}

fn design_info(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let chosen = entity_info(ctx, args.entity())?;
    let catalog = ctx.world.catalog();
    let entities = ctx.world.entities();
    let mut out = format!("The design has {} {}.\n", entities.len(), if entities.len() == 1 { "entity" } else { "entities" });
    for e in &entities {
        let role = match &e.docked_to {
            None => "main".to_string(),
            Some(parent) => format!("docked to \"{parent}\""),
        };
        let kind = if e.kind == crate::world::EntityKind::Ship { "ship" } else { "station" };
        let extent = match e.bounds {
            Some((lo, hi)) => format!("from {} to {}, {} wide (x), {} high (y), {} long (z)", show(lo), show(hi), hi.x - lo.x + 1, hi.y - lo.y + 1, hi.z - lo.z + 1),
            None => "empty".to_string(),
        };
        out.push_str(&format!("- \"{}\" ({role}, {kind}): {} {}, {extent}, {} {}.\n", e.path, e.blocks, plural(e.blocks as usize, "block"), e.links, plural(e.links, "link")));
    }
    let mut counts = BTreeMap::new();
    let mut controllers = Vec::new();
    ctx.world.with_entity(&chosen.path, &mut |entity| {
        counts = entity.counts_by_type();
        controllers = controller_lines(catalog, entity, MAX_CONTROLLERS);
    })?;
    out.push_str(&format!("Blocks of \"{}\" by type:\n{}", chosen.path, type_lines(catalog, &counts)));
    if controllers.is_empty() {
        out.push_str("No links.\n");
    } else {
        out.push_str(&format!("Links of \"{}\":\n{}\n", chosen.path, controllers.join("\n")));
    }
    if let Some(inside) = super::inside::interior(ctx, &chosen.path, None)? {
        match inside.spaces().first() {
            Some(main) => out.push_str(&format!(
                "Inside the hull of \"{}\": {} empty cells enclosed; the largest space spans x {}..{}, y {}..{}, z {}..{}. Systems go there: inside_space shows the room section by section, fill_inside fills it.\n",
                chosen.path, inside.volume, main.min.x, main.max.x, main.min.y, main.max.y, main.min.z, main.max.z
            )),
            None => out.push_str(&format!("The blocks of \"{}\" enclose no empty space: there is no closed hull with room inside.\n", chosen.path)),
        }
    }
    out.push_str("Positions are [x, y, z] from the Ship Core at [0, 0, 0]: +X is the pilot's left, +Y up, +Z the front. The core need not be in the middle of the ship: go by the bounds above.");
    Ok(Done::said(out))
}

/// Block counts as lines, most frequent first.
fn type_lines(catalog: &Catalog, counts: &BTreeMap<u16, u32>) -> String {
    let mut sorted: Vec<(&u16, &u32)> = counts.iter().collect();
    sorted.sort_by_key(|(id, n)| (std::cmp::Reverse(**n), **id));
    let mut out: String = sorted.iter().take(MAX_TYPES).map(|(id, n)| format!("  {n} x {} (id {id})\n", catalog.name(**id))).collect();
    if sorted.len() > MAX_TYPES {
        out.push_str(&format!("  ... and {} more types.\n", sorted.len() - MAX_TYPES));
    }
    if sorted.is_empty() {
        out.push_str("  none\n");
    }
    out
}

/// One line per controller: what it is, where, and what it controls.
fn controller_lines(catalog: &Catalog, entity: &Entity, limit: usize) -> Vec<String> {
    let links = entity.links();
    let mut lines: Vec<String> = links.iter().take(limit).map(|(at, groups)| format!("  {} at {} controls {}.", catalog.name(entity.get(*at).ty()), show(*at), groups_text(catalog, groups, false))).collect();
    if links.len() > limit {
        lines.push(format!("  ... and {} more controllers; list_links with a position shows one in full.", links.len() - limit));
    }
    lines
}

/// "12 Cannon Barrel, 1 White Light", with the boxes they fill if asked.
fn groups_text(catalog: &Catalog, groups: &BTreeMap<u16, BTreeSet<Pos>>, with_boxes: bool) -> String {
    let parts: Vec<String> = groups
        .iter()
        .map(|(ty, cells)| {
            let mut text = format!("{} {}", cells.len(), catalog.name(*ty));
            if with_boxes {
                let boxes = yard_core::links::boxes(cells);
                let shown: Vec<String> = boxes.iter().take(MAX_BOXES).map(|(lo, hi)| if lo == hi { show(*lo) } else { format!("{}..{}", show(*lo), show(*hi)) }).collect();
                let more = if boxes.len() > MAX_BOXES { format!(" and {} more boxes", boxes.len() - MAX_BOXES) } else { String::new() };
                text.push_str(&format!(" at {}{more}", shown.join(", ")));
            }
            text
        })
        .collect();
    parts.join(if with_boxes { "; " } else { ", " })
}

fn find_blocks(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let catalog = ctx.world.catalog();
    let query = args.text("query").unwrap_or("");
    let limit = args.int("limit").unwrap_or(15) as usize;
    // Shape variants are reached through their full block, so they are
    // listed only when the words fit nothing else.
    let hits = blocks::search(catalog, query, 400);
    let full: Vec<_> = hits.iter().filter(|d| d.base == 0 && d.slab == 0 && !matches!(d.shape, Shape::Wedge | Shape::Corner | Shape::Tetra | Shape::Hepta)).copied().collect();
    let listed = if full.is_empty() { hits } else { full };
    if listed.is_empty() {
        return Ok(Done::said(format!("No block matches \"{query}\". Try fewer or other words: a colour (\"grey\"), a material (\"armor\"), a system (\"cannon\", \"reactor\", \"shield\", \"thruster\", \"rail\", \"light\", \"door\").")));
    }
    let mut out = format!("{} {} for \"{query}\"{}:\n", listed.len(), if listed.len() == 1 { "match" } else { "matches" }, if listed.len() > limit { format!(", the first {limit}") } else { String::new() });
    for def in listed.iter().take(limit) {
        let mut notes = Vec::new();
        let shapes = if def.base == 0 { blocks::variant_words(catalog, def) } else { Vec::new() };
        if !shapes.is_empty() {
            notes.push(format!("shapes: {}", shapes.join(", ")));
        }
        match def.shape {
            Shape::Rail => notes.push("turns 24 ways: set facing (arrow face) and pointing".to_string()),
            Shape::Cube if blocks::faces_matter(def) => notes.push("has a front: set facing".to_string()),
            Shape::Cube | Shape::Sprite => {}
            _ => notes.push("sloped: set slope".to_string()),
        }
        let names = |ids: &[u16]| ids.iter().take(6).map(|id| catalog.name(*id)).collect::<Vec<_>>().join(", ");
        if !def.controlling.is_empty() {
            notes.push(format!("controls: {}", names(&def.controlling)));
        }
        if !def.controlled_by.is_empty() {
            notes.push(format!("controlled by: {}", names(&def.controlled_by)));
        }
        notes.push(format!("category {}", def.category));
        out.push_str(&format!("- {} (id {}): {}\n", def.name, def.id, notes.join("; ")));
    }
    Ok(Done::said(out.trim_end().to_string()))
}

/// The box a call names, or the entity's bounds. None if the entity is empty.
fn region(ctx: &Ctx<'_>, args: &Args<'_>, entity: &str) -> Result<Option<(Pos, Pos)>, String> {
    match (args.pos("from")?, args.pos("to")?) {
        (Some(a), Some(b)) => Ok(Some((a.lo(b), a.hi(b)))),
        (None, None) => Ok(entity_info(ctx, entity)?.bounds),
        _ => Err("Give both \"from\" and \"to\", or neither for the whole entity.".to_string()),
    }
}

fn inside(p: Pos, lo: Pos, hi: Pos) -> bool {
    p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && p.z >= lo.z && p.z <= hi.z
}

fn count_blocks(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let Some((lo, hi)) = region(ctx, args, entity)? else { return Ok(Done::said("The entity is empty.")) };
    let mut counts: BTreeMap<u16, u32> = BTreeMap::new();
    ctx.world.with_entity(entity, &mut |e| {
        e.for_each(|p, b| {
            if inside(p, lo, hi) {
                *counts.entry(b.ty()).or_insert(0) += 1;
            }
        })
    })?;
    let total: u32 = counts.values().sum();
    let (_, _, volume) = span(lo, hi);
    Ok(Done::said(format!("{total} {} in {}..{} ({volume} cells, {} empty):\n{}", plural(total as usize, "block"), show(lo), show(hi), volume - total as u64, type_lines(ctx.world.catalog(), &counts).trim_end())))
}

fn scan_region(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let Some((lo, hi)) = region(ctx, args, entity)? else { return Ok(Done::said("The entity is empty.")) };
    let axis = match args.text("axis") {
        Some("x") => 0,
        Some("z") => 2,
        _ => 1,
    };
    let max_layers = args.int("max_layers").unwrap_or(6) as usize;
    let inside = super::inside::interior(ctx, entity, None)?;
    let mut out = String::new();
    ctx.world.with_entity(entity, &mut |e| out = ascii_slices(ctx.world.catalog(), e, lo, hi, axis, max_layers, inside.as_ref()))?;
    Ok(Done::said(out))
}

/// Draws the box from `lo` to `hi` as character maps, one per layer along
/// `axis` (0 X, 1 Y, 2 Z), at most `max_layers` of them, with a legend.
/// Empty cells `enclosure` holds are drawn ':'.
///
/// Deck plans (Y) have the front at the top and the pilot's left on the
/// left; cross-sections (Z) are seen from behind, up at the top and the
/// pilot's left on the left; side profiles (X) have up at the top and the
/// front on the right.
pub fn ascii_slices(catalog: &Catalog, entity: &Entity, lo: Pos, hi: Pos, axis: usize, max_layers: usize, enclosure: Option<&Interior>) -> String {
    // Which design axis runs across and down a map, and whether it is
    // drawn from its high end.
    let (across, down): ((usize, bool), (usize, bool)) = match axis {
        0 => ((2, false), (1, true)),
        1 => ((0, true), (2, true)),
        _ => ((0, true), (1, true)),
    };
    let coord = |p: Pos, a: usize| [p.x, p.y, p.z][a];
    let size = |a: usize| coord(hi, a) - coord(lo, a) + 1;
    // One character covers scale x scale blocks of a large region.
    let scale = ((size(across.0).max(size(down.0)) + MAX_MAP - 1) / MAX_MAP).max(1);
    let (columns, rows) = ((size(across.0) + scale - 1) / scale, (size(down.0) + scale - 1) / scale);
    let fitting = (MAX_SCAN_CHARS / ((columns as usize + 1) * rows as usize).max(1)).max(1);
    let depth = size(axis) as usize;
    let layers = depth.min(max_layers.max(1)).min(fitting);
    let layer_at = |i: usize| coord(lo, axis) + if layers <= 1 { (depth as i32 - 1) / 2 } else { (i * (depth - 1) / (layers - 1)) as i32 };
    let wanted: Vec<i32> = (0..layers).map(layer_at).collect();

    // Per layer and map cell, how many blocks of each type fall into it.
    let mut maps: HashMap<(i32, i32, i32), HashMap<u16, u32>> = HashMap::new();
    let mut totals: HashMap<u16, u32> = HashMap::new();
    entity.for_each(|p, b| {
        if !inside(p, lo, hi) || !wanted.contains(&coord(p, axis)) {
            return;
        }
        let cell = |(a, flipped): (usize, bool)| if flipped { (coord(hi, a) - coord(p, a)) / scale } else { (coord(p, a) - coord(lo, a)) / scale };
        *maps.entry((coord(p, axis), cell(across), cell(down))).or_default().entry(b.ty()).or_insert(0) += 1;
        *totals.entry(b.ty()).or_insert(0) += 1;
    });
    let mut by_count: Vec<(u16, u32)> = totals.into_iter().filter(|(ty, _)| *ty != Catalog::CORE).collect();
    by_count.sort_by_key(|(ty, n)| (std::cmp::Reverse(*n), *ty));
    let symbol = |ty: u16| if ty == Catalog::CORE { '@' } else { by_count.iter().position(|(t, _)| *t == ty).and_then(|i| SYMBOLS.chars().nth(i)).unwrap_or('?') };

    let names = ["x", "y", "z"];
    let ends = |(a, flipped): (usize, bool)| {
        let words = [["right (-X)", "left (+X)"], ["down", "up"], ["back (-Z)", "front (+Z)"]][a];
        if flipped { (words[1], words[0]) } else { (words[0], words[1]) }
    };
    let mut out = format!(
        "Region {}..{}, {} of {} layers along {}. Each map: left edge is {}, right edge is {}; top row is {}, bottom row is {}.",
        show(lo), show(hi), layers, depth, names[axis], ends(across).0, ends(across).1, ends(down).0, ends(down).1
    );
    if scale > 1 {
        out.push_str(&format!(" The region is large: each character covers {scale} x {scale} blocks and shows the most common block there."));
    }
    // Whether a map cell without blocks covers an enclosed empty cell.
    let enclosed = |layer: i32, column: i32, row: i32| {
        let Some(enclosure) = enclosure else { return false };
        let along = |(a, flipped): (usize, bool), cell: i32| {
            (0..scale).map(move |k| if flipped { coord(hi, a) - (cell * scale + k) } else { coord(lo, a) + cell * scale + k }).filter(move |v| *v >= coord(lo, a) && *v <= coord(hi, a))
        };
        along(across, column).any(|u| {
            along(down, row).any(|v| {
                let mut c = [0; 3];
                c[axis] = layer;
                c[across.0] = u;
                c[down.0] = v;
                enclosure.contains(Pos::new(c[0], c[1], c[2]))
            })
        })
    };
    let mut drawn = Vec::new();
    for &layer in &wanted {
        let mut lines = Vec::new();
        for row in 0..rows {
            let line: String = (0..columns)
                .map(|column| match maps.get(&(layer, column, row)) {
                    None if enclosed(layer, column, row) => ':',
                    None => '.',
                    Some(types) if types.contains_key(&Catalog::CORE) => '@',
                    Some(types) => types.iter().max_by_key(|(ty, n)| (**n, std::cmp::Reverse(**ty))).map_or('.', |(ty, _)| symbol(*ty)),
                })
                .collect();
            lines.push(line);
        }
        drawn.push((layer, lines));
    }
    out.push_str("\nLegend: . empty");
    if drawn.iter().any(|(_, lines)| lines.iter().any(|l| l.contains(':'))) {
        out.push_str(", : empty inside the hull");
    }
    if maps.values().any(|types| types.contains_key(&Catalog::CORE)) {
        out.push_str(", @ Ship Core");
    }
    for (i, (ty, n)) in by_count.iter().enumerate().take(SYMBOLS.len()) {
        out.push_str(&format!(", {} {} ({n})", SYMBOLS.chars().nth(i).unwrap_or('?'), catalog.name(*ty)));
    }
    if by_count.len() > SYMBOLS.len() {
        out.push_str(", ? other blocks");
    }
    out.push('\n');
    for (layer, lines) in drawn {
        out.push_str(&format!("{} = {layer}:\n", names[axis]));
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn find_groups(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let catalog = ctx.world.catalog();
    let Some((lo, hi)) = region(ctx, args, entity)? else { return Ok(Done::said("The entity is empty.")) };
    let (target, chosen) = block_in_design(ctx, entity, args.text("block").unwrap_or(""), lo, hi)?;
    let min_size = args.int("min_size").unwrap_or(1) as u64;
    let mut cells: BTreeSet<Pos> = BTreeSet::new();
    ctx.world.with_entity(entity, &mut |e| {
        e.for_each(|p, b| {
            if inside(p, lo, hi) && (b.ty() == target || catalog.get(b.ty()).is_some_and(|d| d.base == target)) {
                cells.insert(p);
            }
        })
    })?;
    let name = catalog.name(target);
    if cells.is_empty() {
        return Ok(Done::said(format!("There is no {name} in {}..{}.", show(lo), show(hi))));
    }
    let total = cells.len();
    let groups: Vec<Group> = groups_of(cells).into_iter().filter(|g| g.count >= min_size).collect();
    let mut out = format!("{chosen}{total} {name} {} (with its shapes) in {}..{}, in {} {}", plural(total, "block"), show(lo), show(hi), groups.len(), plural(groups.len(), "group"));
    if min_size > 1 {
        out.push_str(&format!(" of {min_size} or more"));
    }
    out.push_str(", largest first:\n");
    for g in groups.iter().take(MAX_GROUPS) {
        out.push_str(&format!("- {}\n", g.text()));
    }
    if groups.len() > MAX_GROUPS {
        out.push_str(&format!("- and {} smaller groups; give a box or min_size to narrow the search.\n", groups.len() - MAX_GROUPS));
    }
    Ok(Done::said(out.trim_end().to_string()))
}

/// The full block a search means, by name or id. A name that fits several
/// blocks ("pink") means the one of them the box holds most of, and the
/// answer says so.
pub fn block_in_design(ctx: &Ctx<'_>, entity: &str, text: &str, lo: Pos, hi: Pos) -> Result<(u16, String), String> {
    let catalog = ctx.world.catalog();
    let full = |id: u16| catalog.get(id).map_or(id, |d| if d.base != 0 { d.base } else { d.id });
    let unclear = match blocks::lookup(catalog, text) {
        Ok(def) => return Ok((full(def.id), String::new())),
        Err(unclear) => unclear,
    };
    let candidates: BTreeSet<u16> = blocks::search(catalog, text, 400).into_iter().map(|d| full(d.id)).collect();
    let mut counts: BTreeMap<u16, u64> = BTreeMap::new();
    ctx.world.with_entity(entity, &mut |e| {
        e.for_each(|p, b| {
            let id = full(b.ty());
            if inside(p, lo, hi) && candidates.contains(&id) {
                *counts.entry(id).or_insert(0) += 1;
            }
        })
    })?;
    match counts.iter().max_by_key(|(id, n)| (**n, std::cmp::Reverse(**id))) {
        Some((&id, _)) => {
            let others: Vec<String> = counts.keys().filter(|&&o| o != id).map(|&o| catalog.name(o)).collect();
            let also = if others.is_empty() { String::new() } else { format!(" (the design also has {})", others.join(", ")) };
            Ok((id, format!("\"{text}\" taken as {}, the one the design holds most of{also}.\n", catalog.name(id))))
        }
        None => Err(unclear),
    }
}

/// Blocks that touch face to face.
pub struct Group {
    pub count: u64,
    pub min: Pos,
    pub max: Pos,
    pub centre: Pos,
}

impl Group {
    pub fn text(&self) -> String {
        let (a, b) = (self.min, self.max);
        format!("{} blocks from {} to {} ({} x {} x {}), centre {}", self.count, show(a), show(b), b.x - a.x + 1, b.y - a.y + 1, b.z - a.z + 1, show(self.centre))
    }
}

/// The groups of touching cells, largest first.
pub fn groups_of(mut cells: BTreeSet<Pos>) -> Vec<Group> {
    let mut groups = Vec::new();
    while let Some(start) = cells.pop_first() {
        let (mut count, mut a, mut b, mut sum) = (0u64, start, start, [0i64; 3]);
        let mut stack = vec![start];
        while let Some(p) = stack.pop() {
            count += 1;
            (a, b) = (a.lo(p), b.hi(p));
            for (s, v) in sum.iter_mut().zip(p.to_array()) {
                *s += v as i64;
            }
            for d in super::orient::Dir::ALL {
                let v = d.vector();
                let next = p.offset(v[0], v[1], v[2]);
                if cells.remove(&next) {
                    stack.push(next);
                }
            }
        }
        let c = sum.map(|s| (s as f64 / count as f64).round() as i32);
        groups.push(Group { count, min: a, max: b, centre: Pos::new(c[0], c[1], c[2]) });
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.count));
    groups
}

fn get_block(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let (entity, at) = (args.entity(), args.need_pos("pos")?);
    let catalog = ctx.world.catalog();
    let mut empty = false;
    ctx.world.with_entity(entity, &mut |e| empty = e.get(at).is_air())?;
    let whereabouts = match empty {
        true => match super::inside::interior(ctx, entity, None)? {
            Some(inside) if inside.contains(at) => ", inside the hull",
            _ => ", outside the hull",
        },
        false => "",
    };
    let mut out = String::new();
    ctx.world.with_entity(entity, &mut |e| {
        let block = e.get(at);
        out = format!("{}: {}{whereabouts}.", show(at), blocks::describe(catalog, block));
        if !block.is_air() {
            out.push_str(&format!(" {}", links_of(catalog, e, at, false)));
        }
        let around: Vec<String> = super::orient::Dir::ALL
            .into_iter()
            .map(|d| {
                let v = d.vector();
                let neighbour = e.get(at.offset(v[0], v[1], v[2]));
                format!("{} {}", d.name(), if neighbour.is_air() { "empty".to_string() } else { catalog.name(neighbour.ty()) })
            })
            .collect();
        out.push_str(&format!("\nNeighbours: {}.", around.join(", ")));
    })?;
    Ok(Done::said(out))
}

/// What a block controls and what controls it, in sentences.
fn links_of(catalog: &Catalog, entity: &Entity, at: Pos, with_boxes: bool) -> String {
    let links: &Links = entity.links();
    let controls = match links.get(&at) {
        Some(groups) => format!("It controls {}.", groups_text(catalog, groups, with_boxes)),
        None => "It controls nothing.".to_string(),
    };
    let controllers: Vec<String> = yard_core::links::controllers_of(links, at).into_iter().map(|c| format!("{} at {}", catalog.name(entity.get(c).ty()), show(c))).collect();
    let controlled = if controllers.is_empty() { "Nothing controls it.".to_string() } else { format!("It is controlled by {}.", controllers.join(", ")) };
    format!("{controls} {controlled}")
}

fn list_links(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let (entity, at) = (args.entity(), args.pos("pos")?);
    let catalog = ctx.world.catalog();
    let mut out = String::new();
    ctx.world.with_entity(entity, &mut |e| {
        out = match at {
            Some(at) if e.get(at).is_air() => format!("There is no block at {}.", show(at)),
            Some(at) => format!("{} at {}. {}", catalog.name(e.get(at).ty()), show(at), links_of(catalog, e, at, true)),
            None => {
                let lines = controller_lines(catalog, e, MAX_CONTROLLERS);
                if lines.is_empty() { "This entity has no links.".to_string() } else { format!("{} {}:\n{}", e.links().len(), plural(e.links().len(), "controller"), lines.join("\n")) }
            }
        };
    })?;
    Ok(Done::said(out))
}

fn read_guide(_ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let topic = args.text("topic").unwrap_or("");
    guides::read(topic).map(|text| Done::said(text.trim())).ok_or_else(|| format!("There is no guide \"{topic}\". The guides are: {}.", guides::TOPICS.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::super::testing::Bench;
    use serde_json::json;

    /// A hull plate, a cannon and a light to look at.
    fn scene() -> Bench {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-2, -1, -3], "to": [2, -1, 6], "block": "Grey Basic Armor"}));
        bench.ok("place_system", json!({"computer": "Cannon Computer", "at": [1, 0, 0], "module": "Cannon Barrel", "from": [1, 0, 1], "to": [1, 0, 4]}));
        bench.ok("place_blocks", json!({"blocks": [{"pos": [0, 0, 6], "block": "Grey Basic Armor", "shape": "wedge", "slope": ["up", "front"]}]}));
        bench
    }

    #[test]
    fn design_info_reports_entities_block_counts_and_links() {
        let mut bench = scene();
        let info = bench.ok("design_info", json!({}));
        assert!(info.contains("The design has 1 entity."), "{info}");
        assert!(info.contains("- \"\" (main, ship): 57 blocks, from (-2, -1, -3) to (2, 0, 6), 5 wide (x), 2 high (y), 10 long (z), 5 links."), "{info}");
        assert!(info.contains("  50 x Grey Basic Armor (id 598)") && info.contains("  4 x Cannon Barrel (id 16)"), "{info}");
        assert!(info.contains("Cannon Computer at (1, 0, 0) controls 4 Cannon Barrel."), "{info}");
        assert!(info.contains("Ship Core at (0, 0, 0) controls 1 Cannon Computer."), "{info}");
        assert!(info.contains("+X is the pilot's left"), "{info}");
        assert!(info.contains("The blocks of \"\" enclose no empty space"), "{info}");
        let mut hulled = Bench::new();
        hulled.ok("fill", json!({"from": [-5, -3, -40], "to": [5, 3, 0], "block": "Grey Basic Armor", "mode": "hollow"}));
        let inside = hulled.ok("design_info", json!({}));
        assert!(inside.contains("Inside the hull of \"\": 1755 empty cells enclosed; the largest space spans x -4..4, y -2..2, z -39..-1."), "{inside}");
        assert!(Bench::new().ok("design_info", json!({})).contains("No links."));
    }

    #[test]
    fn find_blocks_lists_names_shapes_and_what_a_block_controls() {
        let mut bench = Bench::new();
        let armor = bench.ok("find_blocks", json!({"query": "basic armor"}));
        assert!(armor.starts_with("2 matches"), "{armor}");
        assert!(armor.contains("- Grey Basic Armor (id 598): shapes: wedge, corner, tetra, hepta, slab_3/4, slab_1/2, slab_1/4; category General/Hulls/Basic"), "{armor}");
        assert!(!armor.contains("Wedge (id"), "variants are reached through their full block: {armor}");
        let cannon = bench.ok("find_blocks", json!({"query": "cannon", "limit": "1"}));
        assert!(cannon.contains("2 matches for \"cannon\", the first 1") && cannon.contains("- Cannon Computer (id 6)") && !cannon.contains("Cannon Barrel (id"), "{cannon}");
        assert!(bench.ok("find_blocks", json!({"query": "barrel"})).contains("Cannon Barrel (id 16): controlled by: Cannon Computer"));
        let computer = bench.ok("find_blocks", json!({"query": "cannon computer"}));
        assert!(computer.contains("has a front: set facing; controls: Cannon Barrel; controlled by: Ship Core"), "{computer}");
        assert!(bench.ok("find_blocks", json!({"query": "rail basic"})).contains("turns 24 ways"));
        assert!(bench.ok("find_blocks", json!({"query": "grey wedge"})).contains("Grey Basic Armor Wedge (id 599): sloped: set slope"));
        assert!(bench.ok("find_blocks", json!({"query": "warp nacelle"})).starts_with("No block matches"));
        assert!(bench.err("find_blocks", json!({})).contains("Missing argument \"query\""));
    }

    #[test]
    fn count_blocks_counts_a_box_or_the_whole_entity() {
        let mut bench = scene();
        let whole = bench.ok("count_blocks", json!({}));
        assert!(whole.starts_with("57 blocks in (-2, -1, -3)..(2, 0, 6) (100 cells, 43 empty):"), "{whole}");
        let part = bench.ok("count_blocks", json!({"from": [1, 0, 0], "to": [1, 0, 2]}));
        assert!(part.starts_with("3 blocks") && part.contains("2 x Cannon Barrel") && part.contains("1 x Cannon Computer"), "{part}");
        assert!(bench.err("count_blocks", json!({"from": [1, 0, 0]})).contains("both \"from\" and \"to\""));
        assert!(bench.err("count_blocks", json!({"from": [1, 0], "to": [1, 0, 0]})).contains("exactly 3 values"));
    }

    #[test]
    fn scan_region_draws_layers_with_the_pilots_left_on_the_left() {
        let mut bench = scene();
        let deck = bench.ok("scan_region", json!({"from": [-2, 0, -1], "to": [2, 0, 6], "axis": "y"}));
        let lines: Vec<&str> = deck.lines().collect();
        assert!(lines[0].contains("1 of 1 layers along y") && lines[0].contains("left edge is left (+X), right edge is right (-X); top row is front (+Z), bottom row is back (-Z)"), "{deck}");
        assert!(lines[1].starts_with("Legend: . empty, @ Ship Core, # Cannon Barrel (4), a Cannon Computer (1), b Grey Basic Armor Wedge (1)"), "{deck}");
        assert_eq!(lines[2], "y = 0:");
        // z = 6 at the top: the wedge on the centre line. The cannon sits at
        // x = 1, one column left of the core.
        assert_eq!(&lines[3..11], ["..b..", ".....", ".#...", ".#...", ".#...", ".#...", ".a@..", "....."], "{deck}");
        let side = bench.ok("scan_region", json!({"axis": "x", "max_layers": 1}));
        assert!(side.contains("1 of 5 layers along x") && side.contains("left edge is back (-Z), right edge is front (+Z); top row is up, bottom row is down"), "{side}");
        assert!(side.contains("x = 0:\n...@.....a\n##########"), "{side}");
    }

    #[test]
    fn scan_region_shrinks_what_is_too_large_to_draw() {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-100, 0, -100], "to": [99, 3, 99], "block": "Grey Basic Armor", "mode": "frame"}));
        let scan = bench.ok("scan_region", json!({"max_layers": 24}));
        assert!(scan.contains("each character covers 4 x 4 blocks"), "{}", &scan[..300]);
        assert!(scan.len() < 7000, "{} characters", scan.len());
        let maps: Vec<&str> = scan.lines().filter(|l| l.len() == 50).collect();
        assert_eq!(maps.len(), 100, "two layers of 50 rows");
    }

    #[test]
    fn find_groups_finds_each_separate_group_of_a_block_and_its_shapes() {
        let mut bench = Bench::new();
        // A plate with two rings on it, joined through the plate.
        bench.ok("fill", json!({"from": [-20, -10, -1], "to": [20, 10, -1], "block": "Grey Basic Armor"}));
        bench.ok("fill", json!({"from": [-12, -3, -2], "to": [-6, 3, -2], "block": "Red Basic Armor", "mode": "frame"}));
        bench.ok("fill", json!({"from": [-12, -3, -2], "to": [-6, 3, -2], "block": "Red Basic Armor", "mode": "hollow"}));
        bench.ok("fill", json!({"from": [-11, -2, -2], "to": [-7, 2, -2], "block": "air"}));
        bench.ok("fill", json!({"from": [4, -2, -2], "to": [8, 2, -2], "block": "Red Basic Armor"}));
        bench.ok("fill", json!({"from": [5, -1, -2], "to": [7, 1, -2], "block": "air"}));
        bench.ok("place_blocks", json!({"blocks": [{"pos": [6, 3, -2], "block": "Red Basic Armor", "shape": "wedge", "slope": ["up", "back"]}]}));
        let rings = bench.ok("find_groups", json!({"block": "Red Basic Armor", "from": [-30, -30, -2], "to": [30, 30, -2]}));
        assert!(rings.starts_with("41 Red Basic Armor blocks (with its shapes) in (-30, -30, -2)..(30, 30, -2), in 2 groups, largest first:"), "{rings}");
        assert!(rings.contains("- 24 blocks from (-12, -3, -2) to (-6, 3, -2) (7 x 7 x 1), centre (-9, 0, -2)"), "{rings}");
        assert!(rings.contains("- 17 blocks from (4, -2, -2) to (8, 3, -2) (5 x 6 x 1), centre (6, 0, -2)"), "the wedge counts with its block: {rings}");
        assert!(bench.ok("find_groups", json!({"block": "Red Basic Armor", "min_size": 20})).contains("in 1 group of 20 or more"));
        assert!(bench.ok("find_groups", json!({"block": "Cannon Barrel"})).starts_with("There is no Cannon Barrel"));
    }

    #[test]
    fn get_block_tells_type_turn_links_and_neighbours() {
        let mut bench = scene();
        let computer = bench.ok("get_block", json!({"pos": [1, 0, 0]}));
        assert!(computer.starts_with("(1, 0, 0): Cannon Computer (id 6, facing front, orientation 0). It controls 4 Cannon Barrel. It is controlled by Ship Core at (0, 0, 0)."), "{computer}");
        assert!(computer.contains("Neighbours: up empty, down Grey Basic Armor, left empty, right Ship Core, front Cannon Barrel, back empty."), "{computer}");
        let wedge = bench.ok("get_block", json!({"pos": [0, 0, 6]}));
        assert!(wedge.contains("Grey Basic Armor Wedge (id 599, slope up, front, orientation 2). It controls nothing. Nothing controls it."), "{wedge}");
        assert!(bench.ok("get_block", json!({"pos": [0, 9, 0]})).starts_with("(0, 9, 0): empty, outside the hull."));
        assert!(bench.err("get_block", json!({})).contains("Missing argument \"pos\""));
    }

    #[test]
    fn list_links_shows_one_controller_in_full_or_all_in_brief() {
        let mut bench = scene();
        let one = bench.ok("list_links", json!({"pos": [1, 0, 0]}));
        assert_eq!(one, "Cannon Computer at (1, 0, 0). It controls 4 Cannon Barrel at (1, 0, 1)..(1, 0, 4). It is controlled by Ship Core at (0, 0, 0).");
        let barrel = bench.ok("list_links", json!({"pos": [1, 0, 3]}));
        assert!(barrel.contains("It controls nothing. It is controlled by Cannon Computer at (1, 0, 0)."), "{barrel}");
        let all = bench.ok("list_links", json!({}));
        assert!(all.starts_with("2 controllers:") && all.contains("Cannon Computer at (1, 0, 0) controls 4 Cannon Barrel."), "{all}");
        assert_eq!(bench.ok("list_links", json!({"pos": [5, 5, 5]})), "There is no block at (5, 5, 5).");
        assert_eq!(Bench::new().ok("list_links", json!({})), "This entity has no links.");
    }

    #[test]
    fn read_guide_serves_every_topic_and_refuses_others() {
        let mut bench = Bench::new();
        for topic in crate::guides::TOPICS {
            let text = bench.ok("read_guide", json!({"topic": topic}));
            assert!(text.starts_with("# ") && text.len() > 300 && text.len() < 4000, "guide {topic} has {} characters", text.len());
        }
        assert!(bench.err("read_guide", json!({"topic": "cooking"})).contains("must be one of"));
    }
}
