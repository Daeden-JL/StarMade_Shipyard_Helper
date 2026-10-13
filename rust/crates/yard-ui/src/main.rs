//! The shipyard web client: open a StarMade blueprint, look at it, edit it.

mod api;
mod camera;
mod chat;
mod gl;
mod looks;
mod panel;
mod work;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlInputElement};
use std::collections::BTreeSet;
use work::{Act, Ctx, Tool};
use yard_core::api::{Summary, SyncDirection, SyncInfo, SyncStatus, SyncVersion};
use yard_core::edit::Op;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// "12,345".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "" -> "Main entity", "ATTACHED_0/ATTACHED_2" -> "Docked 1 > 3".
pub fn entity_label(path: &str) -> String {
    if path.is_empty() {
        return "Main entity".into();
    }
    let parts: Vec<String> = path
        .split('/')
        .map(|p| p.strip_prefix("ATTACHED_").and_then(|n| n.parse::<u32>().ok()).map_or(p.to_string(), |n| (n + 1).to_string()))
        .collect();
    format!("Docked {}", parts.join(" > "))
}

/// How often the page asks the server what has changed, in milliseconds.
const POLL_MS: u64 = 8000;

/// Where a design stands with the game server, in two or three words.
fn sync_label(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::Synced => "In the game",
        SyncStatus::Waiting => "Waiting to sync",
        SyncStatus::Off => "Not synced",
        SyncStatus::Gone => "Deleted in the game",
    }
}

/// Which way a kept version travelled, in a few words.
fn direction_label(direction: SyncDirection) -> &'static str {
    match direction {
        SyncDirection::FromGame => "from the game",
        SyncDirection::ToGame => "to the game",
        SyncDirection::Matched => "same on both sides",
        SyncDirection::Unsynced => "never synced, kept when a version was restored",
        SyncDirection::FromShipyard => "from a shipyard in the game",
    }
}

/// A kept version in a sentence: "the version that came from the game on".
fn version_phrase(direction: SyncDirection) -> &'static str {
    match direction {
        SyncDirection::FromGame => "the version that came from the game on",
        SyncDirection::ToGame => "the version that went to the game on",
        SyncDirection::Matched => "the version both sides had on",
        SyncDirection::Unsynced => "edits that were never synced, kept on",
        SyncDirection::FromShipyard => "the version that came from the shipyard on",
    }
}

/// A time in seconds since 1970 as the visitor's own date and time of day.
fn when(seconds: u64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(seconds as f64 * 1000.0));
    let day: String = date.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED).into();
    let time: String = date.to_locale_time_string("default").into();
    format!("{day} {time}")
}

/// Most designs listed at once; the filter narrows it down.
const DESIGN_ROWS: usize = 150;

/// The header's line about the game server; empty while none has ever called.
fn game_line(info: &SyncInfo) -> String {
    let Some(seconds) = info.seconds_ago else {
        return if info.known { "Game server: not connected".into() } else { String::new() };
    };
    let name = info.server.as_deref().filter(|n| !n.is_empty()).unwrap_or("Game server");
    match seconds {
        0..=45 => format!("{name}: connected, {} blueprints", info.blueprints),
        46..=119 => format!("{name}: last heard from {seconds} seconds ago"),
        _ => format!("{name}: last heard from {} minutes ago", seconds / 60),
    }
}

/// The design id in a `/d/<id>` address, if that is where the page was opened.
fn id_from_location() -> Option<String> {
    let path = window().location().pathname().ok()?;
    path.strip_prefix("/d/").map(|id| id.trim_end_matches('/').to_string()).filter(|id| !id.is_empty())
}

/// Whether a key press was typed into a text field, where shortcuts must not fire.
fn typing(ev: &leptos::ev::KeyboardEvent) -> bool {
    let Some(element) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return false };
    match element.tag_name().as_str() {
        "TEXTAREA" | "SELECT" => true,
        // A ticked checkbox keeps the focus but takes no text.
        "INPUT" => !matches!(element.get_attribute("type").as_deref(), Some("checkbox" | "radio" | "file" | "button")),
        _ => false,
    }
}

/// Takes the keyboard from whatever control of the page has it, so the keys
/// fly the view and move things in it again: a dropdown or a number box
/// keeps the focus after it is used, and keys pressed there are typing.
pub fn let_go_of_focus() {
    if let Some(element) = document().active_element().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = element.blur();
    }
}

/// `MouseEvent.button` of the middle and right mouse buttons.
const MIDDLE_BUTTON: i16 = 1;
const RIGHT_BUTTON: i16 = 2;
/// The keys that move the view, by position on the keyboard, as in the game's
/// build mode: forward, left, back, right, up, down.
const MOVE_KEYS: [&str; 6] = ["KeyW", "KeyA", "KeyS", "KeyD", "KeyE", "KeyQ"];
/// The thickest shell a hollow import may have; the server's importer holds
/// it to the same.
const MAX_SHELL: u8 = 16;
/// A press that moves no further than this many pixels is a click, not a drag.
const CLICK_SLACK: i32 = 4;
/// With mouse look on a double-click, how long a click waits to see whether
/// a second one follows before it does its work.
const DOUBLE_CLICK_MS: u64 = 250;

/// Pointer state while a button is held.
#[derive(Clone, Copy)]
struct Drag {
    x: i32,
    y: i32,
    button: i16,
    pan: bool,
    /// Total distance moved; past a few pixels the press is a drag, not a click.
    travelled: i32,
}

