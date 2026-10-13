package com.starmade.yardhelper;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.starmade.yardhelper.agent.LlmClient;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.List;

/** A stand-in model that replays a fixed script and records what it was sent. */
final class ScriptedLlm implements LlmClient {
    private static final ObjectMapper JSON = new ObjectMapper();

    private final Deque<Reply> script = new ArrayDeque<>();
    final List<List<ObjectNode>> requests = new ArrayList<>();
    private int nextId;

    ScriptedLlm call(String tool, String argsJson) {
        ToolCall call = new ToolCall("call_" + nextId++, tool, argsJson);
        ObjectNode msg = JSON.createObjectNode().put("role", "assistant").put("content", "");
        ObjectNode c = msg.putArray("tool_calls").addObject();
        c.put("id", call.id()).put("type", "function");
        c.putObject("function").put("name", tool).put("arguments", argsJson);
        script.add(new Reply("", List.of(call), msg, false));
        return this;
    }

    /** A reply that ran out of tokens before producing an answer or a tool call. */
    ScriptedLlm cutOff() {
        script.add(new Reply("", List.of(), JSON.createObjectNode().put("role", "assistant").put("content", ""), true));
        return this;
    }

    ScriptedLlm say(String text) {
        script.add(new Reply(text, List.of(), JSON.createObjectNode().put("role", "assistant").put("content", text), false));
        return this;
    }

    @Override
    public Reply chat(List<ObjectNode> messages, ArrayNode tools, Options options) {
        List<ObjectNode> copy = new ArrayList<>();
        messages.forEach(m -> copy.add(m.deepCopy()));
        requests.add(copy);
        if (script.isEmpty()) throw new IllegalStateException("script exhausted");
        return script.poll();
    }

    /** Content of the most recent tool result the model was shown. */
    String lastToolResult() {
        List<ObjectNode> last = requests.get(requests.size() - 1);
        for (int i = last.size() - 1; i >= 0; i--) {
            if ("tool".equals(last.get(i).path("role").asText())) return last.get(i).path("content").asText();
        }
        return null;
    }
}
