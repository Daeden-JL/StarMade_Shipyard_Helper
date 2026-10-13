package com.starmade.yardhelper.sync;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicLong;
import java.util.stream.Stream;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** The agent against a stand-in shipyard that speaks the real protocol over HTTP. */
class SyncAgentTest {
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final String TOKEN = "s3cret";

    /** What the stand-in shipyard was told and what it answers. */
    static final class FakeShipyard {
        final List<JsonNode> reports = new ArrayList<>();
        /** name -> fingerprint of what arrived, as the shipyard itself works it out. */
        final Map<String, String> uploads = new HashMap<>();
        final List<JsonNode> dones = new ArrayList<>();
        final List<String> downloads = new ArrayList<>();
        /** design id -> {zip bytes, fingerprint header}. */
        final Map<String, Object[]> designs = new HashMap<>();
        final List<String> tokens = new ArrayList<>();
        String plan = "{\"send\":[],\"fetch\":[]}";
        int uploadStatus = 200;
        HttpServer http;
        Path scratch;

        void start(Path scratch) throws IOException {
            this.scratch = scratch;
            http = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
            http.createContext("/api/sync/", this::handle);
            http.start();
        }

        String url() {
            return "http://127.0.0.1:" + http.getAddress().getPort() + "/";
        }

        private void handle(HttpExchange x) throws IOException {
            try {
                tokens.add(String.valueOf(x.getRequestHeaders().getFirst("x-yard-sync-token")));
                String path = x.getRequestURI().getPath();
                byte[] body = x.getRequestBody().readAllBytes();
                if (path.equals("/api/sync/report")) {
                    reports.add(JSON.readTree(body));
                    reply(x, 200, plan.getBytes(StandardCharsets.UTF_8));
                } else if (path.equals("/api/sync/blueprints")) {
                    Map<String, String> q = new HashMap<>();
                    for (String pair : x.getRequestURI().getRawQuery().split("&")) {
                        String[] kv = pair.split("=", 2);
                        q.put(kv[0], URLDecoder.decode(kv[1], StandardCharsets.UTF_8));
                    }
                    Path zip = Files.createTempFile(scratch, "upload", ".zip");
                    Files.write(zip, body);
                    Path top = Zips.unzip(zip, Files.createTempDirectory(scratch, "upload"));
                    String print = Fingerprint.of(top);
                    boolean matches = print.equals(q.get("print")) && top.getFileName().toString().equals(q.get("name"));
                    if (matches && uploadStatus == 200) uploads.put(q.get("name"), print);
                    reply(x, matches ? uploadStatus : 409, new byte[0]);
                } else if (path.startsWith("/api/sync/designs/")) {
                    String id = path.split("/")[4];
                    downloads.add(id);
                    Object[] design = designs.get(id);
                    if (design == null) {
                        reply(x, 404, new byte[0]);
                        return;
                    }
                    x.getResponseHeaders().add("x-yard-print", (String) design[1]);
                    reply(x, 200, (byte[]) design[0]);
                } else if (path.equals("/api/sync/done")) {
                    dones.add(JSON.readTree(body));
                    reply(x, 204, new byte[0]);
                } else {
                    reply(x, 404, new byte[0]);
                }
            } catch (Throwable t) {
                t.printStackTrace();
                reply(x, 500, new byte[0]);
            }
        }

        private static void reply(HttpExchange x, int status, byte[] body) throws IOException {
            x.sendResponseHeaders(status, status == 204 ? -1 : body.length);
            if (status != 204) x.getResponseBody().write(body);
            x.close();
        }

        /** Offers a design made from a blueprint folder, as the shipyard would. */
        String offer(String id, Path folder, String topFolder) throws IOException {
            Path zip = Files.createTempFile(scratch, "design", ".zip");
            Zips.zip(folder, topFolder, Fingerprint.stamps(folder), zip);
            String print = Fingerprint.of(folder);
            designs.put(id, new Object[]{Files.readAllBytes(zip), print});
            return print;
        }
    }

    /** The game's catalog, standing in: remembers what it was asked and can refuse. */
    static final class FakeCatalog implements SyncAgent.Catalog {
        final List<String> published = new ArrayList<>();
        int rescans;
        String refuse;

        @Override
        public String publish(String name, String owner, boolean othersCanBuy) throws Exception {
            if (name.equals(refuse)) throw new IllegalStateException("the game cannot read " + name);
            published.add(name + " for " + owner + (othersCanBuy ? ", open to others" : ""));
            return "new";
        }

