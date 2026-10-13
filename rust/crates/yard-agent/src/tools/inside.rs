//! Tools that know the inside of a hull from the outside: where the empty
//! space enclosed by the blocks is, and filling it without touching the
//! shell or anything already built.

use super::{apply, blocks, check_volume, core_link, describe_change, entity_info, fills, plural, rules_for, show, Args, Ctx, Done, Tool};
use crate::schema;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use yard_core::edit::{When, MAX_CELLS};
use yard_core::interior::{Interior, Space, MAX_SEAL};
use yard_core::Pos;

/// Sections of the ship inside_space describes.
const SECTIONS: usize = 8;
/// Enclosed spaces inside_space lists.
const MAX_SPACES: usize = 5;
/// Outlines find_outlines lists.
const MAX_OUTLINES: usize = 24;

/// The sides fill_inside can start from: the axis, and whether from its
/// high end.
const SIDES: [(&str, usize, bool); 6] = [("back", 2, false), ("front", 2, true), ("bottom", 1, false), ("top", 1, true), ("right", 0, false), ("left", 0, true)];

fn seal() -> (&'static str, Value) {
    ("seal", schema::integer_in("Gaps in the shell up to twice this many blocks wide count as closed. Leave out to choose it from the design: 0 for a closed shell, more for one with openings.", 0, MAX_SEAL as i64))
}

pub fn tools() -> Vec<Tool> {
    let mut fill_props = blocks::properties();
    fill_props.extend([
        ("from", schema::position("One corner of a box to keep to, such as the stern. Leave out both corners for the whole entity.")),
        ("to", schema::position("The opposite corner, included.")),
        ("max_blocks", schema::integer_in("Most blocks to place. They are packed against the start_from side, a layer at a time, and the last layer is filled from its middle out. Leave out to fill all the enclosed space in the box.", 1, MAX_CELLS as i64)),
        ("start_from", schema::choice("Which side of the space to fill from when max_blocks is given. Default back (lowest z). left is +X, right is -X.", &SIDES.map(|(name, _, _)| name))),
        seal(),
    ]);
    vec![
        Tool::on_entity(
            "inside_space",
            "Finds the empty space enclosed by the hull: the room inside the ship where systems go, as against the space around it. Says how much there is, the separate enclosed spaces with their bounds, and where the room is section by section from back to front. Call it before putting anything inside an existing hull.",
            vec![("from", schema::position("One corner of a box to describe. Leave out both corners for the whole entity.")), ("to", schema::position("The opposite corner, included.")), seal()],
            &[],
            inside_space,
        ),
        Tool::on_entity(
            "find_outlines",
            "Finds shapes the player outlined in one block on a side of the ship, such as circles marking where engines go on the back: each outline with its centre and size, and a box behind it. With fill_with and blocks_each it also fills inside the hull behind every outline, e.g. 1000 Thruster Module behind each engine outline, in one call. Use it whenever the player says something is outlined, marked or drawn on the ship.",
            vec![
                ("block", schema::string("The block the outlines are drawn in: exact name or id, e.g. from find_blocks \"pink\".")),
                ("side", schema::choice("The side of the ship the outlines are on. Default back. left is +X, right is -X.", &SIDES.map(|(name, _, _)| name))),
                ("depth", schema::integer_in("How many layers in from that side to look. Default 60.", 1, 400)),
                ("min_size", schema::integer_in("Leave out outlines of fewer blocks than this. Default 8.", 1, 100_000)),
                ("fill_with", schema::string("To fill behind every outline in the same call: the block to place, such as \"Thruster Module\". Each outline gets its own blocks inside the hull, packed against its side; the hull and anything built stay as they are.")),
                ("blocks_each", schema::integer_in("With fill_with: how many blocks behind each outline. Default 1000.", 1, MAX_CELLS as i64)),
            ],
            &["block"],
            find_outlines,
        ),
        Tool::on_entity(
            "fill_inside",
            "Fills empty space enclosed by the hull with one block, never touching the hull, anything already built, or the space outside the ship: thrusters, reactors, shields, cargo and other systems that belong inside. Keep to part of the ship with a box, and place a set number of blocks with max_blocks and start_from (e.g. 1000 Thruster Module packed against the back).",
            fill_props,
            &["block"],
            fill_inside,
        ),
    ]
}

