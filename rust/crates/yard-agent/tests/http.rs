//! Both providers over real HTTP against a small local server: streaming in
//! awkward pieces, the exact requests sent, errors, retries and timeouts.

use axum::body::Body;
use axum::http::{HeaderMap, Request as HttpRequest, Response, StatusCode};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yard_agent::{
    run, sample_catalog, CancelFlag, Claude, Conversation, MemoryWorld, OpenAiCompatible, Patience, Provider, ProviderError, Reply, Request, RunOptions, Stop, StopReason, ToolSpec, Turn,
};
use yard_core::Pos;

/// What the server answers to one request.
struct Canned {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    /// The body, sent piece by piece.
    pieces: Vec<Vec<u8>>,
    /// Wait before each piece.
    pause: Duration,
    /// Whether the connection is then held open without another byte.
    stall: bool,
}

impl Canned {
    /// A server-sent event stream cut into pieces of `size` bytes.
    fn stream(body: &str, size: usize) -> Self {
        Self { status: 200, headers: vec![("content-type", "text/event-stream")], pieces: body.as_bytes().chunks(size).map(<[u8]>::to_vec).collect(), pause: Duration::ZERO, stall: false }
    }

    fn json(status: u16, body: Value) -> Self {
        Self { status, headers: vec![("content-type", "application/json")], pieces: vec![body.to_string().into_bytes()], pause: Duration::ZERO, stall: false }
    }

    fn with_header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }
}

/// One request the server received.
#[derive(Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HeaderMap,
    body: Value,
}

/// A server that answers from a queue and remembers what it was asked.
struct Server {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    async fn start(answers: Vec<Canned>) -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::from(answers)));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new().fallback({
            let (queue, seen) = (queue.clone(), seen.clone());
            move |request: HttpRequest<Body>| answer(request, queue.clone(), seen.clone())
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { base, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn answer(request: HttpRequest<Body>, queue: Arc<Mutex<VecDeque<Canned>>>, seen: Arc<Mutex<Vec<Seen>>>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    seen.lock().unwrap().push(Seen { method: parts.method.to_string(), path: parts.uri.path().to_string(), headers: parts.headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) });
    let Some(canned) = queue.lock().unwrap().pop_front() else {
        return Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::from("the test server has no answer left")).unwrap();
    };
    let mut response = Response::builder().status(canned.status);
    for (name, value) in &canned.headers {
        response = response.header(*name, *value);
    }
    let (pause, stall) = (canned.pause, canned.stall);
    let pieces = futures_util::stream::unfold(canned.pieces.into_iter(), move |mut pieces| async move {
        match pieces.next() {
            Some(piece) => {
                tokio::time::sleep(pause).await;
                Some((Ok::<_, std::io::Error>(piece), pieces))
            }
            None if stall => std::future::pending().await,
            None => None,
        }
    });
    response.body(Body::from_stream(pieces)).unwrap()
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap_or("message"), e)).collect()
}

/// A Claude answer: optional thinking, optional text, tool calls, a stop reason.
fn claude_answer(thinking: Option<&str>, text: &str, calls: &[(&str, &str, Value)], stop: &str) -> String {
    let mut events = vec![json!({"type": "message_start", "message": {"id": "msg_1", "role": "assistant", "content": [], "usage": {"input_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 4000, "output_tokens": 1}}})];
    let mut index = 0;
    if let Some(signature) = thinking {
        events.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "signature_delta", "signature": signature}}));
        events.push(json!({"type": "content_block_stop", "index": index}));
        index += 1;
    }
    if !text.is_empty() {
        let middle = text.len() / 2;
        events.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": &text[..middle]}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": &text[middle..]}}));
        events.push(json!({"type": "content_block_stop", "index": index}));
        index += 1;
    }
    for (id, name, input) in calls {
        let input = input.to_string();
        let middle = input.len() / 2;
        events.push(json!({"type": "content_block_start", "index": index, "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": &input[..middle]}}));
        events.push(json!({"type": "content_block_delta", "index": index, "delta": {"type": "input_json_delta", "partial_json": &input[middle..]}}));
        events.push(json!({"type": "content_block_stop", "index": index}));
        index += 1;
    }
    events.push(json!({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 42}}));
    events.push(json!({"type": "message_stop"}));
    sse(&events)
}

fn claude_error(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"})
}

const KEY: &str = "sk-ant-test-0123456789-secret";

