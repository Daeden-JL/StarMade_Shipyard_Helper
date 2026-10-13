package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.nio.file.Path;
import java.util.List;

/** The shipyard's side of the exchange, as the agent talks to it. */
public interface Shipyard {
    /** A design the shipyard wants put into the game. */
    record Fetch(String design, String name, String print) {
    }

    /** A design in a game shipyard, to upload: it changed there. */
    record YardTake(String design, String uid, String print) {
    }

    /**
     * A blueprint from the catalog to put into a game shipyard as a design.
     *
     * @param replace the shipyard design it takes the place of, or null for a new one
     * @param name    the name the shipyard shows for it
     * @param print   the fingerprint the blueprint must have
     * @param expect  the fingerprint the replaced design's copy must have, or null
     */
    record YardGive(String design, String yard, String replace, String name, String blueprint, String print, String expect) {
    }

    /** What to do with designs in game shipyards. */
    record YardPlan(List<String> watch, List<YardTake> take, List<YardGive> give) {
        public static final YardPlan NONE = new YardPlan(List.of(), List.of(), List.of());
    }

    /** What the shipyard asks for after hearing what the game has. */
    record Plan(List<String> send, List<Fetch> fetch, YardPlan yards) {
        public Plan(List<String> send, List<Fetch> fetch) {
            this(send, fetch, YardPlan.NONE);
        }
    }

    /**
     * A game shipyard as reported.
     *
     * @param loadedPrint the fingerprint of a complete copy of the loaded design as it is now, or null
     */
    record Reported(Yards.View view, String loadedPrint) {
    }

    /**
     * Tells the shipyard what the game has.
     *
     * @param yards the game's shipyards, or null where the agent does not deal with them
     */
    Plan report(String server, List<BlueprintFolder.Listed> blueprints, List<Reported> yards) throws IOException;

    /**
     * Hands over a copy of a design in a game shipyard, as a zip.
     *
     * @return the HTTP status: 2xx taken, 409 the fingerprint does not match, 404 the design
     *         is not linked to that shipyard design any more, 422 the shipyard cannot read it
     */
    int uploadYard(String design, String uid, String print, Path zip) throws IOException;

    /** Confirms that a shipyard holds a design made from a design's blueprint. */
    void yardGiven(String design, String uid, String print) throws IOException;

    /** Says what went wrong with a design's shipyard design. */
    void yardTrouble(String design, String why) throws IOException;

    /**
     * Hands over a blueprint as a zip.
     *
     * @return the HTTP status: 200 taken, 409 the fingerprint does not match the file,
     *         422 the shipyard cannot read it
     */
    int upload(String name, String print, Path zip) throws IOException;

    /**
     * Downloads a design as a zip.
     *
     * @return the fingerprint the shipyard gives for it
     */
    String download(String design, Path into) throws IOException;

    /** Confirms that a design is in the game's catalog. */
    void done(String design, String name, String designPrint, String gamePrint) throws IOException;
}
