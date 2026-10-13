package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.function.Consumer;
import java.util.function.LongSupplier;

/**
 * Keeps the game's blueprint catalog and the web shipyard in step. Every few seconds it
 * tells the shipyard which blueprints the game has, and the shipyard answers with the
 * ones it wants uploaded and the designs it wants put into the game. The shipyard
 * remembers what was exchanged; this side keeps nothing between runs.
 *
 * <p>Reading, packing and all network traffic happen on the agent's own thread. Only the
 * moment a blueprint is put in place and listed in the catalog runs on the server's
 * thread, and that is two renames and a rescan. Nothing here deletes a blueprint: one
 * that is replaced is moved to a backup folder.
 */
public final class SyncAgent {
    /** The server's own thread, the only one allowed to touch game state. */
    public interface ServerThread {
        /** Whether the server loop is up and taking work. */
        boolean running();

        /**
         * Runs work on the server thread and waits for it.
         *
         * @throws java.util.concurrent.TimeoutException if the server did not get to it in time; the work will then never run
         */
        <T> T call(Callable<T> work, long timeoutMillis) throws Exception;
    }

    /** The game's catalog. Only ever called on the server thread. */
    public interface Catalog {
        /**
         * Makes the game read its blueprint folder again and lists {@code name} in the
         * catalog: a new entry for a new name, refreshed price and mass for an existing one.
         *
         * @return a word for the log: what was done
         * @throws Exception if the game cannot read the blueprint
         */
        String publish(String name, String owner, boolean othersCanBuy) throws Exception;

        /** Makes the game read its blueprint folder again. */
        void rescan() throws Exception;
    }

    public record Settings(String serverName, int intervalSeconds, String owner, boolean othersCanBuy) {
    }

    /** A blueprint with a file written this recently may still be being saved. */
    static final long QUIET_MILLIS = 5_000;
    /** Most blueprints moved each way per round, so one round stays short. */
    static final int SENDS_PER_ROUND = 4;
    static final int FETCHES_PER_ROUND = 2;
    /** The shipyard takes uploads up to this size. */
    static final long MAX_UPLOAD = 256L * 1024 * 1024;
    /** Versions of a replaced blueprint that are kept. */
    static final int BACKUPS_KEPT = 3;
    private static final long SERVER_THREAD_WAIT = 60_000;

    private final BlueprintFolder folder;
    private final Shipyard shipyard;
    private final ServerThread server;
    private final Catalog catalog;
    private final Settings settings;
    private final Path staging;
    private final Path backups;
    private final Path rejected;
    private final Consumer<String> log;
    private final LongSupplier clock;

    /** Designs the game would not take, as "design|print". Not tried again in this run. */
    private final Set<String> refused = new HashSet<>();
    /** Blueprints too big for the shipyard, as "name|print". Not packed again in this run. */
    private final Set<String> tooBig = new HashSet<>();
    /** Things that failed for now, with when to try again. */
    private final Map<String, long[]> retries = new HashMap<>();
    private String lastTrouble = "";
    private int serial;

    private volatile boolean stopping;
    private Thread thread;
    /** The game's shipyards, or null where designs in shipyards are not exchanged. */
    private final YardSync yards;

    /**
     * @param work a folder of the agent's own, on the same volume as the blueprint folder
     *             and not somewhere the game looks for blueprints
     */
    public SyncAgent(BlueprintFolder folder, Shipyard shipyard, ServerThread server, Catalog catalog, Settings settings,
                     Path work, Consumer<String> log, LongSupplier clock) {
        this(folder, shipyard, server, catalog, null, settings, work, log, clock);
    }

    /** @param yards the game's shipyards, or null to leave designs in shipyards alone */
    public SyncAgent(BlueprintFolder folder, Shipyard shipyard, ServerThread server, Catalog catalog, Yards yards, Settings settings,
                     Path work, Consumer<String> log, LongSupplier clock) {
        this.yards = yards == null ? null : new YardSync(yards, shipyard, server, work.resolve("staging").resolve("yards"), log, clock, Thread::sleep);
        this.folder = folder;
        this.shipyard = shipyard;
        this.server = server;
        this.catalog = catalog;
        this.settings = settings;
        this.staging = work.resolve("staging");
        this.backups = work.resolve("replaced");
        this.rejected = work.resolve("rejected");
        this.log = log;
        this.clock = clock;
    }

    public synchronized void start() {
        if (thread != null) return;
        thread = new Thread(this::loop, "ShipyardHelper-sync");
        thread.setDaemon(true);
        thread.start();
    }