fn quick() -> Patience {
    Patience { silence: Duration::from_secs(5), retries: 2, backoff: Duration::from_millis(10), longest_wait: Duration::from_millis(50) }
}

fn claude(server: &Server) -> Claude {
    Claude::new(KEY, "claude-opus-5-5").with_base_url(&server.base).with_patience(quick())
}

fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec { name: "fill".into(), description: "Fills a box.".into(), schema: json!({"type": "object", "properties": {}}) }]
}

/// One call with a single user turn; returns the reply and the text streamed.
async fn ask(provider: &dyn Provider) -> (Result<Reply, ProviderError>, String) {
    let turns = [Turn::User { text: "Build a hull.".into() }];
    let tools = tools();
    let mut said = String::new();
    let reply = provider.call(&Request { system: "You build ships.", turns: &turns, tools: &tools, max_tokens: None, reasoning_effort: None }, &mut |t| said.push_str(t)).await;
    (reply, said)
}

#[tokio::test]
async fn claude_streams_an_answer_and_sends_the_key_only_as_a_header() {
    let stream = claude_answer(Some("c2lnbmF0dXJl"), "Laying the h\u{fc}ll.", &[("toolu_1", "fill", json!({"from": [0, 0, 1], "block": "Grey Hull"}))], "tool_use");
    let server = Server::start(vec![Canned::stream(&stream, 7)]).await;
    let (reply, said) = ask(&claude(&server)).await;
    let reply = reply.unwrap();
    assert_eq!(said, "Laying the h\u{fc}ll.");
    assert_eq!((reply.text.as_str(), &reply.stop), ("Laying the h\u{fc}ll.", &StopReason::ToolUse));
    assert_eq!(reply.tool_calls[0].arguments, json!({"from": [0, 0, 1], "block": "Grey Hull"}));
    assert_eq!((reply.usage.input, reply.usage.output, reply.usage.cache_read), (4020, 42, 4000));
    assert_eq!(reply.raw.unwrap().content[0], json!({"type": "thinking", "thinking": "", "signature": "c2lnbmF0dXJl"}));

    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/v1/messages"));
    let header = |name: &str| request.headers.get(name).map(|v| v.to_str().unwrap().to_string());
    assert_eq!(header("x-api-key").as_deref(), Some(KEY));
    assert_eq!(header("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(header("anthropic-beta").as_deref(), Some("server-side-fallback-2026-07-01"));
    assert_eq!(header("content-type").as_deref(), Some("application/json"));
    assert_eq!(header("authorization"), None);
    assert!(!request.body.to_string().contains("secret"), "the key is not in the body");
    assert_eq!(request.body["stream"], json!(true));
    assert_eq!(request.body["thinking"], json!({"type": "adaptive"}));
    assert_eq!(request.body["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(request.body["tools"][0]["eager_input_streaming"], json!(true));
}

#[tokio::test]
async fn a_bad_key_is_reported_at_once_without_naming_the_key() {
    let server = Server::start(vec![Canned::json(401, claude_error("authentication_error", "invalid x-api-key"))]).await;
    let (reply, _) = ask(&claude(&server)).await;
    let error = reply.unwrap_err();
    assert!(matches!(&error, ProviderError::Auth(m) if m.contains("did not accept the API key")), "{error:?}");
    assert!(!error.to_string().contains("secret") && !format!("{error:?}").contains("secret"));
    assert_eq!(server.seen().len(), 1, "a refused key is not tried again");
}

#[tokio::test]
async fn rate_limits_and_overloads_are_retried_after_a_short_wait() {
    let stream = claude_answer(None, "Here.", &[], "end_turn");
    let server = Server::start(vec![
        Canned::json(429, claude_error("rate_limit_error", "Too many requests")).with_header("retry-after", "0"),
        Canned::json(529, claude_error("overloaded_error", "Overloaded")),
        Canned::stream(&stream, 64),
    ])
    .await;
    let started = std::time::Instant::now();
    let (reply, said) = ask(&claude(&server)).await;
    assert_eq!((reply.unwrap().text.as_str(), said.as_str()), ("Here.", "Here."));
    assert_eq!(server.seen().len(), 3);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn an_overload_that_lasts_becomes_an_error_after_the_retries() {
    let overloaded = || Canned::json(529, claude_error("overloaded_error", "Overloaded"));
    let server = Server::start(vec![overloaded(), overloaded(), overloaded(), overloaded()]).await;
    let (reply, _) = ask(&claude(&server)).await;
    assert_eq!(reply.unwrap_err(), ProviderError::Overloaded("Claude is overloaded right now. Try again in a moment.".into()));
    assert_eq!(server.seen().len(), 3, "the first try and two retries");
}

#[tokio::test]
async fn a_malformed_request_is_reported_with_the_apis_own_words_and_not_retried() {
    let server = Server::start(vec![Canned::json(400, claude_error("invalid_request_error", "max_tokens: 640000 > 128000, which is the maximum allowed"))]).await;
    let (reply, _) = ask(&claude(&server)).await;
    assert_eq!(reply.unwrap_err(), ProviderError::BadRequest("Claude refused the request: max_tokens: 640000 > 128000, which is the maximum allowed".into()));
    assert_eq!(server.seen().len(), 1);
}

#[tokio::test]
async fn an_account_without_the_fallback_beta_is_served_without_it_from_then_on() {
    let stream = claude_answer(None, "Here.", &[], "end_turn");
    let rejected = "Unexpected value(s) `server-side-fallback-2026-07-01` for the `anthropic-beta` header.";
    let server = Server::start(vec![Canned::json(400, claude_error("invalid_request_error", rejected)), Canned::stream(&stream, 64), Canned::stream(&stream, 64)]).await;
    let provider = claude(&server);
    assert_eq!(ask(&provider).await.0.unwrap().text, "Here.");
    assert_eq!(ask(&provider).await.0.unwrap().text, "Here.");
    let seen = server.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].headers.contains_key("anthropic-beta") && seen[0].body["fallbacks"] == json!("default"));
    for later in &seen[1..] {
        assert!(!later.headers.contains_key("anthropic-beta") && later.body.get("fallbacks").is_none());
    }
    // Switched off by hand it is never sent.
    let server = Server::start(vec![Canned::stream(&stream, 64)]).await;
    ask(&claude(&server).with_refusal_fallback(false)).await.0.unwrap();
    assert!(!server.seen()[0].headers.contains_key("anthropic-beta"));
}

#[tokio::test]
async fn a_server_that_goes_silent_is_given_up_on() {
    let stream = claude_answer(None, "Half an answer and then nothing", &[], "end_turn");
    let half = &stream[..stream.find("content_block_stop").unwrap()];
    let stalled = || Canned { stall: true, ..Canned::stream(half, 4096) };
    let server = Server::start(vec![stalled(), stalled()]).await;
    let patience = Patience { silence: Duration::from_millis(150), ..quick() };
    let (reply, said) = ask(&claude(&server).with_patience(patience)).await;
    assert_eq!(reply.unwrap_err(), ProviderError::Timeout("Claude did not answer in time.".into()));
    assert_eq!(said, "Half an answer and then nothing");
    assert_eq!(server.seen().len(), 1, "text that reached the player is not asked for a second time");
}

#[tokio::test]
async fn an_error_in_the_middle_of_a_stream_is_retried_if_nothing_was_shown_yet() {
    let overloaded = sse(&[json!({"type": "message_start", "message": {"usage": {"input_tokens": 5}}}), json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})]);
    let stream = claude_answer(None, "Here.", &[], "end_turn");
    let server = Server::start(vec![Canned::stream(&overloaded, 32), Canned::stream(&stream, 32)]).await;
    assert_eq!(ask(&claude(&server)).await.0.unwrap().text, "Here.");
    assert_eq!(server.seen().len(), 2);
}

#[tokio::test]
async fn an_unreachable_server_is_a_plain_sentence() {
    let nowhere = Claude::new(KEY, "claude-opus-5-5").with_base_url("http://127.0.0.1:9").with_patience(Patience { retries: 0, ..quick() });
    let error = ask(&nowhere).await.0.unwrap_err();
    assert_eq!(error, ProviderError::Network("Claude could not be reached. Is it running and is the address right?".into()));
    let local = OpenAiCompatible::new("http://127.0.0.1:9/v1", "m").with_patience(Patience { retries: 0, ..quick() });
    assert!(matches!(ask(&local).await.0, Err(ProviderError::Network(m)) if m.starts_with("The model server could not be reached.")));
    assert!(matches!(OpenAiCompatible::list_models("http://127.0.0.1:9/v1", None).await, Err(ProviderError::Network(_))));
}

fn ship() -> MemoryWorld {
    MemoryWorld::new_ship(Arc::new(sample_catalog()), "Test Ship")
}

#[tokio::test]
async fn the_loop_sends_claudes_thinking_back_unmodified_with_the_tool_results() {
    let fill = json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"});
    let server = Server::start(vec![
        Canned::stream(&claude_answer(Some("U0lHTkFUVVJFLW9uZQ=="), "", &[("toolu_a", "fill", fill.clone()), ("toolu_b", "count_blocks", json!({}))], "tool_use"), 11),
        Canned::stream(&claude_answer(Some("U0lHTkFUVVJFLXR3bw=="), "Five grey blocks above the core.", &[], "end_turn"), 13),
    ])
    .await;
    let world = ship();
    let mut conversation = Conversation::new();
    let mut texts = String::new();
    let outcome = run(&claude(&server), &world, &mut conversation, "Put five grey blocks above the core.", &RunOptions::default(), &CancelFlag::new(), &mut |event| {
        if let yard_agent::Event::Text { text } = event {
            texts.push_str(&text);
        }
    })
    .await
    .unwrap();
    assert_eq!((outcome.stop, outcome.steps, outcome.blocks_changed), (Stop::Finished, 2, 5));
    assert_eq!(texts, "Five grey blocks above the core.");
    assert_eq!(world.block("", Pos::new(0, 1, 4)).ty(), 598);
    assert_eq!(outcome.usage.cache_read, 8000);

    let seen = server.seen();
    let second = &seen[1].body;
    // The same prefix as the first request: system prompt, tools, first message.
    assert_eq!(second["system"], seen[0].body["system"]);
    assert_eq!(second["tools"], seen[0].body["tools"]);
    assert!(second["tools"].as_array().unwrap().len() >= 20);
    assert_eq!(second["messages"][0]["content"][0]["text"], "Put five grey blocks above the core.");
    // The assistant turn exactly as it was streamed, thinking block first.
    assert_eq!(
        second["messages"][1],
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "", "signature": "U0lHTkFUVVJFLW9uZQ=="},
            {"type": "tool_use", "id": "toolu_a", "name": "fill", "input": fill},
            {"type": "tool_use", "id": "toolu_b", "name": "count_blocks", "input": {}},
        ]})
    );
    // Both results in one user message, in the order of the calls.
    let results = second["messages"][2]["content"].as_array().unwrap();
    assert_eq!(second["messages"][2]["role"], "user");
    assert_eq!(results.len(), 2);
    assert_eq!((&results[0]["type"], &results[0]["tool_use_id"]), (&json!("tool_result"), &json!("toolu_a")));
    assert!(results[0]["content"].as_str().unwrap().contains("5 blocks placed") && results[0].get("is_error").is_none());
    assert_eq!(results[1]["tool_use_id"], "toolu_b");
    assert_eq!(results[1]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(second["messages"].as_array().unwrap().len(), 3);
    // The final answer is kept with its thinking for the next request.
    assert!(matches!(conversation.turns().last(), Some(Turn::Assistant { raw: Some(raw), .. }) if raw.content[0]["signature"] == "U0lHTkFUVVJFLXR3bw=="));
}

