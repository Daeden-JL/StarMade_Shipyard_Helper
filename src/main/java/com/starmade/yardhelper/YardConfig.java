package com.starmade.yardhelper;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/** Settings, read from config.properties in the mod's data directory. */
public final class YardConfig {
    /** Any OpenAI-compatible endpoint; LM Studio serves one on port 1234. */
    public String llmBaseUrl = "http://127.0.0.1:1234/v1";
    public String llmModel = "google/gemma-4-26b-a4b-qat";
    public String llmApiKey = "";
    public double llmTemperature = 0.2;
    public int llmTimeoutSeconds = 300;
    /**
     * Cap on tokens generated per model call, reasoning included. Bounds a rambling model,
     * and has to be reachable within llmTimeoutSeconds at the server's speed under load
     * (two concurrent requests ran at roughly 20 tokens a second in testing).
     */
    public int llmMaxTokens = 3000;
    /** none, low, medium or high; empty leaves it to the model server. */
    public String llmReasoningEffort = "";
    /** Requests sent to the model at once; further players wait in line. */
    public int llmMaxConcurrent = 2;

    public int agentMaxSteps = 40;
    public int agentMaxBlocksPerOperation = 200_000;
    public int agentContextChars = 60_000;
    public boolean agentShowToolProgress = true;
    /** Log every model step and tool call to the server log. */
    public boolean agentTrace = false;

    public int worldBlocksPerTick = 2000;
    /** processor (the game's own placement queue) or direct (write segments immediately). */
    public String worldWriteMode = "processor";
    /** Build area half-size around the core when the shipyard's own bounds are unknown. */
    public int worldBuildHalfExtent = 250;

    public int sessionIdleMinutes = 10;
    public int journalMaxSteps = 20;
    public boolean filterEnabled = true;

    /** Address of the web shipyard to keep the blueprint catalog in step with; empty turns the exchange off. */
    public String syncUrl = "";
    /** The secret the shipyard asks of its game server, if it has one set. Never logged. */
    public String syncToken = "";
    public int syncIntervalSeconds = 10;
    /** How this server is named in the shipyard. */
    public String syncServerName = "StarMade server";
    /** Catalog owner of blueprints that were made in the shipyard. */
    public String syncOwner = "Shipyard";
    /** Whether players other than that owner may buy blueprints made in the shipyard. */
    public boolean syncOthersCanBuy = true;
    /**
     * Which shipyards in the game designs may be linked to: "owner" for shipyards on stations
     * of the owner's faction, "all" for every shipyard, "off" for none.
     */
    public String syncShipyards = "owner";

    public static YardConfig load(Path file) {
        YardConfig c = new YardConfig();
        Properties p = new Properties();
        if (Files.exists(file)) {
            try (InputStream in = Files.newInputStream(file)) {
                p.load(in);
            } catch (IOException e) {
                System.err.println("[Shipyard Helper] Could not read " + file + ", using defaults: " + e.getMessage());
            }
        }
        c.llmBaseUrl = p.getProperty("llm.baseUrl", c.llmBaseUrl).trim();
        c.llmModel = p.getProperty("llm.model", c.llmModel).trim();
        c.llmApiKey = p.getProperty("llm.apiKey", c.llmApiKey).trim();
        c.llmTemperature = num(p, "llm.temperature", c.llmTemperature);
        c.llmTimeoutSeconds = (int) num(p, "llm.timeoutSeconds", c.llmTimeoutSeconds);
        c.llmMaxTokens = (int) num(p, "llm.maxTokens", c.llmMaxTokens);
        c.llmReasoningEffort = p.getProperty("llm.reasoningEffort", c.llmReasoningEffort).trim();
        c.llmMaxConcurrent = Math.max(1, (int) num(p, "llm.maxConcurrent", c.llmMaxConcurrent));
        c.agentMaxSteps = (int) num(p, "agent.maxSteps", c.agentMaxSteps);
        c.agentMaxBlocksPerOperation = (int) num(p, "agent.maxBlocksPerOperation", c.agentMaxBlocksPerOperation);
        c.agentContextChars = (int) num(p, "agent.contextChars", c.agentContextChars);
        c.agentShowToolProgress = bool(p, "agent.showToolProgress", c.agentShowToolProgress);
        c.agentTrace = bool(p, "agent.trace", c.agentTrace);
        c.worldBlocksPerTick = Math.max(1, (int) num(p, "world.blocksPerTick", c.worldBlocksPerTick));
        c.worldWriteMode = p.getProperty("world.writeMode", c.worldWriteMode).trim();
        c.worldBuildHalfExtent = (int) num(p, "world.buildHalfExtent", c.worldBuildHalfExtent);
        c.sessionIdleMinutes = (int) num(p, "session.idleMinutes", c.sessionIdleMinutes);
        c.journalMaxSteps = (int) num(p, "journal.maxSteps", c.journalMaxSteps);
        c.filterEnabled = bool(p, "filter.enabled", c.filterEnabled);
        c.syncUrl = p.getProperty("sync.url", c.syncUrl).trim();
        c.syncToken = p.getProperty("sync.token", c.syncToken).trim();
        c.syncIntervalSeconds = Math.max(2, (int) num(p, "sync.intervalSeconds", c.syncIntervalSeconds));
        c.syncServerName = p.getProperty("sync.serverName", c.syncServerName).trim();
        c.syncOwner = p.getProperty("sync.owner", c.syncOwner).trim();
        c.syncOthersCanBuy = bool(p, "sync.othersCanBuy", c.syncOthersCanBuy);
        c.syncShipyards = p.getProperty("sync.shipyards", c.syncShipyards).trim().toLowerCase(java.util.Locale.ROOT);
        return c;
    }