#[component]
fn App() -> impl IntoView {
    let ctx = Ctx::new();
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let new_name = RwSignal::new(String::new());

    // Start-up: renderer once the canvas exists, then catalog, list, and the
    // design named in the address if there is one.
    Effect::new(move |started: Option<()>| {
        if started.is_some() {
            return;
        }
        let Some(canvas) = canvas_ref.get() else { return };
        let canvas: HtmlCanvasElement = canvas.unchecked_into();
        if let Err(e) = ctx.start(canvas) {
            ctx.fail(e);
            return;
        }
        spawn_local(async move {
            // Without the 3D models their blocks are drawn as cubes; the
            // editor works all the same.
            let models = api::models().await.unwrap_or_default();
            match api::catalog().await {
                Ok(blocks) => ctx.load_catalog(blocks, models),
                Err(e) => {
                    ctx.fail(e);
                    return;
                }
            }
            ctx.refresh_designs();
            ctx.poll();
            if let Some(id) = id_from_location() {
                ctx.open_design(id, true);
            }
        });
        // Lives as long as the page does.
        let _ = set_interval_with_handle(move || ctx.poll(), std::time::Duration::from_millis(POLL_MS));
    });

    // The brush outline and mirror planes follow the build settings.
    Effect::new(move |_| {
        ctx.tool.track();
        ctx.selected.track();
        ctx.brush.track();
        ctx.brush_shape.track();
        ctx.brush_shown.track();
        ctx.symmetry.track();
        ctx.block_id.track();
        ctx.turn.track();
        ctx.refresh_overlay();
        ctx.redraw();
    });

    // The Move core tool: choosing it starts choosing the core's new place,
    // and choosing another tool calls that off.
    Effect::new(move |was: Option<Tool>| {
        let tool = ctx.tool.get();
        if was.is_some_and(|t| t != tool) {
            if tool != Tool::Clone && ctx.cloning.get_untracked().is_some() {
                ctx.cancel_relocation();
            }
            if tool == Tool::Core && ctx.relocating.get_untracked().is_none() && ctx.entity_path.get_untracked() != work::WHOLE {
                ctx.start_relocation();
            } else if tool != Tool::Core && ctx.relocating.get_untracked().is_some() {
                ctx.cancel_relocation();
            }
        }
        tool
    });

    let _ = window_event_listener(leptos::ev::resize, move |_| ctx.redraw());
    // Mouse look lasts as long as the view holds the pointer; the browser lets
    // it go on Esc.
    let on_lock = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
        let canvas = canvas_ref.get_untracked().map(|c| -> web_sys::Element { c.unchecked_into() });
        let locked = document().pointer_lock_element().is_some_and(|e| Some(e) == canvas);
        ctx.looking.set(locked);
        if !locked {
            ctx.arm(None);
        }
    });
    let _ = document().add_event_listener_with_callback("pointerlockchange", on_lock.as_ref().unchecked_ref());
    on_lock.forget();
    // The movement keys that are down. Which way they push the view is worked
    // out afresh at every change, so opposite keys cancel.
    let held = StoredValue::new(BTreeSet::<String>::new());
    let steer = move |fast: bool| {
        let axis = |plus: &str, minus: &str| held.with_value(|h| h.contains(plus) as i8 - h.contains(minus) as i8) as f32;
        ctx.steer([axis("KeyD", "KeyA"), axis("KeyE", "KeyQ"), axis("KeyW", "KeyS")], fast);
    };
    let _ = window_event_listener(leptos::ev::keydown, move |ev| {
        if typing(&ev) {
            return;
        }
        if ev.key() == "Escape" && ctx.looking.get_untracked() {
            ev.prevent_default();
            return ctx.stop_looking();
        }
        // Choosing a new place for the core: the movement keys move it, not
        // the view. While the move is previewed they fly the view again, and
        // Escape goes back to choosing.
        if (ctx.relocating.get_untracked().is_some() || ctx.cloning.get_untracked().is_some()) && !(ev.ctrl_key() || ev.meta_key() || ev.alt_key()) {
            let code = ev.code();
            let previewing = ctx.core_preview.get_untracked().is_some();
            let movement = matches!(code.as_str(), "KeyW" | "KeyA" | "KeyS" | "KeyD" | "KeyE" | "KeyQ" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "PageUp" | "PageDown");
            match code.as_str() {
                "Enter" => ctx.confirm_relocation(),
                "Escape" if previewing => ctx.end_relocation_preview(),
                "Escape" => ctx.cancel_relocation(),
                _ if movement && !previewing => ctx.nudge_relocation(&code),
                _ => {}
            }
            if matches!(code.as_str(), "Enter" | "Escape") || (movement && !previewing) {
                ev.prevent_default();
                return;
            }
        }
        // A corner of the turret box or its docker in hand: the movement keys move it.
        if ctx.tool.get_untracked() == Tool::Turret && ctx.turret_box.get_untracked().is_some() && !(ev.ctrl_key() || ev.meta_key() || ev.alt_key()) {
            let code = ev.code();
            let grabbed = ctx.turret_grabbed();
            let handled = match code.as_str() {
                "Tab" => {
                    ctx.next_turret_handle();
                    true
                }
                "Enter" if grabbed => {
                    ctx.make_turret();
                    true
                }
                "Escape" if grabbed => {
                    ctx.release_turret();
                    true
                }
                "KeyW" | "KeyA" | "KeyS" | "KeyD" | "KeyE" | "KeyQ" | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "PageUp" | "PageDown" if grabbed => {
                    ctx.nudge_turret(&code);
                    true
                }
                _ => false,
            };
            if handled {
                ev.prevent_default();
                return;
            }
        }
        let command = ev.ctrl_key() || ev.meta_key();
        if command {
            // The browser does not always report keys let go while Ctrl or
            // Cmd is down, which would leave the view drifting.
            held.update_value(BTreeSet::clear);
            steer(false);
        } else if !ev.alt_key() && MOVE_KEYS.contains(&ev.code().as_str()) {
            held.update_value(|h| {
                h.insert(ev.code());
            });
            steer(ev.shift_key());
            ev.prevent_default();
            return;
        }
        match ev.key().to_lowercase().as_str() {
            "z" if command && ev.shift_key() => ctx.history_step(Op::Redo),
            "z" if command => ctx.history_step(Op::Undo),
            "y" if command => ctx.history_step(Op::Redo),
            _ if command => return,
            "shift" => return steer(true),
            "1" => ctx.tool.set(Tool::Pick),
            "2" => ctx.tool.set(Tool::Paint),
            "3" => ctx.tool.set(Tool::Build),
            "4" => ctx.tool.set(Tool::Link),
            "5" => ctx.tool.set(Tool::Use),
            "6" => ctx.tool.set(Tool::Smooth),
            "7" => ctx.tool.set(Tool::Turret),
            "8" => ctx.tool.set(Tool::Core),
            "9" => ctx.tool.set(Tool::Clone),
            // The game's activate key.
            "r" => ctx.use_under_pointer(),
            // As in the game's build mode: C selects the block looked at, V
            // links the block looked at to it, Shift+V its whole group.
            "c" => ctx.select_under_pointer(),
            "v" => ctx.connect_under_pointer(ev.shift_key()),
            "escape" => {
                if ctx.deleting.get_untracked() {
                    return ctx.deleting.set(false);
                }
                ctx.picked.set(None);
                ctx.swap_from.set(None);
                ctx.clear_turret();
                ctx.select(None)
            }
            // The game turns the block to place with the period key.
            "." => ctx.rotate(1),
            "," => ctx.rotate(-1),
            _ => return,
        }
        ev.prevent_default();
    });
    let _ = window_event_listener(leptos::ev::keyup, move |ev| {
        held.update_value(|h| {
            h.remove(&ev.code());
        });
        steer(ev.shift_key());
    });
    let _ = window_event_listener(leptos::ev::blur, move |_| {
        held.update_value(BTreeSet::clear);
        steer(false);
    });

    // A 3D model waiting for the player to say how to turn it into blocks.
    let import = RwSignal::new(None::<(String, Vec<u8>)>);
    let (size, solid, up, quarter, mirror) = (RwSignal::new(64u32), RwSignal::new(true), RwSignal::new("auto".to_string()), RwSignal::new(0u8), RwSignal::new(false));
    let large = move || size.get() > 512;
    let shell = RwSignal::new(1u8);
    let importing = RwSignal::new(false);
    let do_import = move || {
        let Some((name, bytes)) = import.get_untracked() else { return };
        let choices = api::ImportChoices { longest: size.get_untracked(), solid: solid.get_untracked(), shell: shell.get_untracked(), up: up.get_untracked(), turn: quarter.get_untracked(), mirror: mirror.get_untracked(), smooth: false, step: 0 };
        importing.set(true);
        spawn_local(async move {
            match api::import_model(&name, &bytes, &choices).await {
                Ok(done) => {
                    import.set(None);
                    ctx.refresh_designs();
                    ctx.open_design_noting(done.summary.id, done.notes.join(" "));
                }
                Err(e) => ctx.fail(e),
            }
            importing.set(false);
        });
    };


    let on_file = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        let Some(file) = input.files().and_then(|f| f.get(0)) else { return };
        // Clear the field so choosing the same file again fires another change.
        input.set_value("");
        let name = file.name();
        ctx.say(format!("Uploading {name}..."));
        spawn_local(async move {
            let bytes = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buffer) => js_sys::Uint8Array::new(&buffer).to_vec(),
                Err(_) => {
                    ctx.fail(format!("Could not read {name} from disk."));
                    return;
                }
            };
            let lower = name.to_lowercase();
            // Blueprints go straight in. Anything else is a 3D model, and
            // the player says how to turn it into blocks; so is a zip that
            // turns out not to hold a blueprint.
            let model = !lower.ends_with(".sment");
            if model && !lower.ends_with(".zip") {
                ctx.say("");
                import.set(Some((name, bytes)));
                return;
            }
            match api::upload(&name, &bytes).await {
                Ok(summary) => {
                    ctx.refresh_designs();
                    ctx.open_design(summary.id, true);
                }
                Err(_) if model => {
                    ctx.say("");
                    import.set(Some((name, bytes)));
                }
                Err(e) => ctx.fail(e),
            }
        });
    };

    let create = move || {
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            ctx.fail("Type a name for the new ship first.");
            return;
        }
        spawn_local(async move {
            match api::new_design(&name).await {
                Ok(summary) => {
                    new_name.set(String::new());
                    ctx.refresh_designs();
                    ctx.open_design(summary.id, true);
                }
                Err(e) => ctx.fail(e),
            }
        });
    };

    let drag = StoredValue::new(None::<Drag>);
    // In mouse look the pointer stays put and the tools work on the middle of the view.
    let middle = move || {
        let canvas = canvas_ref.get_untracked()?;
        let rect = canvas.get_bounding_client_rect();
        Some(((rect.left() + rect.width() / 2.0) as i32, (rect.top() + rect.height() / 2.0) as i32))
    };
    let aim = move |ev: &leptos::ev::MouseEvent| {
        if ctx.looking.get_untracked() {
            if let Some(at) = middle() {
                return at;
            }
        }
        (ev.client_x(), ev.client_y())
    };
    // A click waiting to see whether it is the first half of a double-click.
    let waiting = StoredValue::new(None::<TimeoutHandle>);
    let on_down = move |ev: leptos::ev::MouseEvent| {
        // Pressing in the view does not move the focus by itself (the default is prevented).
        ev.prevent_default();
        let_go_of_focus();
        let looking = ctx.looking.get_untracked();
        if ev.detail() >= 2 && ev.button() == 0 && ctx.mouse_look.get_untracked() && !looking {
            // The second press of a double-click: the first one's click is called off.
            if let Some(h) = waiting.get_value() {
                h.clear();
            }
            waiting.set_value(None);
            drag.set_value(None);
            return;
        }
        let pan = !looking && (ev.button() != 0 || ev.shift_key());
        let (x, y) = aim(&ev);
        drag.set_value(Some(Drag { x, y, button: ev.button(), pan, travelled: 0 }));
        if ev.button() == RIGHT_BUTTON {
            // Show what letting go without dragging would do.
            ctx.arm(ctx.on_click(true));
        }
    };
    let on_move = move |ev: leptos::ev::MouseEvent| {
        if ctx.looking.get_untracked() {
            ctx.look_camera(ev.movement_x() as f32, ev.movement_y() as f32);
            ctx.pointer_moved(middle());
            return;
        }
        let held = drag.get_value().filter(|_| ev.buttons() != 0);
        let Some(mut d) = held else {
            drag.set_value(None);
            ctx.arm(None);
            ctx.pointer_moved(Some((ev.client_x(), ev.client_y())));
            return;
        };
        let (dx, dy) = (ev.client_x() - d.x, ev.client_y() - d.y);
        d.travelled += dx.abs() + dy.abs();
        d.x = ev.client_x();
        d.y = ev.client_y();
        drag.set_value(Some(d));
        if d.travelled > CLICK_SLACK {
            ctx.arm(None);
            ctx.turn_camera(dx as f32, dy as f32, d.pan);
        }
    };
    let on_up = move |ev: leptos::ev::MouseEvent| {
        let Some(d) = drag.get_value() else { return };
        drag.set_value(None);
        ctx.arm(None);
        if d.travelled > CLICK_SLACK {
            return; // It was a drag: the camera moved, nothing is clicked.
        }
        let act = match d.button {
            0 if !d.pan => ctx.on_click(false),
            RIGHT_BUTTON => ctx.on_click(true),
            // As in the game's advanced build mode: take the block looked at.
            MIDDLE_BUTTON => Some(Act::Pick),
            _ => None,
        };
        let Some(act) = act else { return };
        let ((x, y), shift) = (aim(&ev), ev.shift_key());
        if d.button == 0 && ctx.mouse_look.get_untracked() && !ctx.looking.get_untracked() {
            // Wait: a second click makes it a double-click, which turns on mouse look instead.
            let handle = set_timeout_with_handle(
                move || {
                    waiting.set_value(None);
                    ctx.act(act, x, y, shift);
                },
                std::time::Duration::from_millis(DOUBLE_CLICK_MS),
            );
            waiting.set_value(handle.ok());
            return;
        }
        ctx.act(act, x, y, shift);
    };
    let on_double = move |_: leptos::ev::MouseEvent| {
        if !ctx.mouse_look.get_untracked() || ctx.looking.get_untracked() {
            return;
        }
        let Some(canvas) = canvas_ref.get_untracked() else { return };
        // Called by hand: browsers answer with a promise that fails when they
        // refuse, as they do just after Esc let the mouse go.
        let refused = move |_: wasm_bindgen::JsValue| ctx.say("The browser would not hand the mouse over just then. Double-click again in a moment.");
        let asked = js_sys::Reflect::get(&canvas, &"requestPointerLock".into())
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .map(|f| f.call0(&canvas));
        match asked {
            Some(Ok(answer)) => {
                if let Ok(promise) = answer.dyn_into::<js_sys::Promise>() {
                    let on_refusal = wasm_bindgen::closure::Closure::once(refused);
                    let _ = promise.catch(&on_refusal);
                    on_refusal.forget();
                }
                ctx.pointer_moved(middle());
            }
            _ => refused(wasm_bindgen::JsValue::NULL),
        }
    };
    let on_leave = move |_: leptos::ev::MouseEvent| {
        if ctx.looking.get_untracked() {
            return;
        }
        drag.set_value(None);
        ctx.arm(None);
        ctx.pointer_moved(None);
    };
    let on_wheel = move |ev: leptos::ev::WheelEvent| {
        ev.prevent_default();
        ctx.zoom_camera(ev.delta_y() as f32);
    };

    let current = ctx.current;
    let designs = ctx.designs;
    let status = ctx.status;
    // The designs to list: those matching the filter, by name, and how many
    // more there are than fit.
    let filter = RwSignal::new(String::new());
    let listed = Memo::new(move |_| {
        let needle = filter.get().trim().to_lowercase();
        let mut list: Vec<Summary> =
            designs.get().into_iter().filter(|d| needle.is_empty() || d.name.to_lowercase().contains(&needle)).collect();
        list.sort_by_cached_key(|d| d.name.to_lowercase());
        let more = list.len().saturating_sub(DESIGN_ROWS);
        list.truncate(DESIGN_ROWS);
        (list, more)
    });
    view! {
        <div class="app">
            <Show when=move || ctx.deleting.get()>
                <div class="dialog-back" on:click=move |_| ctx.deleting.set(false)>
                    <div class="dialog" on:click=|ev| ev.stop_propagation()>
                        <h2>"Delete this blueprint?"</h2>
                        <p>
                            <strong>{move || ctx.current.with(|c| c.as_ref().map(|s| s.name.clone()).unwrap_or_default())}</strong>
                            " (" {move || ctx.current.with(|c| thousands(c.as_ref().map_or(0, |s| s.blocks)))} " blocks) is removed from the shipyard, with its docked entities and its sync history."
                        </p>
                        <Show when=move || ctx.current.with(|c| c.as_ref().and_then(|s| ctx.sync_of(&s.id))) == Some(SyncStatus::Synced)>
                            <p class="empty">"The game server has a copy of it. That copy stays there: the shipyard never deletes in the game, and does not bring it back here unless it changes there."</p>
                        </Show>
                        <Show when=move || ctx.current.with(|c| c.as_ref().and_then(|s| ctx.sync_of(&s.id))) == Some(SyncStatus::Waiting)>
                            <p class="empty">"If the game server has an older copy of it, that stays there: the shipyard never deletes in the game."</p>
                        </Show>
                        <p class="empty">"The files are kept in the server's trash folder, so an administrator can still put them back."</p>
                        <div class="row">
                            <button class="danger" on:click=move |_| ctx.delete_design()>"Delete"</button>
                            <button class="current" on:click=move |_| ctx.deleting.set(false)>"Cancel"</button>
                        </div>
                    </div>
                </div>
            </Show>
            <Show when=move || import.get().is_some()>
                <div class="dialog-back">
                    <div class="dialog">
                        <h2>"Make a ship from a 3D model"</h2>
                        <p class="empty">
                            {move || import.get().map(|(name, bytes)| format!("{name}, {} KB. The model is cut into blocks, coloured with the nearest hull colour.", bytes.len() / 1024)).unwrap_or_default()}
                        </p>
                        <label>"Length of the longest side, in blocks"
                            // The number is taken as it is typed when it is in range, and
                            // the field is left alone while it is typed: clamping on every
                            // key turns the "1" of "1054" into a 4. Out of range, it is
                            // brought into range when the field is left.
                            <input type="number" min="4" max="8192" prop:value=move || size.get().to_string()
                                   on:input=move |ev| {
                                       if let Ok(n) = event_target_value(&ev).parse::<u32>() {
                                           if (4..=8192).contains(&n) {
                                               size.set(n);
                                           }
                                       }
                                   }
                                   on:change=move |ev| {
                                       let n = event_target_value(&ev).parse::<u32>().unwrap_or_else(|_| size.get_untracked()).clamp(4, 8192);
                                       size.set(n);
                                       event_target::<web_sys::HtmlInputElement>(&ev).set_value(&n.to_string());
                                   }/>
                        </label>
                        <Show when=large>
                            <p class="empty">
                                "A large ship takes a while to cut and can be millions of blocks even hollow (a hull 4096 long came to about 11 million). Results over 40 million blocks are refused."
                            </p>
                        </Show>
                        <label class="check">
                            <input type="checkbox" prop:checked=move || solid.get() on:change=move |ev| solid.set(event_target_checked(&ev))/>
                            "Solid inside (otherwise a hollow shell)"
                        </label>
                        <Show when=move || !solid.get()>
                            <label title="Walls thicker than one block hold together where the surface slants, and smoothing has room to work on them">
                                {move || format!("Shell thickness: {}", match shell.get() { 1 => "1 block".to_string(), n => format!("{n} blocks") })}
                                <input type="range" min="1" max=MAX_SHELL.to_string() step="1" prop:value=move || shell.get().to_string()
                                       on:input=move |ev| shell.set(event_target_value(&ev).parse().unwrap_or(1))/>
                            </label>
                        </Show>
                        <p class="empty">"To smooth the steps afterwards, use the Smooth tool (6): \"Smooth whole entity\" goes over the whole ship, as many times as you like."</p>
                        <label>"Which way is up in the file"
                            <select on:change=move |ev| up.set(event_target_value(&ev))>
                                <option value="auto" selected=move || up.get() == "auto">"The file type's usual way"</option>
                                <option value="y" selected=move || up.get() == "y">"Y is up (glTF, OBJ, FBX)"</option>
                                <option value="z" selected=move || up.get() == "z">"Z is up (Blender, STL, 3MF)"</option>
                            </select>
                        </label>
                        <label>"Turn the nose"
                            <select on:change=move |ev| quarter.set(event_target_value(&ev).parse().unwrap_or(0))>
                                <option value="0" selected=move || quarter.get() == 0>"As it is"</option>
                                <option value="1" selected=move || quarter.get() == 1>"A quarter turn to the left"</option>
                                <option value="2" selected=move || quarter.get() == 2>"Around"</option>
                                <option value="3" selected=move || quarter.get() == 3>"A quarter turn to the right"</option>
                            </select>
                        </label>
                        <label class="check">
                            <input type="checkbox" prop:checked=move || mirror.get() on:change=move |ev| mirror.set(event_target_checked(&ev))/>
                            "Swap left and right"
                        </label>
                        <div class="row">
                            <button class="current" disabled=move || importing.get() on:click=move |_| do_import()>
                                {move || if importing.get() { "Working..." } else { "Make the ship" }}
                            </button>
                            <button disabled=move || importing.get() on:click=move |_| import.set(None)>"Cancel"</button>
                        </div>
                    </div>
                </div>
            </Show>
            <header class="bar">
                <h1>"Shipyard"</h1>
                <input class="name" type="text" placeholder="New ship name" maxlength="48"
                       prop:value=move || new_name.get()
                       on:input=move |ev| new_name.set(event_target_value(&ev))
                       on:keydown=move |ev: leptos::ev::KeyboardEvent| if ev.key() == "Enter" { create() }/>
                <button on:click=move |_| create()>"New ship"</button>
                <label class="button">
                    "Upload blueprint or 3D model"
                    <input type="file" on:change=on_file/>
                </label>
                <Show when=move || current.get().is_some()>
                    <a class="button" download
                       href=move || current.get().map(|s| format!("/api/designs/{}/file", s.id)).unwrap_or_default()>
                        "Download .sment"
                    </a>
                    <button title="Undo (Ctrl+Z)"
                            disabled=move || ctx.viewing.get().is_some() || ctx.history.get().is_some_and(|(undo, _)| undo == 0)
                            on:click=move |_| ctx.history_step(Op::Undo)>"Undo"</button>
                    <button title="Redo (Ctrl+Shift+Z)"
                            disabled=move || ctx.viewing.get().is_some() || ctx.history.get().is_some_and(|(_, redo)| redo == 0)
                            on:click=move |_| ctx.history_step(Op::Redo)>"Redo"</button>
                    <span class="sync">{move || if ctx.pending.get() > 0 { "Saving..." } else { "Saved" }}</span>
                </Show>
                <Show when=move || current.get().is_some()>
                    <chat::Assistant ctx/>
                </Show>
                <span class="game" class:away=move || ctx.game.get().seconds_ago.is_none_or(|s| s > 45)>
                    {move || game_line(&ctx.game.get())}
                </span>
                <span class="status" class:error=move || status.get().1 title=move || status.get().0>{move || status.get().0}</span>
            </header>
            <div class="main">
                <aside class="side">
                    <Show when=move || current.get().is_some()>
                        <DesignInfo ctx/>
                    </Show>
                    <div class="designs">
                        <h2>"Designs"</h2>
                        <Show when=move || designs.get().is_empty()>
                            <p class="empty">"Nothing here yet. Start a new ship or upload a blueprint."</p>
                        </Show>
                        <Show when=move || { designs.with(|d| d.len() > 8) }>
                            <input class="search" type="search" placeholder="Filter designs"
                                   prop:value=move || filter.get()
                                   on:input=move |ev| filter.set(event_target_value(&ev))/>
                        </Show>
                        <For each=move || listed.get().0 key=|d| (d.id.clone(), d.blocks, d.name.clone(), d.sync) let:d>
                            {
                                let id = d.id.clone();
                                let id_for_class = d.id.clone();
                                view! {
                                    <button class="design"
                                            class:current=move || current.get().is_some_and(|c| c.id == id_for_class)
                                            on:click=move |_| ctx.open_design(id.clone(), true)>
                                        {d.name.clone()}
                                        <small>
                                            {thousands(d.blocks)} " blocks"
                                            {d.sync.map(|s| view! {
                                                <span class="sync" class:waiting=s == SyncStatus::Waiting
                                                      class:off=matches!(s, SyncStatus::Off | SyncStatus::Gone)>
                                                    {sync_label(s)}
                                                </span>
                                            })}
                                        </small>
                                    </button>
                                }
                            }
                        </For>
                        <Show when=move || { listed.get().1 > 0 }>
                            <p class="empty">{move || listed.get().1} " more. Type in the filter to narrow the list."</p>
                        </Show>
                    </div>
                </aside>
                <section class="view">
                    <canvas node_ref=canvas_ref
                            on:mousedown=on_down on:mousemove=on_move on:mouseup=on_up on:mouseleave=on_leave on:dblclick=on_double
                            on:wheel=on_wheel on:contextmenu=move |ev: leptos::ev::MouseEvent| ev.prevent_default()/>
                    <Show when=move || current.get().is_none()>
                        <div class="drop">"Start a new ship or upload a StarMade blueprint (.sment)."</div>
                    </Show>
                    {move || ctx.viewing.get().map(|v| view! { <VersionBanner version=v ctx/> })}
                    {move || ctx.core_preview.get().map(|p| view! { <CorePreviewBanner preview=p ctx/> })}
                    <Show when=move || ctx.looking.get()>
                        <div class="crosshair"></div>
                    </Show>
                    <Show when=move || current.get().is_some() && ctx.entity_path.get() != work::WHOLE>
                        <panel::BrushPanel ctx/>
                    </Show>
                    <Show when=move || ctx.brush_shown.get() && current.get().is_some() && ctx.entity_path.get() != work::WHOLE>
                        <div class="brush-badge">
                            {move || {
                                let b = ctx.brush.get();
                                let shape = match ctx.brush_shape.get() {
                                    work::BrushShape::Cube => "Cube",
                                    work::BrushShape::Cylinder => "Cylinder",
                                    work::BrushShape::Sphere => "Sphere",
                                };
                                format!("{shape} brush {} × {} × {}", b[0], b[1], b[2])
                            }}
                        </div>
                    </Show>
                    <div class="hint">
                        <div class="hover">{move || ctx.hover.get()}</div>
                        {move || if ctx.looking.get() {
                            "Mouse look: move the mouse to look round, click to use the tool on the block in the middle, W A S D E Q to move, Esc to stop".to_string()
                        } else if ctx.entity_path.get() == work::WHOLE {
                            "The whole design, for looking at".to_string()
                        } else {
                            format!("{}. Middle-click picks a block", ctx.tool.get().help(ctx.swap.get()))
                        }}
                        {move || if ctx.looking.get() {
                            "."
                        } else if ctx.mouse_look.get() {
                            ". Drag to rotate, right-drag or shift-drag to pan, scroll to zoom, W A S D E Q to move, double-click for mouse look."
                        } else {
                            ". Drag to rotate, right-drag or shift-drag to pan, scroll to zoom, W A S D E Q to move."
                        }}
                    </div>
                </section>
                <Show when=move || current.get().is_some()>
                    <Show when=move || ctx.entity_path.get() == work::WHOLE fallback=move || view! { <panel::BuildPanel ctx/> }>
                        <panel::DockPanel ctx/>
                    </Show>
                </Show>
            </div>
        </div>
    }
}