/// The enclosed space of an entity, with the seal a call asks for or the
/// one that suits the design. None if the entity is empty.
pub fn interior(ctx: &Ctx<'_>, entity: &str, seal: Option<u32>) -> Result<Option<Interior>, String> {
    let mut found = None;
    ctx.world.with_entity(entity, &mut |e| found = match seal {
        Some(seal) => Interior::of(e, seal),
        None => Interior::find(e),
    })?;
    Ok(found)
}

/// Whether a call asks to keep to the empty space inside the hull.
pub fn only_inside(args: &Args<'_>) -> bool {
    args.text("only_where").map(str::trim) == Some("inside")
}

/// How the shell was read, in a sentence.
fn sealed(inside: &Interior) -> String {
    match inside.seal {
        0 => String::new(),
        seal => format!(" The shell has openings; gaps up to {} blocks wide were counted as closed.", 2 * seal),
    }
}

fn box_of(ctx: &Ctx<'_>, args: &Args<'_>, entity: &str) -> Result<Option<(Pos, Pos)>, String> {
    match (args.pos("from")?, args.pos("to")?) {
        (Some(a), Some(b)) => Ok(Some((a.lo(b), a.hi(b)))),
        (None, None) => Ok(entity_info(ctx, entity)?.bounds),
        _ => Err("\"from\" and \"to\" are two opposite corners of a box; give both, or neither for the whole entity. To place something around a point, make a box around it.".to_string()),
    }
}

/// Where the enclosed room is along the same stretch of the ship as a box
/// that has none, so that a wrong guess can be put right.
fn room_nearby(ctx: &Ctx<'_>, inside: &Interior, entity: &str, lo: Pos, hi: Pos) -> Result<String, String> {
    let Some((whole_lo, whole_hi)) = entity_info(ctx, entity)?.bounds else { return Ok(String::new()) };
    let stretch = inside.layers(Pos::new(whole_lo.x, whole_lo.y, lo.z), Pos::new(whole_hi.x, whole_hi.y, hi.z), 2);
    let mut room: Option<Space> = None;
    for layer in &stretch {
        let r = room.get_or_insert(Space { min: layer.min, max: layer.max, volume: 0 });
        r.min = r.min.lo(layer.min);
        r.max = r.max.hi(layer.max);
        r.volume += layer.volume;
    }
    Ok(match room {
        Some(r) => format!(" Between z {} and {} the room inside the hull is at {} ({} cells); this ship is not centred on its core.", lo.z, hi.z, extent(&r), r.volume),
        None => format!(" There is no room inside the hull anywhere between z {} and {}. inside_space shows where the room is.", lo.z, hi.z),
    })
}

fn extent(space: &Space) -> String {
    format!("x {}..{}, y {}..{}, z {}..{}", space.min.x, space.max.x, space.min.y, space.max.y, space.min.z, space.max.z)
}

