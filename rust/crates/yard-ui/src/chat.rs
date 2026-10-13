//! The assistant: a chat with a model that builds in the open design.
//!
//! The server runs the model and keeps the conversation. This panel sends
//! what the player types, asks for what has happened now and then, and
//! applies the assistant's edits to the design on screen as they come.

use crate::api::{self, ChatModel};
use crate::work::Ctx;
use leptos::prelude::*;
use leptos::task::spawn_local;
use yard_core::edit::Change;

/// One line of the transcript.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    You(String),
    /// What the assistant said.
    Said(String),
    /// A tool it used.
    Tool { name: String, summary: String, ok: bool, changed: u64 },
    /// Something the player should know: a failure, a stop.
    Note(String),
}

/// Where the choice of model is kept between visits.
const MODEL_KEY: &str = "yard.chat_model";
const POLL_MS: i32 = 500;

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

fn stored_model() -> Option<String> {
    window().local_storage().ok().flatten().and_then(|s| s.get_item(MODEL_KEY).ok().flatten())
}

fn remember_model(key: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(MODEL_KEY, key);
    }
}

/// "provider/model", which names a choice in the list.
fn key_of(model: &ChatModel) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn stop_text(stop: &str) -> Option<&'static str> {
    match stop {
        "step_limit" => Some("It reached its step limit. Say \"continue\" to let it carry on."),
        "cancelled" => Some("Stopped."),
        "refused" => Some("The request was declined."),
        "stuck" => Some("It kept repeating a call that fails, so it stopped."),
        _ => None,
    }
}