/// The open design's link to a design held in a shipyard in the game, or the
/// way to make one.
#[component]
fn YardLink(ctx: Ctx) -> impl IntoView {
    view! {
        <div class="yard">
            {move || match (ctx.yard.get(), ctx.yards.get()) {
                (Some(link), _) => view! {
                    <div title="This design and a design held in a shipyard in the game follow each other while that design is loaded in its shipyard">
                        "Linked to "
                        {link.name.clone().map_or_else(|| "a new design".to_string(), |n| format!("\"{n}\""))}
                        " in the shipyard of " {link.station.clone()}
                        {format!(" (at {}, {}, {})", link.at[0], link.at[1], link.at[2])}
                    </div>
                    <div class="sync" class:waiting=!link.in_step>{link.status.clone()}</div>
                    <button class="link" title="Stop following the shipyard design; both keep what they hold"
                            on:click=move |_| ctx.unlink_yard()>"Unlink from the shipyard"</button>
                }.into_any(),
                (None, None) => view! {
                    <button class="link" title="Keep this design in step with a design held in a shipyard in the game"
                            on:click=move |_| ctx.pick_yard(true)>"Link to a shipyard design..."</button>
                }.into_any(),
                (None, Some(yards)) => view! { <YardPicker ctx=ctx yards=yards/> }.into_any(),
            }}
        </div>
    }
}

