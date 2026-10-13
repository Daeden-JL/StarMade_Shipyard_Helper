package com.starmade.yardhelper.session;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;
import java.util.function.Predicate;

/**
 * Keeps players out of each other's way: one session per player, one player per design.
 * A design stays locked to its session's player until that session ends.
 */
public final class SessionManager {
    /** Outcome of asking for a session. Exactly one of the fields is set. */
    public record Acquired(Session session, String heldBy) {}

    private final Map<String, Session> byPlayer = new HashMap<>();
    private final Map<String, Session> byDesign = new HashMap<>();

    /**
     * Returns the player's session for this design, creating it when needed.
     *
     * @param holderPresent tells whether another player is still inside the design; a
     *                      holder who has left and has nothing running gives up the lock
     * @param factory       builds a new session for (player, design)
     */
    public synchronized Acquired acquire(String player, String designId, Predicate<String> holderPresent,
                                         Function<String, Session> factory) {
        Session holder = byDesign.get(designId);
        if (holder != null && !holder.player.equals(player)) {
            if (holder.busy.get() || holderPresent.test(holder.player)) {
                return new Acquired(null, holder.player);
            }
            remove(holder);
        }
        Session mine = byPlayer.get(player);
        if (mine != null && mine.designId.equals(designId)) {
            mine.touch();
            return new Acquired(mine, null);
        }
        if (mine != null) {
            // The player moved to a different design; the old session and its lock go.
            mine.cancel.set(true);
            remove(mine);
        }
        Session created = factory.apply(designId);
        byPlayer.put(player, created);
        byDesign.put(designId, created);
        return new Acquired(created, null);
    }

    public synchronized Session forPlayer(String player) {
        return byPlayer.get(player);
    }

    public synchronized Session forDesign(String designId) {
        return byDesign.get(designId);
    }

    public synchronized void endForPlayer(String player) {
        Session s = byPlayer.get(player);
        if (s != null) {
            s.cancel.set(true);
            remove(s);
        }
    }

    public synchronized void endForDesign(String designId) {
        Session s = byDesign.get(designId);
        if (s != null) {
            s.cancel.set(true);
            remove(s);
        }
    }

    /** Ends sessions that have been quiet for too long and are not mid-request. */
    public synchronized List<Session> expireIdle(long idleMillis) {
        long cutoff = System.currentTimeMillis() - idleMillis;
        List<Session> expired = new ArrayList<>();
        for (Session s : new ArrayList<>(byPlayer.values())) {
            if (!s.busy.get() && s.lastActivityMillis < cutoff) {
                remove(s);
                expired.add(s);
            }
        }
        return expired;
    }

    public synchronized List<Session> all() {
        return new ArrayList<>(byPlayer.values());
    }

    private void remove(Session s) {
        byPlayer.remove(s.player, s);
        byDesign.remove(s.designId, s);
    }
}
