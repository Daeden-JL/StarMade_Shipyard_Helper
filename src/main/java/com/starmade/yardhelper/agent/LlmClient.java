package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.io.IOException;
import java.util.List;

/** A chat model that can call tools. */
public interface LlmClient {
    /**
     * @param messages full conversation in OpenAI chat format
     * @param tools    tool definitions, or null for a plain completion
     */
    Reply chat(List<ObjectNode> messages, ArrayNode tools, Options options) throws IOException;

    /**
     * @param maxTokens       cap on generated tokens (reasoning included), 0 for the server default
     * @param reasoningEffort "none", "low", "medium", "high", or empty for the server default
     */
    record Options(int maxTokens, String reasoningEffort) {
        /** A quick answer with no deliberation, for yes/no style calls. */
        public static final Options QUICK = new Options(64, "none");
    }

    record ToolCall(String id, String name, String arguments) {}

    /**
     * @param content   the assistant's text, never null
     * @param toolCalls tools the assistant wants run, possibly empty
     * @param message   the assistant message to append to the conversation as is
     * @param truncated the model hit its token cap before finishing
     */
    record Reply(String content, List<ToolCall> toolCalls, ObjectNode message, boolean truncated) {}
}
