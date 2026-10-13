//! The agent loop driven by a scripted model against an in-memory design:
//! building, linking, errors fed back, limits, stopping and undoing.

use serde_json::{json, Value};
use std::sync::Arc;
use yard_agent::{
    run, sample_catalog, AgentError, CancelFlag, Conversation, Event, MemoryWorld, Outcome, ProviderError, Raw, Reply, RunOptions, Script, ScriptedProvider, Stop, StopReason, ToolCall, Turn, Usage, World,
};
use yard_core::Pos;

fn ship() -> MemoryWorld {
    MemoryWorld::new_ship(Arc::new(sample_catalog()), "Test Ship")
}

/// Runs one message and collects the events.
async fn drive(provider: &ScriptedProvider, world: &MemoryWorld, conversation: &mut Conversation, message: &str, options: &RunOptions) -> (Result<Outcome, AgentError>, Vec<Event>) {
    let mut events = Vec::new();
    let outcome = run(provider, world, conversation, message, options, &CancelFlag::new(), &mut |event| events.push(event)).await;
    (outcome, events)
}

/// Everything the player was shown as text.
fn shown(events: &[Event]) -> String {
    events.iter().filter_map(|e| if let Event::Text { text } = e { Some(text.as_str()) } else { None }).collect()
}

/// The tool results of a conversation, in order.
fn results(turns: &[Turn]) -> Vec<(String, bool)> {
    turns.iter().filter_map(|t| if let Turn::ToolResults { results } = t { Some(results) } else { None }).flatten().map(|r| (r.text.clone(), r.is_error)).collect()
}

/// Every tool call is answered by the turn after it, call for call.
fn assert_well_formed(turns: &[Turn]) {
    for (i, turn) in turns.iter().enumerate() {
        match turn {
            Turn::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                let Some(Turn::ToolResults { results }) = turns.get(i + 1) else { panic!("the calls of turn {i} have no results") };
                assert_eq!(tool_calls.iter().map(|c| &c.id).collect::<Vec<_>>(), results.iter().map(|r| &r.call_id).collect::<Vec<_>>());
            }
            Turn::ToolResults { .. } => assert!(matches!(&turns[i - 1], Turn::Assistant { tool_calls, .. } if !tool_calls.is_empty())),
            _ => {}
        }
    }
}

fn hull_call() -> (&'static str, Value) {
    let sections = json!([{"z": -8, "half_width": 2, "half_height": 1}, {"z": 0, "half_width": 4, "half_height": 2}, {"z": 10, "half_width": 0, "half_height": 0}]);
    ("hull", json!({"block": "Red Basic Armor", "sections": sections}))
}

