package com.starmade.yardhelper.agent;

/** A tool call the model got wrong; the message is sent back so it can correct itself. */
public class ToolException extends Exception {
    public ToolException(String message) {
        super(message);
    }
}
