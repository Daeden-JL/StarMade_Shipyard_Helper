//! The tool loop: one player message, carried through model calls and tool
//! calls until the model has answered.

use crate::filter;
use crate::message::{Conversation, ToolCall, ToolResult};
use crate::provider::{clip, Provider, ProviderError, Request, StopReason, Usage};
use crate::tools::{self, Toolbox};
use crate::world::World;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The system prompt every conversation starts from.
pub const SYSTEM_PROMPT: &str = include_str!("../resources/system.md");

/// Times a model is told to act after only announcing what it will do.
const MAX_NUDGES: usize = 2;
/// Times the same failing call may be repeated before the request ends.
const MAX_REPEATS: usize = 3;
/// Times in a row a model's reply may be cut off at the output limit.
const MAX_CUT_OFFS: usize = 3;

/// How a request is run.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Most model calls for one player message.
    pub max_steps: usize,
    /// Characters the conversation may hold before old tool outputs are
    /// dropped; None leaves it to the provider.
    pub history_budget: Option<usize>,
    /// Output limit per model call; None leaves it to the provider.
    pub max_tokens: Option<u32>,
    /// `low`, `medium` or `high` (Claude also takes `xhigh` and `max`);
    /// None leaves it to the provider.
    pub reasoning_effort: Option<String>,
    /// Whether requests are screened for sexual content before the model
    /// sees them.
    pub content_filter: bool,
    /// Replaces [`SYSTEM_PROMPT`]. Changing it within a conversation costs
    /// the provider's prompt cache and Claude's kept thinking.
    pub system_prompt: Option<String>,
    /// The entity the player has open: tools work on it when the model
    /// leaves out `entity`. "" is the main entity.
    pub focus: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { max_steps: 40, history_budget: None, max_tokens: None, reasoning_effort: None, content_filter: true, system_prompt: None, focus: String::new() }
    }
}

/// Lets the player stop a request from another task. Cheap to clone; all
/// clones share one flag.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag {
    cancelled: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
}

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops the request: a model call in flight is abandoned, tool calls
    /// not yet run are skipped.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the flag is set.
    pub async fn cancelled(&self) {
        loop {
            let woken = self.wake.notified();
            if self.is_cancelled() {
                return;
            }
            woken.await;
        }
    }
}

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// The model answered.
    Finished,
    /// The step limit was reached; "continue" picks the work up.
    StepLimit,
    /// The player stopped it.
    Cancelled,
    /// The request was declined: by the content screen, the model's
    /// provider, or the model.
    Refused,
    /// The model kept repeating a call that fails, or kept being cut off.
    Stuck,
}

/// What happens during a request, as it happens. Serialises to JSON with a
/// `type` field, ready to be sent to a browser.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A model call begins. `step` counts from 1.
    Step { step: usize },
    /// A piece of the answer, in order. Text before a tool call is a
    /// remark made while working; the text after the last tool is the answer.
    Text { text: String },
    ToolStarted { name: String, summary: String },
    ToolFinished {
        name: String,
        ok: bool,
        /// The first line of what the tool told the model.
        summary: String,
        /// Blocks placed, changed or removed.
        changed: usize,
    },
    /// Tokens used by the model call that just ended.
    Usage { input: u64, output: u64 },
    /// The request is over. Always the last event unless it failed.
    Done { stop: Stop },
    /// The request failed; the message is for the player. Always the last event.
    Failed { message: String },
}

/// A finished request.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub stop: Stop,
    /// The answer for the player.
    pub text: String,
    /// Model calls made.
    pub steps: usize,
    pub tool_calls: usize,
    /// Blocks placed, changed or removed, over all tool calls.
    pub blocks_changed: usize,
    pub usage: Usage,
}

/// Why a request failed. The conversation stays usable: the player's
/// message and what was done so far are in it, and the next message carries on.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Provider(#[from] ProviderError),
}


