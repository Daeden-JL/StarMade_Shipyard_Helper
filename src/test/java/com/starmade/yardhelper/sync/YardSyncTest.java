package com.starmade.yardhelper.sync;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.atomic.AtomicLong;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The agent's shipyard part against a stand-in game shipyard that takes a few ticks per order. */
class YardSyncTest {
    private static final String YARD = "ENTITY_SPACESTATION_Home@42";

    /** One shipyard. Orders take effect after {@link #ticksPerOrder} ticks, as in the game. */
    final class FakeYards implements Yards {
        /** uid -> name and contents. */
        final Map<String, String[]> designs = new LinkedHashMap<>();
        String loaded;
        long changedAt;
        boolean fullyLoaded = true;
        boolean inUse;
        boolean refuseCreate;
        int ticksPerOrder = 3;
        Runnable pending;
        int wait;
        int made;
        final List<String> orders = new ArrayList<>();
        int copies;

        void tick() {
            if (pending != null && --wait <= 0) {
                Runnable p = pending;
                pending = null;
                p.run();
            }
        }

        private void later(Runnable r) {
            pending = r;
            wait = ticksPerOrder;
        }

        @Override
        public List<View> list() {
            State state = pending != null ? State.BUSY : loaded == null ? State.IDLE : State.LOADED;
            List<Design> list = new ArrayList<>();
            designs.forEach((uid, d) -> list.add(new Design(uid, d[0])));
            return List.of(new View(YARD, "Home", new int[]{1, 2, 3}, state, inUse, designs.size() < 5, list, state == State.LOADED ? loaded : null,
                    changedAt, fullyLoaded));
        }

        @Override
        public void copy(String yard, String uid, Path into, String name) throws IOException {
            if (!uid.equals(loaded)) throw new IllegalStateException(uid + " is not loaded");
            copies++;
            Blueprints.write(into, name, designs.get(uid)[1], 1_000);
        }

        @Override
        public void unload(String yard) {
            orders.add("unload");
            later(() -> loaded = null);
        }

        @Override
        public void load(String yard, String uid) {
            orders.add("load " + uid);
            later(() -> loaded = uid);
        }

        @Override
        public void create(String yard, String blueprint, String name) {
            orders.add("create " + name + " from " + blueprint);
            later(() -> {
                if (refuseCreate) return;
                // The game settles a name that is taken by adding to it.
                String uid = "ENTITY_SHIP_" + name;
                while (designs.containsKey(uid)) uid = "ENTITY_SHIP_" + name + "d" + (made++);
                designs.put(uid, new String[]{name, "from catalog " + blueprint});
                loaded = uid;
                changedAt = clock.get();
            });
        }

        @Override
        public void remove(String yard, String uid) {
            orders.add("remove " + uid);
            designs.remove(uid);
        }
    }

    /** What the web shipyard was told. */
    static final class FakeShipyard implements Shipyard {
        final List<String> uploads = new ArrayList<>();
        final List<String> given = new ArrayList<>();
        final List<String> trouble = new ArrayList<>();

        @Override
        public Plan report(String server, List<BlueprintFolder.Listed> blueprints, List<Reported> yards) {
            throw new UnsupportedOperationException();
        }

        @Override
        public int upload(String name, String print, Path zip) {
            throw new UnsupportedOperationException();
        }

        @Override
        public String download(String design, Path into) {
            throw new UnsupportedOperationException();
        }

        @Override
        public void done(String design, String name, String designPrint, String gamePrint) {
            throw new UnsupportedOperationException();
        }

        @Override
        public int uploadYard(String design, String uid, String print, Path zip) throws IOException {
            Path top = Zips.unzip(zip, Files.createTempDirectory(zip.getParent(), "check"));
            assertEquals(print, Fingerprint.of(top), "the zip holds what was announced");
            uploads.add(design + " " + uid);
            return 204;
        }

        @Override
        public void yardGiven(String design, String uid, String print) {
            given.add(design + " " + uid + " " + print);
        }

        @Override
        public void yardTrouble(String design, String why) {
            trouble.add(design + ": " + why);
        }
    }

