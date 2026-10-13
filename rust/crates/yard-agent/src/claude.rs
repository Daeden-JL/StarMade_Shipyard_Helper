//! Anthropic's Claude, over the Messages API with streaming.
//!
//! Spoken as plain HTTP; the request and event shapes follow Anthropic's API
//! reference. Three things shape this file beyond the wire format:
//!
//! - Thinking. The current models think before they answer, and the thinking
//!   blocks they return (with their signatures) must come back unmodified on
//!   the next request, so each assistant turn keeps its raw content.
//! - Caching. The system prompt and tool definitions are resent on every
//!   step of the tool loop; cache markers make the repeats cheap.
//! - The key. It is sent in the `x-api-key` header and appears nowhere else.

use crate::message::{Raw, ToolCall, Turn};
use crate::provider::{clip, network_error, retry_after, ModelInfo, Patience, Provider, TextSink, ProviderError, Reply, Request, StopReason, Usage};
use crate::sse::SseDecoder;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const API_VERSION: &str = "2023-06-01";
/// The beta that lets a request name `fallbacks: "default"`.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Room for thinking and a long answer; streaming keeps the connection alive.
const MAX_TOKENS: u32 = 64_000;
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The Claude provider for one model and one key.
pub struct Claude {
    http: reqwest::Client,
    key: String,
    model: String,
    base: String,
    max_tokens: u32,
    effort: Option<String>,
    /// Whether to ask Anthropic to rerun a request its safety system
    /// declined on another model. Switched off for good if the API rejects
    /// the beta that allows it.
    fallback: AtomicBool,
    patience: Patience,
    history_budget: usize,
}

/// The key is left out: this may end up in a log.
impl std::fmt::Debug for Claude {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claude").field("model", &self.model).field("base", &self.base).field("key", &"[hidden]").finish()
    }
}

impl Claude {
    pub const DEFAULT_MODEL: &'static str = "claude-opus-5-5";
    /// The models offered in the picker, best first after the default.
    pub const MODELS: [(&'static str, &'static str); 4] = [
        ("claude-opus-5-5", "Claude Opus 5.5"),
        ("claude-fable-5-1", "Claude Fable 5.1"),
        ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
    ];

