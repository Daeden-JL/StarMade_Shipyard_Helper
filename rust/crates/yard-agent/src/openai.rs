//! Any endpoint that speaks OpenAI's chat completions API: LM Studio,
//! llama.cpp's server, Ollama, vLLM, and OpenAI itself.
//!
//! Local models are the main use. They are slow, so the waits are long; they
//! think out loud, so reasoning is filtered from what the player sees; and
//! they get tool arguments wrong, which the tools answer for themselves.

use crate::message::{ToolCall, Turn};
use crate::provider::{clip, network_error, retry_after, ModelInfo, Patience, Provider, TextSink, ProviderError, Reply, Request, StopReason, Usage};
use crate::sse::SseDecoder;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// An OpenAI-compatible endpoint and the model to ask there.
pub struct OpenAiCompatible {
    http: reqwest::Client,
    base: String,
    model: String,
    key: Option<String>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    reasoning_effort: Option<String>,
    /// Seconds an idle model may stay loaded, for servers that load models on
    /// demand (LM Studio's `ttl`).
    keep_loaded: Option<u32>,
    patience: Patience,
    history_budget: usize,
}

/// The key is left out: this may end up in a log.
impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatible").field("model", &self.model).field("base", &self.base).field("key", &self.key.as_ref().map(|_| "[hidden]")).finish()
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().unwrap_or_default()
}

fn bearer(key: &str) -> Result<reqwest::header::HeaderValue, ProviderError> {
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key.trim()))
        .map_err(|_| ProviderError::Auth("The API key contains characters that cannot be sent. Enter it again.".to_string()))?;
    value.set_sensitive(true);
    Ok(value)
}

impl OpenAiCompatible {
    /// `base` is the address up to and including `/v1`, as in
    /// `http://127.0.0.1:1234/v1`.
    pub fn new(base: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: http_client(),
            base: base.into().trim_end_matches('/').to_string(),
            model: model.into(),
            key: None,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            keep_loaded: None,
            // A local model may take minutes to read a long prompt before
            // its first word.
            patience: Patience { silence: Duration::from_secs(600), retries: 1, backoff: Duration::from_secs(2), longest_wait: Duration::from_secs(20) },
            // About 12,000 tokens: what is left of a 32k context window
            // after the system prompt, the tools and room for the answer.
            history_budget: 48_000,
        }
    }

    /// Sent as a bearer token. Local servers need none.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into()).filter(|k: &String| !k.trim().is_empty());
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// `low`, `medium` or `high`, for models and servers that take it.
    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    /// Asks a server that loads models on demand to keep this one loaded
    /// while it is idle for this many seconds. LM Studio otherwise applies its
    /// own setting, which can unload a model between two steps of one answer.
    /// Servers without the setting ignore it.
    pub fn with_keep_loaded(mut self, seconds: u32) -> Self {
        self.keep_loaded = Some(seconds);
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

    /// The models an endpoint has on offer, without those that only embed.
    pub async fn list_models(base: &str, key: Option<&str>) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut call = http_client().get(format!("{}/models", base.trim_end_matches('/'))).timeout(Duration::from_secs(15));
        if let Some(key) = key.filter(|k| !k.trim().is_empty()) {
            call = call.header("authorization", bearer(key)?);
        }
        let response = call.send().await.map_err(|e| network_error("The model server", &e))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| network_error("The model server", &e))?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text));
        }
        let listed: Value = serde_json::from_str(&text).map_err(|_| ProviderError::Protocol("The model server's list of models could not be read.".to_string()))?;
        Ok(model_list(&listed))
    }

    /// The JSON body of one request.
    pub fn request_body(&self, request: &Request<'_>) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages(request.system, request.turns),
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } }))
                .collect();
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!("auto");
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = request.max_tokens.or(self.max_tokens) {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(effort) = request.reasoning_effort.or(self.reasoning_effort.as_deref()).filter(|e| !e.is_empty()) {
            body["reasoning_effort"] = json!(effort);
        }
        if let Some(seconds) = self.keep_loaded {
            body["ttl"] = json!(seconds);
        }
        body
    }

    async fn attempt(&self, request: &Request<'_>, on_text: &mut TextSink<'_>) -> Result<Reply, (ProviderError, Option<Duration>)> {
        let mut call = self.http.post(format!("{}/chat/completions", self.base)).json(&self.request_body(request));
        if let Some(key) = &self.key {
            call = call.header("authorization", bearer(key).map_err(|e| (e, None))?);
        }
        let late = || (ProviderError::Timeout(format!("The model did not answer within {} seconds.", self.patience.silence.as_secs())), None);
        let response = tokio::time::timeout(self.patience.silence, call.send()).await.map_err(|_| late())?.map_err(|e| (network_error("The model server", &e), None))?;
        let status = response.status();
        if !status.is_success() {
            let wait = retry_after(response.headers());
            let text = tokio::time::timeout(self.patience.silence, response.text()).await.map_err(|_| late())?.unwrap_or_default();
            return Err((http_error(status.as_u16(), &text), wait));
        }
        let mut bytes = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut answer = OpenAiStream::default();
        while !answer.done {
            let Some(piece) = tokio::time::timeout(self.patience.silence, bytes.next()).await.map_err(|_| late())? else { break };
            let piece = piece.map_err(|e| (network_error("The model server", &e), None))?;
            for event in decoder.push(&piece) {
                answer.event(&event.data, on_text).map_err(|e| (e, None))?;
            }
        }
        if let Some(event) = decoder.finish() {
            answer.event(&event.data, on_text).map_err(|e| (e, None))?;
        }
        answer.finish(on_text).map_err(|e| (e, None))
    }
}

