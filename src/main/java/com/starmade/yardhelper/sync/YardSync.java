package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.LongSupplier;

/**
 * The agent's part in keeping designs in game shipyards in step with designs in the web
 * shipyard. Each round it copies the watched designs that are loaded and changed (a copy is
 * only taken of a design whose parts have all docked and that has been left alone for a while),
 * reports their fingerprints, uploads the ones the shipyard asks for, and swaps a design
 * for one made from the catalog when the shipyard asks for that.
 *
 * <p>A swap is the game's own orders: unload the design, make a new one from the catalog's
 * blueprint, and only once that is loaded, remove the old one. If the new one does not
 * come, the old one is loaded again.
 */
final class YardSync {
    /** A loaded design is copied once it has been left alone this long. */
    static final long COPY_QUIET_MILLIS = 10_000;
    /** How long a shipyard gets to carry out an order. */
    static final long ORDER_MILLIS = 120_000;
    /** A shipyard still idle this long after an order to make a design has not taken it. */
    static final long NOT_TAKEN_MILLIS = 10_000;
    private static final long POLL_MILLIS = 500;
    private static final long SERVER_THREAD_WAIT = 120_000;

    /** The latest copy of a design: when the design had last changed, and the packed copy. */
    private record Copy(long changedAt, String print, Path zip) {
    }

    /** Something a shipyard would not do; reported to the web shipyard. */
    static final class Refused extends Exception {
        Refused(String why) {
            super(why);
        }
    }

    interface Sleeper {
        void sleep(long millis) throws InterruptedException;
    }

    private final Yards yards;
    private final Shipyard shipyard;
    private final SyncAgent.ServerThread server;
    private final Path work;
    private final Consumer<String> log;
    private final LongSupplier clock;
    private final Sleeper sleeper;

    /** Designs the shipyard wants fingerprints of, from its last answer. */
    private Set<String> watched = new HashSet<>();
    private final Map<String, Copy> copies = new HashMap<>();
    private int serial;

    YardSync(Yards yards, Shipyard shipyard, SyncAgent.ServerThread server, Path work, Consumer<String> log, LongSupplier clock, Sleeper sleeper) {
        this.yards = yards;
        this.shipyard = shipyard;
        this.server = server;
        this.work = work;
        this.log = log;
        this.clock = clock;
        this.sleeper = sleeper;
    }

    /** The shipyards as they are now, with fresh copies of the watched designs. */
    List<Shipyard.Reported> survey() throws Exception {
        List<Yards.View> views = server.call(yards::list, SERVER_THREAD_WAIT);
        Set<String> loaded = new HashSet<>();
        List<Shipyard.Reported> out = new ArrayList<>();
        for (Yards.View v : views) {
            String print = null;
            if (v.loaded() != null && watched.contains(v.loaded())) {
                loaded.add(v.loaded());
                Copy copy = copies.get(v.loaded());
                boolean stale = copy == null || copy.changedAt() < v.changedAt();
                if (stale && v.fullyLoaded() && clock.getAsLong() - v.changedAt() >= COPY_QUIET_MILLIS) {
                    try {
                        copy = copy(v);
                        stale = false;
                    } catch (Exception e) {
                        log.accept("Sync: could not copy the design in the shipyard of \"" + v.station() + "\": " + e);
                    }
                }
                if (!stale) print = copy.print();
            }
            out.add(new Shipyard.Reported(v, print));
        }
        // Copies of designs no longer loaded or watched are out of date by the time they are.
        copies.keySet().removeIf(uid -> {
            if (loaded.contains(uid)) return false;
            deleteQuietly(copies.get(uid).zip());
            return true;
        });
        return out;
    }

    private Copy copy(Yards.View v) throws Exception {
        Path dir = work.resolve("copy-" + clock.getAsLong() + "-" + (serial++));
        Files.createDirectories(dir);
        try {
            String uid = v.loaded();
            server.call(() -> {
                yards.copy(v.id(), uid, dir, "design");
                return null;
            }, SERVER_THREAD_WAIT);
            Path folder = dir.resolve("design");
            List<Fingerprint.Stamp> stamps = Fingerprint.stamps(folder);
            String print = Fingerprint.of(folder, stamps);
            Path zip = work.resolve("yard-" + Integer.toHexString(uid.hashCode()) + "-" + (serial++) + ".zip");
            Zips.zip(folder, "design", stamps, zip);
            Copy old = copies.put(uid, new Copy(v.changedAt(), print, zip));
            if (old != null) deleteQuietly(old.zip());
            return copies.get(uid);
        } finally {
            BlueprintFolder.deleteTree(dir);
        }
    }