/// The game's shipyards and the designs they hold, to link the open design to.
#[component]
fn YardPicker(ctx: Ctx, yards: Vec<yard_core::api::GameYard>) -> impl IntoView {
    use yard_core::api::{YardKeep, YardState};
    let empty = yards.is_empty();
    view! {
        <div class="yard-pick">
            <div class="row">
                <b>"Link to a shipyard design"</b>
                <button class="link" on:click=move |_| ctx.pick_yard(false)>"Cancel"</button>
            </div>
            {empty.then(|| view! {
                <p class="empty note">"The game reports no shipyards yet. A shipyard shows up here while its station is loaded in the game (a player is near it) and belongs to the faction of the sync's owner."</p>
            })}
            {yards.into_iter().map(|y| {
                let free = y.state == YardState::Idle;
                let yard_id = y.id.clone();
                let state = match y.state {
                    YardState::Idle => "free".to_string(),
                    YardState::Loaded => y.loaded.as_ref().and_then(|uid| y.designs.iter().find(|d| d.uid == *uid)).map_or("a design loaded".into(), |d| format!("\"{}\" loaded", d.name)),
                    YardState::Busy => "busy".to_string(),
                };
                view! {
                    <div class="yard-one">
                        <div class="yard-name">{format!("{}, shipyard at {}, {}, {}", y.station, y.at[0], y.at[1], y.at[2])} <small>{format!(" ({state})")}</small></div>
                        {y.designs.into_iter().map(|d| {
                            let (here, there) = (yard_id.clone(), yard_id.clone());
                            let (uid_here, uid_there) = (d.uid.clone(), d.uid.clone());
                            view! {
                                <div class="yard-design">
                                    <span>{d.name.clone()}</span>
                                    <button title="This design replaces the shipyard's design, and they follow each other from then on"
                                            on:click=move |_| ctx.link_yard(here.clone(), Some(uid_here.clone()), YardKeep::Here)>"Send this one"</button>
                                    <button title="The shipyard's design replaces this one (what this one holds now is kept as a separate design), and they follow each other from then on"
                                            on:click=move |_| ctx.link_yard(there.clone(), Some(uid_there.clone()), YardKeep::Shipyard)>"Take the shipyard's"</button>
                                </div>
                            }
                        }).collect_view()}
                        <button class="yard-new" disabled=!free
                                title=if free { "Make a new design in this shipyard from this one" } else { "The shipyard has a design loaded; unload it in the game to make a new one here" }
                                on:click=move |_| ctx.link_yard(yard_id.clone(), None, YardKeep::Here)>"New design in this shipyard"</button>
                    </div>
                }
            }).collect_view()}
            <p class="empty note">"Changes move while the design is loaded in its shipyard. A design is replaced in the shipyard by unloading it, making a new one from the game's catalog and removing the old one; this needs one free slot in the shipyard's inventory."</p>
        </div>
    }
}

