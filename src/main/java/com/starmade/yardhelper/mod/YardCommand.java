package com.starmade.yardhelper.mod;

import api.mod.StarMod;
import api.utils.game.PlayerUtils;
import api.utils.game.chat.CommandInterface;
import com.starmade.yardhelper.YardConfig;
import com.starmade.yardhelper.YardService;
import org.schema.game.common.controller.SegmentController;
import org.schema.game.common.controller.SendableSegmentController;
import org.schema.game.common.data.player.PlayerState;
import org.schema.game.common.data.world.SimpleTransformableSendableObject;
import org.schema.game.server.data.GameServerState;

import java.util.ArrayList;
import java.util.List;

/** The /yard chat command: the player's way of talking to the assistant. */
final class YardCommand implements CommandInterface {
    private static final int CHAT_LINE = 220;

    private final StarMod mod;
    private final YardService service;
    private final TickPump pump;
    private final YardConfig config;

    YardCommand(StarMod mod, YardService service, TickPump pump, YardConfig config) {
        this.mod = mod;
        this.service = service;
        this.pump = pump;
        this.config = config;
    }

    @Override
    public String getCommand() {
        return "yard";
    }

    @Override
    public String[] getAliases() {
        return new String[]{"yard"};
    }

    @Override
    public String getDescription() {
        return "Shipyard assistant. While editing a shipyard design:\n"
                + "/yard <what to build or change>\n"
                + "/yard undo - take back the last request\n"
                + "/yard stop - cancel the running request\n"
                + "/yard status | reset | help";
    }

    @Override
    public boolean isAdminOnly() {
        return false;
    }

    @Override
    public boolean onCommand(PlayerState player, String[] args) {
        try {
            handle(player, args);
        } catch (Throwable t) {
            // A command handler must never throw into the server's packet handling.
            System.err.println("[Shipyard Helper] /yard failed: " + t);
            t.printStackTrace();
            PlayerUtils.sendMessage(player, "The assistant hit an internal error.");
        }
        return true;
    }

    @Override
    public void serverAction(PlayerState player, String[] args) {
        // Unused by StarLoader; onCommand already runs on the server.
    }

    @Override
    public StarMod getMod() {
        return mod;
    }

    private void handle(PlayerState player, String[] args) {
        String name = player.getName();
        YardService.Messenger out = messenger(name);
        String text = String.join(" ", args).trim();
        switch (text.toLowerCase()) {
            case "", "help" -> { for (String line : getDescription().split("\n")) out.send(line); return; }
            case "stop" -> { service.stop(name, out); return; }
            case "undo" -> { service.undo(name, out); return; }
            case "status" -> { service.status(name, out); return; }
            case "reset" -> { service.reset(name, out); return; }
            default -> { }
        }

        SendableSegmentController design = designOf(player);
        if (design == null) {
            out.send("Enter a shipyard design in build mode first. The assistant only edits shipyard designs.");
            return;
        }
        int clientId = player.getClientId();
        LiveDesignWorld.WriteMode mode = "direct".equalsIgnoreCase(config.worldWriteMode)
                ? LiveDesignWorld.WriteMode.DIRECT : LiveDesignWorld.WriteMode.PROCESSOR;
        service.submit(name, design.getUniqueIdentifier(), text,
                () -> new LiveGateway(new LiveDesignWorld(design, clientId, config.worldBuildHalfExtent, mode), name, pump),
                other -> isInside(other, design),
                out);
    }

    /** The shipyard design the player is currently editing, or null. */
    static SendableSegmentController designOf(PlayerState player) {
        SimpleTransformableSendableObject<?> controlled = player.getFirstControlledTransformableWOExc();
        if (controlled instanceof SendableSegmentController sc && sc.isVirtualBlueprint()) return sc;
        return null;
    }

    private static boolean isInside(String playerName, SegmentController design) {
        GameServerState state = GameServerState.instance;
        PlayerState other = state == null ? null : state.getPlayerStatesByName().get(playerName);
        return other != null && designOf(other) == design;
    }

    /**
     * Chat lines for one player, safe to call from worker threads: sending is handed to the
     * tick thread, and long replies are split into chat-sized lines.
     */
    private YardService.Messenger messenger(String playerName) {
        return text -> {
            List<String> lines = wrap(text);
            pump.submit(budget -> {
                GameServerState state = GameServerState.instance;
                PlayerState target = state == null ? null : state.getPlayerStatesByName().get(playerName);
                if (target != null) {
                    for (String line : lines) PlayerUtils.sendMessage(target, "[Yard] " + line);
                }
                return -1;
            });
        };
    }

    static List<String> wrap(String text) {
        List<String> lines = new ArrayList<>();
        for (String paragraph : text.replace("**", "").split("\\n+")) {
            String rest = paragraph.trim();
            while (rest.length() > CHAT_LINE) {
                int cut = rest.lastIndexOf(' ', CHAT_LINE);
                if (cut < CHAT_LINE / 2) cut = CHAT_LINE;
                lines.add(rest.substring(0, cut).trim());
                rest = rest.substring(cut).trim();
            }
            if (!rest.isEmpty()) lines.add(rest);
        }
        return lines;
    }
}
