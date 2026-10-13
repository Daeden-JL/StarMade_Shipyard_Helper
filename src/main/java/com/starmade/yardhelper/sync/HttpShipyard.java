package com.starmade.yardhelper.sync;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.io.IOException;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

/** The shipyard over HTTP. */
public final class HttpShipyard implements Shipyard, AutoCloseable {
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final String TOKEN_HEADER = "x-yard-sync-token";
    private static final String PRINT_HEADER = "x-yard-print";
    private static final Duration SHORT = Duration.ofSeconds(30);
    /** Large blueprints over a slow link. */
    private static final Duration LONG = Duration.ofMinutes(10);

    private final HttpClient http;
    private final String base;
    private final String token;

    public HttpShipyard(String url, String token) {
        this.base = url.endsWith("/") ? url.substring(0, url.length() - 1) : url;
        this.token = token == null ? "" : token;
        this.http = HttpClient.newBuilder().version(HttpClient.Version.HTTP_1_1).connectTimeout(Duration.ofSeconds(5)).build();
    }

    private HttpRequest.Builder request(String path, Duration timeout) {
        HttpRequest.Builder b = HttpRequest.newBuilder(URI.create(base + path)).timeout(timeout);
        if (!token.isEmpty()) b.header(TOKEN_HEADER, token);
        return b;
    }

    private <T> HttpResponse<T> send(HttpRequest request, HttpResponse.BodyHandler<T> handler) throws IOException {
        try {
            return http.send(request, handler);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException("interrupted", e);
        }
    }

    private static IOException refused(String what, int status) {
        return new IOException(what + ": the shipyard answered HTTP " + status);
    }

    @Override
    public Plan report(String server, List<BlueprintFolder.Listed> blueprints, List<Reported> yards) throws IOException {
        ObjectNode body = JSON.createObjectNode();
        body.put("server", server);
        ArrayNode list = body.putArray("blueprints");
        for (BlueprintFolder.Listed l : blueprints) {
            ObjectNode one = list.addObject();
            one.put("name", l.name());
            one.put("print", l.print());
            one.put("busy", l.busy());
        }
        if (yards != null) {
            ArrayNode out = body.putArray("yards");
            for (Reported r : yards) {
                Yards.View v = r.view();
                ObjectNode one = out.addObject();
                one.put("id", v.id());
                one.put("station", v.station());
                ArrayNode at = one.putArray("at");
                for (int c : v.at()) at.add(c);
                one.put("state", v.state().name().toLowerCase(java.util.Locale.ROOT));
                one.put("in_use", v.inUse());
                one.put("room", v.room());
                ArrayNode designs = one.putArray("designs");
                for (Yards.Design d : v.designs()) designs.addObject().put("uid", d.uid()).put("name", d.name());
                one.put("loaded", v.loaded());
                one.put("loaded_print", r.loadedPrint());
            }
        }
        HttpRequest request = request("/api/sync/report", SHORT)
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(JSON.writeValueAsString(body), StandardCharsets.UTF_8))
                .build();
        HttpResponse<String> response = send(request, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
        if (response.statusCode() != 200) throw refused("report", response.statusCode());
        JsonNode plan = JSON.readTree(response.body());
        List<String> sends = new ArrayList<>();
        for (JsonNode name : plan.path("send")) sends.add(name.asText());
        List<Fetch> fetches = new ArrayList<>();
        for (JsonNode f : plan.path("fetch")) {
            fetches.add(new Fetch(f.path("design").asText(), f.path("name").asText(), f.path("print").asText()));
        }
        JsonNode y = plan.path("yards");
        List<String> watch = new ArrayList<>();
        for (JsonNode uid : y.path("watch")) watch.add(uid.asText());
        List<YardTake> takes = new ArrayList<>();
        for (JsonNode t : y.path("take")) takes.add(new YardTake(t.path("design").asText(), t.path("uid").asText(), t.path("print").asText()));
        List<YardGive> gives = new ArrayList<>();
        for (JsonNode g : y.path("give")) {
            gives.add(new YardGive(g.path("design").asText(), g.path("yard").asText(), text(g, "replace"), g.path("name").asText(),
                    g.path("blueprint").asText(), g.path("print").asText(), text(g, "expect")));
        }
        return new Plan(sends, fetches, new YardPlan(watch, takes, gives));
    }