    /** Writes the current values so admins have a file to edit. */
    public void save(Path file) throws IOException {
        Properties p = new Properties();
        p.setProperty("llm.baseUrl", llmBaseUrl);
        p.setProperty("llm.model", llmModel);
        p.setProperty("llm.apiKey", llmApiKey);
        p.setProperty("llm.temperature", String.valueOf(llmTemperature));
        p.setProperty("llm.timeoutSeconds", String.valueOf(llmTimeoutSeconds));
        p.setProperty("llm.maxTokens", String.valueOf(llmMaxTokens));
        p.setProperty("llm.reasoningEffort", llmReasoningEffort);
        p.setProperty("llm.maxConcurrent", String.valueOf(llmMaxConcurrent));
        p.setProperty("agent.maxSteps", String.valueOf(agentMaxSteps));
        p.setProperty("agent.maxBlocksPerOperation", String.valueOf(agentMaxBlocksPerOperation));
        p.setProperty("agent.contextChars", String.valueOf(agentContextChars));
        p.setProperty("agent.showToolProgress", String.valueOf(agentShowToolProgress));
        p.setProperty("agent.trace", String.valueOf(agentTrace));
        p.setProperty("world.blocksPerTick", String.valueOf(worldBlocksPerTick));
        p.setProperty("world.writeMode", worldWriteMode);
        p.setProperty("world.buildHalfExtent", String.valueOf(worldBuildHalfExtent));
        p.setProperty("session.idleMinutes", String.valueOf(sessionIdleMinutes));
        p.setProperty("journal.maxSteps", String.valueOf(journalMaxSteps));
        p.setProperty("filter.enabled", String.valueOf(filterEnabled));
        p.setProperty("sync.url", syncUrl);
        p.setProperty("sync.token", syncToken);
        p.setProperty("sync.intervalSeconds", String.valueOf(syncIntervalSeconds));
        p.setProperty("sync.serverName", syncServerName);
        p.setProperty("sync.owner", syncOwner);
        p.setProperty("sync.othersCanBuy", String.valueOf(syncOthersCanBuy));
        p.setProperty("sync.shipyards", syncShipyards);
        Files.createDirectories(file.toAbsolutePath().getParent());
        try (OutputStream out = Files.newOutputStream(file)) {
            p.store(out, "StarMade Shipyard Helper. llm.baseUrl is any OpenAI-compatible endpoint (LM Studio, Ollama, ...)");
        }
    }

    private static double num(Properties p, String key, double fallback) {
        try {
            return Double.parseDouble(p.getProperty(key, String.valueOf(fallback)).trim());
        } catch (NumberFormatException e) {
            return fallback;
        }
    }

    private static boolean bool(Properties p, String key, boolean fallback) {
        return Boolean.parseBoolean(p.getProperty(key, String.valueOf(fallback)).trim());
    }
}