#[tokio::test]
async fn a_request_runs_through_inspecting_building_linking_and_answering() {
    let world = ship();
    let provider = ScriptedProvider::new(vec![
        Script::calls(&[("design_info", json!({}))]),
        Script::calls_saying("Laying the hull.", &[hull_call(), ("place_system", json!({"computer": "Cannon Computer", "at": [1, 0, 0], "module": "Cannon Barrel", "from": [1, 0, 1], "to": [1, 0, 5]}))]),
        Script::says("A red fighter hull, 19 long, with one cannon."),
    ]);
    let mut conversation = Conversation::new();
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Build a small red fighter with a cannon.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();

    assert_eq!((outcome.stop, outcome.steps, outcome.tool_calls), (Stop::Finished, 3, 3));
    assert_eq!(outcome.text, "A red fighter hull, 19 long, with one cannon.");
    assert_eq!(outcome.usage, Usage { input: 300, output: 30, cache_read: 0, cache_write: 0 });
    // The design: a red shell, the cannon inside it, linked.
    let info = &world.entities()[0];
    assert_eq!(info.bounds, Some((Pos::new(-4, -2, -8), Pos::new(4, 2, 10))));
    assert_eq!(outcome.blocks_changed as u64, info.blocks - 1);
    let computer = Pos::new(1, 0, 0);
    assert_eq!((world.block("", computer).ty(), world.block("", Pos::new(1, 0, 5)).ty(), world.block("", Pos::new(0, 0, 10)).ty()), (6, 16, 620));
    world.with_blueprint(|b| {
        let links = b.root.entity.links();
        assert!(yard_core::links::is_linked(links, computer, Pos::new(1, 0, 3)) && yard_core::links::is_linked(links, Pos::new(0, 0, 0), computer));
    });

    // What the player's screen was told, in order.
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::Step { .. } => "step",
            Event::Text { .. } => "text",
            Event::ToolStarted { .. } => "started",
            Event::ToolFinished { .. } => "finished",
            Event::Usage { .. } => "usage",
            Event::Done { .. } => "done",
            Event::Failed { .. } => "failed",
        })
        .collect();
    assert_eq!(
        kinds,
        ["step", "usage", "started", "finished", "step", "text", "text", "usage", "started", "finished", "started", "finished", "text", "step", "text", "text", "usage", "done"],
        "{events:?}"
    );
    assert_eq!(shown(&events), "Laying the hull.\n\nA red fighter hull, 19 long, with one cannon.");
    assert!(events.contains(&Event::ToolStarted { name: "place_system".into(), summary: "place_system: computer Cannon Computer, module Cannon Barrel, from (1, 0, 1), to (1, 0, 5)".into() }));
    assert!(events.iter().any(|e| matches!(e, Event::ToolFinished { name, ok: true, changed: 6, .. } if name == "place_system")));
    assert_eq!(events.last(), Some(&Event::Done { stop: Stop::Finished }));

    // The conversation: the player, three assistant turns, two rounds of results.
    assert_eq!(conversation.turns().len(), 6);
    assert_well_formed(conversation.turns());
    // Each model call saw everything before it.
    let asked = provider.asked();
    assert_eq!(asked.iter().map(Vec::len).collect::<Vec<_>>(), [1, 3, 5]);
    assert!(results(&asked[1])[0].0.contains("The design has 1 entity."));
}

#[tokio::test]
async fn tool_errors_go_back_to_the_model_which_corrects_itself() {
    let world = ship();
    let provider = ScriptedProvider::new(vec![
        // An unknown block, a missing argument, and arguments that were not JSON.
        Script::calls(&[("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Hull Plating"})), ("fill", json!({"from": [0, 1, 0], "block": "Grey Basic Armor"}))]),
        Script::Reply(Reply {
            text: String::new(),
            tool_calls: vec![ToolCall { id: "call_x".into(), name: "fill".into(), arguments: json!({}), malformed: Some("{\"from\": [0, 1, 0], \"to\": [0, 1, 4] \"block\"".into()) }],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
            raw: None,
        }),
        Script::calls(&[("find_blocks", json!({"query": "grey armor"}))]),
        Script::calls(&[("fill", json!({"from": "[0, 1, 0]", "to": {"x": 0, "y": 1, "z": 4}, "block": "Grey Basic Armor"}))]),
        Script::says("Placed a row of five grey blocks."),
    ]);
    let mut conversation = Conversation::new();
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Put five grey blocks above the core.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);
    assert_eq!(world.entities()[0].blocks, 6);

    let results = results(conversation.turns());
    assert!(results[0].1 && results[0].0.starts_with("Error: There is no block called \"Grey Hull Plating\"."), "{}", results[0].0);
    assert!(results[1].1 && results[1].0.starts_with("Error: Missing argument \"to\"."), "{}", results[1].0);
    assert!(results[2].1 && results[2].0.contains("were not a valid JSON object. You sent: {\"from\": [0, 1, 0], \"to\": [0, 1, 4] \"block\""), "{}", results[2].0);
    assert!(!results[3].1 && results[3].0.contains("Grey Basic Armor (id 598)"));
    assert!(!results[4].1 && results[4].0.contains("5 blocks placed"), "arguments right in content and wrong in form are accepted: {}", results[4].0);
    // The model saw each error on its next call.
    assert!(provider.asked()[1].iter().any(|t| matches!(t, Turn::ToolResults { results } if results.len() == 2 && results.iter().all(|r| r.is_error))));
    let failed = events.iter().filter(|e| matches!(e, Event::ToolFinished { ok: false, changed: 0, .. })).count();
    assert_eq!(failed, 3);
    assert_well_formed(conversation.turns());
}