    /** A text field, or null where it is missing or null. */
    private static String text(JsonNode node, String field) {
        JsonNode v = node.get(field);
        return v == null || v.isNull() ? null : v.asText();
    }

    @Override
    public int uploadYard(String design, String uid, String print, Path zip) throws IOException {
        String query = "?design=" + enc(design) + "&uid=" + enc(uid) + "&print=" + enc(print);
        HttpRequest request = request("/api/sync/yards/take" + query, LONG)
                .header("Content-Type", "application/zip")
                .PUT(HttpRequest.BodyPublishers.ofFile(zip))
                .build();
        return send(request, HttpResponse.BodyHandlers.discarding()).statusCode();
    }

    @Override
    public void yardGiven(String design, String uid, String print) throws IOException {
        ObjectNode body = JSON.createObjectNode();
        body.put("design", design);
        body.put("uid", uid);
        body.put("print", print);
        post("/api/sync/yards/given", body, "confirming the shipyard design of " + design);
    }

    @Override
    public void yardTrouble(String design, String why) throws IOException {
        ObjectNode body = JSON.createObjectNode();
        body.put("design", design);
        body.put("why", why);
        post("/api/sync/yards/trouble", body, "reporting on the shipyard design of " + design);
    }

    private void post(String path, ObjectNode body, String what) throws IOException {
        HttpRequest request = request(path, SHORT)
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(JSON.writeValueAsString(body), StandardCharsets.UTF_8))
                .build();
        int status = send(request, HttpResponse.BodyHandlers.discarding()).statusCode();
        if (status / 100 != 2) throw refused(what, status);
    }

    /** URLEncoder writes a space as '+', which is right for a query but easy to misread. */
    private static String enc(String text) {
        return URLEncoder.encode(text, StandardCharsets.UTF_8).replace("+", "%20");
    }

    @Override
    public int upload(String name, String print, Path zip) throws IOException {
        // URLEncoder writes a space as '+', which is right for a query but easy to misread.
        String query = "?name=" + URLEncoder.encode(name, StandardCharsets.UTF_8).replace("+", "%20") + "&print=" + print;
        HttpRequest request = request("/api/sync/blueprints" + query, LONG)
                .header("Content-Type", "application/zip")
                .PUT(HttpRequest.BodyPublishers.ofFile(zip))
                .build();
        return send(request, HttpResponse.BodyHandlers.discarding()).statusCode();
    }

    @Override
    public String download(String design, Path into) throws IOException {
        if (!design.matches("[a-z0-9]{1,64}")) throw new IOException("not a design id: " + design);
        HttpRequest request = request("/api/sync/designs/" + design + "/file", LONG).GET().build();
        HttpResponse<Path> response = send(request, HttpResponse.BodyHandlers.ofFile(into));
        if (response.statusCode() != 200) throw refused("download of design " + design, response.statusCode());
        return response.headers().firstValue(PRINT_HEADER).orElseThrow(() -> new IOException("the download carries no fingerprint"));
    }

    @Override
    public void done(String design, String name, String designPrint, String gamePrint) throws IOException {
        ObjectNode body = JSON.createObjectNode();
        body.put("design", design);
        body.put("name", name);
        body.put("design_print", designPrint);
        body.put("game_print", gamePrint);
        HttpRequest request = request("/api/sync/done", SHORT)
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(JSON.writeValueAsString(body), StandardCharsets.UTF_8))
                .build();
        int status = send(request, HttpResponse.BodyHandlers.discarding()).statusCode();
        if (status / 100 != 2) throw refused("confirming design " + design, status);
    }

    @Override
    public void close() {
        // Stops the client's own selector thread, which would otherwise outlive the mod.
        http.shutdownNow();
    }
}
