package com.starmade.yardhelper.mod;

import api.mod.StarMod;
import api.utils.game.PlayerUtils;
import api.utils.game.chat.CommandInterface;
import com.starmade.yardhelper.YardConfig;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.shape.Shapes;
import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Editor;
import com.starmade.yardhelper.world.Journal;
import com.starmade.yardhelper.world.Vec3;
import org.schema.game.common.controller.SendableSegmentController;
import org.schema.game.common.data.player.PlayerState;
import org.schema.game.server.data.GameServerState;

import java.util.ArrayList;
import java.util.List;

/**
 * Admin-only checks of the things that can only be confirmed in a running game: that
 * blocks and links placed by the mod show up on clients, how fast bulk placement is, which
 * way the axes point and what each orientation value looks like. No model involved.
 */
final class YardDebugCommand implements CommandInterface {
    private final StarMod mod;
    private final TickPump pump;
    private final YardConfig config;

    YardDebugCommand(StarMod mod, TickPump pump, YardConfig config) {
        this.mod = mod;
        this.pump = pump;
        this.config = config;
    }

    @Override
    public String getCommand() {
        return "yard_debug";
    }

    @Override
    public String[] getAliases() {
        return new String[]{"yard_debug"};
    }

    @Override
    public String getDescription() {
        return "Shipyard assistant self-checks (inside a shipyard design):\n"
                + "/yard_debug where - what the mod sees\n"
                + "/yard_debug axes - red +X, green +Y, blue +Z markers\n"
                + "/yard_debug cube <processor|direct> <size> - timed bulk placement\n"
                + "/yard_debug link - cannon computer linked to three barrels\n"
                + "/yard_debug orient <block name or id> - one block per orientation value\n"
                + "/yard_debug clear - remove everything but the core";
    }

    @Override
    public boolean isAdminOnly() {
        return true;
    }

    @Override
    public void serverAction(PlayerState player, String[] args) {
    }

    @Override
    public StarMod getMod() {
        return mod;
    }

    @Override
    public boolean onCommand(PlayerState player, String[] args) {
        String name = player.getName();
        try {
            SendableSegmentController design = YardCommand.designOf(player);
            if (design == null) {
                PlayerUtils.sendMessage(player, "[Yard] Not inside a shipyard design.");
                return true;
            }
            String sub = args.length == 0 ? "where" : args[0].toLowerCase();
            LiveDesignWorld.WriteMode mode = args.length > 1 && args[1].equalsIgnoreCase("direct")
                    ? LiveDesignWorld.WriteMode.DIRECT : LiveDesignWorld.WriteMode.PROCESSOR;
            LiveDesignWorld world = new LiveDesignWorld(design, player.getClientId(), config.worldBuildHalfExtent, mode);
            LiveGateway gateway = new LiveGateway(world, name, pump);
            BlockCatalog catalog = GameCatalog.get();
            Editor editor = new Editor(gateway, catalog, new Journal(1), 2_000_000);
            String[] rest = args;

            // Gateway calls block until the tick thread has run them, so never from here.
            Thread worker = new Thread(() -> {
                try {
                    say(name, run(sub, rest, design, world, gateway, editor, catalog));
                } catch (Throwable t) {
                    t.printStackTrace();
                    say(name, "Failed: " + t);
                }
            }, "ShipyardHelper-debug");
            worker.setDaemon(true);
            worker.start();
        } catch (Throwable t) {
            t.printStackTrace();
            PlayerUtils.sendMessage(player, "[Yard] Failed: " + t);
        }
        return true;
    }

