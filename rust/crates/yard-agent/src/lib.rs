//! The chat agent: talks to a language model and builds with tools.
//!
//! A player types what they want; a model answers and builds by calling
//! tools that edit the design. This crate is the model-facing half: the
//! providers ([`Claude`], [`OpenAiCompatible`]), the tool loop ([`run`]) and
//! the tools ([`tools`]). What it edits is behind the [`World`] trait, which
//! the web server implements over its open designs.

pub mod agent;
pub mod claude;
pub mod filter;
pub mod guides;
pub mod message;
pub mod openai;
pub mod provider;
pub mod schema;
pub mod scripted;
pub mod sse;
pub mod tools;
pub mod world;

pub use agent::{run, AgentError, CancelFlag, Event, Outcome, RunOptions, Stop, SYSTEM_PROMPT};
pub use claude::Claude;
pub use message::{Conversation, Raw, StepMark, ToolCall, ToolResult, Turn};
pub use openai::OpenAiCompatible;
pub use provider::{ModelInfo, Patience, Provider, ProviderError, Reply, Request, StopReason, TextSink, ToolSpec, Usage};
pub use scripted::{Script, ScriptedProvider};
pub use world::{entity_infos, sample_catalog, EntityInfo, EntityKind, MemoryWorld, World};