        @Override
        public void rescan() {
            rescans++;
        }
    }

    /** The server thread, standing in: runs the work at once, or never gets to it. */
    static final class FakeServer implements SyncAgent.ServerThread {
        boolean up = true, stuck;

        @Override
        public boolean running() {
            return up;
        }

        @Override
        public <T> T call(Callable<T> work, long timeoutMillis) throws Exception {
            if (stuck) throw new TimeoutException("the server did not get to it");
            return work.call();
        }
    }

    @TempDir
    Path tmp;
    Path blueprints, work, elsewhere;
    final FakeShipyard yard = new FakeShipyard();
    final FakeCatalog catalog = new FakeCatalog();
    final FakeServer server = new FakeServer();
    final List<String> log = new ArrayList<>();
    final AtomicLong now = new AtomicLong(10_000_000);
    HttpShipyard client;
    SyncAgent agent;

    @BeforeEach
    void setUp() throws IOException {
        blueprints = Files.createDirectories(tmp.resolve("blueprints"));
        work = tmp.resolve("moddata/sync");
        elsewhere = Files.createDirectories(tmp.resolve("elsewhere"));
        yard.start(Files.createDirectories(tmp.resolve("yard")));
        client = new HttpShipyard(yard.url(), TOKEN);
        agent = new SyncAgent(new BlueprintFolder(blueprints), client, server, catalog,
                new SyncAgent.Settings("Test game", 10, "Shipyard", true), work, log::add, now::get);
    }

    @AfterEach
    void tearDown() {
        client.close();
        yard.http.stop(0);
    }

    private long old() {
        return now.get() - 60_000;
    }

    private static String fetchPlan(String design, String name, String print) {
        return "{\"send\":[],\"fetch\":[{\"design\":\"" + design + "\",\"name\":\"" + name + "\",\"print\":\"" + print + "\"}]}";
    }

    private List<String> filesUnder(Path dir) throws IOException {
        if (!Files.exists(dir)) return List.of();
        try (Stream<Path> all = Files.walk(dir)) {
            return all.filter(Files::isRegularFile).map(p -> dir.relativize(p).toString()).sorted().toList();
        }
    }

    @Test
    void reportsEveryBlueprintAndUploadsTheOnesAskedFor() throws IOException {
        Path scout = Blueprints.write(blueprints, "Scout Mk 2", "s", old());
        Blueprints.write(blueprints, "Hauler", "h", old());
        Blueprints.write(blueprints, "Fresh", "f", now.get() - 500);
        yard.plan = "{\"send\":[\"Scout Mk 2\",\"Fresh\",\"Unknown\"],\"fetch\":[]}";

        assertTrue(agent.round());
        JsonNode report = yard.reports.get(0);
        assertEquals("Test game", report.path("server").asText());
        Map<String, String> listed = new HashMap<>();
        for (JsonNode b : report.path("blueprints")) {
            listed.put(b.path("name").asText(), b.path("busy").asBoolean() ? "busy" : b.path("print").asText());
        }
        assertEquals(Map.of("Scout Mk 2", Fingerprint.of(scout), "Hauler", Fingerprint.of(blueprints.resolve("Hauler")), "Fresh", "busy"), listed);
        // Only the one that was asked for and can be read went up, whole and under its name.
        assertEquals(Map.of("Scout Mk 2", Fingerprint.of(scout)), yard.uploads);
        assertTrue(yard.tokens.stream().allMatch(TOKEN::equals), "every request carries the secret");
        assertFalse(String.join("\n", log).contains(TOKEN), "and the log never does");
        assertEquals(List.of(), filesUnder(work.resolve("staging")), "nothing is left in the staging folder");
    }

    @Test
    void doesNothingUntilTheServerLoopIsUp() throws IOException {
        Blueprints.write(blueprints, "Scout", "s", old());
        server.up = false;
        assertFalse(agent.round());
        assertEquals(0, yard.reports.size());
    }

    @Test
    void putsADesignIntoTheGameAndConfirmsIt() throws IOException {
        Path design = Blueprints.write(elsewhere, "Raider", "from the shipyard", old());
        String print = yard.offer("abc123", design, "Raider");
        yard.plan = fetchPlan("abc123", "Raider", print);

        assertTrue(agent.round());
        assertEquals(print, Fingerprint.of(blueprints.resolve("Raider")));
        assertEquals(List.of("Raider for Shipyard, open to others"), catalog.published);
        JsonNode done = yard.dones.get(0);
        assertEquals("abc123", done.path("design").asText());
        assertEquals("Raider", done.path("name").asText());
        assertEquals(print, done.path("design_print").asText());
        assertEquals(print, done.path("game_print").asText());
        assertEquals(List.of(), filesUnder(work.resolve("staging")));
        assertEquals(List.of(), filesUnder(work.resolve("replaced")), "nothing was replaced, so nothing was set aside");
    }