/// The player's message as the model reads it: on a design with docked
/// entities, followed by which entity the player has open, since that is
/// where tools work unless told otherwise.
fn with_focus(world: &dyn World, message: &str, focus: &str) -> String {
    let entities = world.entities();
    if entities.len() < 2 {
        return message.to_string();
    }
    let open = match entities.iter().find(|e| e.path == focus) {
        Some(e) if !e.path.is_empty() => format!("the docked entity \"{}\"", e.path),
        _ => "the main entity".to_string(),
    };
    format!("{message}\n\n[The player has {open} open. Tools work on it when you leave out entity; give entity \"\" for the main entity.]")
}
/// Runs one player message to completion: model call, tool calls, model
/// call... until the model stops, the step limit is hit, or `cancel` is
/// set. Events go to `on_event` as they happen.
pub async fn run(
    provider: &dyn Provider,
    world: &dyn World,
    conversation: &mut Conversation,
    message: &str,
    options: &RunOptions,
    cancel: &CancelFlag,
    on_event: &mut (dyn FnMut(Event) + Send),
) -> Result<Outcome, AgentError> {
    let mut run = Run { outcome: Outcome { stop: Stop::Finished, text: String::new(), steps: 0, tool_calls: 0, blocks_changed: 0, usage: Usage::default() }, on_event };
    if options.content_filter && !filter::allows(message) {
        return Ok(run.end(Stop::Refused, filter::REFUSAL, true));
    }
    conversation.repair();
    conversation.push_user(&with_focus(world, message, &options.focus));
    let toolbox = Toolbox::new().focused_on(&options.focus);
    let specs = toolbox.specs();
    let system = options.system_prompt.as_deref().unwrap_or(SYSTEM_PROMPT);
    let budget = options.history_budget.unwrap_or_else(|| provider.history_budget());
    let (mut nudges, mut cut_offs, mut repeats) = (0, 0, 0);
    let mut last_failure: Option<String> = None;

    while run.outcome.steps < options.max_steps {
        if cancel.is_cancelled() {
            return Ok(run.stopped(conversation));
        }
        conversation.compact(budget);
        run.outcome.steps += 1;
        (run.on_event)(Event::Step { step: run.outcome.steps });
        let mut thinking_dropped = false;
        let reply = loop {
            let request = Request { system, turns: conversation.turns(), tools: &specs, max_tokens: options.max_tokens, reasoning_effort: options.reasoning_effort.as_deref() };
            let mut said = |text: &str| (run.on_event)(Event::Text { text: text.to_string() });
            let result = tokio::select! {
                result = provider.call(&request, &mut said) => result,
                _ = cancel.cancelled() => return Ok(run.stopped(conversation)),
            };
            match result {
                Ok(reply) => break reply,
                // Thinking blocks the provider no longer accepts: drop them
                // all, once, and ask again.
                Err(ProviderError::ThinkingRejected(_)) if !thinking_dropped => {
                    thinking_dropped = true;
                    conversation.forget_raw();
                }
                Err(error) => {
                    (run.on_event)(Event::Failed { message: error.to_string() });
                    return Err(error.into());
                }
            }
        };
        run.outcome.usage.add(reply.usage);
        (run.on_event)(Event::Usage { input: reply.usage.input, output: reply.usage.output });

        if reply.stop == StopReason::Refusal {
            // Nothing of a refused turn is kept or run. A request refused
            // outright is taken out of the history, so that the next one is
            // judged on its own.
            if run.outcome.steps == 1 {
                conversation.pop_user();
            } else {
                conversation.push_assistant(REFUSED, Vec::new(), None);
            }
            let text = if reply.text.trim().is_empty() { REFUSED } else { "" };
            return Ok(run.end(Stop::Refused, text, !text.is_empty()));
        }
        let cut_off = reply.stop == StopReason::MaxTokens;
        cut_offs = if cut_off { cut_offs + 1 } else { 0 };
        if cut_off && cut_offs >= MAX_CUT_OFFS {
            conversation.push_assistant(CUT_OFF_TOO_OFTEN, Vec::new(), None);
            return Ok(run.end(Stop::Stuck, CUT_OFF_TOO_OFTEN, true));
        }

        if reply.tool_calls.is_empty() {
            let text = reply.text.trim();
            if cut_off {
                // Out of room mid-thought: say so and let it try again.
                if !text.is_empty() {
                    conversation.push_assistant(text, Vec::new(), reply.raw);
                }
                conversation.push_user("(system) Your last reply was cut off at the output limit before it finished. Think less and continue: call the next tool, or give your short final answer.");
                continue;
            }
            if nudges < MAX_NUDGES && announces_work_without_doing_it(text) {
                // Models often stop to say "I will now build X" and then never do.
                nudges += 1;
                conversation.push_assistant(text, Vec::new(), reply.raw);
                conversation.push_user("(system) You described what you will do but did not call any tool. Do it now with tool calls, and answer in text only once the work is finished.");
                (run.on_event)(Event::Text { text: "\n\n".to_string() });
                continue;
            }
            let answer = if text.is_empty() { "Done." } else { text };
            conversation.push_assistant(answer, Vec::new(), reply.raw.filter(|_| !text.is_empty()));
            run.outcome.text = answer.to_string();
            return Ok(run.end(Stop::Finished, if text.is_empty() { answer } else { "" }, false));
        }

        // Every call gets an answer, run or not: a history with an
        // unanswered call is refused by every provider.
        let mut results = Vec::new();
        let mut stuck_on = None;
        for call in &reply.tool_calls {
            let output = if cut_off {
                not_run(call, "Not run: your reply was cut off at the output limit, so this call may be incomplete. Make it again, with shorter arguments if it was long.")
            } else if cancel.is_cancelled() {
                not_run(call, "Not run: the player stopped the request.")
            } else {
                run.call_tool(&toolbox, world, conversation, call)
            };
            let mut text = output;
            let signature = format!("{} {}", call.name, call.arguments);
            if text.starts_with("Error") {
                repeats = if last_failure.as_deref() == Some(&signature) { repeats + 1 } else { 0 };
                if repeats >= 1 {
                    text.push_str("\nYou already made this exact call and it failed the same way. Change the arguments.");
                }
                if repeats >= MAX_REPEATS {
                    stuck_on = Some(call.name.clone());
                }
                last_failure = Some(signature);
            } else {
                last_failure = None;
            }
            results.push(ToolResult { call_id: call.id.clone(), is_error: text.starts_with("Error") || text.starts_with("Not run"), text });
        }
        conversation.push_assistant(reply.text.trim(), reply.tool_calls, reply.raw);
        conversation.push_results(results);
        if let Some(tool) = stuck_on {
            // Small models can lock onto one failing call. Stop rather than
            // spend the whole step budget on it.
            let text = format!("I got stuck on one step ({tool}) and stopped. What is built so far is in place; tell me how to go on.");
            conversation.push_assistant(&text, Vec::new(), None);
            return Ok(run.end(Stop::Stuck, &text, true));
        }
        if cancel.is_cancelled() {
            return Ok(run.stopped(conversation));
        }
        if !reply.text.trim().is_empty() {
            // Sets a remark made while working apart from what follows.
            (run.on_event)(Event::Text { text: "\n\n".to_string() });
        }
    }
    conversation.push_assistant(STEP_LIMIT, Vec::new(), None);
    Ok(run.end(Stop::StepLimit, STEP_LIMIT, true))
}