    /**
     * Carries out the shipyard's answer: uploads, and at most one swap.
     *
     * @param catalog the catalog's blueprints by name, with their fingerprints
     * @return whether anything was moved
     */
    boolean act(Shipyard.YardPlan plan, Map<String, String> catalog) throws IOException {
        watched = new HashSet<>(plan.watch());
        boolean moved = false;
        for (Shipyard.YardTake take : plan.take()) {
            Copy copy = copies.get(take.uid());
            if (copy == null || !copy.print().equals(take.print())) continue;
            int status = shipyard.uploadYard(take.design(), take.uid(), take.print(), copy.zip());
            if (status / 100 == 2) {
                log.accept("Sync: sent the design in a shipyard (" + take.uid() + ") to the shipyard's design " + take.design() + ".");
                moved = true;
            } else if (status == 409) {
                copies.remove(take.uid());
                deleteQuietly(copy.zip());
            } else if (status != 404 && status != 422) {
                throw new IOException("the shipyard answered HTTP " + status + " to a shipyard design");
            }
        }
        for (Shipyard.YardGive give : plan.give()) {
            try {
                String made = swap(give, catalog);
                shipyard.yardGiven(give.design(), made, give.print());
                return true;
            } catch (Refused r) {
                log.accept("Sync: shipyard design of " + give.design() + ": " + r.getMessage());
                shipyard.yardTrouble(give.design(), r.getMessage());
                return moved;
            } catch (Exception e) {
                log.accept("Sync: shipyard design of " + give.design() + " failed: " + e);
                shipyard.yardTrouble(give.design(), "the game server ran into an error: " + e.getMessage());
                return moved;
            }
        }
        return moved;
    }

    private Yards.View view(String yard) throws Exception {
        for (Yards.View v : server.call(yards::list, SERVER_THREAD_WAIT)) {
            if (v.id().equals(yard)) return v;
        }
        throw new Refused("the shipyard's station is no longer loaded");
    }

    /** Puts a blueprint from the catalog into a shipyard. Returns the new design's unique name. */
    private String swap(Shipyard.YardGive give, Map<String, String> catalog) throws Exception {
        if (!give.print().equals(catalog.get(give.blueprint()))) {
            throw new Refused("the game's catalog does not have this version of \"" + give.blueprint() + "\" yet");
        }
        Yards.View v = view(give.yard());
        String old = give.replace();
        if (old != null) {
            if (v.state() != Yards.State.LOADED || !old.equals(v.loaded())) throw new Refused("the design is no longer loaded in its shipyard");
            if (v.inUse()) throw new Refused("a player is aboard the design in the shipyard");
            if (give.expect() != null) {
                Copy copy = copies.get(old);
                if (copy == null || !copy.print().equals(give.expect()) || copy.changedAt() < v.changedAt()) {
                    throw new Refused("it changed in the shipyard; that version comes here first");
                }
            }
        } else if (v.state() != Yards.State.IDLE) {
            throw new Refused("the shipyard is not free");
        }
        if (!v.room()) throw new Refused("the shipyard has no free slot in its inventory");
        Set<String> before = new HashSet<>();
        for (Yards.Design d : v.designs()) before.add(d.uid());

        if (old != null) {
            server.call(() -> {
                yards.unload(give.yard());
                return null;
            }, SERVER_THREAD_WAIT);
            await(give.yard(), "unload its design", w -> w.state() == Yards.State.IDLE ? true : null);
        }
        server.call(() -> {
            yards.create(give.yard(), give.blueprint(), give.name());
            return null;
        }, SERVER_THREAD_WAIT);
        long ordered = clock.getAsLong();
        String made;
        try {
            made = await(give.yard(), "make the design", w -> {
                // Listed once it exists. It is then loaded as soon as the game has read
                // its blocks, which it does for a player near the station.
                for (Yards.Design d : w.designs()) {
                    if (!before.contains(d.uid())) return d.uid();
                }
                if (w.state() == Yards.State.IDLE && clock.getAsLong() - ordered > NOT_TAKEN_MILLIS) {
                    throw new Refused("the shipyard would not make a design from \"" + give.blueprint() + "\"");
                }
                return null;
            });
        } catch (Exception e) {
            if (old != null) {
                try {
                    server.call(() -> {
                        yards.load(give.yard(), old);
                        return null;
                    }, SERVER_THREAD_WAIT);
                } catch (Exception again) {
                    log.accept("Sync: could not load the old design in the shipyard again: " + again);
                }
            }
            throw e;
        }
        if (old != null) {
            server.call(() -> {
                yards.remove(give.yard(), old);
                return null;
            }, SERVER_THREAD_WAIT);
            Copy gone = copies.remove(old);
            if (gone != null) deleteQuietly(gone.zip());
        }
        log.accept("Sync: the shipyard of \"" + v.station() + "\" holds \"" + give.name() + "\" from the catalog as " + made + ".");
        return made;
    }

    private interface Check<T> {
        /** @return the result once there, or null to keep waiting */
        T check(Yards.View view) throws Refused;
    }

    private <T> T await(String yard, String what, Check<T> check) throws Exception {
        long deadline = clock.getAsLong() + ORDER_MILLIS;
        while (true) {
            T result = check.check(view(yard));
            if (result != null) return result;
            if (clock.getAsLong() > deadline) throw new Refused("the shipyard did not " + what + " in time");
            sleeper.sleep(POLL_MILLIS);
        }
    }

    private static void deleteQuietly(Path file) {
        try {
            Files.deleteIfExists(file);
        } catch (IOException ignored) {
            // In the agent's own work folder, which is cleared at the next start.
        }
    }
}