#[tokio::test]
async fn the_step_limit_ends_a_request_that_can_be_continued() {
    let world = ship();
    let provider = ScriptedProvider::repeating(vec![Script::calls(&[("count_blocks", json!({}))])]);
    let mut conversation = Conversation::new();
    let options = RunOptions { max_steps: 5, ..RunOptions::default() };
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Count forever.", &options).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps, outcome.tool_calls), (Stop::StepLimit, 5, 5));
    assert!(outcome.text.contains("step limit") && shown(&events).ends_with(&outcome.text));
    assert_eq!(events.last(), Some(&Event::Done { stop: Stop::StepLimit }));
    assert!(matches!(conversation.turns().last(), Some(Turn::Assistant { text, .. }) if text.contains("step limit")));
    assert_well_formed(conversation.turns());

    // "Continue" starts a new request on the same conversation.
    let next = ScriptedProvider::new(vec![Script::says("That is all of them.")]);
    let (outcome, _) = drive(&next, &world, &mut conversation, "continue", &options).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);
    assert_eq!(next.asked()[0].len(), 13, "the player, five rounds of call and result, the note, and the new message");
}

#[tokio::test]
async fn a_model_stuck_on_one_failing_call_is_stopped() {
    let world = ship();
    let provider = ScriptedProvider::repeating(vec![Script::calls(&[("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Unobtainium"}))])]);
    let mut conversation = Conversation::new();
    let (outcome, _) = drive(&provider, &world, &mut conversation, "Build with unobtainium.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Stuck, 4));
    assert!(outcome.text.contains("I got stuck on one step (fill)"));
    let results = results(conversation.turns());
    assert!(!results[0].0.contains("already made this exact call"));
    assert!(results[1].0.ends_with("You already made this exact call and it failed the same way. Change the arguments."));
    assert_well_formed(conversation.turns());
}

