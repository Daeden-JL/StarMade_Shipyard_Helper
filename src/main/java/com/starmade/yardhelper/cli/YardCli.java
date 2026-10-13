package com.starmade.yardhelper.cli;

import com.starmade.yardhelper.YardConfig;
import com.starmade.yardhelper.YardService;
import com.starmade.yardhelper.agent.OpenAiClient;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockConfigParser;
import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.DirectGateway;
import com.starmade.yardhelper.world.InMemoryDesignWorld;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.concurrent.CountDownLatch;

/**
 * Offline harness: chat with the agent against an in-memory design. Same service, tools
 * and prompts as the mod, with no game running.
 *
 * <pre>
 *   --catalog DIR   directory holding BlockConfig.xml and BlockTypes.properties (required)
 *   --config FILE   config.properties to use (default: built-in defaults)
 *   --model NAME    override llm.model
 *   --url URL       override llm.baseUrl
 *   --effort LEVEL  override llm.reasoningEffort (none, low, medium, high)
 *   --prompt TEXT   run one request and exit instead of reading stdin
 * </pre>
 * Lines starting with ':' are harness commands: :slice [y], :side, :info, :undo, :reset, :quit.
 */
public final class YardCli {
    private static final String PLAYER = "cli";
    private static final String DESIGN = "cli-design";

    public static void main(String[] argv) throws Exception {
        Path catalogDir = null;
        Path configFile = null;
        String model = null, url = null, prompt = null, effort = null;
        for (int i = 0; i < argv.length - 1; i++) {
            switch (argv[i]) {
                case "--catalog" -> catalogDir = Paths.get(argv[++i]);
                case "--config" -> configFile = Paths.get(argv[++i]);
                case "--model" -> model = argv[++i];
                case "--url" -> url = argv[++i];
                case "--prompt" -> prompt = argv[++i];
                case "--effort" -> effort = argv[++i];
                default -> { }
            }
        }
        if (catalogDir == null) {
            System.err.println("Usage: --catalog <dir with BlockConfig.xml> [--config file] [--model name] [--url baseUrl] [--prompt text]");
            System.exit(2);
        }

        YardConfig config = configFile == null ? new YardConfig() : YardConfig.load(configFile);
        if (model != null) config.llmModel = model;
        if (url != null) config.llmBaseUrl = url;
        if (effort != null) config.llmReasoningEffort = effort;
        config.agentTrace = true;

        BlockCatalog catalog = BlockConfigParser.load(catalogDir);
        System.out.println("Catalog: " + catalog.size() + " blocks. Model: " + config.llmModel + " at " + config.llmBaseUrl);

        InMemoryDesignWorld world = InMemoryDesignWorld.withCore(BlockCatalog.CORE_ID, config.worldBuildHalfExtent);
        DirectGateway gateway = new DirectGateway(world);
        YardService service = new YardService(config,
                new OpenAiClient(config.llmBaseUrl, config.llmModel, config.llmApiKey, config.llmTemperature, config.llmTimeoutSeconds),
                () -> catalog);

        if (prompt != null) {
            ask(service, gateway, prompt);
            System.out.println(slice(world, catalog, 0));
            service.shutdown();
            return;
        }

        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        System.out.println("Describe what to build. :slice [y], :side, :info, :undo, :reset, :quit");
        String line;
        while (true) {
            System.out.print("> ");
            System.out.flush();
            line = in.readLine();
            if (line == null) break;
            line = line.trim();
            if (line.isEmpty()) continue;
            if (line.equals(":quit")) break;
            if (line.startsWith(":slice")) {
                String[] parts = line.split("\\s+");
                System.out.println(slice(world, catalog, parts.length > 1 ? Integer.parseInt(parts[1]) : 0));
            } else if (line.equals(":side")) {
                System.out.println(side(world));
            } else if (line.equals(":info")) {
                Box b = world.bounds();
                System.out.println(world.blockCount() + " blocks, bounds " + b);
            } else if (line.equals(":undo")) {
                waitFor(done -> service.undo(PLAYER, text -> { System.out.println(text); done.countDown(); }));
            } else if (line.equals(":reset")) {
                service.reset(PLAYER, System.out::println);
            } else {
                ask(service, gateway, line);
            }
        }
        service.shutdown();
    }

    /** Submits a request and blocks until the session is idle again. */
    private static void ask(YardService service, DirectGateway gateway, String text) throws InterruptedException {
        service.submit(PLAYER, DESIGN, text, () -> gateway, other -> false, msg -> System.out.println("[assistant] " + msg));
        while (true) {
            Thread.sleep(100);
            var session = service.sessions().forPlayer(PLAYER);
            if (session == null || !session.busy.get()) break;
        }
    }

    private interface Waitable {
        void start(CountDownLatch done);
    }

    private static void waitFor(Waitable w) throws InterruptedException {
        CountDownLatch done = new CountDownLatch(1);
        w.start(done);
        done.await();
    }

    /** Top-down map of one y layer: front (+z) at the top, right (+x) to the right. */
    static String slice(InMemoryDesignWorld world, BlockCatalog catalog, int y) {
        Box b = world.bounds();
        if (b == null) return "(empty)";
        StringBuilder sb = new StringBuilder("y=" + y + ", x " + b.min().x() + ".." + b.max().x()
                + ", z " + b.max().z() + " down to " + b.min().z() + "\n");
        for (int z = b.max().z(); z >= b.min().z(); z--) {
            for (int x = b.min().x(); x <= b.max().x(); x++) {
                Block blk = world.get(x, y, z);
                sb.append(blk.isAir() ? '.' : blk.type() == BlockCatalog.CORE_ID ? '@' : symbol(catalog, blk));
            }
            sb.append('\n');
        }
        return sb.toString();
    }

    /** Side silhouette: any block along x counts; front (+z) to the right, up at the top. */
    static String side(InMemoryDesignWorld world) {
        Box b = world.bounds();
        if (b == null) return "(empty)";
        boolean[][] filled = new boolean[b.sizeY()][b.sizeZ()];
        world.forEachBlock(b, (x, y, z, blk) -> filled[y - b.min().y()][z - b.min().z()] = true);
        StringBuilder sb = new StringBuilder("side view, z " + b.min().z() + ".." + b.max().z()
                + ", y " + b.max().y() + " down to " + b.min().y() + "\n");
        for (int y = b.sizeY() - 1; y >= 0; y--) {
            for (int z = 0; z < b.sizeZ(); z++) sb.append(filled[y][z] ? '#' : '.');
            sb.append('\n');
        }
        return sb.toString();
    }

    private static char symbol(BlockCatalog catalog, Block b) {
        String name = catalog.name(b.type());
        return name.isEmpty() ? '#' : name.charAt(0);
    }
}