    /** Stops the agent and waits a moment for the round in progress to notice. */
    public synchronized void stop() {
        stopping = true;
        if (thread == null) return;
        thread.interrupt();
        try {
            thread.join(5_000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        thread = null;
    }

    private void loop() {
        try {
            BlueprintFolder.deleteTree(staging);
        } catch (Throwable t) {
            log.accept("Sync: could not clear " + staging + ": " + t);
        }
        while (!stopping) {
            boolean moved = false;
            try {
                moved = round();
                trouble("");
            } catch (Throwable t) {
                // The shipyard being down is ordinary; say so once, not every round.
                trouble(String.valueOf(t.getMessage() == null ? t : t.getMessage()));
            }
            try {
                // With more to move, come back at once; otherwise wait out the interval.
                Thread.sleep(moved ? 1_000 : Math.max(1, settings.intervalSeconds()) * 1_000L);
            } catch (InterruptedException e) {
                return;
            }
        }
    }

    private void trouble(String what) {
        if (what.equals(lastTrouble)) return;
        if (what.isEmpty()) log.accept("Sync: exchanging with the shipyard again.");
        else log.accept("Sync: no exchange with the shipyard: " + what);
        lastTrouble = what;
    }

    /**
     * One exchange: list, report, and carry out part of what the shipyard asks.
     *
     * @return whether anything was moved, which means there may be more waiting
     */
    public boolean round() throws IOException {
        if (!server.running()) return false;
        List<BlueprintFolder.Listed> listed = folder.list(clock.getAsLong(), QUIET_MILLIS);
        List<Shipyard.Reported> reported = null;
        if (yards != null) {
            try {
                Files.createDirectories(staging.resolve("yards"));
                reported = yards.survey();
            } catch (Exception e) {
                // Reported as unknown: nothing about shipyards is decided this round.
                log.accept("Sync: could not look at the game's shipyards: " + e);
            }
        }
        Shipyard.Plan plan = shipyard.report(settings.serverName(), listed, reported);
        boolean moved = false;
        int sends = 0;
        for (String name : plan.send()) {
            if (stopping || sends >= SENDS_PER_ROUND) break;
            String print = printOf(listed, name);
            if (print == null || tooBig.contains(name + "|" + print) || waiting("send|" + name + "|" + print)) continue;
            sends++;
            moved |= guarded("send|" + name + "|" + print, "sending \"" + name + "\"", () -> send(name, print));
        }
        int fetches = 0;
        for (Shipyard.Fetch fetch : plan.fetch()) {
            if (stopping || fetches >= FETCHES_PER_ROUND) break;
            String key = fetch.design() + "|" + fetch.print();
            if (refused.contains(key) || waiting("fetch|" + key)) continue;
            fetches++;
            moved |= guarded("fetch|" + key, "taking in \"" + fetch.name() + "\"", () -> fetch(fetch));
        }
        if (yards != null && reported != null && !stopping) {
            Map<String, String> catalog = new HashMap<>();
            for (BlueprintFolder.Listed l : listed) {
                if (!l.busy()) catalog.put(l.name(), l.print());
            }
            moved |= guarded("yards", "exchanging shipyard designs", () -> yards.act(plan.yards(), catalog));
        }
        return moved;
    }

    /** The fingerprint a blueprint was just reported with, or null if it was busy or is not there. */
    private static String printOf(List<BlueprintFolder.Listed> listed, String name) {
        for (BlueprintFolder.Listed l : listed) {
            if (l.name().equals(name)) return l.busy() ? null : l.print();
        }
        return null;
    }

    /** Whether something that failed is still waiting out its delay. */
    private boolean waiting(String key) {
        long[] retry = retries.get(key);
        return retry != null && clock.getAsLong() < retry[1];
    }

    /** Runs one transfer. A failure is logged and tried again later, each time after a longer wait. */
    private boolean guarded(String key, String what, Callable<Boolean> transfer) {
        try {
            boolean done = transfer.call();
            retries.remove(key);
            return done;
        } catch (Throwable t) {
            long[] retry = retries.computeIfAbsent(key, k -> new long[2]);
            retry[0]++;
            long wait = Math.min(600_000, 30_000L << Math.min(retry[0] - 1, 5));
            retry[1] = clock.getAsLong() + wait;
            log.accept("Sync: " + what + " failed, next try in " + wait / 1000 + " s: " + t);
            return false;
        }
    }

    private Path scratch(String kind) throws IOException {
        Path dir = staging.resolve(kind + "-" + clock.getAsLong() + "-" + (serial++));
        Files.createDirectories(dir);
        return dir;
    }

    private boolean send(String name, String print) throws IOException {
        List<Fingerprint.Stamp> stamps = folder.stampsOf(name);
        if (stamps == null) return false;
        Path dir = scratch("send");
        try {
            Path zip = dir.resolve("blueprint.zip");
            Zips.zip(folder.path(name), name, stamps, zip);
            if (!folder.unchanged(name)) {
                // Saved again while it was being packed; the next listing has the new one.
                folder.forget(name);
                return false;
            }
            if (Files.size(zip) > MAX_UPLOAD) {
                tooBig.add(name + "|" + print);
                log.accept("Sync: \"" + name + "\" is " + Files.size(zip) / (1024 * 1024) + " MB packed, more than the shipyard takes; it is left out.");
                return false;
            }
            int status = shipyard.upload(name, print, zip);
            switch (status) {
                case 200 -> {
                    log.accept("Sync: sent \"" + name + "\" to the shipyard.");
                    return true;
                }
                case 409 -> {
                    folder.forget(name);
                    return false;
                }
                case 422 -> {
                    // The shipyard remembers this version and will not ask for it again.
                    log.accept("Sync: the shipyard cannot read \"" + name + "\"; it is left alone until it changes.");
                    return false;
                }
                default -> throw new IOException("the shipyard answered HTTP " + status);
            }
        } finally {
            BlueprintFolder.deleteTree(dir);
        }
    }

    /** Marks a design version as one this game will not take, and says why. */
    private boolean refuse(Shipyard.Fetch fetch, String why) {
        refused.add(fetch.design() + "|" + fetch.print());
        log.accept("Sync: not taking \"" + fetch.name() + "\" (design " + fetch.design() + "): " + why);
        return false;
    }

    private boolean fetch(Shipyard.Fetch fetch) throws Exception {
        String name = fetch.name();
        if (!Names.valid(name)) return refuse(fetch, "the game does not allow that name");
        String other = folder.spelledDifferently(name);
        if (other != null) return refuse(fetch, "the game already has \"" + other + "\", the same name in other letter case");
        Path dir = scratch("fetch");
        try {
            Path zip = dir.resolve("design.zip");
            String print = shipyard.download(fetch.design(), zip);
            Path top;
            try {
                top = Zips.unzip(zip, dir.resolve("unpacked"));
            } catch (IOException e) {
                return refuse(fetch, e.getMessage());
            }
            if (!top.getFileName().toString().equals(name)) {
                return refuse(fetch, "the file holds a folder called \"" + top.getFileName() + "\"");
            }
            if (!Files.isRegularFile(top.resolve(BlueprintFolder.HEADER)) || !Files.isRegularFile(top.resolve(BlueprintFolder.META))) {
                return refuse(fetch, "the file is not a complete blueprint");
            }
            if (!Fingerprint.of(top).equals(print)) throw new IOException("the download does not match its fingerprint");

            String outcome;
            try {
                outcome = server.call(() -> place(top, name), SERVER_THREAD_WAIT);
            } catch (Unwelcome e) {
                return refuse(fetch, String.valueOf(e.getCause()));
            }
            String gamePrint = Fingerprint.of(folder.path(name));
            shipyard.done(fetch.design(), name, print, gamePrint);
            log.accept("Sync: \"" + name + "\" from the shipyard is in the catalog (" + outcome + ").");
            try {
                BlueprintFolder.prune(backups, name, BACKUPS_KEPT);
            } catch (IOException e) {
                log.accept("Sync: could not tidy old versions of \"" + name + "\": " + e);
            }
            return true;
        } finally {
            BlueprintFolder.deleteTree(dir);
        }
    }

    /** The game turned a blueprint down after it was put in place; it has been taken out again. */
    private static final class Unwelcome extends Exception {
        Unwelcome(Throwable cause) {
            super(cause);
        }
    }

    /** On the server thread: swap the blueprint in and list it. Undone if the game cannot read it. */
    private String place(Path staged, String name) throws Exception {
        if (Files.exists(folder.path(name + "Tmp"))) throw new IOException("the game is saving \"" + name + "\" right now");
        long now = clock.getAsLong();
        Path backup = folder.install(staged, name, backups, now);
        try {
            return catalog.publish(name, settings.owner(), settings.othersCanBuy());
        } catch (Throwable t) {
            try {
                folder.undoInstall(name, backup, rejected, now);
                catalog.rescan();
            } catch (Throwable undo) {
                log.accept("Sync: could not take \"" + name + "\" back out: " + undo);
            }
            throw new Unwelcome(t);
        }
    }
}
