//! What the agent loop needs from a language model, whoever serves it.

use crate::message::{Raw, ToolCall, Turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// A tool as a model is told about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments, an object.
    pub schema: Value,
}

/// Why a model stopped writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// It has said what it had to say.
    EndTurn,
    /// It wants its tool calls run.
    ToolUse,
    /// It ran into the output limit, possibly in the middle of a tool call.
    MaxTokens,
    /// The model or its provider's safety system declined.
    Refusal,
    /// A provider-side tool loop paused. Not expected here: all tools run on
    /// this side.
    PauseTurn,
    Other(String),
}

/// Tokens one model call used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Everything the model read, cached or not.
    pub input: u64,
    pub output: u64,
    /// The part of `input` that was served from the provider's prompt cache.
    pub cache_read: u64,
    /// The part of `input` that was written to the cache.
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// One model call: the whole conversation so far and the tools on offer.
pub struct Request<'a> {
    pub system: &'a str,
    pub turns: &'a [Turn],
    pub tools: &'a [ToolSpec],
    /// Overrides the provider's own output limit.
    pub max_tokens: Option<u32>,
    /// How hard the model should think: `low`, `medium` or `high`
    /// everywhere, `xhigh` and `max` on Claude. Overrides the provider's own.
    pub reasoning_effort: Option<&'a str>,
}

/// What a model answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The text for the player, without any reasoning.
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop: StopReason,
    pub usage: Usage,
    /// The answer as the provider wants it back, if it wants it back.
    pub raw: Option<Raw>,
}

/// Why a model call failed. The messages are fit to show to a player and
/// never contain a key.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    Overloaded(String),
    /// The request was refused as malformed or too large.
    #[error("{0}")]
    BadRequest(String),
    /// Claude refused thinking blocks sent back to it. The loop drops them
    /// from the conversation and asks again.
    #[error("{0}")]
    ThinkingRejected(String),
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Server(String),
    /// The answer could not be understood.
    #[error("{0}")]
    Protocol(String),
}

impl ProviderError {
    /// Whether asking again in a moment may work.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimited(_) | Self::Overloaded(_) | Self::Timeout(_) | Self::Network(_) | Self::Server(_))
    }
}

/// A model that can be offered in the picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    /// A name for the picker.
    pub name: String,
    /// `claude` or `openai`.
    pub provider: String,
}

/// Where a provider sends the answer's text as it arrives.
pub type TextSink<'a> = dyn for<'t> FnMut(&'t str) + Send + 'a;

/// A language model behind some API.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Names the wire format: `claude` or `openai`. An assistant turn's raw
    /// content is only sent back to a provider of the kind that produced it.
    fn kind(&self) -> &'static str;

    fn model(&self) -> &str;

    /// How large the conversation may grow, in characters, before old tool
    /// outputs are dropped from it.
    fn history_budget(&self) -> usize;

    /// Makes one streamed model call. Text for the player goes to `on_text`
    /// piece by piece as it arrives; the whole answer is returned at the end.
    /// Dropping the future abandons the call.
    async fn call(&self, request: &Request<'_>, on_text: &mut TextSink<'_>) -> Result<Reply, ProviderError>;
}

/// How a provider waits and retries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patience {
    /// Longest wait for the answer to begin, and between two pieces of it.
    pub silence: Duration,
    /// Times a call is made again after an overload, a rate limit or a
    /// broken connection, as long as nothing of the answer was passed on.
    pub retries: u32,
    /// Wait before the first retry; doubled for each one after. A
    /// `retry-after` from the server takes precedence, up to `longest_wait`.
    pub backoff: Duration,
    pub longest_wait: Duration,
}

impl Patience {
    /// The wait before retry number `attempt` (counted from 0).
    pub(crate) fn wait(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        retry_after.unwrap_or(self.backoff * 2u32.saturating_pow(attempt)).min(self.longest_wait)
    }
}

/// Seconds from a `retry-after` header.
pub(crate) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds = headers.get("retry-after")?.to_str().ok()?.trim().parse::<f64>().ok()?;
    (seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// A transport failure as a sentence, without the URL's query or any header.
pub(crate) fn network_error(what: &str, e: &reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout(format!("{what} did not answer in time."))
    } else if e.is_connect() {
        ProviderError::Network(format!("{what} could not be reached. Is it running and is the address right?"))
    } else {
        ProviderError::Network(format!("The connection to {what} failed."))
    }
}

/// The first `max` characters of a text.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}
