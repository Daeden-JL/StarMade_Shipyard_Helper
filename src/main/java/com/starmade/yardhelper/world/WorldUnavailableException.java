package com.starmade.yardhelper.world;

/** The design can no longer be edited: it was unloaded, or the player left it. */
public class WorldUnavailableException extends RuntimeException {
    public WorldUnavailableException(String message) {
        super(message);
    }
}