#[component]
pub fn Assistant(ctx: Ctx) -> impl IntoView {
    let open = RwSignal::new(false);
    let models = RwSignal::new(api::ChatModels::default());
    let chosen = RwSignal::new(String::new());
    let effort = RwSignal::new(String::new());
    let lines = RwSignal::new(Vec::<Line>::new());
    let working = RwSignal::new(false);
    let text = RwSignal::new(String::new());
    // Which design the transcript belongs to, how far it has been read, and
    // which polling loop is current.
    let reading = StoredValue::new((String::new(), 0usize, 0u32));
    let transcript = NodeRef::<leptos::html::Div>::new();

    // The models on offer.
    Effect::new(move |_| {
        if open.get() && models.with_untracked(|m| m.models.is_empty() && m.notes.is_empty()) {
            spawn_local(async move {
                match api::chat_models().await {
                    Ok(found) => {
                        let remembered = stored_model();
                        let pick = found.models.iter().find(|m| Some(key_of(m)) == remembered).or(found.models.first()).map(key_of);
                        models.set(found);
                        if chosen.get_untracked().is_empty() {
                            chosen.set(pick.unwrap_or_default());
                        }
                    }
                    Err(e) => ctx.fail(e),
                }
            });
        }
    });

    let scroll_down = move || {
        if let Some(div) = transcript.get_untracked() {
            div.set_scroll_top(div.scroll_height());
        }
    };

    // Takes in what the server has to say from `next` on, and keeps asking
    // while it is working.
    let follow = move || {
        let Some(id) = ctx.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        let generation = {
            let mut generation = 0;
            reading.update_value(|(design, next, g)| {
                if *design != id {
                    *design = id.clone();
                    *next = 0;
                    lines.set(Vec::new());
                }
                *g += 1;
                generation = *g;
            });
            generation
        };
        spawn_local(async move {
            loop {
                let from = reading.with_value(|(_, next, _)| *next);
                let Ok(page) = api::chat_log(&id, from).await else { break };
                if reading.with_value(|(design, _, g)| *design != id || *g != generation) {
                    break;
                }
                let next = page["next"].as_u64().unwrap_or(from as u64) as usize;
                let running = page["running"].as_bool().unwrap_or(false);
                for entry in page["entries"].as_array().into_iter().flatten() {
                    match entry["kind"].as_str() {
                        Some("you") => lines.update(|l| l.push(Line::You(entry["text"].as_str().unwrap_or("").to_string()))),
                        Some("step") => {
                            if let Ok(change) = serde_json::from_value::<Change>(entry["change"].clone()) {
                                let (before, after) = (entry["before"].as_u64().unwrap_or(0), entry["after"].as_u64().unwrap_or(0));
                                ctx.apply_remote(entry["entity"].as_str().unwrap_or("").to_string(), Vec::new(), Some(change), before, after);
                            }
                        }
                        Some("agent") => {
                            let event = &entry["event"];
                            match event["type"].as_str() {
                                Some("text") => lines.update(|l| {
                                    let piece = event["text"].as_str().unwrap_or("");
                                    match l.last_mut() {
                                        Some(Line::Said(said)) => said.push_str(piece),
                                        _ => l.push(Line::Said(piece.to_string())),
                                    }
                                }),
                                Some("tool_finished") => lines.update(|l| {
                                    l.push(Line::Tool {
                                        name: event["name"].as_str().unwrap_or("").to_string(),
                                        summary: event["summary"].as_str().unwrap_or("").to_string(),
                                        ok: event["ok"].as_bool().unwrap_or(true),
                                        changed: event["changed"].as_u64().unwrap_or(0),
                                    })
                                }),
                                Some("failed") => lines.update(|l| l.push(Line::Note(event["message"].as_str().unwrap_or("It failed.").to_string()))),
                                Some("done") => {
                                    if let Some(note) = event["stop"].as_str().and_then(stop_text) {
                                        lines.update(|l| l.push(Line::Note(note.to_string())));
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                reading.update_value(|(_, n, _)| *n = next);
                working.set(running);
                scroll_down();
                if !running {
                    break;
                }
                sleep(POLL_MS).await;
            }
        });
    };

    // Another design: its conversation, read from the start.
    Effect::new(move |_| {
        let id = ctx.current.with(|c| c.as_ref().map(|s| s.id.clone()));
        if id.is_some() && open.get() {
            follow();
        }
    });

    let send = move || {
        let message = text.get_untracked().trim().to_string();
        let Some(id) = ctx.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) else { return };
        if message.is_empty() || working.get_untracked() {
            return;
        }
        let key = chosen.get_untracked();
        let Some(model) = models.with_untracked(|m| m.models.iter().find(|m| key_of(m) == key).cloned()) else {
            ctx.fail("Choose a model first.");
            return;
        };
        if ctx.viewing.get_untracked().is_some() {
            ctx.fail("An old version is on screen. Go back to the current version first.");
            return;
        }
        remember_model(&key);
        text.set(String::new());
        working.set(true);
        let effort = Some(effort.get_untracked()).filter(|e| !e.is_empty());
        let entity = ctx.entity_path.get_untracked();
        spawn_local(async move {
            match api::chat_ask(&id, &model, &message, effort.as_deref(), &entity).await {
                Ok(()) => follow(),
                Err(e) => {
                    working.set(false);
                    lines.update(|l| l.push(Line::Note(e)));
                }
            }
        });
    };

    let stop = move || {
        if let Some(id) = ctx.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) {
            spawn_local(async move {
                let _ = api::chat_stop(&id).await;
            });
        }
    };

    let fresh = move || {
        if let Some(id) = ctx.current.with_untracked(|c| c.as_ref().map(|s| s.id.clone())) {
            spawn_local(async move {
                match api::chat_reset(&id).await {
                    Ok(()) => {
                        reading.update_value(|(_, next, g)| {
                            *next = 0;
                            *g += 1;
                        });
                        lines.set(Vec::new());
                    }
                    Err(e) => ctx.fail(e),
                }
            });
        }
    };

    view! {
        <button class="assistant-toggle" class:current=move || open.get() title="Ask a model to build in this design"
                on:click=move |_| open.update(|o| *o = !*o)>"Assistant"</button>
        <Show when=move || open.get()>
            <section class="assistant">
                <div class="assistant-head">
                    <select title="Which model to use" on:change=move |ev| chosen.set(event_target_value(&ev))>
                        {move || models.get().models.into_iter().map(|m| {
                            let key = key_of(&m);
                            let selected = key == chosen.get_untracked();
                            view! { <option value=key selected=selected>{format!("{} ({})", m.name, if m.provider == "claude" { "Claude" } else { "local" })}</option> }
                        }).collect_view()}
                    </select>
                    <select title="How hard the model thinks" on:change=move |ev| effort.set(event_target_value(&ev))>
                        <option value="">"Default effort"</option>
                        <option value="low">"Low"</option>
                        <option value="medium">"Medium"</option>
                        <option value="high">"High"</option>
                    </select>
                    <button title="Forget this conversation" disabled=move || working.get() on:click=move |_| fresh()>"New"</button>
                </div>
                {move || models.get().notes.into_iter().map(|n| view! { <p class="empty">{n}</p> }).collect_view()}
                <div class="assistant-lines" node_ref=transcript>
                    {move || {
                        let current = lines.get();
                        if current.is_empty() {
                            return view! { <p class="empty">"Say what to build or change, for example: a small fighter hull with two cannons. It works in the entity on the left of the view, and you can undo what it does."</p> }.into_any();
                        }
                        current.into_iter().map(|line| match line {
                            Line::You(t) => view! { <p class="you">{t}</p> }.into_any(),
                            Line::Said(t) => view! { <p class="said">{t}</p> }.into_any(),
                            Line::Tool { name, summary, ok, changed } => view! {
                                <p class="tool" class:bad=!ok>
                                    <b>{name}</b> " " {summary} {(changed > 0).then(|| format!(" ({changed} blocks)"))}
                                </p>
                            }.into_any(),
                            Line::Note(t) => view! { <p class="note">{t}</p> }.into_any(),
                        }).collect_view().into_any()
                    }}
                </div>
                <div class="assistant-input">
                    <input type="text" placeholder="What should it build?" prop:value=move || text.get()
                           disabled=move || ctx.current.with(|c| c.is_none())
                           on:input=move |ev| text.set(event_target_value(&ev))
                           on:keydown=move |ev: leptos::ev::KeyboardEvent| if ev.key() == "Enter" { send() }/>
                    <Show when=move || working.get() fallback=move || view! { <button on:click=move |_| send()>"Send"</button> }>
                        <button on:click=move |_| stop()>"Stop"</button>
                    </Show>
                </div>
            </section>
        </Show>
    }
}
