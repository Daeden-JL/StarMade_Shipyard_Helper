package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.sync.SyncAgent;
import org.schema.game.common.data.player.catalog.CatalogManager;
import org.schema.game.common.data.player.catalog.CatalogPermission;
import org.schema.game.network.objects.NetworkGameState;
import org.schema.game.network.objects.remote.RemoteCatalogEntry;
import org.schema.game.server.controller.BluePrintController;
import org.schema.game.server.data.GameServerState;
import org.schema.game.server.data.blueprintnw.BlueprintEntry;

import java.lang.reflect.Method;

/**
 * The running game's blueprint catalog. The game reads its blueprint folder once at
 * start-up and after its own saves; a folder that appears or changes behind its back is
 * only seen when it is told to look again, and only shows to players once it has a
 * catalog entry. Every method here must be called on the server thread.
 */
final class LiveCatalog implements SyncAgent.Catalog {
    @Override
    public void rescan() {
        CatalogManager catalog = catalog();
        BluePrintController.active.setDirty(true);
        // Reads the folder again (only blueprints whose header, meta or logic file has
        // a new time are parsed) and rebuilds the catalog's table of what exists.
        catalog.refreshServerRawBlueprints();
    }

    @Override
    public String publish(String name, String owner, boolean othersCanBuy) throws Exception {
        CatalogManager catalog = catalog();
        rescan();
        // Throws if the game could not read the blueprint.
        BlueprintEntry entry = BluePrintController.active.getBlueprint(name);
        CatalogPermission existing = find(catalog, name);
        if (existing != null) {
            refresh(catalog, existing, entry);
            return "updated, owner " + existing.ownerUID;
        }
        // Not private: the server's own defaults for new blueprints apply first.
        catalog.addServerEntry(entry, owner, false);
        CatalogPermission added = find(catalog, name);
        if (added != null && othersCanBuy) {
            // Sent to clients after this tick, so the change is part of what they receive.
            added.setPermission(true, CatalogPermission.P_BUY_OTHERS);
        }
        return "new, owner " + owner;
    }

    private static CatalogManager catalog() {
        GameServerState state = GameServerState.instance;
        if (state == null || state.getCatalogManager() == null) throw new IllegalStateException("the game server is not up");
        return state.getCatalogManager();
    }

    private static CatalogPermission find(CatalogManager catalog, String name) {
        for (CatalogPermission p : catalog.getCatalog()) {
            if (p.getUid().equalsIgnoreCase(name)) return p;
        }
        return null;
    }

    /**
     * Brings an existing entry's price, mass and score up to the blueprint's new contents
     * and sends it to the players. The game's own method for this also resets the
     * description and unlocks the entry, so what the owner had set is put back.
     */
    private static void refresh(CatalogManager catalog, CatalogPermission entry, BlueprintEntry blueprint) {
        String description = entry.description;
        int permission = entry.permission;
        boolean done = false;
        try {
            Method update = CatalogManager.class.getDeclaredMethod("updateServerEntry", String.class, BlueprintEntry.class,
                    String.class, boolean.class, boolean.class);
            update.setAccessible(true);
            done = Boolean.TRUE.equals(update.invoke(catalog, entry.getUid(), blueprint, entry.ownerUID, false, true));
        } catch (ReflectiveOperationException | RuntimeException e) {
            System.err.println("[Shipyard Helper] Sync: the game's own catalog update could not be used (" + e + "); updating the entry directly.");
        } finally {
            entry.description = description;
            entry.permission = permission;
        }
        if (!done) {
            entry.price = blueprint.getPrice();
            entry.mass = (float) blueprint.getMass();
            entry.score = blueprint.getScore();
            if (CatalogManager.isValidCatalogType(blueprint.getEntityType())) {
                NetworkGameState net = GameServerState.instance.getGameState().getNetworkObject();
                net.catalogBuffer.add(new RemoteCatalogEntry(entry, net));
            }
        }
    }
}