    pub fn new(key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).build().unwrap_or_default(),
            key: key.into(),
            model: model.into(),
            base: "https://api.anthropic.com".to_string(),
            max_tokens: MAX_TOKENS,
            effort: None,
            fallback: AtomicBool::new(true),
            patience: Patience { silence: Duration::from_secs(180), retries: 2, backoff: Duration::from_secs(2), longest_wait: Duration::from_secs(20) },
            // These models read a million tokens; the budget is about cost.
            history_budget: 600_000,
        }
    }

    /// The models for the picker.
    pub fn models() -> Vec<ModelInfo> {
        Self::MODELS.iter().map(|(id, name)| ModelInfo { id: id.to_string(), name: name.to_string(), provider: "claude".to_string() }).collect()
    }

    /// Sends requests elsewhere: a test server, or a gateway that speaks the
    /// same API. No trailing slash, no `/v1`.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// How hard the model thinks: `low`, `medium`, `high`, `xhigh` or `max`.
    /// Ignored on models without the setting.
    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// Whether a request the safety system declines is rerun by Anthropic on
    /// another model (`fallbacks: "default"`, a beta). On unless switched off.
    pub fn with_refusal_fallback(self, on: bool) -> Self {
        self.fallback.store(on, Ordering::Relaxed);
        self
    }

    pub fn with_patience(mut self, patience: Patience) -> Self {
        self.patience = patience;
        self
    }

    pub fn with_history_budget(mut self, characters: usize) -> Self {
        self.history_budget = characters;
        self
    }

    fn sends_fallback(&self) -> bool {
        self.fallback.load(Ordering::Relaxed) && takes_fallback(&self.model)
    }

    /// The JSON body of one request.
    pub fn request_body(&self, request: &Request<'_>) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens.unwrap_or(self.max_tokens),
            "stream": true,
            // Tools render before the system prompt, so this one marker
            // caches both.
            "system": [{ "type": "text", "text": request.system, "cache_control": { "type": "ephemeral" } }],
            "messages": messages(request.turns),
        });
        if !request.tools.is_empty() {
            // Inputs stream as they are written; they are checked against
            // their schemas on this side before any tool runs.
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema, "eager_input_streaming": true }))
                .collect();
            body["tools"] = Value::Array(tools);
        }
        if thinks(&self.model) {
            body["thinking"] = json!({ "type": "adaptive" });
            let asked = request.reasoning_effort.or(self.effort.as_deref()).filter(|e| EFFORTS.contains(e));
            body["output_config"] = json!({ "effort": asked.unwrap_or_else(|| default_effort(&self.model)) });
        }
        if self.sends_fallback() {
            body["fallbacks"] = json!("default");
        }
        body
    }

    /// One HTTP exchange. On failure, also what the server said about when
    /// to try again.
    async fn attempt(&self, request: &Request<'_>, on_text: &mut TextSink<'_>) -> Result<Reply, (ProviderError, Option<Duration>)> {
        let fallback = self.sends_fallback();
        let mut key = reqwest::header::HeaderValue::from_str(self.key.trim())
            .map_err(|_| (ProviderError::Auth("The Claude API key contains characters that cannot be sent. Enter it again.".to_string()), None))?;
        key.set_sensitive(true);
        let mut call = self
            .http
            .post(format!("{}/v1/messages", self.base))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .json(&self.request_body(request));
        if fallback {
            call = call.header("anthropic-beta", FALLBACK_BETA);
        }
        let late = || (ProviderError::Timeout("Claude did not answer in time.".to_string()), None);
        let response = tokio::time::timeout(self.patience.silence, call.send()).await.map_err(|_| late())?.map_err(|e| (network_error("Claude", &e), None))?;
        let status = response.status();
        if !status.is_success() {
            let wait = retry_after(response.headers());
            let text = tokio::time::timeout(self.patience.silence, response.text()).await.map_err(|_| late())?.unwrap_or_default();
            return Err((self.failure(status.as_u16(), &text, fallback), wait));
        }
        let mut bytes = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut answer = ClaudeStream::default();
        while !answer.done {
            let Some(piece) = tokio::time::timeout(self.patience.silence, bytes.next()).await.map_err(|_| late())? else { break };
            let piece = piece.map_err(|e| (network_error("Claude", &e), None))?;
            for event in decoder.push(&piece) {
                answer.event(&event.data, on_text).map_err(|e| (e, None))?;
            }
        }
        answer.finish(&self.model).map_err(|e| (e, None))
    }

    /// An HTTP error as the player should read it.
    fn failure(&self, status: u16, body: &str, sent_fallback: bool) -> ProviderError {
        let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let kind = parsed["error"]["type"].as_str().unwrap_or("");
        let message = parsed["error"]["message"].as_str().map(|m| clip(m, 400)).unwrap_or_else(|| format!("HTTP {status}"));
        // An account without the fallback beta: carry on without it.
        if status == 400 && sent_fallback && message.contains("anthropic-beta") {
            return ProviderError::Protocol(BETA_REJECTED.to_string());
        }
        api_error(status, kind, &message, &self.model)
    }
}

const BETA_REJECTED: &str = "The API did not accept the refusal-fallback beta.";

/// Whether a model takes `thinking: {"type": "adaptive"}`. Haiku 4.5 and
/// the models before the 4.6 generation think only with a token budget,
/// which is never sent from here; they run without thinking.
fn thinks(model: &str) -> bool {
    !(model.starts_with("claude-haiku") || model.starts_with("claude-3"))
}

/// Whether a declined request may be rerun on another model by the API.
fn takes_fallback(model: &str) -> bool {
    ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5"].contains(&model)
}

/// Opus 5.5 would default to `medium`; building a ship is long multi-step
/// work, for which `high` is the documented choice. Sonnet 5.5's levels run
/// hotter and its guide starts multi-step tool use at `medium`.
fn default_effort(model: &str) -> &'static str {
    if model.starts_with("claude-sonnet-5-5") { "medium" } else { "high" }
}

