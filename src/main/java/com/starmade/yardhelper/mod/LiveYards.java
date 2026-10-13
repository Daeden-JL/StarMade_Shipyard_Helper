package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.sync.Yards;
import org.schema.common.util.linAlg.Vector3i;
import org.schema.game.common.controller.SegmentController;
import org.schema.game.common.controller.elements.ShipyardManagerContainerInterface;
import org.schema.game.common.controller.elements.shipyard.ShipyardCollectionManager;
import org.schema.game.common.controller.elements.shipyard.ShipyardCollectionManager.ShipyardCommandType;
import org.schema.game.common.controller.elements.shipyard.orders.states.DesignLoaded;
import org.schema.game.common.controller.elements.shipyard.orders.states.ShipyardState;
import org.schema.game.common.controller.elements.shipyard.orders.states.WaitingForShipyardOrder;
import org.schema.game.common.controller.rails.RailRelation;
import org.schema.game.common.data.ManagedSegmentController;
import org.schema.game.common.data.element.meta.VirtualBlueprintMetaItem;
import org.schema.game.common.data.player.PlayerState;
import org.schema.game.common.data.player.faction.Faction;
import org.schema.game.common.data.world.SimpleTransformableSendableObject;
import org.schema.game.server.controller.BluePrintController;
import org.schema.game.server.data.GameServerState;
import org.schema.game.server.data.blueprintnw.BlueprintClassification;
import org.schema.schine.network.objects.Sendable;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * The running game's shipyards. Only shipyards on stations the game has loaded are seen,
 * and of those only the ones the configuration allows: by default, stations of the faction
 * the sync's owner belongs to. The game itself checks no permissions for shipyard orders
 * given on the server. Every method here must be called on the server thread.
 */
final class LiveYards implements Yards {
    private static final boolean DEBUG = Boolean.getBoolean("yard.debug");

    /** Which stations' shipyards may be used. */
    enum Allowed { OWNER, ALL }

    private final Allowed allowed;
    private final String owner;

    LiveYards(Allowed allowed, String owner) {
        this.allowed = allowed;
        this.owner = owner;
    }

    private static GameServerState state() {
        GameServerState state = GameServerState.instance;
        if (state == null) throw new IllegalStateException("the game server is not up");
        return state;
    }

    /** The owner's faction, or 0 if they are in none. */
    private int ownerFaction() {
        for (Faction f : state().getFactionManager().getFactionCollection()) {
            for (String member : f.getMembersUID().keySet()) {
                if (member.equalsIgnoreCase(owner)) return f.getIdFaction();
            }
        }
        return 0;
    }

    private boolean allowed(SegmentController station, int faction) {
        return allowed == Allowed.ALL || (faction != 0 && station.getFactionId() == faction);
    }

    private record Found(SegmentController station, ShipyardCollectionManager yard) {
    }

    private List<Found> all() {
        List<Found> out = new ArrayList<>();
        int faction = allowed == Allowed.OWNER ? ownerFaction() : 0;
        StringBuilder seen = new StringBuilder();
        for (Sendable s : state().getLocalAndRemoteObjectContainer().getLocalObjects().values()) {
            if (!(s instanceof SegmentController station) || !(s instanceof ManagedSegmentController<?> managed)) continue;
            if (!(managed.getManagerContainer() instanceof ShipyardManagerContainerInterface holder)) continue;
            List<ShipyardCollectionManager> yards = holder.getShipyard().getCollectionManagers();
            if (yards.isEmpty() && DEBUG && station.getTotalElements() == 0 && requested.add(station.getUniqueIdentifier())) requestAll(station);
            if (station.isVirtualBlueprint()) continue;
            if (yards.isEmpty()) {
                if (DEBUG) seen.append(seen.length() == 0 ? "" : ", ").append("[no shipyard: ").append(station.getRealName()).append(' ')
                        .append(station.getTotalElements()).append(" blocks, loaded ").append(station.isFullyLoaded()).append(']');
                continue;
            }
            boolean ok = allowed(station, faction);
            seen.append(seen.length() == 0 ? "" : ", ").append('"').append(station.getRealName()).append("\" ").append(yards.size())
                    .append(ok ? "" : " (not the owner's faction)");
            if (!ok) continue;
            for (ShipyardCollectionManager yard : yards) out.add(new Found(station, yard));
        }
        String note = seen.length() == 0 ? "none" : seen.toString();
        if (!note.equals(lastSeen)) {
            lastSeen = note;
            System.err.println("[Shipyard Helper] Sync: shipyards on loaded stations: " + note);
        }
        return out;
    }

    /** Stations whose blocks were asked for, with -Dyard.debug=true. */
    private final java.util.Set<String> requested = new java.util.HashSet<>();

    /**
     * For tests without a game client: the game only reads a station's blocks once a player
     * is near it, and without its blocks a station has no shipyard. Asks for all of them.
     */
    private static void requestAll(SegmentController station) {
        Vector3i min = station.getMinPos(), max = station.getMaxPos();
        int n = 0;
        for (int x = min.x; x <= max.x; x++) {
            for (int y = min.y; y <= max.y; y++) {
                for (int z = min.z; z <= max.z; z++) {
                    station.getSegmentProvider().enqueueHightPrio(x * 32, y * 32, z * 32, true);
                    n++;
                }
            }
        }
        System.err.println("[Shipyard Helper] Sync debug: asked for " + n + " parts of " + station.getRealName());
    }