/// The ids in a `/models` answer that can chat.
fn model_list(listed: &Value) -> Vec<ModelInfo> {
    let ids = listed["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str());
    ids.filter(|id| !id.to_ascii_lowercase().contains("embed")).map(|id| ModelInfo { id: id.to_string(), name: id.to_string(), provider: "openai".to_string() }).collect()
}

fn http_error(status: u16, body: &str) -> ProviderError {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let said = parsed["error"]["message"].as_str().or(parsed["error"].as_str()).or(parsed["message"].as_str()).unwrap_or(body);
    let said = if said.trim().is_empty() { format!("HTTP {status}") } else { clip(said.trim(), 400) };
    match status {
        401 | 403 => ProviderError::Auth(format!("The model server did not accept the API key: {said}")),
        429 => ProviderError::RateLimited(format!("The model server is getting too many requests right now: {said}")),
        400..=499 => ProviderError::BadRequest(format!("The model server refused the request: {said}")),
        503 | 529 => ProviderError::Overloaded(format!("The model server is busy: {said}")),
        _ => ProviderError::Server(format!("The model server failed: {said}")),
    }
}

/// The conversation in the API's form. Thinking kept for another provider
/// is left out; turns of the player's that follow each other are joined,
/// because several chat templates insist that roles take turns.
fn messages(system: &str, turns: &[Turn]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    for turn in turns {
        match turn {
            Turn::User { text } => match out.last_mut().filter(|last| last["role"] == "user") {
                Some(last) => last["content"] = json!(format!("{}\n\n{}", last["content"].as_str().unwrap_or(""), text)),
                None => out.push(json!({ "role": "user", "content": text })),
            },
            Turn::Assistant { text, tool_calls, .. } => {
                let mut message = json!({ "role": "assistant", "content": text });
                if !tool_calls.is_empty() {
                    let calls: Vec<Value> = tool_calls
                        .iter()
                        // Arguments travel as JSON text. What a model sent
                        // that was not JSON goes back as an empty object:
                        // servers parse this field to render their prompt.
                        .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments.to_string() } }))
                        .collect();
                    message["tool_calls"] = Value::Array(calls);
                }
                out.push(message);
            }
            Turn::ToolResults { results } => out.extend(results.iter().map(|r| json!({ "role": "tool", "tool_call_id": r.call_id, "content": r.text }))),
        }
    }
    out
}

/// Numbers tool calls whose server gave them no id.
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Puts a streamed answer together from its chunks.
#[derive(Default)]
pub(crate) struct OpenAiStream {
    text: String,
    filter: ThinkFilter,
    /// Tool calls by their index in the answer; each arrives in fragments.
    calls: BTreeMap<u64, PartialCall>,
    finish: Option<String>,
    usage: Usage,
    /// Set by the `[DONE]` that ends a stream.
    pub done: bool,
}

