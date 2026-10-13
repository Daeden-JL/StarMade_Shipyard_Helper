package com.starmade.yardhelper.mod;

import api.listener.Listener;
import api.listener.events.player.PlayerLeaveWorldEvent;
import api.mod.StarLoader;
import api.mod.StarMod;
import api.utils.StarRunnable;
import com.starmade.yardhelper.YardConfig;
import com.starmade.yardhelper.YardService;
import com.starmade.yardhelper.agent.OpenAiClient;
import com.starmade.yardhelper.sync.BlueprintFolder;
import com.starmade.yardhelper.sync.HttpShipyard;
import com.starmade.yardhelper.sync.SyncAgent;
import org.schema.game.server.controller.BluePrintController;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/** Mod entry point: wires the assistant into the server. */
public class ShipyardHelperMod extends StarMod {
    /** Matches the mod "name" in mod.json, so our files sit with StarLoader's own. */
    public static final String MODDATA_DIR = "moddata/StarMade_Shipyard_Helper";
    /** Server ticks between housekeeping runs. */
    private static final long HOUSEKEEPING_TICKS = 600;

    private YardService service;
    private SyncAgent sync;
    private HttpShipyard shipyard;

    @Override
    public void onEnable() {
        Path configFile = Paths.get(MODDATA_DIR, "config.properties");
        YardConfig config = YardConfig.load(configFile);
        if (!Files.exists(configFile)) {
            try {
                config.save(configFile);
            } catch (Exception e) {
                logWarning("Could not write default config: " + e.getMessage());
            }
        }

        TickPump pump = new TickPump(config.worldBlocksPerTick);
        service = new YardService(config,
                new OpenAiClient(config.llmBaseUrl, config.llmModel, config.llmApiKey, config.llmTemperature, config.llmTimeoutSeconds),
                GameCatalog::get);

        StarLoader.registerCommand(new YardCommand(this, service, pump, config));
        StarLoader.registerCommand(new YardDebugCommand(this, pump, config));

        StarLoader.registerListener(PlayerLeaveWorldEvent.class, new Listener<PlayerLeaveWorldEvent>() {
            @Override
            public void onEvent(PlayerLeaveWorldEvent event) {
                service.sessions().endForPlayer(event.getPlayerName());
            }
        }, this);

        new StarRunnable() {
            @Override
            public void run() {
                pump.tick();
            }
        }.runTimer(this, 1);

        final YardService svc = service;
        new StarRunnable() {
            @Override
            public void run() {
                try {
                    svc.expireIdleSessions();
                } catch (Throwable t) {
                    System.err.println("[Shipyard Helper] Housekeeping failed: " + t);
                }
            }
        }.runTimer(this, HOUSEKEEPING_TICKS);

        logInfo("Shipyard Helper enabled. Model " + config.llmModel + " at " + config.llmBaseUrl);
        startSync(config, pump);
    }

    /** Starts the exchange of blueprints with the web shipyard, if one is configured. */
    private void startSync(YardConfig config, TickPump pump) {
        if (config.syncUrl.isEmpty()) return;
        try {
            Path blueprints = Paths.get(BluePrintController.active.entityBluePrintPath).toAbsolutePath().normalize();
            // Next to the config: on the server's volume, and not a place the game scans.
            Path work = Paths.get(MODDATA_DIR, "sync").toAbsolutePath().normalize();
            shipyard = new HttpShipyard(config.syncUrl, config.syncToken);
            LiveYards yards = switch (config.syncShipyards) {
                case "all" -> new LiveYards(LiveYards.Allowed.ALL, config.syncOwner);
                case "off", "none", "false" -> null;
                default -> new LiveYards(LiveYards.Allowed.OWNER, config.syncOwner);
            };
            sync = new SyncAgent(new BlueprintFolder(blueprints), shipyard, new PumpThread(pump), new LiveCatalog(), yards,
                    new SyncAgent.Settings(config.syncServerName, config.syncIntervalSeconds, config.syncOwner, config.syncOthersCanBuy),
                    work, line -> System.err.println("[Shipyard Helper] " + line), System::currentTimeMillis);
            sync.start();
            logInfo("Blueprint sync on: " + blueprints + " with the shipyard at " + config.syncUrl
                    + " every " + config.syncIntervalSeconds + " s" + (config.syncToken.isEmpty() ? "" : ", with a secret")
                    + (yards == null ? ", shipyards left alone" : ", shipyards: " + config.syncShipyards));
        } catch (Throwable t) {
            // A bad address or anything else here must not keep the rest of the mod from loading.
            logWarning("Blueprint sync could not start: " + t);
        }
    }

    @Override
    public void onDisable() {
        // An Error escaping onDisable hangs server shutdown, so catch everything.
        try {
            if (sync != null) sync.stop();
            if (shipyard != null) shipyard.close();
        } catch (Throwable t) {
            System.err.println("[Shipyard Helper] Error stopping the blueprint sync: " + t);
        }
        try {
            if (service != null) service.shutdown();
        } catch (Throwable t) {
            System.err.println("[Shipyard Helper] Error during shutdown: " + t);
        }
    }
}
