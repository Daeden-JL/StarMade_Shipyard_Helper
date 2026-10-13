package com.starmade.yardhelper.agent;

import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;

/** Game-mechanics notes bundled in the jar that the model can read on demand. */
public final class Guides {
    private static final String[] TOPICS = {"building", "power", "weapons", "shields-thrust", "logic", "rails"};

    private final Map<String, String> byTopic = new LinkedHashMap<>();

    public static Guides load() {
        Guides g = new Guides();
        for (String topic : TOPICS) {
            String text = resource("guides/" + topic + ".md");
            if (text != null) g.byTopic.put(topic, text);
        }
        return g;
    }

    public String get(String topic) {
        return byTopic.get(topic);
    }

    public String topics() {
        return String.join(", ", byTopic.keySet());
    }

    public static String resource(String path) {
        try (InputStream in = Guides.class.getClassLoader().getResourceAsStream(path)) {
            return in == null ? null : new String(in.readAllBytes(), StandardCharsets.UTF_8);
        } catch (IOException e) {
            return null;
        }
    }
}