    @Test
    void aReplacedBlueprintIsKeptNotDeleted() throws IOException {
        Path mine = Blueprints.write(blueprints, "Raider", "saved in the game", old());
        String before = Fingerprint.of(mine);
        String print = yard.offer("abc123", Blueprints.write(elsewhere, "Raider", "edited in the shipyard", old()), "Raider");
        yard.plan = fetchPlan("abc123", "Raider", print);

        assertTrue(agent.round());
        assertEquals(print, Fingerprint.of(blueprints.resolve("Raider")));
        try (Stream<Path> kept = Files.list(work.resolve("replaced"))) {
            List<Path> versions = kept.toList();
            assertEquals(1, versions.size());
            assertEquals(before, Fingerprint.of(versions.get(0)));
        }
    }

    @Test
    void aDesignTheGameCannotReadIsTakenBackOutAndNotTriedAgain() throws IOException {
        Path mine = Blueprints.write(blueprints, "Raider", "saved in the game", old());
        String before = Fingerprint.of(mine);
        String print = yard.offer("abc123", Blueprints.write(elsewhere, "Raider", "broken", old()), "Raider");
        yard.plan = fetchPlan("abc123", "Raider", print);
        catalog.refuse = "Raider";

        assertFalse(agent.round());
        assertEquals(before, Fingerprint.of(blueprints.resolve("Raider")), "the game's own version is back in place");
        assertEquals(1, catalog.rescans);
        assertEquals(0, yard.dones.size());
        try (Stream<Path> rejected = Files.list(work.resolve("rejected"))) {
            assertEquals(print, Fingerprint.of(rejected.toList().get(0)));
        }
        // The same version is not downloaded a second time, however long the agent runs.
        now.addAndGet(3_600_000);
        Blueprints.age(blueprints.resolve("Raider"), old());
        agent.round();
        assertEquals(List.of("abc123"), yard.downloads);
        // A newer version of the design is tried.
        String newer = yard.offer("abc123", Blueprints.write(elsewhere, "Raider", "fixed", old()), "Raider");
        yard.plan = fetchPlan("abc123", "Raider", newer);
        catalog.refuse = null;
        assertTrue(agent.round());
        assertEquals(newer, Fingerprint.of(blueprints.resolve("Raider")));
    }

    @Test
    void designsTheGameMustNotTakeAreRefusedWithoutWritingAnything() throws IOException {
        Path design = Blueprints.write(elsewhere, "Raider", "x", old());
        String print = Fingerprint.of(design);
        Blueprints.write(blueprints, "scout", "the game's", old());
        String scoutBefore = Fingerprint.of(blueprints.resolve("scout"));

        // A name the game does not allow.
        yard.offer("d1", design, "Bad.Name");
        // A name the game has in other letter case.
        yard.offer("d2", design, "Scout");
        // A file whose folder is not called what the shipyard said.
        yard.offer("d3", design, "Somebody Else");
        // A file without a header.
        Path headless = Blueprints.write(elsewhere, "Headless", "x", old());
        Files.delete(headless.resolve("header.smbph"));
        String headlessPrint = yard.offer("d4", headless, "Headless");
        String[][] cases = {{"d1", "Bad.Name", print}, {"d2", "Scout", print}, {"d3", "Raider", print}, {"d4", "Headless", headlessPrint}};
        for (String[] c : cases) {
            yard.plan = fetchPlan(c[0], c[1], c[2]);
            assertFalse(agent.round(), c[0]);
            assertFalse(agent.round(), c[0] + " again");
        }
        assertEquals(List.of("scout"), filesUnder(blueprints).stream().map(f -> f.split("/")[0]).distinct().toList());
        assertEquals(scoutBefore, Fingerprint.of(blueprints.resolve("scout")));
        assertEquals(List.of(), catalog.published);
        assertEquals(0, yard.dones.size());
        // d1 and d2 are turned down before anything is downloaded; d3 and d4 once each.
        assertEquals(List.of("d3", "d4"), yard.downloads);
        assertEquals(4, log.stream().filter(l -> l.contains("not taking")).count());
    }