    @TempDir
    Path tmp;
    final AtomicLong clock = new AtomicLong(1_000_000);
    FakeYards game;
    FakeShipyard web;
    YardSync sync;

    @BeforeEach
    void setUp() throws IOException {
        game = new FakeYards();
        web = new FakeShipyard();
        SyncAgent.ServerThread direct = new SyncAgent.ServerThread() {
            @Override
            public boolean running() {
                return true;
            }

            @Override
            public <T> T call(Callable<T> work, long timeoutMillis) throws Exception {
                return work.call();
            }
        };
        Path work = Files.createDirectories(tmp.resolve("work"));
        sync = new YardSync(game, web, direct, work, line -> {
        }, clock::get, millis -> {
            clock.addAndGet(millis);
            game.tick();
        });
    }

    private static Shipyard.YardPlan watch(String uid) {
        return new Shipyard.YardPlan(List.of(uid), List.of(), List.of());
    }

    private String reportedPrint() throws Exception {
        List<Shipyard.Reported> r = sync.survey();
        assertEquals(1, r.size());
        return r.get(0).loadedPrint();
    }

    @Test
    void a_watched_design_is_copied_once_it_is_complete_and_left_alone() throws Exception {
        game.designs.put("ENTITY_SHIP_Arrow", new String[]{"Arrow", "v1"});
        game.loaded = "ENTITY_SHIP_Arrow";
        game.changedAt = clock.get();
        // Not watched: never copied.
        assertNull(reportedPrint());
        sync.act(watch("ENTITY_SHIP_Arrow"), Map.of());
        // Changed just now, or still loading: not yet.
        assertNull(reportedPrint());
        clock.addAndGet(YardSync.COPY_QUIET_MILLIS);
        game.fullyLoaded = false;
        assertNull(reportedPrint());
        game.fullyLoaded = true;
        String first = reportedPrint();
        assertNotNull(first);
        // Unchanged: the same copy, not taken again.
        assertEquals(first, reportedPrint());
        assertEquals(1, game.copies);
        // Changed: nothing is reported until a fresh copy has been taken.
        game.designs.get("ENTITY_SHIP_Arrow")[1] = "v2";
        game.changedAt = clock.get();
        assertNull(reportedPrint());
        clock.addAndGet(YardSync.COPY_QUIET_MILLIS);
        String second = reportedPrint();
        assertNotNull(second);
        assertFalse(second.equals(first));

        // Asked for, it is uploaded; asked for an older copy, nothing is sent.
        sync.act(new Shipyard.YardPlan(List.of("ENTITY_SHIP_Arrow"), List.of(new Shipyard.YardTake("d1", "ENTITY_SHIP_Arrow", first)), List.of()), Map.of());
        assertEquals(List.of(), web.uploads);
        sync.act(new Shipyard.YardPlan(List.of("ENTITY_SHIP_Arrow"), List.of(new Shipyard.YardTake("d1", "ENTITY_SHIP_Arrow", second)), List.of()), Map.of());
        assertEquals(List.of("d1 ENTITY_SHIP_Arrow"), web.uploads);
    }

    @Test
    void a_design_is_swapped_for_one_from_the_catalog_and_the_old_one_removed() throws Exception {
        game.designs.put("ENTITY_SHIP_Arrow", new String[]{"Arrow", "v1"});
        game.loaded = "ENTITY_SHIP_Arrow";
        game.changedAt = clock.get() - YardSync.COPY_QUIET_MILLIS;
        sync.act(watch("ENTITY_SHIP_Arrow"), Map.of());
        String seen = reportedPrint();
        Shipyard.YardGive give = new Shipyard.YardGive("d1", YARD, "ENTITY_SHIP_Arrow", "Arrow", "Arrow", "g2", seen);

        // Not before the catalog has that version.
        sync.act(new Shipyard.YardPlan(List.of(), List.of(), List.of(give)), Map.of("Arrow", "g1"));
        assertEquals(List.of(), game.orders);
        assertTrue(web.trouble.get(0).contains("catalog"), web.trouble.toString());

        assertTrue(sync.act(new Shipyard.YardPlan(List.of(), List.of(), List.of(give)), Map.of("Arrow", "g2")));
        assertEquals(List.of("unload", "create Arrow from Arrow", "remove ENTITY_SHIP_Arrow"), game.orders);
        assertEquals(List.of("ENTITY_SHIP_Arrowd0"), new ArrayList<>(game.designs.keySet()));
        assertEquals("Arrow", game.designs.get("ENTITY_SHIP_Arrowd0")[0]);
        assertEquals("ENTITY_SHIP_Arrowd0", game.loaded);
        assertEquals(List.of("d1 ENTITY_SHIP_Arrowd0 g2"), web.given);
    }

