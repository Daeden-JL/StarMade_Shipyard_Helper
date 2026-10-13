//! The conversation as the agent keeps it: turns in a form no provider owns,
//! each assistant turn with the raw content its provider wants back.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What an old tool result is replaced with when the history must shrink.
pub const TRIMMED: &str = "[older tool output removed to save space]";
/// Results of the request in progress that are kept whole when even that
/// request has outgrown the budget.
const KEEP_RECENT_RESULTS: usize = 4;
/// Steps of its own the agent remembers for `undo`.
const MAX_STEP_MARKS: usize = 50;

/// One tool call a model asked for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as a JSON object. Empty if the model's text was not JSON.
    pub arguments: Value,
    /// The model's own text where it could not be read as a JSON object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub malformed: Option<String>,
}

/// What a tool call came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub text: String,
    #[serde(default)]
    pub is_error: bool,
}

/// An assistant turn exactly as its provider sent it. Claude's thinking
/// blocks carry signatures that are only valid sent back unmodified.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Raw {
    /// [`crate::Provider::kind`] of the provider that produced it.
    pub provider: String,
    pub model: String,
    pub content: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Turn {
    /// The player, or a note from the agent loop to the model.
    User { text: String },
    Assistant {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<Raw>,
    },
    /// The answers to every tool call of the assistant turn before.
    ToolResults { results: Vec<ToolResult> },
}

/// A step the agent made in an entity, remembered so that `undo` takes back
/// that step and no one else's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepMark {
    pub entity: String,
    /// A fingerprint of the change; see [`crate::tools::fingerprint`].
    pub print: u64,
    /// What the step was, for the report after undoing it.
    pub what: String,
}

/// One chat about one design. The server keeps one per design and hands it
/// to [`crate::run`] with every message.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    turns: Vec<Turn>,
    #[serde(default)]
    steps: Vec<StepMark>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Forgets everything said, and with it which steps were the agent's.
    pub fn clear(&mut self) {
        self.turns.clear();
        self.steps.clear();
    }

    pub fn push_user(&mut self, text: &str) {
        self.turns.push(Turn::User { text: text.to_string() });
    }

    pub fn push_assistant(&mut self, text: &str, tool_calls: Vec<ToolCall>, raw: Option<Raw>) {
        self.turns.push(Turn::Assistant { text: text.to_string(), tool_calls, raw });
    }

    pub fn push_results(&mut self, results: Vec<ToolResult>) {
        self.turns.push(Turn::ToolResults { results });
    }

    /// Takes the last turn back if it is the player's: a message the model
    /// never answered.
    pub fn pop_user(&mut self) -> bool {
        let is_user = matches!(self.turns.last(), Some(Turn::User { .. }));
        if is_user {
            self.turns.pop();
        }
        is_user
    }

    /// Answers tool calls left without a result, which a crash between a
    /// model's reply and its tools can leave behind. Providers refuse a
    /// history with an unanswered call.
    pub fn repair(&mut self) {
        let mut i = 0;
        while i < self.turns.len() {
            let Turn::Assistant { tool_calls, .. } = &self.turns[i] else {
                i += 1;
                continue;
            };
            let answered: Vec<&str> = match self.turns.get(i + 1) {
                Some(Turn::ToolResults { results }) => results.iter().map(|r| r.call_id.as_str()).collect(),
                _ => Vec::new(),
            };
            let missing: Vec<ToolResult> = tool_calls
                .iter()
                .filter(|c| !answered.contains(&c.id.as_str()))
                .map(|c| ToolResult { call_id: c.id.clone(), text: "Not run: the request was interrupted.".to_string(), is_error: true })
                .collect();
            if !missing.is_empty() {
                match self.turns.get_mut(i + 1) {
                    Some(Turn::ToolResults { results }) => results.extend(missing),
                    _ => self.turns.insert(i + 1, Turn::ToolResults { results: missing }),
                }
                self.forget_raw();
            }
            i += 1;
        }
    }

    /// Roughly how much the history weighs in a request, in characters.
    pub fn size(&self) -> usize {
        self.turns.iter().map(turn_size).sum()
    }

    /// Drops what every provider wanted back unmodified, so that each
    /// assistant turn is next sent as its plain text and tool calls. Needed
    /// whenever earlier turns change: Claude's thinking blocks are bound to
    /// the history that came before them.
    pub fn forget_raw(&mut self) {
        for turn in &mut self.turns {
            if let Turn::Assistant { raw, .. } = turn {
                *raw = None;
            }
        }
    }

    /// Shrinks the history once it has outgrown `budget` characters, well
    /// below the budget so that it is not done again on every step. Old tool
    /// outputs go first (the model has acted on them), then whole exchanges
    /// from the oldest on, and last the older outputs of the request in
    /// progress. A tool call always keeps a result. Returns whether anything
    /// was changed.
    pub fn compact(&mut self, budget: usize) -> bool {
        if self.size() <= budget {
            return false;
        }
        let target = budget * 6 / 10;
        let current = self.turns.iter().rposition(|t| matches!(t, Turn::User { .. })).unwrap_or(0);
        self.trim_results(0..current, target);
        // An exchange starts at a user turn; everything before the second
        // such turn is the oldest exchange.
        while self.size() > target {
            let Some(next) = self.turns.iter().skip(1).position(|t| matches!(t, Turn::User { .. })).map(|i| i + 1) else { break };
            self.turns.drain(..next);
        }
        if self.size() > target {
            let results = self.turns.iter().filter(|t| matches!(t, Turn::ToolResults { .. })).count();
            let keep_from = self.turns.iter().enumerate().filter(|(_, t)| matches!(t, Turn::ToolResults { .. })).map(|(i, _)| i).nth(results.saturating_sub(KEEP_RECENT_RESULTS));
            self.trim_results(0..keep_from.unwrap_or(0), target);
        }
        self.forget_raw();
        true
    }

    /// Replaces tool outputs in a range of turns, oldest first, until the
    /// history fits `target`.
    fn trim_results(&mut self, range: std::ops::Range<usize>, target: usize) {
        let mut size = self.size();
        for turn in self.turns[range].iter_mut() {
            let Turn::ToolResults { results } = turn else { continue };
            for result in results.iter_mut().filter(|r| r.text.len() > TRIMMED.len()) {
                if size <= target {
                    return;
                }
                size -= result.text.len() - TRIMMED.len();
                result.text = TRIMMED.to_string();
            }
        }
    }

    /// Remembers a step the agent made.
    pub fn note_step(&mut self, mark: StepMark) {
        self.steps.push(mark);
        if self.steps.len() > MAX_STEP_MARKS {
            self.steps.remove(0);
        }
    }

    /// The agent's latest step in an entity that it has not taken back.
    pub fn last_step(&self, entity: &str) -> Option<&StepMark> {
        self.steps.iter().rev().find(|s| s.entity == entity)
    }

    /// Forgets the agent's latest step in an entity, once undone.
    pub fn forget_step(&mut self, entity: &str) {
        if let Some(i) = self.steps.iter().rposition(|s| s.entity == entity) {
            self.steps.remove(i);
        }
    }
}