    @Test
    void aBadDownloadOrABusyServerIsTriedAgainLater() throws IOException {
        Path design = Blueprints.write(elsewhere, "Raider", "x", old());
        String print = yard.offer("abc123", design, "Raider");
        yard.plan = fetchPlan("abc123", "Raider", print);
        // The file does not have the fingerprint it is announced with.
        yard.designs.get("abc123")[1] = "0".repeat(64);
        assertFalse(agent.round());
        assertFalse(Files.exists(blueprints.resolve("Raider")));
        // Not at once...
        yard.designs.get("abc123")[1] = print;
        assertFalse(agent.round());
        assertEquals(1, yard.downloads.size());
        // ...but after a while. This time the server does not get to it.
        now.addAndGet(31_000);
        server.stuck = true;
        assertFalse(agent.round());
        assertFalse(Files.exists(blueprints.resolve("Raider")));
        assertEquals(List.of(), filesUnder(work.resolve("staging")));
        // And then it does.
        now.addAndGet(61_000);
        server.stuck = false;
        assertTrue(agent.round());
        assertEquals(print, Fingerprint.of(blueprints.resolve("Raider")));
        assertEquals(1, yard.dones.size());
    }

    @Test
    void aSaveInTheGameWhilePuttingADesignInPlaceWaits() throws IOException {
        Blueprints.write(blueprints, "Raider", "v1", old());
        // The game has begun saving over it: the previous version sits beside it.
        Blueprints.write(blueprints, "RaiderTmp", "v0", old());
        String print = yard.offer("abc123", Blueprints.write(elsewhere, "Raider", "x", old()), "Raider");
        yard.plan = fetchPlan("abc123", "Raider", print);
        String before = Fingerprint.of(blueprints.resolve("Raider"));
        assertFalse(agent.round());
        assertEquals(before, Fingerprint.of(blueprints.resolve("Raider")));
        assertEquals(List.of(), catalog.published);
        for (JsonNode b : yard.reports.get(0).path("blueprints")) assertTrue(b.path("busy").asBoolean());
    }

    @Test
    void anUploadTheShipyardTurnsDownIsNotCountedAsSent() throws IOException {
        Blueprints.write(blueprints, "Scout", "s", old());
        yard.plan = "{\"send\":[\"Scout\"],\"fetch\":[]}";
        yard.uploadStatus = 422;
        assertFalse(agent.round());
        assertTrue(log.stream().anyMatch(l -> l.contains("cannot read")));
        yard.uploadStatus = 500;
        assertFalse(agent.round());
        // After a server error the agent waits before it tries that blueprint again.
        yard.uploadStatus = 200;
        assertFalse(agent.round());
        now.addAndGet(31_000);
        Blueprints.age(blueprints.resolve("Scout"), old());
        assertTrue(agent.round());
        assertEquals(1, yard.uploads.size());
    }

    @Test
    void atMostAFewBlueprintsMovePerRound() throws IOException {
        StringBuilder names = new StringBuilder();
        for (int i = 0; i < 7; i++) {
            Blueprints.write(blueprints, "Ship " + i, "s" + i, old());
            names.append(i == 0 ? "" : ",").append("\"Ship ").append(i).append('"');
        }
        yard.plan = "{\"send\":[" + names + "],\"fetch\":[]}";
        assertTrue(agent.round());
        assertEquals(SyncAgent.SENDS_PER_ROUND, yard.uploads.size());
    }

    @Test
    void startsAndStopsCleanly() throws Exception {
        Blueprints.write(blueprints, "Scout", "s", old());
        agent.start();
        long deadline = System.currentTimeMillis() + 5_000;
        while (yard.reports.isEmpty() && System.currentTimeMillis() < deadline) Thread.sleep(20);
        assertEquals(1, yard.reports.size());
        agent.stop();
        Thread.sleep(100);
        assertFalse(Thread.getAllStackTraces().keySet().stream().anyMatch(t -> t.getName().equals("ShipyardHelper-sync") && t.isAlive()));
    }

    @Test
    void anUnreachableShipyardIsReportedOnceNotEveryRound() throws Exception {
        yard.http.stop(0);
        SyncAgent quick = new SyncAgent(new BlueprintFolder(blueprints), client, server, catalog,
                new SyncAgent.Settings("Test game", 1, "Shipyard", true), work, log::add, now::get);
        quick.start();
        Thread.sleep(2_600);
        quick.stop();
        assertEquals(1, log.stream().filter(l -> l.contains("no exchange with the shipyard")).count(), String.join("\n", log));
    }
}