fn inside_space(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let Some((lo, hi)) = box_of(ctx, args, entity)? else { return Ok(Done::said("The entity is empty.")) };
    let Some(inside) = interior(ctx, entity, args.int("seal").map(|s| s as u32))? else { return Ok(Done::said("The entity is empty.")) };
    let whole = !args.has("from");
    let layers = inside.layers(lo, hi, 2);
    let total: u64 = layers.iter().map(|l| l.volume).sum();
    if total == 0 {
        let why = if whole {
            format!("The blocks of \"{entity}\" enclose no empty space: either the hull is solid, or it is not a closed shell (openings wider than {} blocks let the outside in). Build a hollow hull first, or give a larger seal.", 2 * MAX_SEAL)
        } else {
            format!("There is no enclosed empty space in {}..{}: it is solid, outside the hull, or the hull is open there.", show(lo), show(hi))
        };
        return Ok(Done::said(why));
    }
    let mut out = if whole {
        format!("Inside the hull of \"{entity}\": {total} empty {} enclosed.{}", plural(total as usize, "cell"), sealed(&inside))
    } else {
        format!("In {}..{}: {total} empty {} enclosed by the hull.{}", show(lo), show(hi), plural(total as usize, "cell"), sealed(&inside))
    };
    if whole {
        let spaces = inside.spaces();
        out.push_str(&format!(" {} separate {}, largest first:\n", spaces.len(), plural(spaces.len(), "space")));
        for space in spaces.iter().take(MAX_SPACES) {
            out.push_str(&format!("- {} cells: {}\n", space.volume, extent(space)));
        }
        if spaces.len() > MAX_SPACES {
            let rest: u64 = spaces[MAX_SPACES..].iter().map(|s| s.volume).sum();
            out.push_str(&format!("- and {} smaller ones, {rest} cells in all.\n", spaces.len() - MAX_SPACES));
        }
    } else {
        out.push('\n');
    }
    // Sections of about equal length from back to front.
    let (z0, z1) = (layers[0].min.z, layers[layers.len() - 1].min.z);
    let length = (z1 - z0 + 1) as usize;
    let count = SECTIONS.min(length);
    out.push_str("Enclosed room from back to front:\n");
    for i in 0..count {
        let (from, to) = (z0 + (i * length / count) as i32, z0 + ((i + 1) * length / count) as i32 - 1);
        let mut section: Option<Space> = None;
        for layer in layers.iter().filter(|l| l.min.z >= from && l.min.z <= to) {
            let s = section.get_or_insert(Space { min: layer.min, max: layer.max, volume: 0 });
            s.min = s.min.lo(layer.min);
            s.max = s.max.hi(layer.max);
            s.volume += layer.volume;
        }
        match section {
            Some(s) => out.push_str(&format!("- z {from}..{to}: {} cells, x {}..{}, y {}..{}\n", s.volume, s.min.x, s.max.x, s.min.y, s.max.y)),
            None => out.push_str(&format!("- z {from}..{to}: none\n")),
        }
    }
    out.push_str("fill_inside fills this space with a block; only_where \"inside\" keeps fill, line, cylinder, ellipsoid and hull to it. Neither touches the hull or the space outside. scan_region marks enclosed empty cells with ':'.");
    Ok(Done::said(out))
}

