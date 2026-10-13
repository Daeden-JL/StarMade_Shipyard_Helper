package com.starmade.yardhelper.sync;

import java.nio.file.Path;
import java.util.List;

/**
 * The game's shipyards, as the exchange sees them. A design held in a shipyard is an entity
 * of its own: it is made from a blueprint in the catalog, and copied out as a blueprint while
 * it is loaded. Every method must be called on the server thread.
 */
public interface Yards {
    enum State { IDLE, LOADED, BUSY }

    /** A design a shipyard holds. */
    record Design(String uid, String name) {
    }

    /**
     * One shipyard on a loaded station.
     *
     * @param changedAt   when the loaded design or anything docked to it last changed, in
     *                    milliseconds; 0 if none is loaded
     * @param fullyLoaded every part of the loaded design is there (none still to dock), so a
     *                    copy is complete
     */
    record View(String id, String station, int[] at, State state, boolean inUse, boolean room, List<Design> designs,
                String loaded, long changedAt, boolean fullyLoaded) {
    }

    /** The shipyards on the stations the game has loaded, that the exchange may use. */
    List<View> list();

    /**
     * Writes the design loaded in a shipyard, with everything docked to it, as a blueprint
     * folder {@code into/name}.
     *
     * @throws IllegalStateException if that design is not the one loaded
     */
    void copy(String yard, String uid, Path into, String name) throws Exception;

    /** Orders a shipyard to unload its design. */
    void unload(String yard);

    /** Orders a shipyard to load one of its designs. */
    void load(String yard, String uid);

    /**
     * Orders a shipyard to make a new design called {@code name} from a blueprint in the
     * catalog, which it then holds loaded. The game picks the design's unique name.
     */
    void create(String yard, String blueprint, String name);

    /** Removes a design from a shipyard, which deletes it from the game. */
    void remove(String yard, String uid);
}