#[tokio::test]
async fn thinking_claude_no_longer_accepts_is_dropped_and_the_request_made_again() {
    let fill = json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"});
    let binding = "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block.";
    let server = Server::start(vec![
        Canned::stream(&claude_answer(Some("b2xk"), "", &[("toolu_a", "fill", fill)], "tool_use"), 64),
        Canned::json(400, claude_error("invalid_request_error", binding)),
        Canned::stream(&claude_answer(Some("bmV3"), "Done.", &[], "end_turn"), 64),
    ])
    .await;
    let (world, mut conversation) = (ship(), Conversation::new());
    let outcome = run(&claude(&server), &world, &mut conversation, "Build.", &RunOptions::default(), &CancelFlag::new(), &mut |_| {}).await.unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Finished, 2));
    let seen = server.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen[1].body.to_string().contains("b2xk"));
    let retried = &seen[2].body;
    assert!(!retried.to_string().contains("\"thinking\":\"\"") && !retried.to_string().contains("b2xk"), "no thinking block is sent again");
    assert_eq!(retried["messages"][1]["content"][0]["type"], "tool_use", "the tool call itself stays");
    assert_eq!(retried["messages"][2]["content"][0]["type"], "tool_result");
}

fn openai_chunks(chunks: &[Value]) -> String {
    let mut out: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    out.push_str("data: [DONE]\n\n");
    out
}