const STEP_LIMIT: &str = "I reached the step limit for one request. Tell me to continue and I will pick up where I left off.";
const STOPPED: &str = "Stopped. What was built so far is in place.";
const REFUSED: &str = "I can't help with that request. Tell me what else you would like built.";
const CUT_OFF_TOO_OFTEN: &str = "My replies keep getting cut off before they finish, so I stopped. What is built so far is in place; try asking for a smaller step.";

/// The bookkeeping of one request.
struct Run<'a> {
    outcome: Outcome,
    on_event: &'a mut (dyn FnMut(Event) + Send),
}

impl Run<'_> {
    /// Ends the request. `text` is said by the loop itself rather than the
    /// model; with `say` it is sent to the player as text.
    fn end(mut self, stop: Stop, text: &str, say: bool) -> Outcome {
        if say && !text.is_empty() {
            (self.on_event)(Event::Text { text: text.to_string() });
        }
        if !text.is_empty() {
            self.outcome.text = text.to_string();
        }
        self.outcome.stop = stop;
        (self.on_event)(Event::Done { stop });
        self.outcome
    }

    /// Ends a request the player stopped, leaving the conversation ending
    /// on an assistant turn like any other.
    fn stopped(self, conversation: &mut Conversation) -> Outcome {
        conversation.push_assistant(STOPPED, Vec::new(), None);
        self.end(Stop::Cancelled, STOPPED, true)
    }

    /// Runs one tool call and tells the player's screen about it.
    fn call_tool(&mut self, toolbox: &Toolbox, world: &dyn World, conversation: &mut Conversation, call: &ToolCall) -> String {
        (self.on_event)(Event::ToolStarted { name: call.name.clone(), summary: tools::summary(&call.name, &call.arguments) });
        let output = match &call.malformed {
            // The model's own text goes back to it, so it can see its mistake.
            Some(text) => tools::ToolOutput { text: format!("Error: the arguments were not a valid JSON object. You sent: {}\nCall {} again with the arguments as one JSON object.", clip(text, 600), call.name), ok: false, changed: 0 },
            None => toolbox.call(world, conversation, &call.name, &call.arguments),
        };
        self.outcome.tool_calls += 1;
        self.outcome.blocks_changed += output.changed;
        let first_line = output.text.lines().next().unwrap_or("");
        (self.on_event)(Event::ToolFinished { name: call.name.clone(), ok: output.ok, summary: clip(first_line, 200), changed: output.changed });
        output.text
    }
}