fn find_outlines(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let catalog = ctx.world.catalog();
    let Some((lo, hi)) = entity_info(ctx, entity)?.bounds else { return Ok(Done::said("The entity is empty.")) };
    let (target, chosen) = super::inspect::block_in_design(ctx, entity, args.text("block").unwrap_or(""), lo, hi)?;
    let name = catalog.name(target);
    let side = args.text("side").unwrap_or("back");
    let &(_, axis, from_high) = SIDES.iter().find(|(n, _, _)| *n == side).unwrap_or(&SIDES[0]);
    let depth = args.int("depth").unwrap_or(60) as i32;
    let min_size = args.int("min_size").unwrap_or(8) as u64;
    let at = |p: Pos| p.to_array()[axis];
    // Layers counted from the outside in, for the block asked for and for
    // the other blocks of its colour, in case the player meant one of those.
    let (outer, inward) = if from_high { (at(hi), -1) } else { (at(lo), 1) };
    let full = |id: u16| catalog.get(id).map_or(id, |d| if d.base != 0 { d.base } else { d.id });
    let colour = name.split_whitespace().next().unwrap_or("").to_string();
    let mut layers: BTreeMap<u16, Vec<BTreeSet<Pos>>> = BTreeMap::new();
    ctx.world.with_entity(entity, &mut |e| {
        e.for_each(|p, b| {
            let i = (at(p) - outer) * inward;
            if !(0..depth).contains(&i) {
                return;
            }
            let id = full(b.ty());
            if id == target || (!colour.is_empty() && catalog.name(id).split_whitespace().next() == Some(colour.as_str())) {
                layers.entry(id).or_insert_with(|| vec![BTreeSet::new(); depth as usize])[i as usize].insert(p);
            }
        })
    })?;
    let mut found = layers.remove(&target).and_then(|l| best_layer(l, axis, min_size)).map(|o| (target, o));
    let mut instead = String::new();
    if found.is_none() {
        found = layers.into_iter().filter_map(|(id, l)| best_layer(l, axis, min_size).map(|o| (id, o))).max_by_key(|(_, o)| o.score);
        if let Some((id, _)) = &found {
            instead = format!("There are no outlines of {name}; {} has some, so these are of it.\n", catalog.name(*id));
        }
    }
    let Some((target, Outlines { index: i, score: (rings, _), groups, others })) = found else {
        return Ok(Done::said(format!("No outlines of {name} of {min_size} or more blocks within {depth} layers of the {side}. find_groups searches the whole entity; find_blocks gives exact names.")));
    };
    let name = catalog.name(target);
    let chosen = format!("{chosen}{instead}");
    let layer = outer + inward * i as i32;
    let far = if from_high { at(lo) } else { at(hi) };
    let axis_name = ["x", "y", "z"][axis];
    let shape = if rings > 0 { "closed outline" } else { "outline" };
    let mut out = format!("{chosen}{} {} of {name} on the {side}, where they stand apart best at {axis_name} = {layer}, largest first:\n", groups.len(), plural(groups.len(), shape));
    for g in groups.iter().take(MAX_OUTLINES) {
        let (mut a, mut b) = (g.min.to_array(), g.max.to_array());
        a[axis] = layer;
        b[axis] = far;
        out.push_str(&format!("- {}; box behind it: from [{}, {}, {}] to [{}, {}, {}]\n", g.text(), a[0], a[1], a[2], b[0], b[1], b[2]));
    }
    if groups.len() > MAX_OUTLINES {
        out.push_str(&format!("- and {} smaller ones; raise min_size to leave them out.\n", groups.len() - MAX_OUTLINES));
    }
    if others > 0 {
        out.push_str(&format!("({others} other {} of {name} in that layer enclose nothing and are left out.)\n", plural(others, "piece")));
    }
    let Some(fill_with) = args.text("fill_with") else {
        out.push_str(&format!("To fill behind every outline, call find_outlines again with fill_with and blocks_each, or fill_inside once per outline with its box, start_from {side}, and max_blocks."));
        return Ok(Done::said(out));
    };
    // Each outline's own blocks, behind it, none shared.
    let block = blocks::resolve(ctx.world.catalog(), &Args::of(&serde_json::json!({ "block": fill_with })), "block", true)?;
    if block.is_air() {
        return Err("fill_with places blocks; to clear space use remove or fill with block \"air\".".to_string());
    }
    let each = args.int("blocks_each").unwrap_or(1000) as u64;
    check_volume(each * groups.len() as u64)?;
    let Some(inside) = interior(ctx, entity, None)? else { return Ok(Done::said(out)) };
    let mut taken: BTreeSet<Pos> = BTreeSet::new();
    let mut counts = Vec::new();
    for g in &groups {
        let (mut a, mut b) = (g.min, g.max);
        let (mut lo_a, mut hi_a) = (a.to_array(), b.to_array());
        lo_a[axis] = layer.min(far);
        hi_a[axis] = layer.max(far);
        (a, b) = (Pos::new(lo_a[0], lo_a[1], lo_a[2]), Pos::new(hi_a[0], hi_a[1], hi_a[2]));
        let cells = packed(&inside, a, b, axis, from_high, each, &taken);
        counts.push(cells.len());
        taken.extend(cells);
    }
    let info = entity_info(ctx, entity)?;
    let link = core_link(&rules_for(ctx, &info), block.ty());
    let change = apply(ctx, entity, fills(&taken, block, When::Empty, link), "a fill behind outlines")?;
    let fill_name = ctx.world.catalog().name(block.ty());
    let short: Vec<String> = counts.iter().enumerate().filter(|(_, &n)| (n as u64) < each).map(|(i, n)| format!("outline {} only {n}", i + 1)).collect();
    out.push_str(&format!("Filled behind {} {} with {fill_name}: {} in all. {}", groups.len(), plural(groups.len(), "outline"), taken.len(), describe_change(&change)));
    if short.is_empty() {
        out.push_str(&format!(" Each outline got {each}."));
    } else {
        out.push_str(&format!(" There was not room inside the hull for {each} behind every outline: {}.", short.join(", ")));
    }
    let blocks_now = entity_info(ctx, entity)?.blocks;
    out.push_str(&format!(" The entity now has {blocks_now} {}.", plural(blocks_now as usize, "block")));
    Ok(Done { text: out, changed: change.cells.len() })
}

/// The layer of a side where a block's outlines stand apart best.
struct Outlines {
    /// Layers in from the side.
    index: usize,
    /// Closed rings, then groups.
    score: (usize, usize),
    groups: Vec<super::inspect::Group>,
    /// Groups left out because others close round a hole and these do not.
    others: usize,
}

