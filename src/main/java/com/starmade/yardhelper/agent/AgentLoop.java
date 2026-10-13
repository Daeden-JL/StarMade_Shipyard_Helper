package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.node.ArrayNode;
import com.starmade.yardhelper.world.WorldUnavailableException;

import java.io.IOException;
import java.util.function.BooleanSupplier;
import java.util.regex.Pattern;

/** Runs one player request: ask the model, run the tools it calls, repeat until it answers. */
public final class AgentLoop {
    /** Progress sink; the game shows these to the player as chat lines. */
    public interface Listener {
        /** The model said something while still working. */
        void note(String text);

        /** A tool finished. */
        void tool(String name, String result);
    }

    private static final int MAX_NUDGES = 2;
    /** Identical failing calls in a row (after the first) before the request is ended. */
    private static final int MAX_REPEATS = 3;
    private static final Pattern INTENT = Pattern.compile(
            "\\b(I will|I'll|I am going to|I'm going to|Let me|I am now going|Now I|Next,? I|I shall)\\b",
            Pattern.CASE_INSENSITIVE);

    private final LlmClient llm;
    private final ToolRegistry tools;
    private final String systemPrompt;
    private final int maxSteps;
    private final int contextChars;
    private final LlmClient.Options options;
    private final boolean trace;

    public AgentLoop(LlmClient llm, ToolRegistry tools, String systemPrompt, int maxSteps, int contextChars,
                     LlmClient.Options options, boolean trace) {
        this.llm = llm;
        this.tools = tools;
        this.systemPrompt = systemPrompt;
        this.maxSteps = maxSteps;
        this.contextChars = contextChars;
        this.options = options;
        this.trace = trace;
    }

    /** @return the model's final reply to the player */
    public String run(Conversation conversation, String userText, ToolContext ctx, Listener listener,
                      BooleanSupplier cancelled) throws IOException, WorldUnavailableException {
        conversation.addUser(userText);
        ArrayNode definitions = tools.definitions();
        int nudges = 0;
        String lastCall = null;
        int repeats = 0;
        for (int step = 0; step < maxSteps; step++) {
            if (cancelled.getAsBoolean()) return "Stopped.";
            long started = System.currentTimeMillis();
            LlmClient.Reply reply = llm.chat(conversation.forRequest(systemPrompt, contextChars), definitions, options);
            if (trace) {
                trace("step " + step + " took " + (System.currentTimeMillis() - started) + " ms, "
                        + reply.toolCalls().size() + " tool calls" + (reply.truncated() ? ", TRUNCATED" : "")
                        + (reply.content().isBlank() ? "" : ", says: " + reply.content()));
            }
            conversation.addAssistant(reply.message());
            if (reply.toolCalls().isEmpty()) {
                if (reply.truncated()) {
                    // The model ran out of tokens thinking. Nudge it to act instead of ending
                    // the request with half a thought.
                    conversation.addUser("(system) Your last reply was cut off before it finished. "
                            + "Think less and continue: call the next tool, or give your short final answer.");
                    continue;
                }
                if (nudges < MAX_NUDGES && announcesWorkWithoutDoingIt(reply.content())) {
                    // Models often stop to say "I will now build X" and then never do.
                    nudges++;
                    listener.note(reply.content());
                    conversation.addUser("(system) You described what you will do but did not call any tool. "
                            + "Do it now with tool calls, and only reply in text once the work is finished.");
                    continue;
                }
                return reply.content().isBlank() ? "Done." : reply.content();
            }
            if (!reply.content().isBlank()) listener.note(reply.content());
            for (LlmClient.ToolCall call : reply.toolCalls()) {
                String signature = call.name() + " " + call.arguments();
                repeats = signature.equals(lastCall) ? repeats + 1 : 0;
                lastCall = signature;
                // Every call needs an answer or the next request is malformed, so after a
                // cancel the remaining calls are answered without being run.
                String result = cancelled.getAsBoolean()
                        ? "Not run: the player stopped the request."
                        : tools.run(call, ctx);
                if (repeats >= MAX_REPEATS && result.startsWith("Error")) {
                    // Small models can lock onto one failing call. Stop rather than burn
                    // the whole step budget on it.
                    conversation.addToolResult(call.id(), result);
                    if (trace) trace("  giving up: " + signature + " failed " + (repeats + 1) + " times");
                    return "I got stuck on one step (" + call.name() + ") and stopped. What is built so far is in place; "
                            + "tell me what to do next or use /yard undo.";
                }
                if (repeats >= 1 && result.startsWith("Error")) {
                    result += "\nYou already made this exact call and it failed the same way. Change the arguments.";
                }
                if (trace) trace("  " + call.name() + " " + call.arguments() + "\n    -> " + abbreviate(result));
                conversation.addToolResult(call.id(), result);
                listener.tool(call.name(), result);
            }
        }
        return "I reached the step limit for one request. Tell me to continue and I'll pick up where I left off.";
    }

    /** A statement of intent rather than a result or a question to the player. */
    static boolean announcesWorkWithoutDoingIt(String text) {
        String t = text.trim();
        if (t.isEmpty() || t.endsWith("?")) return false;
        return INTENT.matcher(t).find();
    }

    private static void trace(String line) {
        System.err.println("[yard trace] " + line);
    }

    private static String abbreviate(String s) {
        String flat = s.replace("\n", " | ");
        return flat.length() > 300 ? flat.substring(0, 300) + "..." : flat;
    }
}