fn delta(delta: Value, finish: Option<&str>) -> Value {
    json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
}

fn openai_tool_call(name: &str, arguments: &Value) -> String {
    let arguments = arguments.to_string();
    let middle = arguments.len() / 2;
    openai_chunks(&[
        delta(json!({"role": "assistant", "reasoning_content": "The player wants blocks."}), None),
        delta(json!({"content": "<think>five of them</think>"}), None),
        delta(json!({"tool_calls": [{"index": 0, "id": "call_77", "type": "function", "function": {"name": name, "arguments": ""}}]}), None),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": &arguments[..middle]}}]}), None),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": &arguments[middle..]}}]}), None),
        delta(json!({}), Some("tool_calls")),
        json!({"id": "chatcmpl-1", "choices": [], "usage": {"prompt_tokens": 900, "completion_tokens": 30}}),
    ])
}

fn openai_text(text: &str) -> String {
    let middle = text.len() / 2;
    openai_chunks(&[delta(json!({"role": "assistant", "content": &text[..middle]}), None), delta(json!({"content": &text[middle..]}), None), delta(json!({}), Some("stop"))])
}

fn local(server: &Server) -> OpenAiCompatible {
    OpenAiCompatible::new(format!("{}/v1", server.base), "google/gemma-4-26b-a4b-qat").with_patience(quick())
}