#[tokio::test]
async fn the_player_can_stop_a_model_call_in_flight() {
    let world = ship();
    let provider = ScriptedProvider::new(vec![Script::calls(&[("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}))]), Script::Hang]);
    let mut conversation = Conversation::new();
    let cancel = CancelFlag::new();
    let mut events = Vec::new();
    let stopper = cancel.clone();
    let outcome = run(&provider, &world, &mut conversation, "Build.", &RunOptions::default(), &cancel, &mut |event| {
        // The second model call never answers; stop once it has begun.
        if event == (Event::Step { step: 2 }) {
            let stopper = stopper.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                stopper.cancel();
            });
        }
        events.push(event);
    })
    .await
    .unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Cancelled, 2));
    assert_eq!(world.entities()[0].blocks, 6, "what was built before the stop stays");
    assert_eq!(events.last(), Some(&Event::Done { stop: Stop::Cancelled }));
    assert!(matches!(conversation.turns().last(), Some(Turn::Assistant { text, .. }) if text.starts_with("Stopped.")));
    assert_well_formed(conversation.turns());
}

#[tokio::test]
async fn stopping_between_tool_calls_answers_the_rest_without_running_them() {
    let world = ship();
    let fill = |y: i32| ("fill", json!({"from": [0, y, 0], "to": [0, y, 4], "block": "Grey Basic Armor"}));
    let provider = ScriptedProvider::new(vec![Script::calls(&[fill(1), fill(2), fill(3)])]);
    let mut conversation = Conversation::new();
    let cancel = CancelFlag::new();
    let stopper = cancel.clone();
    let outcome = run(&provider, &world, &mut conversation, "Build three rows.", &RunOptions::default(), &cancel, &mut |event| {
        if matches!(event, Event::ToolFinished { .. }) {
            stopper.cancel();
        }
    })
    .await
    .unwrap();
    assert_eq!((outcome.stop, outcome.tool_calls, world.entities()[0].blocks), (Stop::Cancelled, 1, 6));
    let results = results(conversation.turns());
    assert_eq!(results.len(), 3);
    assert!(results[1].1 && results[1].0 == "Not run: the player stopped the request." && results[2].0 == results[1].0);
    assert_well_formed(conversation.turns());
    // A flag that is already set stops a request before it asks the model.
    let (outcome, _) = {
        let mut events = Vec::new();
        let outcome = run(&provider, &world, &mut conversation, "More.", &RunOptions::default(), &cancel, &mut |e| events.push(e)).await;
        (outcome, events)
    };
    assert_eq!(outcome.unwrap().steps, 0);
}

#[tokio::test]
async fn the_agent_undoes_its_own_step_also_in_a_later_request() {
    let world = ship();
    let mut conversation = Conversation::new();
    let build = ScriptedProvider::new(vec![Script::calls(&[hull_call()]), Script::says("Built.")]);
    drive(&build, &world, &mut conversation, "Build a hull.", &RunOptions::default()).await.0.unwrap();
    assert!(world.entities()[0].blocks > 100);

    // The conversation is stored and loaded between requests.
    let mut conversation: Conversation = serde_json::from_str(&serde_json::to_string(&conversation).unwrap()).unwrap();
    let undo = ScriptedProvider::new(vec![Script::calls(&[("undo", json!({}))]), Script::calls(&[("undo", json!({}))]), Script::says("Taken back.")]);
    let (outcome, _) = drive(&undo, &world, &mut conversation, "Undo that.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);
    assert_eq!(world.entities()[0].blocks, 1);
    let results = results(conversation.turns());
    assert!(results[1].0.starts_with("Took back a hull."), "{}", results[1].0);
    assert!(results[2].1 && results[2].0.contains("made no change"), "a second undo has nothing of the agent's left: {}", results[2].0);
}

#[tokio::test]
async fn a_failing_provider_fails_the_request_and_leaves_the_conversation_usable() {
    let world = ship();
    let mut conversation = Conversation::new();
    let broken = ScriptedProvider::new(vec![Script::calls(&[("design_info", json!({}))]), Script::Fail(ProviderError::Overloaded("Claude is overloaded right now. Try again in a moment.".into()))]);
    let (outcome, events) = drive(&broken, &world, &mut conversation, "Build a hull.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap_err().to_string(), "Claude is overloaded right now. Try again in a moment.");
    assert_eq!(events.last(), Some(&Event::Failed { message: "Claude is overloaded right now. Try again in a moment.".into() }));
    assert_well_formed(conversation.turns());

    let working = ScriptedProvider::new(vec![Script::says("Here I am again.")]);
    let (outcome, _) = drive(&working, &world, &mut conversation, "Try again.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);
    assert_eq!(working.asked()[0].len(), 4, "the first message, the call, its result and the new message");
}

#[tokio::test]
async fn sexual_requests_never_reach_the_model_and_a_models_refusal_is_passed_on() {
    let world = ship();
    let mut conversation = Conversation::new();
    let provider = ScriptedProvider::new(vec![Script::Reply(Reply { text: String::new(), tool_calls: Vec::new(), stop: StopReason::Refusal, usage: Usage::default(), raw: None })]);
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Build a giant penis.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Refused, 0));
    assert!(provider.asked().is_empty() && conversation.is_empty());
    assert_eq!(shown(&events), yard_agent::filter::REFUSAL);

    // The provider's safety system declines a request the screen let through.
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Build something the provider will not.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Refused, 1));
    assert!(shown(&events).contains("can't help with that request"));
    assert!(conversation.is_empty(), "a request refused outright leaves no trace for the next one to be judged by");
    assert_eq!(events.last(), Some(&Event::Done { stop: Stop::Refused }));

    // With the screen switched off the message goes to the model.
    let open = ScriptedProvider::new(vec![Script::says("No.")]);
    let options = RunOptions { content_filter: false, ..RunOptions::default() };
    drive(&open, &world, &mut conversation, "Build a giant penis.", &options).await.0.unwrap();
    assert_eq!(open.asked().len(), 1);
}