fn not_run(_call: &ToolCall, why: &str) -> String {
    why.to_string()
}

/// Whether a text states an intent rather than a result or a question to
/// the player.
pub fn announces_work_without_doing_it(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.ends_with('?') {
        return false;
    }
    let lower = text.to_lowercase();
    let padded = format!(" {} ", lower.replace(|c: char| !c.is_alphanumeric() && c != '\'', " "));
    ["i will", "i'll", "i am going to", "i'm going to", "let me", "i am now going", "now i", "next i", "i shall"].iter().any(|phrase| padded.contains(&format!(" {phrase} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_of_intent_are_told_from_answers_and_questions() {
        assert!(announces_work_without_doing_it("I'll start with the hull, then add engines."));
        assert!(announces_work_without_doing_it("Great idea. Let me build that now."));
        assert!(announces_work_without_doing_it("Next, I will add the cannons."));
        assert!(!announces_work_without_doing_it("The frigate is built: 60 blocks long, grey hull, two cannons."));
        assert!(!announces_work_without_doing_it("How long should it be? I will build whatever you prefer?"));
        assert!(!announces_work_without_doing_it(""));
        assert!(!announces_work_without_doing_it("The hull will be red."));
    }

    #[test]
    fn events_have_a_stable_json_form() {
        let json = |event: Event| serde_json::to_string(&event).unwrap();
        assert_eq!(json(Event::Text { text: "Hi".into() }), r#"{"type":"text","text":"Hi"}"#);
        assert_eq!(json(Event::ToolFinished { name: "fill".into(), ok: true, summary: "Filled.".into(), changed: 12 }), r#"{"type":"tool_finished","name":"fill","ok":true,"summary":"Filled.","changed":12}"#);
        assert_eq!(json(Event::Done { stop: Stop::StepLimit }), r#"{"type":"done","stop":"step_limit"}"#);
        assert_eq!(json(Event::Failed { message: "No.".into() }), r#"{"type":"failed","message":"No."}"#);
    }

    #[tokio::test]
    async fn a_cancel_flag_wakes_whoever_waits_on_it() {
        let flag = CancelFlag::new();
        let waiting = tokio::spawn({
            let flag = flag.clone();
            async move { flag.cancelled().await }
        });
        tokio::task::yield_now().await;
        assert!(!flag.is_cancelled() && !waiting.is_finished());
        flag.cancel();
        waiting.await.unwrap();
        assert!(flag.is_cancelled());
        flag.cancelled().await;
    }
}
