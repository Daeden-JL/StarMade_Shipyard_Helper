//! The chat agent as the server runs it: one conversation per design, run
//! in the background while the browser asks for what has happened so far.
//!
//! The model's key stays here. The browser is told which models can be
//! used and nothing else about how they are reached.

use crate::app::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use yard_agent::{CancelFlag, Claude, Conversation, EntityInfo, Event, ModelInfo, OpenAiCompatible, Provider, RunOptions, World};
use yard_core::edit::{Change, Op};
use yard_core::Entity;

/// How long LM Studio keeps an idle model the assistant asked for.
const KEEP_LOADED_SECS: u32 = 3600;

/// Most events kept for the browser to read back, per design.
const LOG_KEPT: usize = 4000;

/// How the server reaches models.
#[derive(Clone)]
pub struct ChatConfig {
    /// A Claude API key. For testing it is the owner's; in production each
    /// server admin supplies their own. Never sent to a browser or logged.
    anthropic_key: Option<String>,
    /// An OpenAI-compatible endpoint such as LM Studio, with `/v1`.
    pub local_url: String,
    local_key: Option<String>,
}

impl std::fmt::Debug for ChatConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatConfig").field("claude", &self.anthropic_key.is_some()).field("local_url", &self.local_url).finish()
    }
}

impl ChatConfig {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        ChatConfig {
            anthropic_key: var("YARD_ANTHROPIC_KEY").or_else(|| var("ANTHROPIC_API_KEY")),
            local_url: var("YARD_LOCAL_MODEL_URL").unwrap_or_else(|| "http://127.0.0.1:1234/v1".to_string()),
            local_key: var("YARD_LOCAL_MODEL_KEY"),
        }
    }

}

/// What the picker offers.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Models {
    pub models: Vec<ModelInfo>,
    /// Sentences about models that cannot be offered, for the player.
    pub notes: Vec<String>,
}

pub async fn models(config: &ChatConfig) -> Models {
    let mut out = Models::default();
    if config.anthropic_key.is_some() {
        out.models.extend(Claude::models());
    } else {
        out.notes.push("Claude models are not offered: the server has no Claude API key (set YARD_ANTHROPIC_KEY).".to_string());
    }
    let listed = tokio::time::timeout(std::time::Duration::from_secs(4), OpenAiCompatible::list_models(&config.local_url, config.local_key.as_deref())).await;
    match listed {
        Ok(Ok(local)) => out.models.extend(local),
        _ => out.notes.push("No local model server answered, so no local models are offered.".to_string()),
    }
    out
}

/// What the player asked for.
#[derive(Deserialize, Debug)]
pub struct Ask {
    /// "claude" or "openai"
    pub provider: String,
    pub model: String,
    pub message: String,
    /// "low", "medium", "high"...; leave out for the model's own default.
    #[serde(default)]
    pub effort: Option<String>,
    /// The entity the player has open; tools work on it unless the model
    /// names another. Leave out for the main entity.
    #[serde(default)]
    pub entity: String,
}

/// One thing that happened, for the browser.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    /// The player's message.
    You { text: String },
    /// A model event: text, a tool call, the end.
    Agent { event: Event },
    /// The design changed: the browser applies the same change to its copy.
    Step { entity: String, change: Change, before: u64, after: u64 },
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct LogPage {
    /// The number of the first entry in `entries`.
    pub from: usize,
    /// Where to ask from next time.
    pub next: usize,
    pub running: bool,
    pub entries: Vec<Entry>,
}

#[derive(Default)]
struct Log {
    /// How many entries have been dropped from the front.
    base: usize,
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Chat {
    conversation: Arc<tokio::sync::Mutex<Conversation>>,
    log: Arc<Mutex<Log>>,
    running: Arc<AtomicBool>,
    cancel: Arc<Mutex<CancelFlag>>,
}

#[derive(Default)]
pub struct Chats {
    chats: Mutex<HashMap<String, Chat>>,
}

/// Why a message was not taken.
#[derive(Debug)]
pub enum Refusal {
    Busy,
    NoKey,
    UnknownProvider,
}

impl Refusal {
    pub fn sentence(&self) -> &'static str {
        match self {
            Refusal::Busy => "The assistant is still working on the last message for this design.",
            Refusal::NoKey => "The server has no Claude API key, so Claude cannot be used.",
            Refusal::UnknownProvider => "Choose a model from the list.",
        }
    }
}

/// The agent's view of one open design.
struct DesignWorld {
    state: Arc<AppState>,
    id: String,
    log: Arc<Mutex<Log>>,
}

fn push(log: &Mutex<Log>, entry: Entry) {
    let mut log = log.lock().unwrap();
    log.entries.push(entry);
    if log.entries.len() > LOG_KEPT {
        let drop = log.entries.len() - LOG_KEPT / 2;
        log.entries.drain(..drop);
        log.base += drop;
    }
}

impl World for DesignWorld {
    fn catalog(&self) -> &yard_core::catalog::Catalog {
        &self.state.catalog
    }