#[tokio::test]
async fn a_model_that_only_announces_work_is_told_to_do_it() {
    let world = ship();
    let provider = ScriptedProvider::new(vec![
        Script::says("I'll build the hull now."),
        Script::calls(&[("fill", json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}))]),
        Script::says("The row is in place."),
    ]);
    let mut conversation = Conversation::new();
    let (outcome, events) = drive(&provider, &world, &mut conversation, "Build a row.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps, outcome.text.as_str()), (Stop::Finished, 3, "The row is in place."));
    assert!(matches!(&conversation.turns()[2], Turn::User { text } if text.starts_with("(system) You described what you will do")));
    assert_eq!(shown(&events), "I'll build the hull now.\n\nThe row is in place.");
    // A question to the player is an answer, not an announcement.
    let asking = ScriptedProvider::new(vec![Script::says("I'll need a size. How long should it be?")]);
    let (outcome, _) = drive(&asking, &world, &mut Conversation::new(), "Build a ship.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().steps, 1);
}

#[tokio::test]
async fn a_reply_cut_off_at_the_output_limit_runs_no_tools() {
    let world = ship();
    let cut = |text: &str, calls: Vec<ToolCall>| Script::Reply(Reply { text: text.into(), tool_calls: calls, stop: StopReason::MaxTokens, usage: Usage::default(), raw: None });
    let call = ToolCall { id: "c1".into(), name: "fill".into(), arguments: json!({"from": [0, 1, 0], "to": [0, 1, 4], "block": "Grey Basic Armor"}), malformed: None };
    let provider = ScriptedProvider::new(vec![cut("", vec![call.clone()]), cut("Thinking about the hull, I", vec![]), Script::says("Could not finish in one go.")]);
    let mut conversation = Conversation::new();
    let (outcome, _) = drive(&provider, &world, &mut conversation, "Build a row.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);
    assert_eq!(world.entities()[0].blocks, 1, "a call that may have been cut short is not run");
    let results = results(conversation.turns());
    assert!(results[0].1 && results[0].0.starts_with("Not run: your reply was cut off"));
    assert!(conversation.turns().iter().any(|t| matches!(t, Turn::User { text } if text.starts_with("(system) Your last reply was cut off"))));
    assert_well_formed(conversation.turns());

    // Cut off again and again: give up instead of looping.
    let looping = ScriptedProvider::repeating(vec![cut("", vec![call])]);
    let (outcome, _) = drive(&looping, &world, &mut Conversation::new(), "Build a row.", &RunOptions::default()).await;
    let outcome = outcome.unwrap();
    assert_eq!((outcome.stop, outcome.steps), (Stop::Stuck, 3));
}

#[tokio::test]
async fn thinking_a_provider_refuses_is_dropped_once_and_the_call_made_again() {
    let world = ship();
    let mut conversation = Conversation::new();
    conversation.push_user("Hello.");
    let thinking = Raw { provider: "claude".into(), model: "claude-opus-5-5".into(), content: json!([{"type": "thinking", "thinking": "", "signature": "c2ln"}, {"type": "text", "text": "Hello."}]) };
    conversation.push_assistant("Hello.", vec![], Some(thinking));
    let rejected = || Script::Fail(ProviderError::ThinkingRejected("Claude refused the request: Invalid `signature` in `thinking` block.".into()));
    let provider = ScriptedProvider::new(vec![rejected(), Script::says("Built.")]);
    let (outcome, _) = drive(&provider, &world, &mut conversation, "Build.", &RunOptions::default()).await;
    assert_eq!(outcome.unwrap().steps, 1);
    let asked = provider.asked();
    assert!(matches!(&asked[0][1], Turn::Assistant { raw: Some(_), .. }) && matches!(&asked[1][1], Turn::Assistant { raw: None, text, .. } if text == "Hello."));
    // Refused a second time, it is a failure like any other.
    let stubborn = ScriptedProvider::new(vec![rejected(), rejected()]);
    assert!(matches!(drive(&stubborn, &world, &mut conversation, "Build.", &RunOptions::default()).await.0, Err(AgentError::Provider(ProviderError::ThinkingRejected(_)))));
}

#[tokio::test]
async fn a_long_request_stays_within_its_history_budget() {
    let world = ship();
    let mut script: Vec<Script> = (0..12).map(|_| Script::calls(&[("read_guide", json!({"topic": "weapons"}))])).collect();
    script.push(Script::says("Read."));
    let provider = ScriptedProvider::new(script);
    let mut conversation = Conversation::new();
    let options = RunOptions { history_budget: Some(6000), ..RunOptions::default() };
    let (outcome, _) = drive(&provider, &world, &mut conversation, "Read the weapons guide twelve times.", &options).await;
    assert_eq!(outcome.unwrap().steps, 13);
    for turns in provider.asked() {
        assert_well_formed(&turns);
        let size: usize = turns.iter().map(|t| serde_json::to_string(t).unwrap().len()).sum();
        assert!(size < 12_000, "a request carried {size} characters of history");
        assert!(matches!(&turns[0], Turn::User { text } if text.starts_with("Read the weapons guide")), "the request itself is never dropped");
    }
    let trimmed = results(conversation.turns()).iter().filter(|(text, _)| text == yard_agent::message::TRIMMED).count();
    assert!(trimmed >= 6, "{trimmed} old outputs were dropped");
}

#[tokio::test]
async fn tools_work_on_a_docked_entity_when_given_its_path() {
    let world = ship();
    world.with_blueprint_mut(|blueprint| {
        let mut turret = yard_core::Blueprint::new_ship("turret", yard_core::blueprint::FALLBACK_GAME_VERSION).root;
        turret.dir = "ATTACHED_0".to_string();
        blueprint.root.children.push(turret);
    });
    let provider = ScriptedProvider::new(vec![
        Script::calls(&[("fill", json!({"from": [0, 1, 0], "to": [0, 3, 0], "block": "Grey Basic Armor", "entity": "ATTACHED_0"})), ("design_info", json!({"entity": "ATTACHED_0"}))]),
        Script::says("The turret has a mast."),
    ]);
    let mut conversation = Conversation::new();
    drive(&provider, &world, &mut conversation, "Give the turret a mast.", &RunOptions::default()).await.0.unwrap();
    let entities = world.entities();
    assert_eq!((entities[0].blocks, entities[1].blocks, entities[1].docked_to.as_deref()), (1, 4, Some("")));
    let info = &results(conversation.turns())[1].0;
    assert!(info.contains("- \"ATTACHED_0\" (docked to \"\", ship): 4 blocks") && info.contains("Blocks of \"ATTACHED_0\" by type:"), "{info}");
}

#[tokio::test]
async fn tools_work_on_the_entity_the_player_has_open_unless_told_otherwise() {
    let world = ship();
    world.with_blueprint_mut(|bp| {
        let mut turret = yard_core::Blueprint::new_ship("Turret", "0.304.7").root;
        turret.dir = "ATTACHED_0".to_string();
        bp.root.children.push(turret);
    });
    let provider = ScriptedProvider::new(vec![
        Script::calls(&[
            ("fill", json!({"from": [1, 0, 0], "to": [3, 0, 0], "block": "Red Basic Armor"})),
            ("fill", json!({"from": [0, 2, 0], "to": [0, 2, 0], "block": "Red Basic Armor", "entity": ""})),
        ]),
        Script::says("Done."),
    ]);
    let mut conversation = Conversation::new();
    let options = RunOptions { focus: "ATTACHED_0".to_string(), ..RunOptions::default() };
    let (outcome, _) = drive(&provider, &world, &mut conversation, "Add armour.", &options).await;
    assert_eq!(outcome.unwrap().stop, Stop::Finished);

    let blocks = |path: &str| world.entities().into_iter().find(|e| e.path == path).unwrap().blocks;
    assert_eq!((blocks("ATTACHED_0"), blocks("")), (4, 2), "the fill without an entity went into the open turret, the one naming \"\" into the main ship");
    let Some(Turn::User { text }) = conversation.turns().first() else { panic!("the conversation starts with the player's message") };
    assert!(text.starts_with("Add armour.") && text.contains("the docked entity \"ATTACHED_0\" open"), "{text}");
}
