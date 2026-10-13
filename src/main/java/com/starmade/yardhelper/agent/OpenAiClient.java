package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

/**
 * Client for any OpenAI-compatible /chat/completions endpoint (LM Studio, Ollama,
 * llama.cpp server, vLLM).
 */
public final class OpenAiClient implements LlmClient {
    private static final int KEEP_LOADED_SECONDS = 3600;

    private static final ObjectMapper JSON = new ObjectMapper();

    private final HttpClient http;
    private final URI endpoint;
    private final String model;
    private final String apiKey;
    private final double temperature;
    private final Duration timeout;

    public OpenAiClient(String baseUrl, String model, String apiKey, double temperature, int timeoutSeconds) {
        String base = baseUrl.endsWith("/") ? baseUrl.substring(0, baseUrl.length() - 1) : baseUrl;
        this.endpoint = URI.create(base + "/chat/completions");
        this.model = model;
        this.apiKey = apiKey == null ? "" : apiKey;
        this.temperature = temperature;
        this.timeout = Duration.ofSeconds(timeoutSeconds);
        // HTTP/1.1: local model servers generally don't speak h2c, and the upgrade attempt
        // confuses some of them.
        this.http = HttpClient.newBuilder()
                .version(HttpClient.Version.HTTP_1_1)
                .connectTimeout(Duration.ofSeconds(10))
                .build();
    }

    @Override
    public Reply chat(List<ObjectNode> messages, ArrayNode tools, Options options) throws IOException {
        ObjectNode body = JSON.createObjectNode();
        body.put("model", model);
        body.put("temperature", temperature);
        body.put("stream", false);
        if (options.maxTokens() > 0) body.put("max_tokens", options.maxTokens());
        if (!options.reasoningEffort().isEmpty()) body.put("reasoning_effort", options.reasoningEffort());
        // LM Studio keeps a model it loaded on demand for this long while idle, whatever its own
        // setting (which can unload it between two steps of one answer). Other servers ignore it.
        body.put("ttl", KEEP_LOADED_SECONDS);
        ArrayNode msgs = body.putArray("messages");
        messages.forEach(msgs::add);
        if (tools != null && !tools.isEmpty()) {
            body.set("tools", tools);
            body.put("tool_choice", "auto");
        }

        HttpRequest.Builder req = HttpRequest.newBuilder(endpoint)
                .timeout(timeout)
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(JSON.writeValueAsString(body), StandardCharsets.UTF_8));
        if (!apiKey.isEmpty()) req.header("Authorization", "Bearer " + apiKey);

        HttpResponse<String> resp;
        try {
            resp = http.send(req.build(), HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException("Interrupted while waiting for the model", e);
        }
        if (resp.statusCode() / 100 != 2) {
            throw new IOException("Model endpoint returned HTTP " + resp.statusCode() + ": " + abbreviate(resp.body(), 400));
        }

        JsonNode choice = JSON.readTree(resp.body()).path("choices").path(0);
        JsonNode message = choice.path("message");
        if (!message.isObject()) {
            throw new IOException("Model endpoint returned no message: " + abbreviate(resp.body(), 400));
        }

        String content = stripThinking(message.path("content").asText(""));
        List<ToolCall> calls = new ArrayList<>();
        ObjectNode clean = JSON.createObjectNode();
        clean.put("role", "assistant");
        clean.put("content", content);
        JsonNode rawCalls = message.path("tool_calls");
        if (rawCalls.isArray() && !rawCalls.isEmpty()) {
            ArrayNode outCalls = clean.putArray("tool_calls");
            int n = 0;
            for (JsonNode c : rawCalls) {
                JsonNode fn = c.path("function");
                String id = c.path("id").asText("");
                if (id.isEmpty()) id = "call_" + System.nanoTime() + "_" + n;
                // Some servers return arguments as an object rather than a JSON string.
                JsonNode rawArgs = fn.path("arguments");
                String args = rawArgs.isTextual() ? rawArgs.asText() : rawArgs.isMissingNode() ? "{}" : rawArgs.toString();
                String name = fn.path("name").asText("");
                calls.add(new ToolCall(id, name, args));
                ObjectNode oc = outCalls.addObject();
                oc.put("id", id);
                oc.put("type", "function");
                ObjectNode ofn = oc.putObject("function");
                ofn.put("name", name);
                ofn.put("arguments", args);
                n++;
            }
        }
        return new Reply(content, calls, clean, "length".equals(choice.path("finish_reason").asText()));
    }

    /** Reasoning models may wrap private thoughts in think tags; players shouldn't see them. */
    static String stripThinking(String content) {
        String out = content.replaceAll("(?s)<think>.*?</think>", "");
        int open = out.indexOf("<think>");
        if (open >= 0) out = out.substring(0, open);
        return out.trim();
    }

    private static String abbreviate(String s, int max) {
        return s.length() <= max ? s : s.substring(0, max) + "...";
    }
}
