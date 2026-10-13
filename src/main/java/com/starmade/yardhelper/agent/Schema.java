package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/** Small builder for tool parameter schemas. */
public final class Schema {
    private static final ObjectMapper JSON = new ObjectMapper();

    private final ObjectNode root = JSON.createObjectNode();
    private final ObjectNode props;
    private final ArrayNode required;

    private Schema() {
        root.put("type", "object");
        props = root.putObject("properties");
        required = root.putArray("required");
    }

    public static Schema object() {
        return new Schema();
    }

    public Schema str(String name, String description) {
        return add(name, "string", description);
    }

    public Schema integer(String name, String description) {
        return add(name, "integer", description);
    }

    public Schema number(String name, String description) {
        return add(name, "number", description);
    }

    public Schema bool(String name, String description) {
        return add(name, "boolean", description);
    }

    public Schema choice(String name, String description, String... options) {
        ObjectNode p = props.putObject(name);
        p.put("type", "string");
        p.put("description", description);
        ArrayNode e = p.putArray("enum");
        for (String o : options) e.add(o);
        return this;
    }

    /** A position or size as [x, y, z]. */
    public Schema vec(String name, String description) {
        ObjectNode p = props.putObject(name);
        p.put("type", "array");
        p.put("description", description + " As [x, y, z] integers.");
        p.putObject("items").put("type", "integer");
        p.put("minItems", 3);
        p.put("maxItems", 3);
        return this;
    }

    public Schema array(String name, String description, ObjectNode items) {
        ObjectNode p = props.putObject(name);
        p.put("type", "array");
        p.put("description", description);
        p.set("items", items);
        return this;
    }

    public Schema require(String... names) {
        for (String n : names) required.add(n);
        return this;
    }

    public ObjectNode build() {
        return root;
    }

    public static ObjectNode vecItems() {
        ObjectNode items = JSON.createObjectNode();
        items.put("type", "array");
        items.putObject("items").put("type", "integer");
        items.put("minItems", 3);
        items.put("maxItems", 3);
        return items;
    }

    private Schema add(String name, String type, String description) {
        ObjectNode p = props.putObject(name);
        p.put("type", type);
        p.put("description", description);
        return this;
    }
}
