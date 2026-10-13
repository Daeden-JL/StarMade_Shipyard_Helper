//! The build panel (tools, rotation and the block picker) and the floating
//! panel of brush size and mirror planes.

use crate::entity_label;
use crate::work::{self, BrushShape, Ctx, Grab, PaintMode, Tool, MAX_BRUSH};
use leptos::prelude::*;
use std::collections::BTreeSet;
use yard_core::api::ClientBlock;
use yard_core::catalog::Shape;
use yard_core::edit;

/// Most blocks listed at once; the search narrows it down.
const LIST_LIMIT: usize = 200;

/// The category shown in the filter: the first two levels of the game's own tree.
/// The sides a core's nose or top can point to, as numbered in
/// [`yard_core::shape::SIDE_DIRS`], in words.
const CORE_SIDES: [(usize, &str); 6] =
    [(0, "Front (+Z)"), (1, "Back (-Z)"), (2, "Up (+Y)"), (3, "Down (-Y)"), (4, "Side +X"), (5, "Side -X")];

fn group(b: &ClientBlock) -> String {
    b.category.split('/').take(2).collect::<Vec<_>>().join(" / ")
}

/// Whether a block gets a row in the list: placeable, and not a wedge or slab
/// of another block (those are reached through the shape buttons).
fn listed(b: &ClientBlock) -> bool {
    b.placeable && b.base == 0 && b.id != edit::CORE_TYPE
}

fn shape_label(b: &ClientBlock) -> &'static str {
    match (b.shape, b.slab) {
        (Shape::Cube, 1) => "3/4",
        (Shape::Cube, 2) => "1/2",
        (Shape::Cube, 3) => "1/4",
        (Shape::Wedge, _) => "Wedge",
        (Shape::Corner, _) => "Corner",
        (Shape::Tetra, _) => "Tetra",
        (Shape::Hepta, _) => "Hepta",
        (Shape::Sprite, _) => "Sprite",
        _ => "Block",
    }
}

/// Style for a block's swatch: its texture tile cut out of the game's sheet,
/// over its flat colour for when the sheet has not loaded.
fn swatch(b: &ClientBlock) -> String {
    let colour = format!("rgb({}, {}, {})", b.color[0], b.color[1], b.color[2]);
    // Blocks with three textures keep their side on slot 5; the rest lead with slot 0.
    let tile = if b.sides == 3 { b.textures[5] } else { b.textures[0] };
    if tile < 0 {
        return format!("background: {colour}");
    }
    let (page, cell) = (tile / 256, tile % 256);
    // Sixteen tiles a row: position n of 15 steps, sheet scaled to 16 swatches.
    let (x, y) = ((cell % 16) as f32 / 15.0 * 100.0, (cell / 16) as f32 / 15.0 * 100.0);
    format!("background: {colour} url(/api/textures/{page}) {x:.3}% {y:.3}% / 1600% 1600%")
}