impl OpenAiStream {
    /// Takes the `data` of one server-sent event.
    pub fn event(&mut self, data: &str, on_text: &mut TextSink<'_>) -> Result<(), ProviderError> {
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else { return Ok(()) };
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let said = error["message"].as_str().or(error.as_str()).unwrap_or("unknown error");
            return Err(ProviderError::Server(format!("The model server failed: {}", clip(said, 400))));
        }
        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            self.usage.input = usage["prompt_tokens"].as_u64().unwrap_or(self.usage.input);
            self.usage.output = usage["completion_tokens"].as_u64().unwrap_or(self.usage.output);
            self.usage.cache_read = usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(self.usage.cache_read);
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        // `reasoning_content` and `reasoning` hold a model's thinking; they
        // are not for the player and are not read.
        if let Some(more) = delta["content"].as_str() {
            let shown = self.filter.push(more);
            self.say(&shown, on_text);
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let slot = self.calls.entry(call["index"].as_u64().unwrap_or(0)).or_default();
            if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                slot.id = id.to_string();
            }
            let function = &call["function"];
            if let Some(name) = function["name"].as_str() {
                slot.name.push_str(name);
            }
            match &function["arguments"] {
                Value::String(more) => slot.arguments.push_str(more),
                // Some servers send the arguments as an object, all at once.
                Value::Object(_) => slot.arguments = function["arguments"].to_string(),
                _ => {}
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(reason.to_string());
        }
        Ok(())
    }

    fn say(&mut self, text: &str, on_text: &mut TextSink<'_>) {
        // Nothing but line breaks before the first word.
        let text = if self.text.is_empty() { text.trim_start() } else { text };
        if !text.is_empty() {
            self.text.push_str(text);
            on_text(text);
        }
    }

    /// The whole answer, once the stream has ended.
    pub fn finish(mut self, on_text: &mut TextSink<'_>) -> Result<Reply, ProviderError> {
        let rest = self.filter.finish();
        self.say(&rest, on_text);
        if self.finish.is_none() && self.text.is_empty() && self.calls.is_empty() {
            return Err(ProviderError::Network("The model's answer broke off before it began.".to_string()));
        }
        let tool_calls: Vec<ToolCall> = std::mem::take(&mut self.calls).into_values().filter(|c| !c.name.trim().is_empty()).map(finished_call).collect();
        let stop = match self.finish.as_deref() {
            // Some servers say "stop" after a tool call.
            _ if !tool_calls.is_empty() && self.finish.as_deref() != Some("length") => StopReason::ToolUse,
            Some("stop") | None => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            Some("tool_calls" | "function_call") => StopReason::EndTurn,
            Some(other) => StopReason::Other(other.to_string()),
        };
        Ok(Reply { text: self.text.trim_end().to_string(), tool_calls, stop, usage: self.usage, raw: None })
    }
}