    @Test
    void a_design_that_changed_in_the_shipyard_or_has_a_player_aboard_is_not_swapped_out() throws Exception {
        game.designs.put("ENTITY_SHIP_Arrow", new String[]{"Arrow", "v1"});
        game.loaded = "ENTITY_SHIP_Arrow";
        game.changedAt = clock.get() - YardSync.COPY_QUIET_MILLIS;
        sync.act(watch("ENTITY_SHIP_Arrow"), Map.of());
        String seen = reportedPrint();
        Shipyard.YardGive give = new Shipyard.YardGive("d1", YARD, "ENTITY_SHIP_Arrow", "Arrow", "Arrow", "g2", seen);
        game.inUse = true;
        sync.act(new Shipyard.YardPlan(List.of("ENTITY_SHIP_Arrow"), List.of(), List.of(give)), Map.of("Arrow", "g2"));
        game.inUse = false;
        game.changedAt = clock.get();
        sync.act(new Shipyard.YardPlan(List.of("ENTITY_SHIP_Arrow"), List.of(), List.of(give)), Map.of("Arrow", "g2"));
        assertEquals(List.of(), game.orders);
        assertEquals(2, web.trouble.size(), web.trouble.toString());
        assertTrue(web.trouble.get(0).contains("aboard"));
        assertTrue(web.trouble.get(1).contains("changed in the shipyard"));
    }

    @Test
    void when_the_shipyard_will_not_make_the_design_the_old_one_is_loaded_again() throws Exception {
        game.designs.put("ENTITY_SHIP_Arrow", new String[]{"Arrow", "v1"});
        game.loaded = "ENTITY_SHIP_Arrow";
        game.refuseCreate = true;
        Shipyard.YardGive give = new Shipyard.YardGive("d1", YARD, "ENTITY_SHIP_Arrow", "Arrow", "Arrow", "g2", null);
        sync.act(new Shipyard.YardPlan(List.of(), List.of(), List.of(give)), Map.of("Arrow", "g2"));
        assertEquals(List.of("unload", "create Arrow from Arrow", "load ENTITY_SHIP_Arrow"), game.orders);
        for (int i = 0; i < 5; i++) game.tick();
        assertEquals("ENTITY_SHIP_Arrow", game.loaded);
        assertEquals(List.of(), web.given);
        assertTrue(web.trouble.get(0).contains("would not make"), web.trouble.toString());
    }

    @Test
    void a_design_counts_as_made_once_the_shipyard_lists_it() throws Exception {
        // The game lists a new design at once, and loads it once its blocks are read.
        game.ticksPerOrder = 1;
        game.fullyLoaded = false;
        Shipyard.YardGive give = new Shipyard.YardGive("d1", YARD, null, "Arrow", "Arrow", "g1", null);
        FakeYards slow = game;
        slow.pending = null;
        sync.act(new Shipyard.YardPlan(List.of(), List.of(), List.of(give)), Map.of("Arrow", "g1"));
        assertEquals(List.of("d1 ENTITY_SHIP_Arrow g1"), web.given);
    }

    @Test
    void a_new_design_goes_into_a_free_shipyard() throws Exception {
        Shipyard.YardGive give = new Shipyard.YardGive("d1", YARD, null, "Arrow", "Arrow", "g1", null);
        sync.act(new Shipyard.YardPlan(List.of(), List.of(), List.of(give)), Map.of("Arrow", "g1"));
        assertEquals(List.of("create Arrow from Arrow"), game.orders);
        assertEquals(List.of("d1 ENTITY_SHIP_Arrow g1"), web.given);
    }
}