/// An API error, from its HTTP status or the `type` in its body.
fn api_error(status: u16, kind: &str, message: &str, model: &str) -> ProviderError {
    match (kind, status) {
        ("authentication_error", _) | (_, 401) => ProviderError::Auth(format!("Claude did not accept the API key ({message}). Check that the key is right and still active.")),
        ("permission_error", _) | (_, 403) => ProviderError::Auth(format!("This API key is not allowed to do that: {message}")),
        ("billing_error", _) | (_, 402) => ProviderError::Auth(format!("Claude reports a billing problem with this key's account: {message}")),
        ("rate_limit_error", _) | (_, 429) => ProviderError::RateLimited("Claude is getting too many requests from this key right now. Try again in a moment.".to_string()),
        ("overloaded_error", _) | (_, 529) => ProviderError::Overloaded("Claude is overloaded right now. Try again in a moment.".to_string()),
        ("not_found_error", _) | (_, 404) => ProviderError::BadRequest(format!("Claude does not offer the model \"{model}\" to this key: {message}")),
        ("request_too_large", _) | (_, 413) => ProviderError::BadRequest("The conversation has grown too large for Claude. Start a new chat.".to_string()),
        ("invalid_request_error", _) | (_, 400) if message.contains("thinking") && message.contains("signature") => ProviderError::ThinkingRejected(format!("Claude refused the request: {message}")),
        ("invalid_request_error", _) | (_, 400..=499) => ProviderError::BadRequest(format!("Claude refused the request: {message}")),
        _ => ProviderError::Server(format!("Claude had an internal error ({message}). Try again in a moment.")),
    }
}

/// The conversation in the API's form: user and assistant messages taking
/// turns, each a list of content blocks, with cache markers on the last
/// blocks of the two latest user messages. Each request then reads the
/// cache the one before it wrote.
fn messages(turns: &[Turn]) -> Vec<Value> {
    let mut out: Vec<(&str, Vec<Value>)> = Vec::new();
    for turn in turns {
        let (role, blocks) = match turn {
            Turn::User { text } => ("user", vec![json!({ "type": "text", "text": or_placeholder(text, "(empty message)") })]),
            Turn::ToolResults { results } => ("user", results.iter().map(result_block).collect()),
            Turn::Assistant { text, tool_calls, raw } => ("assistant", assistant_blocks(text, tool_calls, raw.as_ref())),
        };
        match out.last_mut() {
            // Two user messages in a row are one message to the API.
            Some((last, content)) if *last == "user" && role == "user" => content.extend(blocks),
            _ => out.push((role, blocks)),
        }
    }
    let mut marked = 0;
    for (role, content) in out.iter_mut().rev() {
        if *role == "user" && marked < 2 {
            if let Some(last) = content.last_mut() {
                last["cache_control"] = json!({ "type": "ephemeral" });
                marked += 1;
            }
        }
    }
    out.into_iter().map(|(role, content)| json!({ "role": role, "content": content })).collect()
}

fn result_block(result: &crate::message::ToolResult) -> Value {
    let mut block = json!({ "type": "tool_result", "tool_use_id": wire_id(&result.call_id), "content": or_placeholder(&result.text, "(no output)") });
    if result.is_error {
        block["is_error"] = json!(true);
    }
    block
}

/// An assistant turn's content: what Claude itself sent, unmodified, or
/// else the turn rebuilt from its text and tool calls. Thinking blocks of
/// another provider's turn have no meaning here and are not rebuilt.
fn assistant_blocks(text: &str, tool_calls: &[ToolCall], raw: Option<&Raw>) -> Vec<Value> {
    if let Some(blocks) = raw.filter(|r| r.provider == "claude").and_then(|r| r.content.as_array()).filter(|b| !b.is_empty()) {
        return blocks.clone();
    }
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    for call in tool_calls {
        let input = if call.arguments.is_object() { call.arguments.clone() } else { json!({}) };
        blocks.push(json!({ "type": "tool_use", "id": wire_id(&call.id), "name": call.name, "input": input }));
    }
    if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "(no text)" }));
    }
    blocks
}

/// The API refuses empty text.
fn or_placeholder<'a>(text: &'a str, placeholder: &'a str) -> &'a str {
    if text.trim().is_empty() { placeholder } else { text }
}

/// A tool call id in the characters the API allows. Ids made by other
/// providers may hold others.
fn wire_id(id: &str) -> String {
    let clean: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if clean.is_empty() { "call".to_string() } else { clean }
}

/// Puts a streamed answer together from its events.
#[derive(Default)]
pub(crate) struct ClaudeStream {
    /// Content blocks by their index in the answer.
    blocks: BTreeMap<u64, Value>,
    /// The JSON text of each tool call's input, as it arrives in fragments.
    inputs: BTreeMap<u64, String>,
    /// Inputs that did not turn out to be JSON objects.
    malformed: BTreeMap<u64, String>,
    stop: Option<StopReason>,
    usage: Usage,
    /// Set by the last event of an answer.
    pub done: bool,
    said_something: bool,
    /// A text block has started after earlier text: a paragraph break is due.
    gap_due: bool,
}