/// The layer with the most closed rings, or failing those the most
/// separate groups; the outermost of equals.
fn best_layer(layers: Vec<BTreeSet<Pos>>, axis: usize, min_size: u64) -> Option<Outlines> {
    let mut best: Option<Outlines> = None;
    for (index, cells) in layers.into_iter().enumerate() {
        let mut groups: Vec<(bool, super::inspect::Group)> = super::inspect::groups_of(cells.clone())
            .into_iter()
            .filter(|g| g.count >= min_size)
            .map(|g| (encloses(&cells, &g, axis), g))
            .collect();
        let rings = groups.iter().filter(|(ring, _)| *ring).count();
        let score = (rings, groups.len());
        if !groups.is_empty() && best.as_ref().is_none_or(|b| score > b.score) {
            // With rings found, the rest of the layer is trim, not outlines.
            let others = if rings > 0 { groups.len() - rings } else { 0 };
            if rings > 0 {
                groups.retain(|(ring, _)| *ring);
            }
            best = Some(Outlines { index, score, groups: groups.into_iter().map(|(_, g)| g).collect(), others });
        }
    }
    best
}

/// Whether a group lying in one layer across `axis` closes round a hole:
/// cells of its bounds that the group walls off from the bounds' edge.
fn encloses(cells: &BTreeSet<Pos>, group: &super::inspect::Group, axis: usize) -> bool {
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let (a, b) = (group.min.to_array(), group.max.to_array());
    let (w, h) = ((b[u] - a[u] + 1) as usize, (b[v] - a[v] + 1) as usize);
    if w < 3 || h < 3 {
        return false;
    }
    let wall = |i: usize, j: usize| {
        let mut c = a;
        c[u] += i as i32;
        c[v] += j as i32;
        cells.contains(&Pos::new(c[0], c[1], c[2]))
    };
    let mut seen = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for i in 0..w {
        stack.extend([(i, 0), (i, h - 1)]);
    }
    for j in 0..h {
        stack.extend([(0, j), (w - 1, j)]);
    }
    while let Some((i, j)) = stack.pop() {
        if seen[i + w * j] || wall(i, j) {
            continue;
        }
        seen[i + w * j] = true;
        if i > 0 {
            stack.push((i - 1, j));
        }
        if i + 1 < w {
            stack.push((i + 1, j));
        }
        if j > 0 {
            stack.push((i, j - 1));
        }
        if j + 1 < h {
            stack.push((i, j + 1));
        }
    }
    (0..w).any(|i| (0..h).any(|j| !seen[i + w * j] && !wall(i, j)))
}

/// Up to `wanted` enclosed cells of a box that `taken` does not hold:
/// whole layers from one side, then as much of the next as is still
/// wanted, nearest its middle first.
fn packed(inside: &Interior, lo: Pos, hi: Pos, axis: usize, from_high: bool, wanted: u64, taken: &BTreeSet<Pos>) -> BTreeSet<Pos> {
    let mut layers = inside.layers(lo, hi, axis);
    if from_high {
        layers.reverse();
    }
    let mut cells: BTreeSet<Pos> = BTreeSet::new();
    for layer in &layers {
        let left = wanted - cells.len() as u64;
        if left == 0 {
            break;
        }
        let mut found: Vec<Pos> = inside.cells_in(layer.min, layer.max).into_iter().filter(|p| !taken.contains(p)).collect();
        if (found.len() as u64) > left {
            let middle = |a: usize| (layer.min.to_array()[a] + layer.max.to_array()[a]) as i64;
            let off = |p: &Pos| (0..3).filter(|&a| a != axis).map(|a| (2 * p.to_array()[a] as i64 - middle(a)).pow(2)).sum::<i64>();
            found.sort_by_key(off);
            found.truncate(left as usize);
        }
        cells.extend(found);
    }
    cells
}

