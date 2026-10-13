package com.starmade.yardhelper;

import com.starmade.yardhelper.session.Session;
import com.starmade.yardhelper.session.SessionManager;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SessionManagerTest {
    private final SessionManager sessions = new SessionManager();

    private SessionManager.Acquired acquire(String player, String design, boolean holderPresent) {
        return sessions.acquire(player, design, other -> holderPresent, id -> new Session(player, id, null));
    }

    @Test
    void samePlayerSameDesignReusesTheSession() {
        Session first = acquire("alice", "d1", true).session();
        assertSame(first, acquire("alice", "d1", true).session());
    }

    @Test
    void secondPlayerIsRefusedWhileHolderIsPresent() {
        acquire("alice", "d1", true);
        SessionManager.Acquired got = acquire("bob", "d1", true);
        assertNull(got.session());
        assertEquals("alice", got.heldBy());
    }

    @Test
    void twoPlayersOnDifferentDesignsDoNotInterfere() {
        Session a = acquire("alice", "d1", true).session();
        Session b = acquire("bob", "d2", true).session();
        assertNotNull(a);
        assertNotNull(b);
        assertNotSame(a, b);
    }

    @Test
    void lockPassesOnWhenHolderLeftAndIsIdle() {
        acquire("alice", "d1", true);
        SessionManager.Acquired got = acquire("bob", "d1", false);
        assertNotNull(got.session());
        assertNull(sessions.forPlayer("alice"));
    }

    @Test
    void lockDoesNotPassOnWhileHolderHasARequestRunning() {
        Session a = acquire("alice", "d1", true).session();
        a.busy.set(true);
        assertNull(acquire("bob", "d1", false).session());
    }

    @Test
    void movingToAnotherDesignReleasesTheOldOne() {
        Session old = acquire("alice", "d1", true).session();
        Session fresh = acquire("alice", "d2", true).session();
        assertNotSame(old, fresh);
        assertTrue(old.cancel.get());
        assertNotNull(acquire("bob", "d1", true).session());
    }

    @Test
    void endingForDesignFreesItAndCancelsWork() {
        Session a = acquire("alice", "d1", true).session();
        sessions.endForDesign("d1");
        assertTrue(a.cancel.get());
        assertNull(sessions.forPlayer("alice"));
    }

    @Test
    void idleSessionsExpireButBusyOnesDoNot() throws InterruptedException {
        Session a = acquire("alice", "d1", true).session();
        Session b = acquire("bob", "d2", true).session();
        b.busy.set(true);
        Thread.sleep(5);
        assertEquals(1, sessions.expireIdle(1).size());
        assertNull(sessions.forPlayer("alice"));
        assertSame(b, sessions.forPlayer("bob"));
        assertNotNull(a);
    }
}