impl ClaudeStream {
    /// Takes the `data` of one server-sent event.
    pub fn event(&mut self, data: &str, on_text: &mut TextSink<'_>) -> Result<(), ProviderError> {
        let Ok(event) = serde_json::from_str::<Value>(data) else { return Ok(()) };
        let index = event["index"].as_u64().unwrap_or(0);
        match event["type"].as_str().unwrap_or("") {
            "message_start" => self.count(&event["message"]["usage"]),
            "content_block_start" => self.block_start(index, &event["content_block"], on_text),
            "content_block_delta" => self.delta(index, &event["delta"], on_text),
            "content_block_stop" => self.block_stop(index),
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_reason(reason));
                }
                self.count(&event["usage"]);
            }
            "message_stop" => self.done = true,
            "error" => {
                let (kind, message) = (event["error"]["type"].as_str().unwrap_or(""), event["error"]["message"].as_str().unwrap_or("unknown error"));
                return Err(api_error(if kind == "overloaded_error" { 529 } else { 500 }, kind, &clip(message, 400), ""));
            }
            // Pings, and event types added to the API later.
            _ => {}
        }
        Ok(())
    }

    /// Token counts are cumulative; a later event replaces an earlier one's.
    fn count(&mut self, usage: &Value) {
        let field = |name: &str| usage[name].as_u64();
        if let Some(fresh) = field("input_tokens") {
            self.usage.cache_read = field("cache_read_input_tokens").unwrap_or(0);
            self.usage.cache_write = field("cache_creation_input_tokens").unwrap_or(0);
            // `input_tokens` is only what the cache did not cover.
            self.usage.input = fresh + self.usage.cache_read + self.usage.cache_write;
        }
        if let Some(output) = field("output_tokens") {
            self.usage.output = output;
        }
    }

    fn block_start(&mut self, index: u64, block: &Value, on_text: &mut TextSink<'_>) {
        match block["type"].as_str().unwrap_or("") {
            "tool_use" => {
                self.inputs.insert(index, String::new());
            }
            "text" => {
                self.gap_due = self.said_something;
                self.say(block["text"].as_str().unwrap_or(""), on_text);
            }
            // Another model took over after the safety system declined.
            // What the first model thought and the tools it asked for are
            // void; its text stays.
            "fallback" => {
                self.blocks.retain(|_, b| b["type"] == "text");
                self.inputs.clear();
                self.malformed.clear();
                return;
            }
            _ => {}
        }
        self.blocks.insert(index, block.clone());
    }

    fn delta(&mut self, index: u64, delta: &Value, on_text: &mut TextSink<'_>) {
        let append = |block: Option<&mut Value>, field: &str, more: &str| {
            if let Some(block) = block {
                let so_far = block[field].as_str().unwrap_or("");
                block[field] = Value::String(format!("{so_far}{more}"));
            }
        };
        match delta["type"].as_str().unwrap_or("") {
            "text_delta" => {
                let more = delta["text"].as_str().unwrap_or("");
                append(self.blocks.get_mut(&index), "text", more);
                self.say(more, on_text);
            }
            "input_json_delta" => {
                if let Some(input) = self.inputs.get_mut(&index) {
                    input.push_str(delta["partial_json"].as_str().unwrap_or(""));
                }
            }
            // Thinking is kept for the next request and never shown.
            "thinking_delta" => append(self.blocks.get_mut(&index), "thinking", delta["thinking"].as_str().unwrap_or("")),
            "signature_delta" => append(self.blocks.get_mut(&index), "signature", delta["signature"].as_str().unwrap_or("")),
            _ => {}
        }
    }

    fn say(&mut self, text: &str, on_text: &mut TextSink<'_>) {
        if text.is_empty() {
            return;
        }
        if std::mem::take(&mut self.gap_due) {
            on_text("\n\n");
        }
        self.said_something = true;
        on_text(text);
    }

    /// A tool call's input is complete: read it. With eager input streaming
    /// the API no longer checks that it is JSON.
    fn block_stop(&mut self, index: u64) {
        let Some(text) = self.inputs.remove(&index) else { return };
        let parsed = if text.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(&text) };
        let input = match parsed {
            Ok(input) if input.is_object() => input,
            _ => {
                self.malformed.insert(index, text);
                json!({})
            }
        };
        if let Some(block) = self.blocks.get_mut(&index) {
            block["input"] = input;
        }
    }

    /// The whole answer, once the stream has ended.
    pub fn finish(mut self, model: &str) -> Result<Reply, ProviderError> {
        let Some(stop) = self.stop.take() else {
            return Err(ProviderError::Network("The answer from Claude broke off before it was finished.".to_string()));
        };
        // A tool call the stream ended in the middle of.
        for index in self.inputs.keys().copied().collect::<Vec<_>>() {
            self.block_stop(index);
        }
        let mut texts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut content = Vec::new();
        for (index, block) in self.blocks {
            match block["type"].as_str().unwrap_or("") {
                "text" => {
                    let text = block["text"].as_str().unwrap_or("");
                    // The API refuses an empty text block sent back to it.
                    if text.trim().is_empty() {
                        continue;
                    }
                    texts.push(text.to_string());
                }
                "tool_use" => tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or("").to_string(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                    arguments: block["input"].clone(),
                    malformed: self.malformed.remove(&index),
                }),
                _ => {}
            }
            content.push(block);
        }
        let raw = (!content.is_empty()).then(|| Raw { provider: "claude".to_string(), model: model.to_string(), content: Value::Array(content) });
        Ok(Reply { text: texts.join("\n\n"), tool_calls, stop, usage: self.usage, raw })
    }
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        "pause_turn" => StopReason::PauseTurn,
        other => StopReason::Other(other.to_string()),
    }
}