/// The side panel for the open design: its entities, block totals and warnings.
/// It stays in place while the design changes under it, so what is unfolded
/// or scrolled to stays so through every edit.
#[component]
fn DesignInfo(ctx: Ctx) -> impl IntoView {
    let id = Memo::new(move |_| ctx.current.with(|c| c.as_ref().map(|s| s.id.clone()).unwrap_or_default()));
    let status = Memo::new(move |_| ctx.sync_of(&id.get()));
    // Read one thing of the open design, re-read when the design changes.
    fn of<T: Default>(ctx: Ctx, read: impl Fn(&Summary) -> T) -> T {
        ctx.current.with(|c| c.as_ref().map(&read).unwrap_or_default())
    }
    let block_search = RwSignal::new(String::new());
    view! {
        <div class="info">
            <div class="name">{move || of(ctx, |s| s.name.clone())}</div>
            <div class="empty">{move || of(ctx, |s| thousands(s.blocks))} " blocks"</div>
            {move || ctx.dims.with(|d| d.whole).map(|size| view! {
                <div class="empty" title="The space the design takes up in blocks: across (X), up (Y) and from back to front (Z), with every docked entity at rest where it docks. Turrets that turn can sweep a little further.">
                    {format!("{} wide × {} high × {} long", size[0], size[1], size[2])}
                    {(of(ctx, |s| s.entities.len()) > 1).then_some(", docked parts included")}
                </div>
            })}
            <Show when=move || status.get().is_some()>
                <label class="check" title="While ticked, this design and the blueprint of the same name in the game follow each other">
                    <input type="checkbox" prop:checked=move || !matches!(status.get(), Some(SyncStatus::Off | SyncStatus::Gone))
                           on:change=move |ev| ctx.switch_sync(id.get_untracked(), event_target_checked(&ev))/>
                    "Sync with the game server"
                    <span class="sync">{move || status.get().map(sync_label)}</span>
                </label>
                <Show when=move || status.get() == Some(SyncStatus::Waiting)>
                    <button class="sync-now" title="Send this design to the game now instead of waiting until it has been left alone for a while"
                            on:click=move |_| ctx.sync_now(id.get_untracked())>"Sync now"</button>
                </Show>
                <Show when=move || matches!(status.get(), Some(SyncStatus::Synced | SyncStatus::Waiting)) || ctx.yard.with(Option::is_some)>
                    <YardLink ctx=ctx/>
                </Show>
            </Show>
            <Show when=move || ctx.renaming.with(Option::is_some)
                  fallback=move || view! {
                      <div class="row">
                          <button class="link" title="Give this design another name"
                                  on:click=move |_| ctx.renaming.set(Some(of(ctx, |s| s.name.clone())))>"Rename..."</button>
                          <button class="link-danger" title="Remove this blueprint from the shipyard" on:click=move |_| ctx.deleting.set(true)>"Delete design..."</button>
                      </div>
                  }>
                <form class="rename" on:submit=move |ev| { ev.prevent_default(); ctx.rename_design(); }>
                    <input type="text" maxlength=yard_core::blueprint::MAX_NAME.to_string() autofocus
                           prop:value=move || ctx.renaming.get().unwrap_or_default()
                           on:input=move |ev| ctx.renaming.set(Some(event_target_value(&ev)))
                           on:keydown=move |ev| if ev.key() == "Escape" { ctx.renaming.set(None); }/>
                    <div class="row">
                        <button type="submit" class="current">"Rename"</button>
                        <button type="button" on:click=move |_| ctx.renaming.set(None)>"Cancel"</button>
                    </div>
                    <p class="empty note">"Letters, digits, spaces, - and _. The game cannot rename a blueprint: one it has under the old name stays there, and this design goes to the game under the new one."</p>
                </form>
            </Show>
            {move || of(ctx, |s| s.warnings.clone()).into_iter().map(|w| view! { <p class="warning">{w}</p> }).collect_view()}
            <Show when=move || !ctx.versions.get().is_empty()>
                <button class="fold" class:open=move || ctx.versions_open.get()
                        on:click=move |_| ctx.versions_open.update(|open| *open = !*open)>
                    "Sync history (" {move || ctx.versions.get().len()} ")"
                </button>
                <Show when=move || ctx.versions_open.get()>
                    <p class="empty note">"The last " {yard_core::api::SYNC_VERSIONS_KEPT} " versions that were synced with the game are kept. Click one to look at it."</p>
                    <For each=move || ctx.versions.get() key=|v| (v.seq, v.current, v.direction) let:v>
                        {
                            let seq = v.seq;
                            let shown = v.clone();
                            view! {
                                <button class="version"
                                        class:current=move || ctx.viewing.get().is_some_and(|x| x.seq == seq)
                                        on:click=move |_| ctx.view_version(shown.clone())>
                                    {when(v.time)}
                                    <small>
                                        {direction_label(v.direction)} ", " {thousands(v.blocks)} " blocks"
                                        {v.current.then_some(", the design as it is now")}
                                    </small>
                                </button>
                            }
                        }
                    </For>
                </Show>
            </Show>
            <h2>"Entities"</h2>
            <Show when=move || { of(ctx, |s| s.entities.len()) > 1 }>
                <button class="entity" class:current=move || ctx.entity_path.get() == work::WHOLE
                        title="Every docked entity in its place on the main one. For looking, and for trying how the docked parts move."
                        on:click=move |_| ctx.show_entity(work::WHOLE.to_string(), true)>
                    "Whole design"
                    <br/>
                    <small>{move || of(ctx, |s| s.entities.len())} " entities, docked together"</small>
                </button>
            </Show>
            {move || {
                of(ctx, |s| s.entities.clone())
                    .into_iter()
                    .map(|e| {
                        let path = e.path.clone();
                        let path_for_class = e.path.clone();
                        view! {
                            <button class="entity"
                                    class:current=move || ctx.entity_path.get() == path_for_class
                                    on:click=move |_| ctx.show_entity(path.clone(), true)>
                                {entity_label(&e.path)}
                                <br/>
                                <small>{thousands(e.blocks)} " blocks, " {thousands(e.links as u64)} " links"</small>
                                {
                                    let path = e.path.clone();
                                    move || ctx.dims.with(|d| d.entities.get(&path).copied()).map(|size| view! {
                                        <br/>
                                        <small>{format!("{} × {} × {} (wide × high × long)", size[0], size[1], size[2])}</small>
                                    })
                                }
                            </button>
                        }
                    })
                    .collect_view()
            }}
            <h2>{move || if ctx.entity_path.get() == work::WHOLE { "Blocks used" } else { "Blocks in this entity" }}</h2>
            <input type="search" class="block-search" placeholder="Search blocks"
                   prop:value=move || block_search.get()
                   on:input=move |ev| block_search.set(event_target_value(&ev))/>
            <div class="empty block-count">
                {move || {
                    let words = search_words(&block_search.get());
                    let (all, shown) = ctx.used_blocks.with(|l| (l.len(), l.iter().filter(|(_, name, _)| matches_words(name, &words)).count()));
                    match (all, shown == all) {
                        (1, true) => "1 type".to_string(),
                        (_, true) => format!("{all} types"),
                        _ => format!("{shown} of {all} types"),
                    }
                }}
            </div>
            <table class="used">
                {move || {
                    let words = search_words(&block_search.get());
                    ctx.used_blocks
                        .get()
                        .into_iter()
                        .filter(|(_, name, _)| matches_words(name, &words))
                        .map(|(id, name, count)| {
                            let label = name.clone();
                            view! {
                                <tr class="pick" class:current=move || ctx.highlight.get() == Some(id)
                                    title="Click to mark these blocks through the entity"
                                    on:click=move |_| ctx.used_block_clicked(id, name.clone())>
                                    <td>{label}</td><td>{thousands(count)}</td>
                                </tr>
                            }
                        })
                        .collect_view()
                }}
            </table>
        </div>
    }
}