fn fill_inside(ctx: &mut Ctx<'_>, args: &Args<'_>) -> Result<Done, String> {
    let entity = args.entity();
    let info = entity_info(ctx, entity)?;
    let Some((lo, hi)) = box_of(ctx, args, entity)? else { return Err("The entity is empty: there is no hull to fill.".to_string()) };
    let block = blocks::resolve(ctx.world.catalog(), args, "block", true)?;
    if block.is_air() {
        return Err("fill_inside places blocks; to clear space use remove or fill with block \"air\".".to_string());
    }
    let Some(inside) = interior(ctx, entity, args.int("seal").map(|s| s as u32))? else { return Err("The entity is empty: there is no hull to fill.".to_string()) };
    let side = args.text("start_from").unwrap_or("back");
    let &(_, axis, from_high) = SIDES.iter().find(|(name, _, _)| *name == side).unwrap_or(&SIDES[0]);
    let room: u64 = inside.layers(lo, hi, axis).iter().map(|l| l.volume).sum();
    if room == 0 {
        let nearby = room_nearby(ctx, &inside, entity, lo, hi)?;
        return Ok(Done::said(format!("There is no empty space enclosed by the hull in {}..{}; nothing was placed.{nearby}{}", show(lo), show(hi), sealed(&inside))));
    }
    let wanted = args.int("max_blocks").map_or(room, |m| (m as u64).min(room));
    check_volume(wanted)?;
    let cells = packed(&inside, lo, hi, axis, from_high, wanted, &BTreeSet::new());
    let link = core_link(&rules_for(ctx, &info), block.ty());
    let change = apply(ctx, entity, fills(&cells, block, When::Empty, link), "a fill inside the hull")?;
    let (mut a, mut b) = (Pos::new(i32::MAX, i32::MAX, i32::MAX), Pos::new(i32::MIN, i32::MIN, i32::MIN));
    for p in &cells {
        (a, b) = (a.lo(*p), b.hi(*p));
    }
    let name = ctx.world.catalog().name(block.ty());
    let mut text = format!("Filled {} enclosed empty {} with {name}, from {} to {}", cells.len(), plural(cells.len(), "cell"), show(a), show(b));
    if args.has("max_blocks") {
        text.push_str(&format!(", packed against the {side}"));
    }
    text.push_str(&format!(". {}", describe_change(&change)));
    if (cells.len() as u64) < args.int("max_blocks").unwrap_or(0) as u64 {
        text.push_str(&format!(" That is all the enclosed room in {}..{}; there was not space for {}.", show(lo), show(hi), args.int("max_blocks").unwrap_or(0)));
    }
    let blocks_now = entity_info(ctx, entity)?.blocks;
    text.push_str(&format!(" The entity now has {blocks_now} {}.{}", plural(blocks_now as usize, "block"), sealed(&inside)));
    Ok(Done { text, changed: change.cells.len() })
}

#[cfg(test)]
mod tests {
    use super::super::testing::Bench;
    use serde_json::json;
    use yard_core::Pos;

    const GREY: u16 = 598;

    /// A closed box hull 11 wide, 7 high and 41 long behind the core.
    fn hulled() -> Bench {
        let mut bench = Bench::new();
        bench.ok("fill", json!({"from": [-5, -3, -40], "to": [5, 3, 0], "block": "Grey Basic Armor", "mode": "hollow"}));
        bench
    }

    fn thrusters(bench: &Bench) -> u32 {
        let id = crate::world::World::catalog(&bench.world).by_name("Thruster Module").map(|d| d.id).unwrap();
        bench.count(id)
    }

    #[test]
    fn inside_space_tells_the_room_inside_the_hull_from_the_space_around_it() {
        let mut bench = hulled();
        let text = bench.ok("inside_space", json!({}));
        // 9 x 5 x 39 inside the walls; the core is part of the front wall.
        assert!(text.starts_with("Inside the hull of \"\": 1755 empty cells enclosed. 1 separate space, largest first:\n- 1755 cells: x -4..4, y -2..2, z -39..-1"), "{text}");
        assert!(text.contains("- z -39..-36: 180 cells, x -4..4, y -2..2\n- z -35..-31: 225 cells"), "{text}");
        let stern = bench.ok("inside_space", json!({"from": [-9, -9, -60], "to": [9, 9, -30]}));
        assert!(stern.starts_with("In (-9, -9, -60)..(9, 9, -30): 450 empty cells enclosed by the hull."), "{stern}");
        let open = Bench::new();
        let mut open = open;
        open.ok("fill", json!({"from": [-5, -3, -40], "to": [5, -3, 0], "block": "Grey Basic Armor"}));
        assert!(open.ok("inside_space", json!({})).contains("enclose no empty space"));
    }

