package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.ArrayList;
import java.util.List;

/** Chat history for one session, kept within a size the local model can handle. */
public final class Conversation {
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final String TRIMMED = "[older tool output removed to save space]";

    private final List<ObjectNode> messages = new ArrayList<>();

    public void addUser(String text) {
        messages.add(message("user", text));
    }

    public void addAssistant(ObjectNode assistantMessage) {
        messages.add(assistantMessage);
    }

    public void addToolResult(String callId, String result) {
        ObjectNode m = message("tool", result);
        m.put("tool_call_id", callId);
        messages.add(m);
    }

    public void clear() {
        messages.clear();
    }

    public int size() {
        return messages.size();
    }

    /** The messages to send: system prompt first, history compacted to fit. */
    public List<ObjectNode> forRequest(String systemPrompt, int maxChars) {
        compact(maxChars);
        List<ObjectNode> out = new ArrayList<>(messages.size() + 1);
        out.add(message("system", systemPrompt));
        out.addAll(messages);
        return out;
    }

    /**
     * Shrinks history until it fits: first blank out old tool outputs (the model has
     * already acted on them), then drop whole oldest turns. The current turn is kept.
     */
    void compact(int maxChars) {
        if (chars() <= maxChars) return;
        int lastUser = lastUserIndex();
        for (int i = 0; i < lastUser && chars() > maxChars; i++) {
            ObjectNode m = messages.get(i);
            if ("tool".equals(m.path("role").asText()) && m.path("content").asText().length() > TRIMMED.length()) {
                m.put("content", TRIMMED);
            }
        }
        while (chars() > maxChars) {
            int secondUser = nextUserIndex(1);
            if (secondUser < 0) break; // only the current turn is left
            messages.subList(0, secondUser).clear();
        }
    }

    private int chars() {
        int n = 0;
        for (ObjectNode m : messages) n += m.toString().length();
        return n;
    }

    private int lastUserIndex() {
        for (int i = messages.size() - 1; i >= 0; i--) {
            if ("user".equals(messages.get(i).path("role").asText())) return i;
        }
        return 0;
    }

    private int nextUserIndex(int from) {
        for (int i = from; i < messages.size(); i++) {
            if ("user".equals(messages.get(i).path("role").asText())) return i;
        }
        return -1;
    }

    private static ObjectNode message(String role, String content) {
        ObjectNode m = JSON.createObjectNode();
        m.put("role", role);
        m.put("content", content);
        return m;
    }
}