#[component]
pub fn BuildPanel(ctx: Ctx) -> impl IntoView {
    let search = RwSignal::new(String::new());
    let category = RwSignal::new(String::new());

    let groups = move || {
        ctx.catalog.with(|blocks| blocks.iter().filter(|b| listed(b)).map(group).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>())
    };

    // (id, name, swatch style) of the blocks that match, and how many more there are.
    let matches = move || {
        let needle = search.get().trim().to_lowercase();
        let wanted = category.get();
        ctx.catalog.with(|blocks| {
            let mut found: Vec<(u16, String, String)> = blocks
                .iter()
                .filter(|b| listed(b))
                .filter(|b| wanted.is_empty() || group(b) == wanted)
                .filter(|b| needle.is_empty() || b.name.to_lowercase().contains(&needle))
                .map(|b| (b.id, b.name.clone(), swatch(b)))
                .collect();
            found.sort_by(|a, b| a.1.cmp(&b.1));
            let more = found.len().saturating_sub(LIST_LIMIT);
            found.truncate(LIST_LIMIT);
            (found, more)
        })
    };

    // The selected block and the other shapes of the same material.
    let selected = move || {
        let id = ctx.block_id.get();
        ctx.catalog.with(|blocks| blocks.iter().find(|b| b.id == id).cloned())
    };
    let family = move || {
        let Some(current) = selected() else { return Vec::new() };
        let base = if current.base != 0 { current.base } else { current.id };
        ctx.catalog.with(|blocks| {
            let mut out: Vec<(u16, &'static str, (u8, Shape))> = blocks
                .iter()
                .filter(|b| b.placeable && (b.id == base || b.base == base))
                .map(|b| (b.id, shape_label(b), (b.slab, b.shape)))
                .collect();
            // Full block first, then the slabs, then wedge, corner, tetra, hepta.
            out.sort_by_key(|(_, _, (slab, shape))| (*shape != Shape::Cube, *shape, *slab));
            out
        })
    };

    let tool_button = move |tool: Tool, label: &'static str, key: &'static str| {
        view! {
            <button class:current=move || ctx.tool.get() == tool
                    title=move || format!("{} (key {key})", tool.help(ctx.swap.get()))
                    on:click=move |_| ctx.tool.set(tool)>
                {label}
                <kbd>{key}</kbd>
            </button>
        }
    };

    let turn_text = move || {
        let id = ctx.block_id.get();
        let (turn, n) = (ctx.turn.get(), ctx.turns(id));
        let turns_24_ways = ctx.catalog.with(|blocks| blocks.iter().any(|b| b.id == id && b.shape == Shape::Rail));
        if turns_24_ways {
            let name = |d: [i32; 3]| match d {
                [0, 1, 0] => "up",
                [0, -1, 0] => "down",
                [0, 0, 1] => "forward",
                [0, 0, -1] => "back",
                [1, 0, 0] => "+X",
                _ => "-X",
            };
            let (up, forward) = yard_core::shape::turned_axes(turn);
            format!("Faces {}, points {}", name(up), name(forward))
        } else if n <= 1 {
            "No rotation".to_string()
        } else if n == 7 && turn == 0 {
            "Faces out from the surface".to_string()
        } else if n == 7 {
            ["Front", "Back", "Up", "Down", "Toward -X", "Toward +X"][(turn as usize - 1) % 6].to_string()
        } else {
            format!("Rotation {} of {}", turn % n + 1, n)
        }
    };

    view! {
        <aside class="build">
            <h2>"Tool"</h2>
            <div class="tools">
                {tool_button(Tool::Pick, "Pick", "1")}
                {tool_button(Tool::Paint, "Paint", "2")}
                {tool_button(Tool::Build, "Place / Remove", "3")}
                {tool_button(Tool::Link, "Link", "4")}
                {tool_button(Tool::Use, "Use", "5")}
                {tool_button(Tool::Smooth, "Smooth", "6")}
                {tool_button(Tool::Turret, "Make turret", "7")}
                {tool_button(Tool::Core, "Move core", "8")}
                {tool_button(Tool::Clone, "Clone", "9")}
            </div>
            <Show when=move || ctx.simulating.get()>
                <div class="trying">
                    <p>"Trying the design out: buttons, switches, doors and lights work as in the game. None of it is saved."</p>
                    <button on:click=move |_| ctx.stop_sim()>"Put back as saved"</button>
                </div>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Use>
                <Switches ctx/>
            </Show>
            <Show when=move || ctx.structure_undo.get().is_some()>
                <div class="trying">
                    <p>{move || format!("{}. Undo cannot follow this; this button can, until the next edit.", ctx.structure_undo.get().unwrap_or_default())}</p>
                    <button on:click=move |_| ctx.restore_structure()>"Put back as it was"</button>
                </div>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Pick>
                <h2>"Turn all of a type"</h2>
                <p class="empty">
                    {move || match ctx.picked.get() {
                        Some(_) => format!("Turn the picked {} with the rotation arrows below until it faces the way you want, then this turns every {} in the entity on screen the same way.", ctx.block_name(ctx.block_id.get()), ctx.block_name(ctx.block_id.get())),
                        None => "Click a block in the view to pick it. Turn it with the rotation arrows below, then this turns every block of its type the same way.".to_string(),
                    }}
                </p>
                <button disabled=move || ctx.picked.get().is_none() || ctx.turns(ctx.block_id.get()) <= 1
                        on:click=move |_| ctx.turn_all_like_picked()>"Turn all of this type"</button>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Build>
                <h2>"Place / Remove"</h2>
                <p class="empty">
                    {move || match ctx.highlight.get() {
                        Some(ty) => format!("Removes every {} from the entity on screen. Undo puts them back.", ctx.block_name(ty)),
                        None => "Click a block in the list on the left, then this removes every block of that type from the entity on screen.".to_string(),
                    }}
                </p>
                <button disabled=move || ctx.highlight.get().is_none_or(|ty| ty == edit::CORE_TYPE)
                        on:click=move |_| ctx.remove_marked_type()>"Remove block type"</button>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Turret>
                <h2>"Make turret"</h2>
                <p class="empty">
                    "Click two blocks to mark opposite corners of the part to cut out (blocks inside the box, empty space included). It becomes its own docked entity on a turret axis, so it can turn. W A S D E Q move the corner in hand; click any corner of the box, or press Tab, to take it and adjust the box."
                </p>
                <label>"The side of the box that rests on the ship"
                    <select on:change=move |ev| { ctx.turret_base.set(event_target_value(&ev).parse().unwrap_or(3)); ctx.turret_base_changed(); crate::let_go_of_focus(); }>
                        {[(3u8, "Bottom"), (2, "Top"), (0, "Front (+Z)"), (1, "Back (-Z)"), (4, "Side +X"), (5, "Side -X")]
                            .into_iter()
                            .map(|(side, name)| view! { <option value=side.to_string() selected=move || ctx.turret_base.get() == side>{name}</option> })
                            .collect_view()}
                    </select>
                </label>
                <Show when=move || matches!(ctx.turret_box.get(), Some((_, Some(_))))>
                    <div class="row">
                        <button class=move || if ctx.turret_grab.get() == Grab::Docker { "current" } else { "" }
                                title="Then W A S D E Q move it along the face that rests on the ship"
                                on:click=move |_| ctx.grab_turret(Grab::Docker)>"Move the rail docker"</button>
                        {move || match ctx.turret_grab.get() {
                            Grab::Nothing => "Nothing in hand".to_string(),
                            Grab::Docker => "Docker in hand".to_string(),
                            Grab::Corner(_) => "Corner in hand".to_string(),
                        }}
                    </div>
                </Show>
                <Show when=move || matches!(ctx.turret_box.get(), Some((_, Some(_))))>
                    <p class="empty">
                        "The blocks that go with the turret are tinted orange. Right-click blocks in the box to leave them on the ship (red): as many as the brush size below takes, a ridge of the hull, say. Right-click a red one to take those in the brush back. Shift+right-click takes the whole section the block is part of: every block touching it (at a face, an edge or a corner) and those touching them, as far as air or the edge of the box."
                    </p>
                    <Show when=move || { ctx.turret_left_out_count() != 0 }>
                        <div class="row">
                            <span class="empty">{move || match ctx.turret_left_out_count() { 1 => "1 block left out".to_string(), n => format!("{n} blocks left out") }}</span>
                            <button on:click=move |_| ctx.include_all_turret()>"Take them all back in"</button>
                        </div>
                    </Show>
                </Show>
                <p class="empty">
                    "Yellow is the rail docker, on the face that rests on the ship; the turret axis goes in the blue cell outside it and replaces what is there, and the new core takes the green cell beyond the docker. Links that cross the box edge are cut."
                </p>
                <div class="row">
                    <button class="current" disabled=move || !matches!(ctx.turret_box.get(), Some((_, Some(_)))) on:click=move |_| ctx.make_turret()>"Make turret"</button>
                    <button disabled=move || ctx.turret_box.get().is_none() on:click=move |_| ctx.clear_turret()>"Clear"</button>
                </div>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Core>
                <h2>"Move core"</h2>
                <Show when=move || ctx.relocating.get().is_none()
                      fallback=move || view! {
                          <p class="empty">"Choosing the new place for the core: click a block (Shift-click for the space in front of it) or use W A S D E Q. The whole design moves round it."</p>
                          <Show when=move || !ctx.rails_to_move.get().is_empty()>
                              <label>"Move"
                                  <select on:change=move |ev| { ctx.choose_rail(event_target_value(&ev).parse().ok()); crate::let_go_of_focus(); }>
                                      <option value="" selected=move || ctx.rail_move.get().is_none()>"The core"</option>
                                      {move || ctx.rails_to_move.get().into_iter().enumerate().map(|(i, (child, own, label))| {
                                          view! { <option value=i.to_string() selected=move || ctx.rail_move.get().is_some_and(|r| r.child == child && r.own == own)>{label}</option> }
                                      }).collect_view()}
                                  </select>
                              </label>
                          </Show>
                          <Show when=move || ctx.rail_move.get().is_some()>
                              <label class="check" title="With it, the docked entity moves with its rail, docked as it was. Without, it stays where it is and its rail docker moves within it to meet the rail.">
                                  <input type="checkbox" prop:checked=move || ctx.rail_move.get().is_some_and(|r| r.carried)
                                         on:change=move |ev| ctx.set_rail_carried(event_target_checked(&ev))/>
                                  "What hangs from it comes along"
                              </label>
                              <p class="empty">"Choose the rail's new cell as for the core: click a block, Shift-click the space in front of it, or use W A S D E Q. For the rail this entity rests on, the cell is the one of this entity that is to rest on it. The orange box is where the rail (or this entity's docker) is now."</p>
                          </Show>
                          <Show when=move || ctx.rail_move.get().is_none()>
                          <p class="empty">"The green arrow is the way the core's nose will point, which is the way the ship flies; its short stalk is the top. As it is, the nose points to the front and the top up. If the arrow points the wrong way, turn it here: the design turns round the core to match."</p>
                          <label>"Nose points"
                              <select on:change=move |ev| { ctx.set_core_facing(event_target_value(&ev).parse().unwrap_or(0), ctx.core_facing.get_untracked().1); crate::let_go_of_focus(); }>
                                  {CORE_SIDES
                                      .into_iter()
                                      .map(|(side, name)| view! { <option value=side.to_string() selected=move || ctx.core_facing.get().0 == side>{name}</option> })
                                      .collect_view()}
                              </select>
                          </label>
                          <label>"Top points"
                              <select on:change=move |ev| { ctx.set_core_facing(ctx.core_facing.get_untracked().0, event_target_value(&ev).parse().unwrap_or(2)); crate::let_go_of_focus(); }>
                                  {move || {
                                      let (nose, top) = ctx.core_facing.get();
                                      CORE_SIDES
                                          .into_iter()
                                          .filter(|(side, _)| side / 2 != nose / 2)
                                          .map(|(side, name)| view! { <option value=side.to_string() selected=side == top>{name}</option> })
                                          .collect_view()
                                  }}
                              </select>
                          </label>
                          <Show when=move || ctx.turret_parts.get().is_some()>
                              <label class="check" title="The rail docker, the turret axis it rests on and the axes of the turrets docked to this one shift by as much as the core, so the turret keeps its place and turns about the new line. Unticked, the core alone swaps with the block in its new cell, and the turret shifts on its docker.">
                                  <input type="checkbox" prop:checked=move || ctx.dock_follows.get()
                                         on:change=move |ev| ctx.set_dock_follows(event_target_checked(&ev))/>
                                  "The dock goes with the core"
                              </label>
                              <p class="empty">
                                  {move || match ctx.turret_parts.get() {
                                      Some(0) => "This is a turret. With the dock going along, the rail docker under the core and the turret axis it rests on move by as much as the core: the turret keeps its place and turns about the new line.".to_string(),
                                      Some(n) => format!("This is a turret with {} docked on its turret {}. With the dock going along, the rail docker under the core, the axis it rests on and the {} the {} hang{} from, with {} docker{}, move by as much as the core: everything keeps its place and turns about the new line.",
                                          if n == 1 { "one entity".to_string() } else { format!("{n} entities") }, if n == 1 { "axis" } else { "axes" }, if n == 1 { "axis" } else { "axes" }, if n == 1 { "entity" } else { "entities" }, if n == 1 { "s" } else { "" }, if n == 1 { "its" } else { "their" }, if n == 1 { "" } else { "s" }),
                                      None => String::new(),
                                  }}
                              </p>
                              {move || ctx.turret_fixed.get().then(|| view! {
                                  <small class="warning">"Its rail docker faces front or back on this entity. The game only turns a turret whose docker faces up, down or sideways, so this one is held still wherever the core goes; Auto-fit has nothing to fit. For it to turn, the docker must face up, down or sideways."</small>
                              })}
                          </Show>
                          <Show when=move || ctx.turret_parts.get().is_some()>
                              <button title="For this turret and each section docked to it, tries every cell of the core's row and takes the one where it turns furthest before meeting the ship or another part, with the dock following and no block left floating outside the structure; then previews the moves" on:click=move |_| ctx.fit_relocation()>"Auto-fit for the widest sweep"</button>
                          </Show>
                          </Show>
                          <div class="row">
                              <button class="current" on:click=move |_| ctx.confirm_relocation()>"Confirm"</button>
                              <button title="Shows a copy of the design with the core moved, in the whole-design view with the turrets in motion. Nothing is saved until Confirm." on:click=move |_| ctx.preview_relocation()>"Preview"</button>
                              <button on:click=move |_| ctx.cancel_relocation()>"Cancel"</button>
                          </div>
                      }>
                    <p class="empty">"Click a block to choose another cell for the core; the whole design moves round it. On a turret, the rail docker and turret axis go with the core, and Preview shows the turret turning about its new line."</p>
                    <button on:click=move |_| ctx.start_relocation()>"Choose a new place"</button>
                </Show>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Clone>
                <h2>"Clone"</h2>
                {move || {
                    let path = ctx.entity_path.get();
                    match ctx.cloning.get() {
                        Some(job) if job.replace.is_some() => view! {
                            <p class="empty">{format!("A copy of {} in place of {}, which goes with everything docked to it; the copy hangs from the rail it hung from. Confirm makes it.", entity_label(&job.source).to_lowercase(), entity_label(job.replace.as_deref().unwrap_or_default()).to_lowercase())}</p>
                            <div class="row">
                                <button class="current" on:click=move |_| ctx.confirm_relocation()>"Confirm"</button>
                                <button on:click=move |_| ctx.cancel_relocation()>"Cancel"</button>
                            </div>
                        }.into_any(),
                        Some(job) => view! {
                            <p class="empty">{format!("Choosing where the copy of {} goes on {}: click a block to make it the rail (the copy rests on the face clicked), or Shift-click the space in front of it; W A S D E Q move it. The orange box is the original's rail; the cell in front of the new rail must be empty for the copy's rail docker.", entity_label(&job.source).to_lowercase(), entity_label(&job.parent).to_lowercase())}</p>
                            <label>"The copy rests on the rail's"
                                <select on:change=move |ev| { ctx.set_clone_facing(event_target_value(&ev).parse().unwrap_or(2)); crate::let_go_of_focus(); }>
                                    {CORE_SIDES
                                        .into_iter()
                                        .map(|(side, name)| view! { <option value=side.to_string() selected=move || ctx.cloning.get().is_some_and(|c| c.facing == yard_core::shape::SIDE_DIRS[side])>{name}</option> })
                                        .collect_view()}
                                </select>
                            </label>
                            <div class="row">
                                <button class="current" on:click=move |_| ctx.confirm_relocation()>"Confirm"</button>
                                <button title="Shows a copy of the design with the copy in place, in the whole-design view with the turrets in motion. Nothing is saved until Confirm." on:click=move |_| ctx.preview_relocation()>"Preview"</button>
                                <button on:click=move |_| ctx.cancel_relocation()>"Cancel"</button>
                            </div>
                        }.into_any(),
                        None if path == work::WHOLE || path.is_empty() => view! {
                            <p class="empty">"Choose a docked entity on the left (a turret, say). It can be copied in place of another docked entity, or onto a new rail on the entity it hangs from."</p>
                        }.into_any(),
                        None => {
                            let targets = ctx.clone_targets();
                            let parent = entity_label(&path.rsplit_once('/').map_or(String::new(), |(p, _)| p.to_string())).to_lowercase();
                            view! {
                                <p class="empty">{format!("Copies {}: the same blocks, links, docked entities and AI settings, under a new name.", entity_label(&path).to_lowercase())}</p>
                                <label>"In place of"
                                    <select on:change=move |ev| { ctx.clone_target.set(event_target_value(&ev)); crate::let_go_of_focus(); }>
                                        <option value="" selected=move || ctx.clone_target.get().is_empty()>"Choose a docked entity..."</option>
                                        {targets.into_iter().map(|(p, label)| { let value = p.clone(); view! { <option value=p selected=move || ctx.clone_target.get() == value>{label}</option> } }).collect_view()}
                                    </select>
                                </label>
                                <button disabled=move || ctx.clone_target.get().is_empty() on:click=move |_| ctx.clone_in_place_of(ctx.clone_target.get_untracked())>"Preview the copy in its place"</button>
                                <p class="empty">"The entity replaced is removed with everything docked to it, and the copy hangs from the rail it hung from."</p>
                                <button on:click=move |_| ctx.start_cloning()>{format!("Put a copy on a new rail: choose the cell on {parent}")}</button>
                                <p class="empty">"The new rail is of the kind this entity hangs from (a turret axis for a turret), placed where you click; the copy rests on the face clicked."</p>
                            }.into_any()
                        }
                    }
                }}
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Smooth>
                <h2>"Smooth"</h2>
                <label>
                    {move || format!("Bevel the edges of steps up to {} high", match ctx.smooth_step.get() { 1 => "1 block".to_string(), n => format!("{n} blocks") })}
                    <input type="range" min="1" max="4" step="1" prop:value=move || ctx.smooth_step.get().to_string()
                           on:input=move |ev| ctx.smooth_step.set(event_target_value(&ev).parse().unwrap_or(1))/>
                </label>
                <label class="check" title="Rooms sealed inside the hull are smoothed too, and the notches on the inner face of a hull one block thick get wedges">
                    <input type="checkbox" prop:checked=move || ctx.smooth_inside.get()
                           on:change=move |ev| ctx.smooth_inside.set(event_target_checked(&ev))/>
                    "Smooth the inside too"
                </label>
                <label class="check" title="Where a slope (one a pass makes, or wedges placed by hand) ends against a whole block, that block is cut the same way, and so on along the edge while both sides of it face open air; where three slopes meet at a tip, the tip becomes a tetra. Off, a slope ends at a square edge unless the next block is a step too.">
                    <input type="checkbox" prop:checked=move || ctx.smooth_sharpen.get()
                           on:change=move |ev| ctx.smooth_sharpen.set(event_target_checked(&ev))/>
                    "Sharpen edges: carry slopes on through the blocks they end against"
                </label>
                <p class="empty">
                    "Click to put wedges on the outer edges of steps inside the brush (set its size below), with corner pieces and heptas where the edges meet. A higher step height bevels taller drops. With the inside too, the inner face of a thin hull gets wedges in its notches, so it slopes like the outer face. Right-click turns wedges, corners, tetras and heptas in the brush back into whole blocks (and takes out the notch wedges)."
                </p>
                <button title="One pass over every block of the entity on screen, at the step height above" on:click=move |_| ctx.smooth_whole()>"Smooth whole entity"</button>
                <p class="empty">"Press again for another pass (a higher step height bevels taller steps); Undo takes a pass back."</p>
                <button title="Turns every wedge, corner, hepta and tetra of hull blocks back into a whole block, takes out the notch wedges, then smooths once, all as one step" on:click=move |_| ctx.resmooth_whole()>"Smooth again from scratch"</button>
                <p class="empty">"A pass leaves blocks that already have a shape alone. To redo a design smoothed before (or by an older version of this tool), smooth it again from scratch. Shapes placed by hand in hull blocks are redone too."</p>
            </Show>
            <Show when=move || ctx.tool.get() == Tool::Paint>
                <h2>"Paint"</h2>
                <div class="shapes">
                    {paint_mode_button(ctx, PaintMode::Single, "Single block")}
                    {paint_mode_button(ctx, PaintMode::Brush, "Brush")}
                    {paint_mode_button(ctx, PaintMode::Fill, "Fill can")}
                    {paint_mode_button(ctx, PaintMode::Swap, "Swap")}
                </div>
                <p class="empty">{move || ctx.paint_mode.get().help()}</p>
                <Show when=move || ctx.paint_mode.get() == PaintMode::Brush>
                    <label class="check" title="Without it, the brush paints only blocks with an open face, so a solid hull keeps its filling. With it, the brush paints every block in its box.">
                        <input type="checkbox" prop:checked=move || ctx.paint_through.get()
                               on:change=move |ev| { ctx.paint_through.set(event_target_checked(&ev)); ctx.refresh_overlay(); ctx.redraw(); }/>
                        "Inside blocks too"
                    </label>
                </Show>
                <Show when=move || ctx.paint_mode.get() == PaintMode::Fill>
                    <label class="check" title="Paints as deep below the surface as the brush size along the face you click (set in the brush panel), instead of the surface only">
                        <input type="checkbox" prop:checked=move || ctx.fill_brush_depth.get() prop:disabled=move || ctx.fill_all.get()
                               on:change=move |ev| { ctx.fill_brush_depth.set(event_target_checked(&ev)); ctx.refresh_overlay(); ctx.redraw(); }/>
                        "Use brush size"
                    </label>
                    <label class="check" title="Paints every touching block of the same material, all the way through a solid part">
                        <input type="checkbox" prop:checked=move || ctx.fill_all.get()
                               on:change=move |ev| { ctx.fill_all.set(event_target_checked(&ev)); ctx.refresh_overlay(); ctx.redraw(); }/>
                        "All touching"
                    </label>
                </Show>
                <Show when=move || ctx.paint_mode.get() == PaintMode::Brush>
                    <label class="check" title="The brush repaints only armour (hull blocks and glass) and lights, and leaves systems, logic, doors and every other block as they are">
                        <input type="checkbox" prop:checked=move || ctx.paint_armor_only.get()
                               on:change=move |ev| ctx.paint_armor_only.set(event_target_checked(&ev))/>
                        "Armor and lights only"
                    </label>
                </Show>
                <Show when=move || ctx.paint_mode.get() == PaintMode::Swap>
                    <label class="check" title="The swap goes through the ship and everything docked to it, turrets included, not only the entity on screen. Undo takes back the entity on screen's part; the others are undone from their own view.">
                        <input type="checkbox" prop:checked=move || ctx.swap_whole.get()
                               on:change=move |ev| ctx.swap_whole.set(event_target_checked(&ev))/>
                        "Whole design (every docked entity)"
                    </label>
                </Show>
                <Show when=move || ctx.paint_mode.get() == PaintMode::Swap && ctx.swap_from.get().is_some()>
                    <div class="row">
                        <span class="empty">"Swapping out the chosen material: pick the new block below."</span>
                        <button on:click=move |_| { ctx.swap_from.set(None); ctx.highlight.set(None); ctx.refresh_links(); ctx.redraw(); }>"Cancel"</button>
                    </div>
                </Show>
            </Show>
            <label class="check" title="The game has the same setting: Switch build/remove mouse buttons">
                <input type="checkbox" prop:checked=move || ctx.swap.get()
                       on:change=move |ev| ctx.set_swap(event_target_checked(&ev))/>
                "Swap place and remove buttons"
            </label>
            <label class="check" title="Double-click the view to look round with the mouse, as in the game; Esc lets the mouse go. A click then waits a moment to see whether a second one follows.">
                <input type="checkbox" prop:checked=move || ctx.mouse_look.get()
                       on:change=move |ev| ctx.set_mouse_look(event_target_checked(&ev))/>
                "Mouse look on double-click"
            </label>

            <Show when=move || ctx.tool.get() == Tool::Link || ctx.selected.get().is_some()>
                <LinkPanel ctx/>
            </Show>
            <h2>"Block"</h2>
            {move || selected().map(|b| view! {
                <div class="chosen">
                    <span class="swatch" style=swatch(&b)></span>
                    <span>{b.name.clone()}</span>
                </div>
            })}
            <div class="shapes">
                <For each=family key=|(id, _, _)| *id let:variant>
                    <button class:current=move || ctx.block_id.get() == variant.0
                            on:click=move |_| ctx.choose_block(variant.0)>{variant.1}</button>
                </For>
            </div>
            <div class="turn">
                <button title="Rotate back (comma key)" on:click=move |_| ctx.rotate(-1)>"<"</button>
                <span>{turn_text}</span>
                <button title="Rotate (period key, as in the game)" on:click=move |_| ctx.rotate(1)>">"</button>
            </div>

            <h2>"Blocks"</h2>
            <input class="search" type="search" placeholder="Search blocks"
                   prop:value=move || search.get()
                   on:input=move |ev| search.set(event_target_value(&ev))/>
            <select on:change=move |ev| { category.set(event_target_value(&ev)); crate::let_go_of_focus(); }>
                <option value="">"All categories"</option>
                <For each=groups key=|g| g.clone() let:g>
                    <option value=g.clone()>{g.clone()}</option>
                </For>
            </select>
            <div class="blocks">
                <For each=move || matches().0 key=|(id, _, _)| *id let:row>
                    <button class="block" class:current=move || ctx.block_id.get() == row.0
                            on:click=move |_| ctx.choose_block(row.0)>
                        <span class="swatch" style=row.2.clone()></span>
                        {row.1.clone()}
                    </button>
                </For>
                <Show when=move || { matches().1 > 0 }>
                    <p class="empty">{move || matches().1} " more. Search to narrow the list."</p>
                </Show>
            </div>
        </aside>
    }
}

/// What the selected block is linked to, and how to change it.
#[component]
fn LinkPanel(ctx: Ctx) -> impl IntoView {
    let at = |p: yard_core::Pos| format!("{}, {}, {}", p.x, p.y, p.z);
    // A row that selects the block it names.
    let jump = move |p: yard_core::Pos, name: String| {
        view! {
            <button class="linked" title="Select this block" on:click=move |_| ctx.select(Some(p))>
                {name} <small>{at(p)}</small>
            </button>
        }
    };
    view! {
        <h2>"Links"</h2>
        <button class:current=move || ctx.show_unlinked.get()
                title="Marks in orange the modules no computer runs (cannon barrels, missile tubes, beam modules and the like) and the computers that run none of their modules"
                on:click=move |_| ctx.toggle_unlinked()>
            {move || if ctx.show_unlinked.get() { "Hide unlinked" } else { "Show unlinked" }}
        </button>
        {move || match ctx.link_info.get() {
            None => view! {
                <p class="empty">
                    "Click a block, or press C over it, to see what it is linked to. Then right-click another block, or press V over it, to link the two."
                </p>
            }
            .into_any(),
            Some(info) => {
                let pos = info.pos;
                let nothing = info.controls.is_empty();
                let more = info.unlisted_controllers;
                view! {
                    <div class="chosen">
                        <span class="dot selected"></span>
                        <span>{info.name.clone()} <small>{at(pos)}</small></span>
                    </div>
                    <div class="row">
                        <button on:click=move |_| ctx.look_at(pos)>"Look at it"</button>
                        <button title="Esc, or C over the block again" on:click=move |_| ctx.select(None)>"Deselect"</button>
                    </div>
                    <h3>"Controls"</h3>
                    {nothing.then(|| view! {
                        <p class="empty">
                            {if info.can_control {
                                "Nothing yet. Right-click a block, or press V over it, to link it. Hold Shift to take every block of that kind touching it."
                            } else {
                                "Nothing: the game does not let this kind of block control others."
                            }}
                        </p>
                    })}
                    <table class="links">
                        {info.controls.iter().map(|(ty, name, count, colour)| {
                            let ty = *ty;
                            let dot = format!("background: rgb({}, {}, {})", colour[0], colour[1], colour[2]);
                            view! {
                                <tr>
                                    <td><span class="dot" style=dot></span>{name.clone()}</td>
                                    <td>{crate::thousands(*count as u64)}</td>
                                    <td><button title="Unlink all of these" on:click=move |_| ctx.unlink_type(ty)>"Unlink"</button></td>
                                </tr>
                            }
                        }).collect_view()}
                    </table>
                    {info.controlled_controllers.iter().map(|(p, name)| jump(*p, name.clone())).collect_view()}
                    {(more > 0).then(|| view! { <p class="empty">{more} " more linked controllers are not listed."</p> })}
                    <h3><span class="dot by"></span>"Controlled by"</h3>
                    {info.controlled_by.is_empty().then(|| view! { <p class="empty">"Nothing."</p> })}
                    {info.controlled_by.iter().map(|(p, name)| jump(*p, name.clone())).collect_view()}
                }
                .into_any()
            }
        }}
        <p class="empty shift-help">
            "Shift+right-click (or Shift+V) links every block of that kind touching the one under the pointer, and the blocks touching those, all at once: select a cannon computer, then Shift+right-click one cannon barrel to link the whole barrel. Shift+right-click a linked block to unlink its group the same way."
        </p>
    }
}

/// The panel of the whole-design view: how each docked entity can move, with
/// a slider to move it by hand and a button to set them all in motion.
#[component]
pub fn DockPanel(ctx: Ctx) -> impl IntoView {
    use yard_core::dock::Motion;
    let moving = move || ctx.parts.with(|parts| parts.iter().filter(|p| p.range.0 < p.range.1).count());
    view! {
        <aside class="build">
            <h2>"Whole design"</h2>
            <p class="empty">
                "Every docked entity where the design docks it. This view is for looking: to change blocks or links, choose an entity on the left."
            </p>
            <h2>"Logic and rails"</h2>
            <p class="empty">
                "Click a button, a switch, a door or a light, or press R over it, to use it as in the game. Logic runs by the game's rules, and rails and rotators carry what is docked to them. None of it is saved."
            </p>
            <div class="row">
                <button class:current=move || ctx.simulating.get()
                        title="What the design does by itself when the game loads it: riders travel their rails, rotators turn once"
                        on:click=move |_| { ctx.start_sim(); }>
                    {move || if ctx.simulating.get() { "Running" } else { "Run" }}
                </button>
                <button title="Every block and every part back to how the design was saved" disabled=move || !ctx.simulating.get()
                        on:click=move |_| ctx.stop_sim()>"Put back as saved"</button>
            </div>
            <Switches ctx/>
            <h2>"Docked parts in motion"</h2>
            <p class="empty">
                {move || match moving() {
                    0 => "Nothing here can move: no part sits on a turret axis, a rotator or a run of rail.".to_string(),
                    n => format!("{n} parts can move. Play shows turrets sweeping, rotators turning and rail riders travelling their track; the sliders move one part at a time. A part stops where it would run into another, as in the game."),
                }}
            </p>
            <div class="row">
                <button class:current=move || ctx.playing.get() disabled=move || moving() == 0
                        on:click=move |_| ctx.play(!ctx.playing.get_untracked())>
                    {move || if ctx.playing.get() { "Pause" } else { "Play" }}
                </button>
                <button title="Every part back to how the design was saved" on:click=move |_| ctx.reset_poses()>"As saved"</button>
                <label class="check" title="Start over when every part has been through its motion. Without it, Play runs through once and stops.">
                    <input type="checkbox" prop:checked=move || ctx.looping.get()
                           on:change=move |ev| ctx.looping.set(event_target_checked(&ev))/>
                    "Loop"
                </label>
            </div>
            <Show when=move || { ctx.unplaced.get() > 0 }>
                <p class="warning">
                    {move || ctx.unplaced.get()} " docked entities are not shown: the design has no rail dock on record for them, so the game would not place them either."
                </p>
            </Show>
            <div class="parts">
                <For each=move || ctx.parts.get() key=|p| (p.path.clone(), p.track, p.blocks) let:part>
                    {
                        let (lo, hi) = part.range;
                        let (path, path_for_value, path_for_edit) = (part.path.clone(), part.path.clone(), part.path.clone());
                        let value = Signal::derive(move || ctx.poses.with(|poses| poses.get(&path_for_value).copied().unwrap_or(0.0)));
                        let (unit, step) = match part.motion {
                            Motion::Turret => (" degrees", "1"),
                            Motion::Rotator { .. } => (" degrees", "90"),
                            _ => (" blocks", "1"),
                        };
                        let kind = match part.motion {
                            Motion::Turret => "Turret axis: turns freely".to_string(),
                            Motion::Rotator { clockwise: true } => "Rotator: quarter turns clockwise".to_string(),
                            Motion::Rotator { clockwise: false } => "Rotator: quarter turns counter-clockwise".to_string(),
                            Motion::Rail if hi > lo => format!("Rail: {} blocks of track", part.track.0 + part.track.1 + 1),
                            Motion::Rail => "Rail: one block, held in place".to_string(),
                            Motion::Fixed if part.axis => "Turret axis, but its rail docker faces front or back on it, so the game holds it still".to_string(),
                            Motion::Fixed => "Held in place".to_string(),
                        };
                        view! {
                            <div class="part">
                                <div class="part-name">
                                    <span>{crate::entity_label(&part.path)}</span>
                                    <button title="Open this entity to edit it" on:click=move |_| ctx.show_entity(path_for_edit.clone(), true)>"Edit"</button>
                                </div>
                                <small>{kind} ", " {crate::thousands(part.blocks)} " blocks"</small>
                                {part.overlapping.then(|| view! {
                                    <small class="warning">"As saved it sits inside other blocks, so nothing stops it here."</small>
                                })}
                                {(hi > lo).then(|| view! {
                                    <div class="pose">
                                        <input type="range" min=lo.to_string() max=hi.to_string() step=step
                                               prop:value=move || value.get().to_string()
                                               on:input=move |ev| {
                                                   if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                                       ctx.play(false);
                                                       ctx.set_pose(path.clone(), v);
                                                   }
                                               }/>
                                        <span>{move || format!("{:.0}{unit}", value.get().round() + 0.0)}</span>
                                    </div>
                                })}
                            </div>
                        }
                    }
                </For>
            </div>
        </aside>
    }
}