    #[test]
    fn fill_inside_packs_a_number_of_blocks_against_one_side_and_never_touches_the_hull() {
        let mut bench = hulled();
        let hull_blocks = bench.count(GREY);
        let text = bench.ok("fill_inside", json!({"block": "Thruster Module", "max_blocks": 100}));
        assert!(text.starts_with("Filled 100 enclosed empty cells with Thruster Module, from (-4, -2, -39) to (4, 2, -37), packed against the back. 100 blocks placed."), "{text}");
        assert_eq!(thrusters(&bench), 100);
        assert_eq!(bench.count(GREY), hull_blocks);
        // Two layers of 45 are full, the third filled from its middle.
        assert!(bench.ty(4, 2, -39) != 0 && bench.ty(4, 2, -38) != 0 && bench.ty(0, 0, -37) != 0 && bench.ty(4, 2, -37) == 0);
        // More than there is room for fills what there is.
        let all = bench.ok("fill_inside", json!({"block": "Thruster Module", "from": [-9, -9, -39], "to": [9, 9, -36], "max_blocks": 5000}));
        assert!(all.starts_with("Filled 80 enclosed") && all.contains("there was not space for 5000"), "{all}");
        let front = bench.ok("fill_inside", json!({"block": "Thruster Module", "max_blocks": 45, "start_from": "front"}));
        assert!(front.contains("from (-4, -2, -1) to (4, 2, -1)"), "{front}");
        assert_eq!(bench.ty(0, 0, 0), yard_core::edit::CORE_TYPE);
        let beside = bench.ok("fill_inside", json!({"block": "Thruster Module", "from": [20, 0, -20], "to": [30, 5, -10]}));
        assert!(beside.contains("nothing was placed. Between z -20 and -10 the room inside the hull is at x -4..4, y -2..2, z -20..-10 (495 cells)"), "{beside}");
        assert!(bench.ok("fill_inside", json!({"block": "Thruster Module", "from": [0, 0, 20], "to": [5, 5, 30]})).contains("There is no room inside the hull anywhere between z 20 and 30."));
        assert!(bench.err("fill_inside", json!({"block": "Thruster Module", "from": [0, 0, -20]})).contains("two opposite corners of a box"));
    }

    #[test]
    fn only_where_inside_keeps_shapes_to_the_enclosed_space() {
        let mut bench = hulled();
        let hull_blocks = bench.count(GREY);
        // A cylinder wider than the hull and longer than it.
        let text = bench.ok("cylinder", json!({"block": "Thruster Module", "from": [0, 0, -50], "axis": "z", "length": 15, "radius": 8, "only_where": "inside"}));
        assert!(text.contains("Only the enclosed empty cells were filled."), "{text}");
        assert_eq!(bench.count(GREY), hull_blocks);
        assert_eq!(thrusters(&bench), 9 * 5 * 4);
        assert_eq!(bench.ty(0, 0, -41), 0);
        let boxed = bench.ok("fill", json!({"block": "Thruster Module", "from": [-20, -20, -30], "to": [20, 20, -30], "only_where": "inside"}));
        assert!(boxed.contains("45 blocks placed"), "{boxed}");
        assert_eq!(bench.count(GREY), hull_blocks);
        assert!(bench.err("place_blocks", json!({"blocks": [{"pos": [0, 0, -5], "block": "Thruster Module"}], "only_where": "inside"})).contains("fill_inside"));
    }

