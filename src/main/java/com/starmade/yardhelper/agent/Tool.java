package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.node.ObjectNode;

/**
 * One action the model can take.
 *
 * @param parameters JSON schema of the arguments object
 */
public record Tool(String name, String description, ObjectNode parameters, Handler handler) {
    public interface Handler {
        String run(Args args, ToolContext ctx) throws ToolException;
    }
}