fn finished_call(call: PartialCall) -> ToolCall {
    let text = call.arguments.trim();
    let parsed = if text.is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(text) };
    let (arguments, malformed) = match parsed {
        Ok(arguments) if arguments.is_object() => (arguments, None),
        _ => (json!({}), Some(call.arguments.clone())),
    };
    let id = if call.id.is_empty() { format!("call_{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed)) } else { call.id };
    ToolCall { id, name: call.name.trim().to_string(), arguments, malformed }
}

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Takes `<think>...</think>` passages out of text that arrives in pieces.
/// Reasoning models write their thoughts there; players should not see them.
#[derive(Default)]
pub(crate) struct ThinkFilter {
    /// Text held back because it may be the start of a tag.
    held: String,
    thinking: bool,
}

impl ThinkFilter {
    /// Takes the next piece and returns what of the text so far may be shown.
    pub fn push(&mut self, piece: &str) -> String {
        self.held.push_str(piece);
        let mut shown = String::new();
        loop {
            let tag = if self.thinking { THINK_CLOSE } else { THINK_OPEN };
            match self.held.find(tag) {
                Some(at) => {
                    if !self.thinking {
                        shown.push_str(&self.held[..at]);
                    }
                    self.held.drain(..at + tag.len());
                    self.thinking = !self.thinking;
                }
                None => {
                    // Keep back an ending that could grow into the tag.
                    let keep = (1..tag.len()).rev().find(|n| self.held.len() >= *n && self.held.is_char_boundary(self.held.len() - n) && tag.starts_with(&self.held[self.held.len() - n..])).unwrap_or(0);
                    let cut = self.held.len() - keep;
                    if !self.thinking {
                        shown.push_str(&self.held[..cut]);
                    }
                    self.held.drain(..cut);
                    return shown;
                }
            }
        }
    }

    /// What was held back when the text ended. Thoughts never closed stay hidden.
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.held);
        if self.thinking { String::new() } else { rest }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatible {
    fn kind(&self) -> &'static str {
        "openai"
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
    use crate::message::{Raw, ToolResult};
    use crate::provider::ToolSpec;

    fn sse(chunks: &[Value]) -> String {
        let mut out: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        out.push_str("data: [DONE]\n\n");
        out
    }

    fn delta(delta: Value) -> Value {
        json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": null}]})
    }

    fn finish(reason: &str) -> Value {
        json!({"id": "chatcmpl-1", "choices": [{"index": 0, "delta": {}, "finish_reason": reason}]})
    }

    fn parse(stream: &str, size: usize) -> (Result<Reply, ProviderError>, String) {
        let mut decoder = SseDecoder::new();
        let mut answer = OpenAiStream::default();
        let mut said = String::new();
        let mut on_text = |t: &str| said.push_str(t);
        for chunk in stream.as_bytes().chunks(size) {
            for event in decoder.push(chunk) {
                if let Err(e) = answer.event(&event.data, &mut on_text) {
                    return (Err(e), said);
                }
            }
        }
        let reply = answer.finish(&mut on_text);
        (reply, said)
    }

    #[test]
    fn tool_calls_split_over_many_deltas_are_put_together_by_index() {
        let stream = sse(&[
            delta(json!({"role": "assistant", "content": ""})),
            delta(json!({"reasoning_content": "The player wants a hull."})),
            delta(json!({"content": "Building "})),
            delta(json!({"content": "the h\u{fc}ll."})),
            delta(json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function", "function": {"name": "fill", "arguments": ""}}]})),
            delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"from\": [0, 0,"}}]})),
            delta(json!({"tool_calls": [{"index": 1, "id": "call_b", "type": "function", "function": {"name": "design_info", "arguments": "{}"}}]})),
            delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": " 1], \"block\": \"Grey Hull\"}"}}]})),
            finish("tool_calls"),
            json!({"id": "chatcmpl-1", "choices": [], "usage": {"prompt_tokens": 2100, "completion_tokens": 64, "total_tokens": 2164}}),
        ]);
        for size in [1, 2, 5, 13, 64, 300, stream.len()] {
            let (reply, said) = parse(&stream, size);
            let reply = reply.unwrap();
            assert_eq!(said, "Building the h\u{fc}ll.", "pieces of {size} bytes");
            assert_eq!(reply.text, said);
            assert_eq!(reply.stop, StopReason::ToolUse);
            assert_eq!(reply.usage, Usage { input: 2100, output: 64, cache_read: 0, cache_write: 0 });
            assert_eq!(
                reply.tool_calls,
                vec![
                    ToolCall { id: "call_a".into(), name: "fill".into(), arguments: json!({"from": [0, 0, 1], "block": "Grey Hull"}), malformed: None },
                    ToolCall { id: "call_b".into(), name: "design_info".into(), arguments: json!({}), malformed: None },
                ]
            );
            assert_eq!(reply.raw, None);
        }
    }

    #[test]
    fn thinking_out_loud_is_kept_from_the_player() {
        let stream = sse(&[
            delta(json!({"content": "<thi"})),
            delta(json!({"content": "nk>The core is at the origin.</th"})),
            delta(json!({"content": "ink>\n\nThe hull is "})),
            delta(json!({"content": "done. 3 < 4"})),
            delta(json!({"content": " <"})),
            finish("stop"),
        ]);
        for size in [1, 9, 4000] {
            let (reply, said) = parse(&stream, size);
            assert_eq!(said, "The hull is done. 3 < 4 <");
            let reply = reply.unwrap();
            assert_eq!((reply.text.as_str(), reply.stop), ("The hull is done. 3 < 4 <", StopReason::EndTurn));
        }
        // Thoughts the model never closed stay hidden.
        let (reply, said) = parse(&sse(&[delta(json!({"content": "Sure.<think>but how"})), finish("length")]), 10);
        assert_eq!((said.as_str(), reply.unwrap().stop), ("Sure.", StopReason::MaxTokens));
    }

    #[test]
    fn the_think_filter_passes_ordinary_text_through_whole() {
        let mut filter = ThinkFilter::default();
        let mut shown = String::new();
        for piece in ["a <b> c", " <", "t", "hin", "g> d ", "<think>x</think>", "e"] {
            shown.push_str(&filter.push(piece));
        }
        shown.push_str(&filter.finish());
        assert_eq!(shown, "a <b> c <thing> d e");
    }

    #[test]
    fn sloppy_servers_are_understood() {
        // No id, arguments as an object, "stop" after a tool call, no [DONE].
        let stream = format!(
            "data: {}\n\ndata: {}\n\n",
            delta(json!({"tool_calls": [{"index": 0, "function": {"name": "remove", "arguments": {"from": [0, 0, 0]}}}]})),
            finish("stop")
        );
        let (reply, _) = parse(&stream, 30);
        let reply = reply.unwrap();
        assert_eq!(reply.stop, StopReason::ToolUse);
        assert!(reply.tool_calls[0].id.starts_with("call_"));
        assert_eq!(reply.tool_calls[0].arguments, json!({"from": [0, 0, 0]}));
    }

    #[test]
    fn arguments_that_are_not_json_are_kept_as_text_for_the_model_to_see() {
        let stream = sse(&[delta(json!({"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "fill", "arguments": "{\"from\": [0, 0, 0], block: Grey"}}]})), finish("tool_calls")]);
        let reply = parse(&stream, 50).0.unwrap();
        assert_eq!(reply.tool_calls[0].arguments, json!({}));
        assert_eq!(reply.tool_calls[0].malformed.as_deref(), Some("{\"from\": [0, 0, 0], block: Grey"));
    }

    #[test]
    fn an_error_in_the_stream_and_an_empty_stream_are_failures() {
        let failed = "data: {\"error\": {\"message\": \"model crashed\"}}\n\n";
        assert!(matches!(parse(failed, 16).0, Err(ProviderError::Server(m)) if m.contains("model crashed")));
        assert!(matches!(parse("", 16).0, Err(ProviderError::Network(_))));
    }

    fn turns() -> Vec<Turn> {
        vec![
            Turn::User { text: "Build a hull.".into() },
            Turn::Assistant {
                text: "On it.".into(),
                tool_calls: vec![ToolCall { id: "toolu_1".into(), name: "fill".into(), arguments: json!({"block": "Grey Hull"}), malformed: None }],
                raw: Some(Raw { provider: "claude".into(), model: "claude-opus-5-5".into(), content: json!([{"type": "thinking", "thinking": "", "signature": "c2ln"}]) }),
            },
            Turn::ToolResults { results: vec![ToolResult { call_id: "toolu_1".into(), text: "Filled 20 blocks.".into(), is_error: false }] },
            Turn::User { text: "(system) Carry on.".into() },
            Turn::User { text: "And paint it red.".into() },
        ]
    }

    #[test]
    fn the_request_carries_tools_history_and_settings() {
        let tools = vec![ToolSpec { name: "fill".into(), description: "Fills a box.".into(), schema: json!({"type": "object", "properties": {}}) }];
        let provider = OpenAiCompatible::new("http://127.0.0.1:1234/v1/", "google/gemma").with_temperature(0.5).with_reasoning_effort("low").with_max_tokens(900);
        let turns = turns();
        let body = provider.request_body(&Request { system: "You build ships.", turns: &turns, tools: &tools, max_tokens: None, reasoning_effort: None });
        assert_eq!(
            body,
            json!({
                "model": "google/gemma",
                "stream": true,
                "stream_options": {"include_usage": true},
                "temperature": 0.5,
                "max_tokens": 900,
                "reasoning_effort": "low",
                "tool_choice": "auto",
                "tools": [{"type": "function", "function": {"name": "fill", "description": "Fills a box.", "parameters": {"type": "object", "properties": {}}}}],
                "messages": [
                    {"role": "system", "content": "You build ships."},
                    {"role": "user", "content": "Build a hull."},
                    {"role": "assistant", "content": "On it.", "tool_calls": [{"id": "toolu_1", "type": "function", "function": {"name": "fill", "arguments": "{\"block\":\"Grey Hull\"}"}}]},
                    {"role": "tool", "tool_call_id": "toolu_1", "content": "Filled 20 blocks."},
                    {"role": "user", "content": "(system) Carry on.\n\nAnd paint it red."},
                ],
            })
        );
        // Another provider's thinking is not passed on.
        assert!(!body.to_string().contains("c2ln"));
        let kept = OpenAiCompatible::new("http://x/v1", "m").with_keep_loaded(3600).request_body(&Request { system: "s", turns: &[], tools: &[], max_tokens: None, reasoning_effort: None });
        assert_eq!(kept["ttl"], json!(3600));
        let plain = OpenAiCompatible::new("http://x/v1", "m").request_body(&Request { system: "s", turns: &[], tools: &[], max_tokens: Some(50), reasoning_effort: Some("high") });
        assert_eq!(plain, json!({"model": "m", "stream": true, "stream_options": {"include_usage": true}, "max_tokens": 50, "reasoning_effort": "high", "messages": [{"role": "system", "content": "s"}]}));
    }

    #[test]
    fn embedding_models_are_left_out_of_the_list() {
        let listed = json!({"object": "list", "data": [{"id": "google/gemma-4-26b"}, {"id": "text-embedding-nomic-embed-text-v1.5"}, {"id": "prism-ml/bonsai-27b"}]});
        let ids: Vec<String> = model_list(&listed).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["google/gemma-4-26b", "prism-ml/bonsai-27b"]);
    }

    #[test]
    fn the_key_stays_out_of_debug_output() {
        let provider = OpenAiCompatible::new("http://x/v1", "m").with_key("sk-secret");
        assert!(!format!("{provider:?}").contains("secret"));
    }
}