/// The switches of what is on screen: buttons, activation modules, gates.
/// Most sit inside a hull, out of reach of a click from outside, so each
/// can be used from here.
#[component]
fn Switches(ctx: Ctx) -> impl IntoView {
    let whole = move || ctx.entity_path.get() == crate::work::WHOLE;
    view! {
        <div class="switches">
            {move || {
                let (list, total) = ctx.switches.get();
                if total == 0 {
                    return view! { <p class="empty">"No buttons or switches here."</p> }.into_any();
                }
                let more = total.saturating_sub(list.len());
                view! {
                    <h3>{format!("Switches ({total})")}</h3>
                    <div class="switch-list">
                        {list.into_iter().map(|s| {
                            let at = format!("{}, {}, {}", s.pos.x, s.pos.y, s.pos.z);
                            let place = if whole() { format!("{at} of {}", crate::entity_label(&s.path).to_lowercase()) } else { at };
                            let (entity, pos) = (s.entity, s.pos);
                            view! {
                                <button class="switch" class:on=s.on title="Use it, as the game's R key does"
                                        on:click=move |_| ctx.use_block(entity, pos)>
                                    <span class="lamp"></span>
                                    <span class="switch-name">{s.name.clone()}</span>
                                    <small>{place}</small>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    {(more > 0).then(|| view! { <p class="empty">{format!("And {more} more, which can be used by clicking them in the view.")}</p> })}
                }.into_any()
            }}
        </div>
    }
}

/// A button that picks how the Paint tool covers blocks.
fn paint_mode_button(ctx: Ctx, mode: PaintMode, label: &'static str) -> impl IntoView {
    view! {
        <button class:current=move || ctx.paint_mode.get() == mode
                on:click=move |_| { ctx.paint_mode.set(mode); ctx.swap_from.set(None); }>{label}</button>
    }
}

/// Where the brush panel's open or closed state is kept between visits.
const BRUSH_PANEL_KEY: &str = "yard.brush_panel_open";

/// The brush size and the mirror planes, in a small panel floating over the
/// view that folds away to its title.
#[component]
pub fn BrushPanel(ctx: Ctx) -> impl IntoView {
    let storage = || window().local_storage().ok().flatten();
    let open = RwSignal::new(storage().and_then(|s| s.get_item(BRUSH_PANEL_KEY).ok().flatten()).is_none_or(|v| v == "1"));
    let toggle = move |_| {
        let now = !open.get_untracked();
        open.set(now);
        if let Some(s) = storage() {
            let _ = s.set_item(BRUSH_PANEL_KEY, if now { "1" } else { "0" });
        }
    };
    // One size slider: for one axis, or for all three at once (showing the largest).
    let brush_slider = move |axis: Option<usize>, label: &'static str, title: &'static str| {
        let value = move || {
            let b = ctx.brush.get();
            axis.map_or_else(|| b.into_iter().max().unwrap_or(1), |a| b[a])
        };
        view! {
            <label class="slider" title=title>
                <span>{label}</span>
                <input type="range" min="1" max=MAX_BRUSH.to_string() step="1"
                       prop:value=move || value().to_string()
                       on:input=move |ev| {
                           if let Ok(n) = event_target_value(&ev).parse::<i32>() {
                               let n = n.clamp(1, MAX_BRUSH);
                               ctx.brush.update(|b| match axis {
                                   Some(a) => b[a] = n,
                                   None => *b = [n; 3],
                               });
                               ctx.show_brush();
                           }
                       }/>
                <span class="value">{value}</span>
            </label>
        }
    };

    // One mirror plane: a switch, and where the plane sits (in blocks; a value
    // ending in .5 puts it between two rows).
    let plane_row = move |axis: usize, label: &'static str| {
        let get = move || {
            let s = ctx.symmetry.get();
            [s.x, s.y, s.z][axis]
        };
        let set = move |value: Option<i32>| {
            ctx.symmetry.update(|s| match axis {
                0 => s.x = value,
                1 => s.y = value,
                _ => s.z = value,
            })
        };
        let position = RwSignal::new(0i32);
        view! {
            <div class="plane">
                <label class="check">
                    <input type="checkbox" prop:checked=move || get().is_some()
                           on:change=move |ev| set(event_target_checked(&ev).then(|| position.get_untracked()))/>
                    {label}
                </label>
                <input type="number" step="0.5" title="Where the mirror plane sits"
                       prop:value=move || format!("{}", position.get() as f32 / 2.0)
                       on:input=move |ev| {
                           if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                               let twice = (v * 2.0).round() as i32;
                               position.set(twice);
                               if get().is_some() {
                                   set(Some(twice));
                               }
                           }
                       }/>
            </div>
        }
    };

    let fill_by_brush = move || ctx.tool.get() == Tool::Paint && ctx.paint_mode.get() == PaintMode::Fill && ctx.fill_brush_depth.get() && !ctx.fill_all.get();
    let mirrored = move || {
        let s = ctx.symmetry.get();
        [s.x, s.y, s.z].iter().filter(|p| p.is_some()).count()
    };
    let summary = move || {
        let b = ctx.brush.get();
        let shape = match ctx.brush_shape.get() {
            BrushShape::Cube => "",
            BrushShape::Cylinder => " cylinder",
            BrushShape::Sphere => " sphere",
        };
        match mirrored() {
            0 => format!("Brush {} x {} x {}{shape}", b[0], b[1], b[2]),
            n => format!("Brush {} x {} x {}{shape}, mirror {n}", b[0], b[1], b[2]),
        }
    };
    view! {
        <div class="floating" class:open=move || open.get() on:mousedown=|ev| ev.stop_propagation()>
            <button class="floating-title" title="Show or hide the brush size and mirror settings" on:click=toggle>
                <span>{move || if open.get() { "Brush and mirror".to_string() } else { summary() }}</span>
                <span class="fold-word">{move || if open.get() { "hide" } else { "show" }}</span>
            </button>
            <Show when=move || open.get()>
                <h2>"Brush size"</h2>
                <Show when=move || ctx.tool.get() == Tool::Paint && ctx.paint_mode.get() != PaintMode::Brush && !fill_by_brush()>
                    <p class="empty">"Painting does not use the brush size in this mode."</p>
                </Show>
                <Show when=fill_by_brush>
                    <p class="empty">"The fill can paints as many blocks deep as the brush size along the face you click."</p>
                </Show>
                <div class="shapes brush-shape">
                    {BrushShape::ALL.map(|(shape, label)| view! {
                        <button class:current=move || ctx.brush_shape.get() == shape
                                on:click=move |_| { ctx.brush_shape.set(shape); ctx.show_brush(); }>{label}</button>
                    })}
                </div>
                <Show when=move || ctx.brush_shape.get() == BrushShape::Cylinder>
                    <p class="empty">"The cylinder stands along the direction you click into: its depth is the size along that axis."</p>
                </Show>
                <div class="brush">
                    {brush_slider(None, "All", "Sets X, Y and Z together")}
                    {brush_slider(Some(0), "X", "Size along X")}
                    {brush_slider(Some(1), "Y", "Size along Y")}
                    {brush_slider(Some(2), "Z", "Size along Z")}
                </div>
                <button class="brush-reset" title="Back to a single block" on:click=move |_| { ctx.brush.set([1; 3]); ctx.show_brush(); }>"Reset size"</button>
                <h2>"Mirror"</h2>
                {plane_row(0, "Across X")}
                {plane_row(1, "Across Y")}
                {plane_row(2, "Across Z")}
            </Show>
        </div>
    }
}