    private String run(String sub, String[] args, SendableSegmentController design, LiveDesignWorld world,
                       LiveGateway gateway, Editor editor, BlockCatalog catalog) {
        switch (sub) {
            case "where": {
                return gateway.call(w -> "Design " + design.getUniqueIdentifier() + ", " + w.blockCount()
                        + " blocks, bounds " + w.bounds() + ", block at core position: "
                        + catalog.name(w.get(0, 0, 0).type()) + ", catalog " + catalog.size() + " blocks");
            }
            case "axes": {
                place(editor, catalog, "Red Standard Armor", Shapes.line(new Vec3(2, 0, 0), new Vec3(6, 0, 0)));
                place(editor, catalog, "Green Standard Armor", Shapes.line(new Vec3(0, 2, 0), new Vec3(0, 6, 0)));
                place(editor, catalog, "Blue Standard Armor", Shapes.line(new Vec3(0, 0, 2), new Vec3(0, 0, 6)));
                return "Placed markers: red should point right (+X), green up (+Y), blue forward (+Z).";
            }
            case "cube": {
                int size = args.length > 2 ? Integer.parseInt(args[2]) : 10;
                List<Vec3> positions = Shapes.box(Box.of(new Vec3(8, 0, 0), new Vec3(7 + size, size - 1, size - 1)),
                        Shapes.Fill.SOLID, 2_000_000);
                long start = System.currentTimeMillis();
                Editor.Result r = place(editor, catalog, "Grey Standard Armor", positions);
                long ms = System.currentTimeMillis() - start;
                int after = gateway.call(w -> w.blockCount());
                return r + " in " + ms + " ms (" + (ms == 0 ? "-" : positions.size() * 1000L / ms)
                        + " blocks/s). Design now has " + after + " blocks. Check they are visible without relogging.";
            }
            case "link": {
                place(editor, catalog, "Cannon Computer", List.of(new Vec3(0, 3, 1)));
                place(editor, catalog, "Cannon Barrel", Shapes.line(new Vec3(0, 3, 2), new Vec3(0, 3, 4)));
                String linked = editor.link(new Vec3(0, 3, 1), Shapes.line(new Vec3(0, 3, 2), new Vec3(0, 3, 4)));
                int seen = gateway.call(w -> w.linksFrom(new Vec3(0, 3, 1)).size());
                return linked + " Server now reports " + seen + " links. Select the computer in build mode to check the client shows them.";
            }
            case "orient": {
                if (args.length < 2) return "Usage: /yard_debug orient <block name or id>";
                String query = String.join(" ", java.util.Arrays.copyOfRange(args, 1, args.length));
                BlockDef def = query.matches("\\d+") ? catalog.get(Integer.parseInt(query)) : catalog.byName(query);
                if (def == null) return "No block named \"" + query + "\".";
                // Four rows of six, two apart, above the core: orientation n is at
                // x = 2 * (n % 6), z = 2 * (n / 6), y = 8.
                for (int o = 0; o < 24; o++) {
                    editor.place(List.of(new Vec3(2 * (o % 6), 8, 2 * (o / 6))), new Block(def.id(), (byte) o, false), false);
                }
                return "Placed " + def.name() + " with orientations 0..23 at y=8: x = 2*(n%6), z = 2*(n/6).";
            }
            case "clear": {
                Box b = gateway.call(w -> w.bounds());
                if (b == null) return "Nothing to clear.";
                List<Vec3> all = new ArrayList<>();
                gateway.call(w -> {
                    w.forEachBlock(b, (x, y, z, block) -> all.add(new Vec3(x, y, z)));
                    return null;
                });
                return editor.remove(all).changed() + " blocks removed.";
            }
            default:
                return "Unknown check. " + getDescription();
        }
    }

    private static Editor.Result place(Editor editor, BlockCatalog catalog, String blockName, List<Vec3> positions) {
        BlockDef def = catalog.byName(blockName);
        if (def == null) throw new IllegalStateException("Catalog has no block named " + blockName);
        return editor.place(positions, Block.of(def.id()), false);
    }

    private void say(String playerName, String text) {
        pump.submit(budget -> {
            GameServerState state = GameServerState.instance;
            PlayerState target = state == null ? null : state.getPlayerStatesByName().get(playerName);
            if (target != null) {
                for (String line : YardCommand.wrap(text)) PlayerUtils.sendMessage(target, "[Yard] " + line);
            }
            return -1;
        });
    }
}