    fn entities(&self) -> Vec<EntityInfo> {
        self.state.sessions.read(&self.state.store, &self.id, yard_agent::entity_infos).unwrap_or_default()
    }

    fn with_entity(&self, path: &str, read: &mut dyn FnMut(&Entity)) -> Result<(), String> {
        let found = self.state.sessions.read(&self.state.store, &self.id, |bp| bp.entity(path).map(|e| read(&e.entity)).is_some());
        match found {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("There is no entity \"{path}\" in this design.")),
            Err(_) => Err("The design could not be read.".to_string()),
        }
    }

    fn step(&self, path: &str, ops: Vec<Op>) -> Result<Change, String> {
        let done = self.state.sessions.step_here(&self.state.store, &self.state.catalog, &self.id, path, &ops);
        match done {
            Ok((before, after, change)) => {
                if !change.is_empty() {
                    push(&self.log, Entry::Step { entity: path.to_string(), change: change.clone(), before, after });
                }
                Ok(change)
            }
            Err(crate::sessions::StepError::Refused(e)) => Err(e.to_string()),
            Err(crate::sessions::StepError::NoSuchEntity) => Err(format!("There is no entity \"{path}\" in this design.")),
            Err(_) => Err("The design could not be edited.".to_string()),
        }
    }
}

impl Chats {
    fn chat(&self, id: &str) -> Chat {
        self.chats
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert_with(|| Chat {
                conversation: Arc::default(),
                log: Arc::default(),
                running: Arc::default(),
                cancel: Arc::default(),
            })
            .clone()
    }

    /// Starts the assistant on a message. It runs in the background.
    pub fn ask(&self, config: &ChatConfig, state: Arc<AppState>, id: &str, ask: Ask) -> Result<(), Refusal> {
        let provider = provider_for(config, &ask)?;
        self.start(provider, state, id, ask.message, ask.entity)
    }

    /// Starts the given model on a message.
    pub fn start(&self, provider: Box<dyn Provider>, state: Arc<AppState>, id: &str, message: String, focus: String) -> Result<(), Refusal> {
        let chat = self.chat(id);
        if chat.running.swap(true, Ordering::SeqCst) {
            return Err(Refusal::Busy);
        }
        let cancel = CancelFlag::new();
        *chat.cancel.lock().unwrap() = cancel.clone();
        push(&chat.log, Entry::You { text: message.clone() });
        let world = DesignWorld { state, id: id.to_string(), log: chat.log.clone() };
        let (log, running, conversation) = (chat.log.clone(), chat.running.clone(), chat.conversation.clone());
        tokio::spawn(async move {
            let mut conversation = conversation.lock().await;
            let mut seen_end = false;
            let mut sink = |event: Event| {
                seen_end |= matches!(event, Event::Done { .. } | Event::Failed { .. });
                push(&log, Entry::Agent { event });
            };
            let options = RunOptions { focus, ..RunOptions::default() };
            let result = yard_agent::run(provider.as_ref(), &world, &mut conversation, &message, &options, &cancel, &mut sink).await;
            if let (Err(e), false) = (result, seen_end) {
                push(&log, Entry::Agent { event: Event::Failed { message: e.to_string() } });
            }
            running.store(false, Ordering::SeqCst);
        });
        Ok(())
    }

    /// What has happened since entry number `from`.
    pub fn log(&self, id: &str, from: usize) -> LogPage {
        let chat = self.chat(id);
        let log = chat.log.lock().unwrap();
        let start = from.max(log.base);
        let entries: Vec<Entry> = log.entries.iter().skip(start - log.base).cloned().collect();
        LogPage { from: start, next: start + entries.len(), running: chat.running.load(Ordering::SeqCst), entries }
    }

    pub fn stop(&self, id: &str) {
        self.chat(id).cancel.lock().unwrap().cancel();
    }

    /// Forgets the conversation. Refused while the assistant is working.
    pub fn reset(&self, id: &str) -> bool {
        let chat = self.chat(id);
        if chat.running.load(Ordering::SeqCst) {
            return false;
        }
        self.chats.lock().unwrap().remove(id);
        true
    }
}

fn provider_for(config: &ChatConfig, ask: &Ask) -> Result<Box<dyn Provider>, Refusal> {
    Ok(match ask.provider.as_str() {
        "claude" => {
            let key = config.anthropic_key.clone().ok_or(Refusal::NoKey)?;
            let mut claude = Claude::new(key, ask.model.clone());
            if let Some(effort) = &ask.effort {
                claude = claude.with_effort(effort.clone());
            }
            Box::new(claude)
        }
        "openai" => {
            // An hour: a model LM Studio loaded for the question stays for the
            // whole answer and the next question, whatever its own setting.
            let mut local = OpenAiCompatible::new(config.local_url.clone(), ask.model.clone()).with_keep_loaded(KEEP_LOADED_SECS);
            if let Some(key) = &config.local_key {
                local = local.with_key(key.clone());
            }
            if let Some(effort) = &ask.effort {
                local = local.with_reasoning_effort(effort.clone());
            }
            Box::new(local)
        }
        _ => return Err(Refusal::UnknownProvider),
    })
}
