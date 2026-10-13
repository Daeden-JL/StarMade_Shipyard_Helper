//! A provider that plays back a script instead of asking a model: for tests
//! of the agent loop, here and in the server.

use crate::message::{ToolCall, Turn};
use crate::provider::{Provider, ProviderError, Reply, Request, StopReason, TextSink, Usage};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Mutex;

/// One thing the scripted model does when asked.
#[derive(Clone, Debug)]
pub enum Script {
    Reply(Reply),
    Fail(ProviderError),
    /// Never answers, like a model that has stalled.
    Hang,
}

impl Script {
    /// An answer in text, ending the turn.
    pub fn says(text: &str) -> Script {
        Script::Reply(Reply { text: text.to_string(), tool_calls: Vec::new(), stop: StopReason::EndTurn, usage: Usage { input: 100, output: 10, ..Usage::default() }, raw: None })
    }

    /// Tool calls, each a name and its arguments. Ids are `call_1`, `call_2`...
    pub fn calls(calls: &[(&str, Value)]) -> Script {
        Script::calls_saying("", calls)
    }

    /// Tool calls with a remark before them.
    pub fn calls_saying(text: &str, calls: &[(&str, Value)]) -> Script {
        let tool_calls = calls.iter().enumerate().map(|(i, (name, arguments))| ToolCall { id: format!("call_{}", i + 1), name: name.to_string(), arguments: arguments.clone(), malformed: None }).collect();
        Script::Reply(Reply { text: text.to_string(), tool_calls, stop: StopReason::ToolUse, usage: Usage { input: 100, output: 10, ..Usage::default() }, raw: None })
    }
}

/// Plays a script back, one entry per model call, and keeps what it was asked.
pub struct ScriptedProvider {
    script: Mutex<VecDeque<Script>>,
    /// Whether the last entry is played again and again once reached.
    repeat_last: bool,
    asked: Mutex<Vec<Vec<Turn>>>,
    kind: &'static str,
}

impl ScriptedProvider {
    /// Once the script has run out, the model answers "Done.".
    pub fn new(script: Vec<Script>) -> Self {
        Self { script: Mutex::new(script.into()), repeat_last: false, asked: Mutex::new(Vec::new()), kind: "scripted" }
    }

    /// Plays the last entry forever: a model that never stops.
    pub fn repeating(script: Vec<Script>) -> Self {
        Self { repeat_last: true, ..Self::new(script) }
    }

    /// Answers to another [`Provider::kind`], to stand in for that provider.
    pub fn of_kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }

    /// The conversation as it stood at each call, in order.
    pub fn asked(&self) -> Vec<Vec<Turn>> {
        self.asked.lock().map(|asked| asked.clone()).unwrap_or_default()
    }

    fn next(&self) -> Script {
        let Ok(mut script) = self.script.lock() else { return Script::says("Done.") };
        if self.repeat_last && script.len() == 1 {
            return script[0].clone();
        }
        script.pop_front().unwrap_or_else(|| Script::says("Done."))
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn model(&self) -> &str {
        "scripted"
    }

    fn history_budget(&self) -> usize {
        1_000_000
    }

    async fn call(&self, request: &Request<'_>, on_text: &mut TextSink<'_>) -> Result<Reply, ProviderError> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(request.turns.to_vec());
        }
        match self.next() {
            Script::Reply(reply) => {
                // In two pieces, as a stream would bring it.
                let middle = reply.text.char_indices().nth(reply.text.chars().count() / 2).map_or(0, |(i, _)| i);
                for piece in [&reply.text[..middle], &reply.text[middle..]] {
                    if !piece.is_empty() {
                        on_text(piece);
                    }
                }
                Ok(reply)
            }
            Script::Fail(error) => Err(error),
            Script::Hang => std::future::pending().await,
        }
    }
}