#[async_trait::async_trait]
impl Provider for Claude {
    fn kind(&self) -> &'static str {
        "claude"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn history_budget(&self) -> usize {
        self.history_budget
    }

    async fn call(&self, request: &Request<'_>, on_text: &mut TextSink<'_>) -> Result<Reply, ProviderError> {
        let mut retried = 0;
        loop {
            let mut passed_on = false;
            let result = {
                let mut forward = |text: &str| {
                    passed_on = true;
                    on_text(text);
                };
                self.attempt(request, &mut forward).await
            };
            match result {
                Ok(reply) => return Ok(reply),
                Err((ProviderError::Protocol(why), _)) if why == BETA_REJECTED && self.fallback.swap(false, Ordering::Relaxed) => {}
                // Text that has reached the player cannot be taken back, so
                // an answer that broke off is not asked for again.
                Err((error, wait)) if error.is_transient() && !passed_on && retried < self.patience.retries => {
                    tokio::time::sleep(self.patience.wait(retried, wait)).await;
                    retried += 1;
                }
                Err((error, _)) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolResult;
    use crate::provider::ToolSpec;

    fn sse(events: &[Value]) -> String {
        events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect()
    }

    /// Feeds a stream to the parser in pieces of `size` bytes.
    fn parse(stream: &str, size: usize) -> (Result<Reply, ProviderError>, String) {
        let mut decoder = SseDecoder::new();
        let mut answer = ClaudeStream::default();
        let mut said = String::new();
        let mut on_text = |t: &str| said.push_str(t);
        for chunk in stream.as_bytes().chunks(size) {
            for event in decoder.push(chunk) {
                if let Err(e) = answer.event(&event.data, &mut on_text) {
                    return (Err(e), said);
                }
            }
        }
        (answer.finish("claude-opus-5-5"), said)
    }

    /// A thinking block, a line of text, and two tool calls whose inputs
    /// arrive in fragments.
    fn tool_use_stream() -> String {
        sse(&[
            json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [],
                "usage": {"input_tokens": 12, "cache_creation_input_tokens": 3000, "cache_read_input_tokens": 500, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "A hull, then "}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "the guns."}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBCkYIAxgC"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Building the "}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "h\u{fc}ll."}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_01A", "name": "fill", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": ""}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"from\": [0, 0,"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": " 1], \"block\": \"Gr"}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "ey Hull\"}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "tool_use", "id": "toolu_01B", "name": "design_info", "input": {}}}),
            json!({"type": "content_block_stop", "index": 3}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 87}}),
            json!({"type": "message_stop"}),
        ])
    }

    #[test]
    fn a_streamed_answer_is_put_together_however_the_bytes_are_cut() {
        let stream = tool_use_stream();
        for size in [1, 2, 3, 7, 16, 61, 500, stream.len()] {
            let (reply, said) = parse(&stream, size);
            let reply = reply.unwrap();
            assert_eq!(said, "Building the h\u{fc}ll.", "pieces of {size} bytes");
            assert_eq!(reply.text, "Building the h\u{fc}ll.");
            assert_eq!(reply.stop, StopReason::ToolUse);
            assert_eq!(reply.usage, Usage { input: 3512, output: 87, cache_read: 500, cache_write: 3000 });
            assert_eq!(
                reply.tool_calls,
                vec![
                    ToolCall { id: "toolu_01A".into(), name: "fill".into(), arguments: json!({"from": [0, 0, 1], "block": "Grey Hull"}), malformed: None },
                    ToolCall { id: "toolu_01B".into(), name: "design_info".into(), arguments: json!({}), malformed: None },
                ]
            );
            // What goes back to the API: every block, the thinking block
            // with its signature exactly as it came.
            let raw = reply.raw.unwrap();
            assert_eq!((raw.provider.as_str(), raw.model.as_str()), ("claude", "claude-opus-5-5"));
            assert_eq!(
                raw.content,
                json!([
                    {"type": "thinking", "thinking": "A hull, then the guns.", "signature": "EqQBCkYIAxgC"},
                    {"type": "text", "text": "Building the h\u{fc}ll."},
                    {"type": "tool_use", "id": "toolu_01A", "name": "fill", "input": {"from": [0, 0, 1], "block": "Grey Hull"}},
                    {"type": "tool_use", "id": "toolu_01B", "name": "design_info", "input": {}},
                ])
            );
        }
    }

    fn short_stream(blocks: &[Value], stop: &str) -> String {
        let mut events = vec![json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 0}}})];
        events.extend(blocks.iter().cloned());
        events.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 9}}));
        events.push(json!({"type": "message_stop"}));
        sse(&events)
    }

    #[test]
    fn stop_reasons_are_told_apart() {
        for (wire, stop) in [
            ("end_turn", StopReason::EndTurn),
            ("tool_use", StopReason::ToolUse),
            ("max_tokens", StopReason::MaxTokens),
            ("refusal", StopReason::Refusal),
            ("pause_turn", StopReason::PauseTurn),
            ("something_new", StopReason::Other("something_new".into())),
        ] {
            let (reply, _) = parse(&short_stream(&[], wire), 40);
            assert_eq!(reply.unwrap().stop, stop);
        }
    }

    #[test]
    fn empty_thinking_is_kept_and_empty_text_is_not() {
        // With thinking display left at its default the thinking text is
        // empty and the signature carries everything.
        let stream = short_stream(
            &[
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2ln"}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "\n"}}),
                json!({"type": "content_block_stop", "index": 1}),
                json!({"type": "content_block_start", "index": 2, "content_block": {"type": "redacted_thinking", "data": "b3BhcXVl"}}),
                json!({"type": "content_block_stop", "index": 2}),
                json!({"type": "content_block_start", "index": 3, "content_block": {"type": "text", "text": "Done."}}),
                json!({"type": "content_block_stop", "index": 3}),
            ],
            "end_turn",
        );
        let (reply, _) = parse(&stream, 33);
        let reply = reply.unwrap();
        assert_eq!(reply.text, "Done.");
        assert_eq!(
            reply.raw.unwrap().content,
            json!([{"type": "thinking", "thinking": "", "signature": "c2ln"}, {"type": "redacted_thinking", "data": "b3BhcXVl"}, {"type": "text", "text": "Done."}])
        );
    }

    #[test]
    fn text_blocks_are_shown_as_paragraphs() {
        let stream = short_stream(
            &[
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "First."}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Second."}}),
                json!({"type": "content_block_stop", "index": 1}),
            ],
            "end_turn",
        );
        let (reply, said) = parse(&stream, 50);
        assert_eq!(said, "First.\n\nSecond.");
        assert_eq!(reply.unwrap().text, said);
    }

    #[test]
    fn a_tool_input_that_is_not_json_is_kept_as_text_for_the_model_to_see() {
        let stream = short_stream(
            &[
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "fill", "input": {}}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"from\": [0, 0"}}),
            ],
            "max_tokens",
        );
        let (reply, _) = parse(&stream, 64);
        let reply = reply.unwrap();
        assert_eq!(reply.stop, StopReason::MaxTokens);
        assert_eq!(reply.tool_calls, vec![ToolCall { id: "toolu_1".into(), name: "fill".into(), arguments: json!({}), malformed: Some("{\"from\": [0, 0".into()) }]);
        assert_eq!(reply.raw.unwrap().content[0]["input"], json!({}));
    }

    #[test]
    fn after_a_fallback_only_the_first_models_text_is_kept() {
        let stream = short_stream(
            &[
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": "old"}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "Let me look."}}),
                json!({"type": "content_block_stop", "index": 1}),
                json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_old", "name": "fill", "input": {}}}),
                json!({"type": "content_block_start", "index": 3, "content_block": {"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-4-8"}}}),
                json!({"type": "content_block_stop", "index": 3}),
                json!({"type": "content_block_start", "index": 4, "content_block": {"type": "tool_use", "id": "toolu_new", "name": "design_info", "input": {}}}),
                json!({"type": "content_block_stop", "index": 4}),
            ],
            "tool_use",
        );
        let (reply, _) = parse(&stream, 45);
        let reply = reply.unwrap();
        assert_eq!(reply.tool_calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["toolu_new"]);
        assert_eq!(reply.raw.unwrap().content, json!([{"type": "text", "text": "Let me look."}, {"type": "tool_use", "id": "toolu_new", "name": "design_info", "input": {}}]));
    }

    #[test]
    fn an_error_in_the_stream_and_a_stream_that_breaks_off_are_failures() {
        let overloaded = sse(&[json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})]);
        assert!(matches!(parse(&overloaded, 20).0, Err(ProviderError::Overloaded(_))));
        let cut = sse(&[
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 5}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Half an ans"}}),
        ]);
        let (reply, said) = parse(&cut, 20);
        assert_eq!(said, "Half an ans");
        assert!(matches!(reply, Err(ProviderError::Network(_))));
    }

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec { name: "fill".into(), description: "Fills a box.".into(), schema: json!({"type": "object", "properties": {}}) }]
    }

    fn body(model: &str, turns: &[Turn], effort: Option<&str>) -> Value {
        let tools = tools();
        Claude::new("sk-test", model).request_body(&Request { system: "You build ships.", turns, tools: &tools, max_tokens: None, reasoning_effort: effort })
    }

    #[test]
    fn the_request_caches_the_system_prompt_and_tools_and_streams() {
        let turns = [Turn::User { text: "Build a frigate.".into() }];
        let body = body("claude-opus-5-5", &turns, None);
        assert_eq!(
            body,
            json!({
                "model": "claude-opus-5-5",
                "max_tokens": 64000,
                "stream": true,
                "system": [{"type": "text", "text": "You build ships.", "cache_control": {"type": "ephemeral"}}],
                "tools": [{"name": "fill", "description": "Fills a box.", "input_schema": {"type": "object", "properties": {}}, "eager_input_streaming": true}],
                "thinking": {"type": "adaptive"},
                "output_config": {"effort": "high"},
                "fallbacks": "default",
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Build a frigate.", "cache_control": {"type": "ephemeral"}}]}],
            })
        );
    }

    #[test]
    fn thinking_is_adaptive_where_the_model_has_it_and_absent_on_haiku() {
        let turns = [Turn::User { text: "Hi".into() }];
        for model in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5"] {
            let body = body(model, &turns, None);
            assert_eq!(body["thinking"], json!({"type": "adaptive"}), "{model}");
            assert_eq!(body["fallbacks"], json!("default"), "{model}");
        }
        assert_eq!(body("claude-sonnet-5-5", &turns, None)["output_config"], json!({"effort": "medium"}));
        assert_eq!(body("claude-fable-5-1", &turns, Some("xhigh"))["output_config"], json!({"effort": "xhigh"}));
        assert_eq!(body("claude-fable-5-1", &turns, Some("extreme"))["output_config"], json!({"effort": "high"}), "an unknown level falls back to the default");
        let haiku = body("claude-haiku-4-5-20251001", &turns, Some("high"));
        for absent in ["thinking", "output_config", "fallbacks"] {
            assert!(haiku.get(absent).is_none(), "Haiku 4.5 takes no {absent}");
        }
        // No model is ever sent a thinking budget.
        for model in Claude::MODELS.map(|(id, _)| id) {
            assert!(!body(model, &turns, None).to_string().contains("budget_tokens"));
        }
        assert_eq!(Claude::models()[0].id, Claude::DEFAULT_MODEL);
    }

    fn tool_exchange(raw_from: Option<&str>) -> Vec<Turn> {
        let thinking = json!([
            {"type": "thinking", "thinking": "", "signature": "c2lnbmF0dXJl"},
            {"type": "tool_use", "id": "toolu_1", "name": "fill", "input": {"block": "Grey Hull"}},
        ]);
        vec![
            Turn::User { text: "Build a hull.".into() },
            Turn::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCall { id: "toolu_1".into(), name: "fill".into(), arguments: json!({"block": "Grey Hull"}), malformed: None }],
                raw: raw_from.map(|provider| Raw { provider: provider.into(), model: "m".into(), content: thinking.clone() }),
            },
            Turn::ToolResults { results: vec![ToolResult { call_id: "toolu_1".into(), text: "Filled 20 blocks.".into(), is_error: false }] },
        ]
    }

    #[test]
    fn an_assistant_turn_goes_back_exactly_as_claude_sent_it() {
        let body = body("claude-opus-5-5", &tool_exchange(Some("claude")), None);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "Build a hull.", "cache_control": {"type": "ephemeral"}}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "", "signature": "c2lnbmF0dXJl"},
                    {"type": "tool_use", "id": "toolu_1", "name": "fill", "input": {"block": "Grey Hull"}},
                ]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "Filled 20 blocks.", "cache_control": {"type": "ephemeral"}}]},
            ])
        );
    }

    #[test]
    fn a_turn_from_another_provider_is_rebuilt_without_its_raw_content() {
        let mut turns = tool_exchange(Some("openai"));
        if let Turn::Assistant { text, tool_calls, .. } = &mut turns[1] {
            *text = "On it.".into();
            tool_calls[0].id = "call:7/a".into();
        }
        if let Turn::ToolResults { results } = &mut turns[2] {
            results[0].call_id = "call:7/a".into();
            results[0].is_error = true;
        }
        let body = body("claude-opus-5-5", &turns, None);
        assert_eq!(
            body["messages"][1],
            json!({"role": "assistant", "content": [{"type": "text", "text": "On it."}, {"type": "tool_use", "id": "call_7_a", "name": "fill", "input": {"block": "Grey Hull"}}]})
        );
        assert_eq!(body["messages"][2]["content"][0], json!({"type": "tool_result", "tool_use_id": "call_7_a", "content": "Filled 20 blocks.", "is_error": true, "cache_control": {"type": "ephemeral"}}));
        assert!(!body.to_string().contains("c2lnbmF0dXJl"));
    }

    #[test]
    fn user_turns_in_a_row_become_one_message_and_roles_alternate() {
        let mut turns = tool_exchange(None);
        turns.push(Turn::User { text: "Now paint it red.".into() });
        turns.push(Turn::User { text: " ".into() });
        let body = body("claude-opus-5-5", &turns, None);
        let roles: Vec<&str> = body["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(
            body["messages"][2]["content"],
            json!([
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "Filled 20 blocks."},
                {"type": "text", "text": "Now paint it red."},
                {"type": "text", "text": "(empty message)", "cache_control": {"type": "ephemeral"}},
            ])
        );
        // Markers on the two latest user messages only: with the system
        // prompt's that is three of the four the API allows.
        assert_eq!(body.to_string().matches("cache_control").count(), 3);
    }

    #[test]
    fn the_key_stays_out_of_debug_output_and_error_messages() {
        let claude = Claude::new("sk-ant-very-secret", "claude-opus-5-5");
        assert!(!format!("{claude:?}").contains("secret"));
        let failure = claude.failure(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#, true);
        assert!(matches!(&failure, ProviderError::Auth(m) if m.contains("did not accept the API key") && !m.contains("secret")));
    }

    #[test]
    fn http_errors_become_sentences_by_kind() {
        let claude = Claude::new("k", "claude-opus-5-5");
        let error = |status: u16, kind: &str, message: &str| claude.failure(status, &json!({"type": "error", "error": {"type": kind, "message": message}}).to_string(), false);
        assert!(matches!(error(429, "rate_limit_error", "slow down"), ProviderError::RateLimited(_)));
        assert!(matches!(error(529, "overloaded_error", "Overloaded"), ProviderError::Overloaded(_)));
        assert!(matches!(error(500, "api_error", "boom"), ProviderError::Server(_)));
        assert!(matches!(error(400, "invalid_request_error", "max_tokens: too large"), ProviderError::BadRequest(m) if m.contains("max_tokens: too large")));
        assert!(matches!(error(404, "not_found_error", "model: nope"), ProviderError::BadRequest(m) if m.contains("claude-opus-5-5")));
        let binding = "messages.5.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.";
        assert!(matches!(error(400, "invalid_request_error", binding), ProviderError::ThinkingRejected(_)));
        assert!(matches!(claude.failure(502, "<html>bad gateway</html>", false), ProviderError::Server(m) if m.contains("HTTP 502")));
        let beta = "Unexpected value(s) `server-side-fallback-2026-07-01` for the `anthropic-beta` header.";
        assert_eq!(claude.failure(400, &json!({"error": {"type": "invalid_request_error", "message": beta}}).to_string(), true), ProviderError::Protocol(BETA_REJECTED.into()));
    }
}
