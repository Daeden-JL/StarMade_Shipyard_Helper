package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.starmade.yardhelper.shape.Shapes;
import com.starmade.yardhelper.world.WorldUnavailableException;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** The set of tools offered to the model, and the dispatcher that runs its calls. */
public final class ToolRegistry {
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final int MAX_RESULT_CHARS = 12000;

    private final Map<String, Tool> tools = new LinkedHashMap<>();

    public static ToolRegistry standard() {
        ToolRegistry r = new ToolRegistry();
        List<Tool> all = new ArrayList<>();
        all.addAll(InspectTools.all());
        all.addAll(BuildTools.all());
        all.forEach(t -> r.tools.put(t.name(), t));
        return r;
    }

    /** Tool definitions in OpenAI function-calling format. */
    public ArrayNode definitions() {
        ArrayNode out = JSON.createArrayNode();
        for (Tool t : tools.values()) {
            ObjectNode def = out.addObject();
            def.put("type", "function");
            ObjectNode fn = def.putObject("function");
            fn.put("name", t.name());
            fn.put("description", t.description());
            fn.set("parameters", t.parameters());
        }
        return out;
    }

    public Map<String, Tool> tools() {
        return tools;
    }

    /**
     * Runs one call. Mistakes by the model come back as "Error: ..." text so it can retry;
     * only a design that has gone away propagates as an exception.
     */
    public String run(LlmClient.ToolCall call, ToolContext ctx) throws WorldUnavailableException {
        Tool tool = tools.get(call.name());
        if (tool == null) {
            return "Error: there is no tool named \"" + call.name() + "\". Available: " + String.join(", ", tools.keySet());
        }
        JsonNode args;
        try {
            args = JSON.readTree(call.arguments() == null || call.arguments().isBlank() ? "{}" : call.arguments());
        } catch (Exception e) {
            return "Error: arguments were not valid JSON.";
        }
        try {
            String result = tool.handler().run(new Args(args), ctx);
            return result.length() > MAX_RESULT_CHARS
                    ? result.substring(0, MAX_RESULT_CHARS) + "\n[output cut off; ask for less at once]"
                    : result;
        } catch (ToolException | Shapes.TooLargeException e) {
            return "Error: " + e.getMessage();
        } catch (WorldUnavailableException e) {
            throw e;
        } catch (RuntimeException e) {
            System.err.println("[Shipyard Helper] Tool " + call.name() + " failed: " + e);
            e.printStackTrace();
            return "Error: internal failure in " + call.name() + " (" + e.getClass().getSimpleName() + ").";
        }
    }
}