#[tokio::test]
async fn the_loop_builds_through_an_openai_compatible_server() {
    let fill = json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"});
    let server = Server::start(vec![Canned::stream(&openai_tool_call("fill", &fill), 9), Canned::stream(&openai_text("Five grey blocks above the core."), 5)]).await;
    let (world, mut conversation) = (ship(), Conversation::new());
    let mut texts = String::new();
    let options = RunOptions { reasoning_effort: Some("low".into()), ..RunOptions::default() };
    let outcome = run(&local(&server), &world, &mut conversation, "Put five grey blocks above the core.", &options, &CancelFlag::new(), &mut |event| {
        if let yard_agent::Event::Text { text } = event {
            texts.push_str(&text);
        }
    })
    .await
    .unwrap();
    assert_eq!((outcome.stop, outcome.steps, outcome.blocks_changed), (Stop::Finished, 2, 5));
    assert_eq!(texts, "Five grey blocks above the core.", "thinking out loud never reaches the player");
    assert_eq!((outcome.usage.input, outcome.usage.output), (900, 30));
    assert_eq!(world.block("", Pos::new(0, 1, 2)).ty(), 598);

    let seen = server.seen();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("POST", "/v1/chat/completions"));
    assert!(!seen[0].headers.contains_key("authorization"), "a local server is sent no key");
    assert_eq!(seen[0].body["model"], "google/gemma-4-26b-a4b-qat");
    assert_eq!((&seen[0].body["stream"], &seen[0].body["reasoning_effort"]), (&json!(true), &json!("low")));
    assert_eq!(seen[0].body["messages"][0]["role"], "system");
    assert_eq!(seen[0].body["tools"][0]["type"], "function");
    let second = &seen[1].body["messages"];
    assert_eq!(second[2], json!({"role": "assistant", "content": "", "tool_calls": [{"id": "call_77", "type": "function", "function": {"name": "fill", "arguments": fill.to_string()}}]}));
    assert_eq!((&second[3]["role"], &second[3]["tool_call_id"]), (&json!("tool"), &json!("call_77")));
    assert!(second[3]["content"].as_str().unwrap().contains("5 blocks placed"));
}

#[tokio::test]
async fn a_conversation_moves_between_providers_without_carrying_thinking_across() {
    let fill = json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"});
    let claude_server = Server::start(vec![
        Canned::stream(&claude_answer(Some("Y2xhdWRlLXNpZw=="), "", &[("toolu_a", "fill", fill)], "tool_use"), 64),
        Canned::stream(&claude_answer(Some("Y2xhdWRlLXR3bw=="), "A grey row.", &[], "end_turn"), 64),
        Canned::stream(&claude_answer(None, "Still here.", &[], "end_turn"), 64),
    ])
    .await;
    let local_server = Server::start(vec![Canned::stream(&openai_text("Now it is red."), 64)]).await;
    let (world, mut conversation) = (ship(), Conversation::new());
    let quiet = &mut |_: yard_agent::Event| {};
    run(&claude(&claude_server), &world, &mut conversation, "Build a grey row.", &RunOptions::default(), &CancelFlag::new(), quiet).await.unwrap();
    run(&local(&local_server), &world, &mut conversation, "Make it red.", &RunOptions::default(), &CancelFlag::new(), quiet).await.unwrap();
    run(&claude(&claude_server), &world, &mut conversation, "Are you still there?", &RunOptions::default(), &CancelFlag::new(), quiet).await.unwrap();

    // The local model got the turns as plain text and tool calls.
    let to_local = local_server.seen()[0].body.to_string();
    assert!(!to_local.contains("Y2xhdWRl") && !to_local.contains("thinking") && to_local.contains("toolu_a") && to_local.contains("A grey row."));
    // Back on Claude: its own turns go back as they were, the local model's as text.
    let back = &claude_server.seen()[2].body["messages"];
    assert_eq!(back[1]["content"][0], json!({"type": "thinking", "thinking": "", "signature": "Y2xhdWRlLXNpZw=="}));
    assert_eq!(back[3]["content"][0]["signature"], "Y2xhdWRlLXR3bw==");
    assert_eq!(back[5], json!({"role": "assistant", "content": [{"type": "text", "text": "Now it is red."}]}));
    assert_eq!(back[6]["content"][0]["text"], "Are you still there?");
}