    #[test]
    fn find_outlines_finds_marks_on_a_side_and_the_box_behind_each() {
        let mut bench = hulled();
        // Two red rings standing a layer proud of the back wall, joined to
        // each other by red trim in the wall.
        for (x0, x1) in [(-4, -1), (1, 4)] {
            bench.ok("fill", json!({"from": [x0, -2, -41], "to": [x1, 1, -41], "block": "Red Basic Armor", "mode": "hollow"}));
            bench.ok("fill", json!({"from": [x0 + 1, -1, -41], "to": [x1 - 1, 0, -41], "block": "air"}));
        }
        bench.ok("fill", json!({"from": [-5, 1, -40], "to": [5, 1, -40], "block": "Red Basic Armor"}));
        let text = bench.ok("find_outlines", json!({"block": "Red Basic Armor"}));
        assert!(text.starts_with("2 closed outlines of Red Basic Armor on the back, where they stand apart best at z = -41, largest first:"), "{text}");
        assert!(text.contains("- 12 blocks from (-4, -2, -41) to (-1, 1, -41) (4 x 4 x 1), centre (-3, -1, -41); box behind it: from [-4, -2, -41] to [-1, 1, 0]"), "{text}");
        assert!(text.contains("call find_outlines again with fill_with and blocks_each"), "{text}");
        let filled = bench.ok("fill_inside", json!({"block": "Thruster Module", "from": [-4, -2, -41], "to": [-1, 1, 0], "max_blocks": 20}));
        assert!(filled.contains("from (-4, -2, -39) to (-1, 1, -38)"), "{filled}");
        bench.ok("undo", json!({}));
        // Filling behind every outline in one call: the same blocks behind each.
        let both = bench.ok("find_outlines", json!({"block": "Red Basic Armor", "fill_with": "Thruster Module", "blocks_each": 30}));
        assert!(both.contains("Filled behind 2 outlines with Thruster Module: 60 in all. 60 blocks placed. Each outline got 30."), "{both}");
        assert_eq!(thrusters(&bench), 60);
        assert!(bench.ty(-4, -2, -39) != 0 && bench.ty(4, -2, -39) != 0 && bench.ty(0, -2, -39) == 0, "behind the rings, not between them");
        let full = bench.ok("find_outlines", json!({"block": "Red Basic Armor", "fill_with": "Thruster Module", "blocks_each": 1000}));
        assert!(full.contains("There was not room inside the hull for 1000 behind every outline"), "{full}");
        assert!(bench.ok("find_outlines", json!({"block": "Red Basic Armor", "side": "front", "depth": 5})).starts_with("No outlines"));
        // "basic armor" fits grey and red; the one the searched part of the
        // design holds is meant.
        let red = bench.ok("find_groups", json!({"block": "basic armor", "from": [-9, -9, -41], "to": [9, 9, -41]}));
        assert!(red.starts_with("\"basic armor\" taken as Red Basic Armor, the one the design holds most of.\n24 Red Basic Armor blocks"), "{red}");
        let grey = bench.ok("find_groups", json!({"block": "basic armor"}));
        assert!(grey.starts_with("\"basic armor\" taken as Grey Basic Armor, the one the design holds most of (the design also has Red Basic Armor)."), "{grey}");
        assert!(bench.err("find_outlines", json!({"block": "pink"})).contains("pink"));
        // Asked for a block the outline is not made of, it finds the one of
        // the same first word that is.
        let mut other = hulled();
        other.ok("fill", json!({"from": [-4, -2, -41], "to": [-1, 1, -41], "block": "Reactor Stabilizer", "mode": "hollow"}));
        other.ok("fill", json!({"from": [-3, -1, -41], "to": [-2, 0, -41], "block": "air"}));
        let stabilizer = other.ok("find_outlines", json!({"block": "Reactor Power"}));
        assert!(stabilizer.starts_with("There are no outlines of Reactor Power; Reactor Stabilizer has some, so these are of it.\n1 closed outline of Reactor Stabilizer"), "{stabilizer}");
        assert!(other.ok("find_outlines", json!({"block": "Cannon Barrel"})).starts_with("No outlines of Cannon Barrel"));
    }

    #[test]
    fn get_block_and_scan_region_tell_inside_from_outside() {
        let mut bench = hulled();
        assert!(bench.ok("get_block", json!({"pos": [0, 0, -20]})).starts_with("(0, 0, -20): empty, inside the hull."));
        assert!(bench.ok("get_block", json!({"pos": [0, 0, -45]})).starts_with("(0, 0, -45): empty, outside the hull."));
        let scan = bench.ok("scan_region", json!({"from": [-6, 0, -3], "to": [6, 0, -1], "axis": "y"}));
        assert!(scan.contains(": empty inside the hull"), "{scan}");
        assert!(scan.contains(".#:::::::::#."), "{scan}");
        assert_eq!(bench.world.block("", Pos::new(0, 0, 0)).ty(), yard_core::edit::CORE_TYPE);
    }
}
