package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.JsonNode;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Vec3;

import java.util.ArrayList;
import java.util.List;

/**
 * Tool arguments with lenient parsing. Local models are sloppy about types (numbers as
 * strings, positions as objects), so accept the common variations and give errors that say
 * exactly what to send instead.
 */
public final class Args {
    private final JsonNode node;

    public Args(JsonNode node) {
        this.node = node;
    }

    public boolean has(String name) {
        return node.hasNonNull(name);
    }

    public JsonNode raw(String name) {
        return node.path(name);
    }

    public String str(String name) throws ToolException {
        if (!has(name)) throw new ToolException("Missing required argument \"" + name + "\".");
        return node.get(name).asText();
    }

    public String strOr(String name, String fallback) {
        return has(name) ? node.get(name).asText() : fallback;
    }

    public int integer(String name) throws ToolException {
        if (!has(name)) throw new ToolException("Missing required argument \"" + name + "\" (an integer).");
        return toInt(node.get(name), name);
    }

    public int intOr(String name, int fallback) throws ToolException {
        return has(name) ? toInt(node.get(name), name) : fallback;
    }

    public double numberOr(String name, double fallback) {
        return has(name) ? node.get(name).asDouble(fallback) : fallback;
    }

    public boolean boolOr(String name, boolean fallback) {
        if (!has(name)) return fallback;
        JsonNode v = node.get(name);
        return v.isBoolean() ? v.asBoolean() : "true".equalsIgnoreCase(v.asText());
    }

    public Vec3 vec(String name) throws ToolException {
        if (!has(name)) throw new ToolException("Missing required argument \"" + name + "\" (a position as [x, y, z]).");
        return toVec(node.get(name), name);
    }

    public Box box(String from, String to) throws ToolException {
        return Box.of(vec(from), vec(to));
    }

    public List<Vec3> vecList(String name) throws ToolException {
        List<Vec3> out = new ArrayList<>();
        JsonNode list = node.path(name);
        if (!list.isArray()) return out;
        for (JsonNode item : list) out.add(toVec(item, name));
        return out;
    }

    static Vec3 toVec(JsonNode v, String name) throws ToolException {
        try {
            if (v.isArray() && v.size() == 3) {
                return new Vec3(toInt(v.get(0), name), toInt(v.get(1), name), toInt(v.get(2), name));
            }
            if (v.isObject() && v.has("x") && v.has("y") && v.has("z")) {
                return new Vec3(toInt(v.get("x"), name), toInt(v.get("y"), name), toInt(v.get("z"), name));
            }
            if (v.isTextual()) {
                String[] parts = v.asText().replaceAll("[\\[\\]() ]", "").split(",");
                if (parts.length == 3) {
                    return new Vec3(Integer.parseInt(parts[0]), Integer.parseInt(parts[1]), Integer.parseInt(parts[2]));
                }
            }
        } catch (NumberFormatException ignored) {
            // fall through to the error below
        }
        throw new ToolException("Argument \"" + name + "\" must be a position as [x, y, z] integers, got: " + v);
    }

    private static int toInt(JsonNode v, String name) throws ToolException {
        if (v.isNumber()) return (int) Math.round(v.asDouble());
        try {
            return (int) Math.round(Double.parseDouble(v.asText().trim()));
        } catch (NumberFormatException e) {
            throw new ToolException("Argument \"" + name + "\" must be an integer, got: " + v);
        }
    }
}