/// The words of a search, in lower case.
fn search_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether a name holds every word of a search.
fn matches_words(name: &str, words: &[String]) -> bool {
    let name = name.to_lowercase();
    words.iter().all(|w| name.contains(w.as_str()))
}

/// Shown over the viewport while an old version is on screen: what it is and
/// what can be done with it.
/// Over the whole-design view while a core move is previewed on a copy of
/// the design: what it would do, and the way to make it or to go back.
#[component]
fn CorePreviewBanner(preview: work::CorePreview, ctx: Ctx) -> impl IntoView {
    let what = format!("Previewing {}; nothing is saved. Play shows the turrets turning about their lines. {}", preview.about, preview.notes.join(" "));
    view! {
        <div class="banner">
            <span>{what}</span>
            <button class="primary" on:click=move |_| ctx.confirm_relocation()>"Confirm"</button>
            <button title="The design as it is, with the cell chosen for the core still marked" on:click=move |_| ctx.end_relocation_preview()>"Back to choosing"</button>
            <button on:click=move |_| ctx.cancel_relocation()>"Cancel"</button>
        </div>
    }
}

#[component]
fn VersionBanner(version: SyncVersion, ctx: Ctx) -> impl IntoView {
    let seq = version.seq;
    let file = ctx.current.with_untracked(|c| c.as_ref().map(|s| api::version_file_url(&s.id, seq))).unwrap_or_default();
    view! {
        <div class="banner">
            <span>
                "Looking at " {version_phrase(version.direction)} " " {when(version.time)}
                {if version.current { ". The design is the same now." } else { ". It cannot be edited here." }}
            </span>
            <button class="primary" on:click=move |_| ctx.back_to_current()>"Back to the current version"</button>
            <Show when=move || !version.current>
                <button title="Makes this version the design again; the version it replaces stays in the history"
                        on:click=move |_| ctx.restore_version(seq)>"Restore this version"</button>
            </Show>
            <button title="Keeps the design as it is and stores this version as a separate design, not synced"
                    on:click=move |_| ctx.copy_version(seq)>"Save as a new design"</button>
            <a class="button" download href=file>"Download"</a>
        </div>
    }
}