fn turn_size(turn: &Turn) -> usize {
    match turn {
        Turn::User { text } => text.len(),
        Turn::Assistant { raw: Some(raw), .. } => raw.content.to_string().len(),
        Turn::Assistant { text, tool_calls, .. } => text.len() + tool_calls.iter().map(|c| c.name.len() + c.arguments.to_string().len()).sum::<usize>(),
        Turn::ToolResults { results } => results.iter().map(|r| r.text.len()).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str) -> ToolCall {
        ToolCall { id: id.to_string(), name: "fill".to_string(), arguments: json!({"n": 1}), malformed: None }
    }

    fn result(id: &str, len: usize) -> ToolResult {
        ToolResult { call_id: id.to_string(), text: "x".repeat(len), is_error: false }
    }

    /// `exchanges` requests, each: the player, a tool call with a long
    /// result, and an answer.
    fn chat(exchanges: usize, result_len: usize) -> Conversation {
        let mut c = Conversation::new();
        for i in 0..exchanges {
            c.push_user(&format!("request {i}"));
            let raw = Raw { provider: "claude".into(), model: "m".into(), content: json!([{"type": "thinking", "thinking": "", "signature": "s"}]) };
            c.push_assistant("", vec![call(&format!("c{i}"))], Some(raw));
            c.push_results(vec![result(&format!("c{i}"), result_len)]);
            c.push_assistant(&format!("answer {i}"), vec![], None);
        }
        c
    }

    /// Every tool call is followed by a turn that answers it, and every
    /// result answers a call of the turn before.
    fn assert_paired(c: &Conversation) {
        for (i, turn) in c.turns().iter().enumerate() {
            match turn {
                Turn::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                    let Some(Turn::ToolResults { results }) = c.turns().get(i + 1) else { panic!("call at turn {i} has no results") };
                    assert_eq!(tool_calls.iter().map(|c| &c.id).collect::<Vec<_>>(), results.iter().map(|r| &r.call_id).collect::<Vec<_>>());
                }
                Turn::ToolResults { .. } => {
                    assert!(matches!(c.turns().get(i.wrapping_sub(1)), Some(Turn::Assistant { tool_calls, .. }) if !tool_calls.is_empty()), "results at turn {i} answer nothing");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn a_history_within_budget_is_left_alone() {
        let mut c = chat(3, 100);
        let before = c.clone();
        assert!(!c.compact(10_000));
        assert_eq!(c, before);
    }

    #[test]
    fn old_tool_outputs_go_first_and_no_call_loses_its_result() {
        let mut c = chat(4, 1000);
        assert!(c.compact(3500));
        assert_paired(&c);
        assert_eq!(c.turns().len(), 16, "trimming outputs was enough; no exchange was dropped");
        let texts: Vec<&str> = c.turns().iter().filter_map(|t| if let Turn::ToolResults { results } = t { Some(results[0].text.as_str()) } else { None }).collect();
        assert_eq!(&texts[..2], &[TRIMMED, TRIMMED]);
        assert_eq!(texts[3].len(), 1000, "the latest exchange keeps its output");
        assert!(c.size() <= 3500);
    }

    #[test]
    fn whole_old_exchanges_are_dropped_when_outputs_are_not_enough() {
        let mut c = chat(6, 10);
        for _ in 0..6 {
            c.push_user(&"long request ".repeat(40));
            c.push_assistant(&"long answer ".repeat(40), vec![], None);
        }
        assert!(c.compact(3000));
        assert_paired(&c);
        assert!(matches!(c.turns().first(), Some(Turn::User { .. })), "the history still starts with the player");
        assert!(c.size() <= 3000 && c.turns().len() < 12);
        assert!(matches!(c.turns().last(), Some(Turn::Assistant { .. })));
    }

    #[test]
    fn the_request_in_progress_keeps_its_latest_outputs() {
        let mut c = Conversation::new();
        c.push_user("build a ship");
        for i in 0..10 {
            c.push_assistant("", vec![call(&format!("c{i}"))], None);
            c.push_results(vec![result(&format!("c{i}"), 1000)]);
        }
        assert!(c.compact(5000));
        assert_paired(&c);
        let lens: Vec<usize> = c.turns().iter().filter_map(|t| if let Turn::ToolResults { results } = t { Some(results[0].text.len()) } else { None }).collect();
        assert_eq!(lens.len(), 10);
        assert!(lens[..6].iter().all(|l| *l == TRIMMED.len()), "{lens:?}");
        assert!(lens[6..].iter().all(|l| *l == 1000), "{lens:?}");
    }

    #[test]
    fn changing_the_history_drops_what_providers_wanted_back_unmodified() {
        let mut c = chat(4, 1000);
        assert!(c.turns().iter().any(|t| matches!(t, Turn::Assistant { raw: Some(_), .. })));
        c.compact(3500);
        assert!(!c.turns().iter().any(|t| matches!(t, Turn::Assistant { raw: Some(_), .. })));
    }

    #[test]
    fn an_unanswered_tool_call_is_answered_before_the_next_request() {
        let mut c = Conversation::new();
        c.push_user("go");
        c.push_assistant("", vec![call("a"), call("b")], None);
        c.push_results(vec![result("a", 5)]);
        c.push_assistant("", vec![call("c")], None);
        c.repair();
        assert_paired(&c);
        let Turn::ToolResults { results } = &c.turns()[2] else { panic!() };
        assert!(results[1].is_error && results[1].call_id == "b");
        assert!(matches!(&c.turns()[4], Turn::ToolResults { results } if results[0].call_id == "c"));
    }

    #[test]
    fn it_survives_a_round_trip_through_json() {
        let mut c = chat(2, 10);
        c.note_step(StepMark { entity: String::new(), print: 7, what: "fill".into() });
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Conversation>(&json).unwrap(), c);
        assert!(json.contains(r#""role":"tool_results""#) && json.contains(r#""role":"user""#));
    }

    #[test]
    fn steps_are_remembered_per_entity_latest_first() {
        let mut c = Conversation::new();
        c.note_step(StepMark { entity: String::new(), print: 1, what: "a".into() });
        c.note_step(StepMark { entity: "ATTACHED_0".into(), print: 2, what: "b".into() });
        c.note_step(StepMark { entity: String::new(), print: 3, what: "c".into() });
        assert_eq!(c.last_step("").unwrap().print, 3);
        c.forget_step("");
        assert_eq!(c.last_step("").unwrap().print, 1);
        assert_eq!(c.last_step("ATTACHED_0").unwrap().print, 2);
        assert!(c.last_step("ATTACHED_1").is_none());
    }
}