    /** What the last look at the stations found, so a change is logged once. */
    private String lastSeen = "";

    private static String id(Found f) {
        return f.station().getUniqueIdentifier() + "@" + f.yard().getControllerIndex();
    }

    private Found find(String id) {
        for (Found f : all()) {
            if (id(f).equals(id)) return f;
        }
        throw new IllegalStateException("no such shipyard on a loaded station: " + id);
    }

    /** A part made this recently may still be waiting for parts to dock to it. */
    private static final long DOCKING_MILLIS = 30_000;

    /**
     * Whether every part of a design is there, so a copy is complete. A copy is taken from
     * the design's files after the game has written the parts it holds in memory, so parts
     * of the design not in memory do not matter; what matters is that no docked part is
     * still to come. The game's own "fully loaded" flag is not used: it counts the parts read
     * since the design was loaded against a total saved with it, and for a large design read
     * from disk it can stay false for good, which would stop the design being copied until it
     * was unloaded and loaded again.
     */
    private boolean settled(List<SegmentController> parts) {
        long now = state().getUpdateTime();
        for (SegmentController part : parts) {
            if (!part.railController.getExpectedToDock().isEmpty() && now - part.getTimeCreated() < DOCKING_MILLIS) return false;
        }
        return true;
    }

    /** A design and everything docked to it, however deep. */
    private static List<SegmentController> withDocked(SegmentController root) {
        List<SegmentController> out = new ArrayList<>();
        out.add(root);
        for (int i = 0; i < out.size(); i++) {
            for (RailRelation r : out.get(i).railController.next) out.add(r.docked.getSegmentController());
        }
        return out;
    }

    @Override
    public List<View> list() {
        List<View> out = new ArrayList<>();
        List<SegmentController> controlled = new ArrayList<>();
        for (PlayerState p : state().getPlayerStatesByName().values()) {
            SimpleTransformableSendableObject<?> c = p.getFirstControlledTransformableWOExc();
            if (c instanceof SegmentController sc) controlled.add(sc);
        }
        for (Found f : all()) {
            ShipyardCollectionManager yard = f.yard();
            ShipyardState now = yard.getCurrentStateServer();
            SegmentController docked = yard.getCurrentDocked();
            VirtualBlueprintMetaItem current = yard.getCurrentDesignObject();
            State state;
            if (now instanceof WaitingForShipyardOrder && docked == null) state = State.IDLE;
            else if (now instanceof DesignLoaded && docked != null && current != null && docked.isVirtualBlueprint()) state = State.LOADED;
            else state = State.BUSY;
            List<Design> designs = new ArrayList<>();
            for (VirtualBlueprintMetaItem m : yard.getDesignList()) designs.add(new Design(m.UID, m.virtualName));
            String loaded = null;
            long changedAt = 0;
            boolean fullyLoaded = false;
            boolean inUse = false;
            if (state == State.LOADED) {
                loaded = docked.getUniqueIdentifier();
                List<SegmentController> parts = withDocked(docked);
                fullyLoaded = settled(parts);
                for (SegmentController part : parts) {
                    changedAt = Math.max(changedAt, part.getSegmentBuffer().getLastBufferChanged());
                    inUse |= controlled.contains(part);
                }
            }
            Vector3i at = yard.getControllerPos();
            out.add(new View(id(f), f.station().getRealName(), new int[]{at.x, at.y, at.z}, state, inUse,
                    yard.getInventory().hasFreeSlot(), designs, loaded, changedAt, fullyLoaded));
        }
        return out;
    }

    @Override
    public void copy(String yard, String uid, Path into, String name) throws Exception {
        SegmentController docked = find(yard).yard().getCurrentDocked();
        if (docked == null || !docked.getUniqueIdentifier().equals(uid)) throw new IllegalStateException(uid + " is not loaded");
        BluePrintController writer = new BluePrintController(into.toString(), into.resolve("DATA").toString());
        writer.writeBluePrint(docked, name, false, BlueprintClassification.NONE);
    }

    private static void order(Found f, ShipyardCommandType type, Object... args) {
        f.yard().handleShipyardCommandOnServer(f.station().getFactionId(), type, args);
    }

    @Override
    public void unload(String yard) {
        order(find(yard), ShipyardCommandType.UNLOAD_DESIGN);
    }

    @Override
    public void load(String yard, String uid) {
        Found f = find(yard);
        for (VirtualBlueprintMetaItem m : f.yard().getDesignList()) {
            if (m.UID.equals(uid)) {
                order(f, ShipyardCommandType.LOAD_DESIGN, Integer.valueOf(m.getId()));
                return;
            }
        }
        throw new IllegalStateException("the shipyard does not hold " + uid);
    }

    @Override
    public void create(String yard, String blueprint, String name) {
        order(find(yard), ShipyardCommandType.CATALOG_TO_DESIGN, name, blueprint);
    }

    @Override
    public void remove(String yard, String uid) {
        Found f = find(yard);
        for (VirtualBlueprintMetaItem m : f.yard().getDesignList()) {
            if (m.UID.equals(uid)) {
                // Deletes the design's entity along with the item.
                f.yard().getInventory().removeMetaItem(m);
                return;
            }
        }
    }
}