#[tokio::test]
async fn an_openai_compatible_server_gets_the_key_as_a_bearer_token_and_its_errors_are_told_apart() {
    let server = Server::start(vec![
        Canned::stream(&openai_text("Hello."), 64),
        Canned::json(400, json!({"error": {"message": "context length exceeded: 9000 > 8192"}})),
        Canned::json(401, json!({"error": {"message": "Incorrect API key provided"}})),
        Canned::json(500, json!({"error": "model crashed"})),
        Canned::json(503, json!({"error": {"message": "loading model"}})),
        Canned::stream(&openai_text("Back."), 64),
    ])
    .await;
    let provider = local(&server).with_key("sk-local-secret");
    assert_eq!(ask(&provider).await.0.unwrap().text, "Hello.");
    assert_eq!(server.seen()[0].headers.get("authorization").unwrap(), "Bearer sk-local-secret");
    assert_eq!(ask(&provider).await.0.unwrap_err(), ProviderError::BadRequest("The model server refused the request: context length exceeded: 9000 > 8192".into()));
    let unauthorised = ask(&provider).await.0.unwrap_err();
    assert!(matches!(&unauthorised, ProviderError::Auth(m) if !m.contains("secret")));
    // A crash and a busy server are each tried again; here the third try works.
    assert_eq!(ask(&provider).await.0.unwrap().text, "Back.");
    assert_eq!(server.seen().len(), 6);
}

#[tokio::test]
async fn the_models_an_endpoint_offers_are_listed_without_embedding_models() {
    let listed = json!({"object": "list", "data": [
        {"id": "google/gemma-4-26b-a4b-qat", "object": "model"}, {"id": "text-embedding-nomic-embed-text-v1.5", "object": "model"},
        {"id": "meta/muse-glimmer", "object": "model"}, {"id": "prism-ml/bonsai-27b", "object": "model"},
    ]});
    let server = Server::start(vec![Canned::json(200, listed), Canned::json(401, json!({"error": {"message": "no key"}}))]).await;
    let base = format!("{}/v1/", server.base);
    let models = OpenAiCompatible::list_models(&base, Some("sk-local")).await.unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["google/gemma-4-26b-a4b-qat", "meta/muse-glimmer", "prism-ml/bonsai-27b"]);
    assert!(models.iter().all(|m| m.provider == "openai"));
    let seen = server.seen();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "/v1/models"));
    assert_eq!(seen[0].headers.get("authorization").unwrap(), "Bearer sk-local");
    assert!(matches!(OpenAiCompatible::list_models(&base, None).await, Err(ProviderError::Auth(_))));
}

#[tokio::test]
async fn stopping_abandons_a_model_call_that_is_still_streaming() {
    let stream = openai_text("A very long answer that will never be finished");
    let slow = Canned { pause: Duration::from_millis(40), stall: true, ..Canned::stream(&stream[..stream.len() / 2], 24) };
    let server = Server::start(vec![slow]).await;
    let (world, mut conversation) = (ship(), Conversation::new());
    let cancel = CancelFlag::new();
    let stopper = cancel.clone();
    let started = std::time::Instant::now();
    let outcome = run(&local(&server), &world, &mut conversation, "Tell me a story.", &RunOptions::default(), &cancel, &mut |event| {
        if matches!(event, yard_agent::Event::Text { .. }) {
            stopper.cancel();
        }
    })
    .await
    .unwrap();
    assert_eq!(outcome.stop, Stop::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3), "the request ended at once, not at the silence limit");
}
