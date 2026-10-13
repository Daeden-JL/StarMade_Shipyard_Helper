package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.DesignWorld;
import com.starmade.yardhelper.world.Vec3;
import it.unimi.dsi.fastutil.longs.LongIterator;
import it.unimi.dsi.fastutil.shorts.Short2ObjectOpenHashMap;
import org.schema.game.common.controller.BlockProcessor;
import org.schema.game.common.controller.SegmentController;
import org.schema.game.common.controller.SendableSegmentController;
import org.schema.game.common.controller.Ship;
import org.schema.game.common.data.SegmentPiece;
import org.schema.game.common.data.VoidSegmentPiece;
import org.schema.game.common.data.element.ControlElementMap;
import org.schema.game.common.data.element.ElementCollection;
import org.schema.game.common.data.element.ElementKeyMap;
import org.schema.game.common.util.FastCopyLongOpenHashSet;
import org.schema.schine.graphicsengine.forms.BoundingBox;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;

/**
 * A shipyard design entity seen through {@link DesignWorld}. Design space puts the core at
 * the origin; the game stores it at {@link Ship#core}, so every position is shifted by that.
 *
 * <p>Tick thread only.
 */
public final class LiveDesignWorld implements DesignWorld {
    /** How block writes reach the game. */
    public enum WriteMode {
        /**
         * Queue the write where the server queues blocks placed by players, so the game's
         * own code creates segments, updates bounds and tells clients. Applied on the
         * entity's next update, not immediately.
         */
        PROCESSOR,
        /**
         * Write the segment directly. Immediate, but only works where a segment already
         * exists; other positions fall back to PROCESSOR.
         */
        DIRECT
    }

    private static final Field DELAYED_MODS = findDelayedMods();

    private final SendableSegmentController entity;
    private final int senderClientId;
    private final Box limit;
    private final WriteMode mode;
    private final int ox = Ship.core.x, oy = Ship.core.y, oz = Ship.core.z;
    private final SegmentPiece scratch = new SegmentPiece();

    /**
     * @param senderClientId client id of the player the edits are made for; the game checks
     *                       build permission against that player, exactly as if they had
     *                       placed the blocks themselves
     */
    public LiveDesignWorld(SendableSegmentController entity, int senderClientId, int halfExtent, WriteMode mode) {
        if (!entity.isVirtualBlueprint()) {
            throw new IllegalArgumentException("Refusing to edit a real entity: " + entity.getUniqueIdentifier());
        }
        this.entity = entity;
        this.senderClientId = senderClientId;
        this.limit = new Box(new Vec3(-halfExtent, -halfExtent, -halfExtent), new Vec3(halfExtent, halfExtent, halfExtent));
        this.mode = mode;
    }

    public SegmentController entity() {
        return entity;
    }

    @Override
    public Block get(int x, int y, int z) {
        SegmentPiece p = entity.getSegmentBuffer().getPointUnsave(x + ox, y + oy, z + oz, scratch);
        if (p == null || p.getType() == 0) return Block.AIR;
        return new Block(p.getType(), p.getOrientation(), p.isActive());
    }

    @Override
    public void set(int x, int y, int z, Block block) {
        guard();
        int ax = x + ox, ay = y + oy, az = z + oz;
        if (mode == WriteMode.DIRECT) {
            SegmentPiece p = entity.getSegmentBuffer().getPointUnsave(ax, ay, az);
            if (p != null && p.getSegment() != null && p.getSegment().getSegmentData() != null) {
                fill(p, block);
                p.applyToSegment(true);
                return;
            }
        }
        VoidSegmentPiece piece = new VoidSegmentPiece();
        piece.voidPos.set(ax, ay, az);
        fill(piece, block);
        piece.senderId = senderClientId;
        piece.controllerPos = Long.MIN_VALUE;
        delayedMods().add(piece);
    }

    /** Writes queued through the game's block processor that it has not applied yet. */
    public int pendingWrites() {
        return delayedMods().size();
    }

    @Override
    public Box bounds() {
        if (entity.getTotalElements() <= 0) return null;
        BoundingBox bb = entity.getSegmentBuffer().getBoundingBox();
        if (bb == null || !bb.isValid()) return null;
        // The box is in entity-local space with half-block margins around block centres.
        return new Box(
                new Vec3(Math.round(bb.min.x + 0.5f), Math.round(bb.min.y + 0.5f), Math.round(bb.min.z + 0.5f)),
                new Vec3(Math.round(bb.max.x - 0.5f), Math.round(bb.max.y - 0.5f), Math.round(bb.max.z - 0.5f)));
    }

    @Override
    public int blockCount() {
        return entity.getTotalElements();
    }

    @Override
    public Box buildLimit() {
        return limit;
    }

    @Override
    public void link(Vec3 controller, Vec3 target) {
        guard();
        short targetType = get(target.x(), target.y(), target.z()).type();
        if (targetType == 0) return;
        controlMap().addControllerForElement(index(controller), index(target), targetType);
    }

    @Override
    public void unlink(Vec3 controller, Vec3 target) {
        guard();
        short targetType = get(target.x(), target.y(), target.z()).type();
        if (targetType == 0) return;
        controlMap().removeControllerForElement(index(controller), index(target), targetType);
    }

    @Override
    public List<Vec3> linksFrom(Vec3 controller) {
        List<Vec3> out = new ArrayList<>();
        Short2ObjectOpenHashMap<FastCopyLongOpenHashSet> byType = controlMap().getControllingMap().get(index(controller));
        if (byType == null) return out;
        for (FastCopyLongOpenHashSet targets : byType.values()) {
            for (LongIterator it = targets.iterator(); it.hasNext(); ) {
                long idx = it.nextLong();
                out.add(new Vec3(ElementCollection.getPosX(idx) - ox, ElementCollection.getPosY(idx) - oy,
                        ElementCollection.getPosZ(idx) - oz));
            }
        }
        return out;
    }

    @Override
    public void forEachBlock(Box region, BlockVisitor visitor) {
        Box b = bounds();
        Box r = b == null ? null : region.intersect(b);
        if (r == null) return;
        for (int y = r.min().y(); y <= r.max().y(); y++) {
            for (int z = r.min().z(); z <= r.max().z(); z++) {
                for (int x = r.min().x(); x <= r.max().x(); x++) {
                    Block block = get(x, y, z);
                    if (!block.isAir()) visitor.visit(x, y, z, block);
                }
            }
        }
    }

    private static void fill(SegmentPiece p, Block block) {
        p.setType(block.type());
        p.setOrientation(block.orientation());
        p.setActive(block.active());
        if (block.type() != 0 && ElementKeyMap.isValidType(block.type())) {
            p.setHitpointsFull(ElementKeyMap.getInfoFast(block.type()).getMaxHitPointsFull());
        }
    }

    private long index(Vec3 p) {
        return ElementCollection.getIndex(p.x() + ox, p.y() + oy, p.z() + oz);
    }

    private ControlElementMap controlMap() {
        return entity.getControlElementMap();
    }

    /** Checked on every write: a design can be turned into a real ship while a session is open. */
    private void guard() {
        if (!entity.isVirtualBlueprint()) {
            throw new IllegalStateException("Entity is no longer a shipyard design");
        }
    }

    @SuppressWarnings("unchecked")
    private List<VoidSegmentPiece> delayedMods() {
        try {
            return (List<VoidSegmentPiece>) DELAYED_MODS.get(entity.getBlockProcessor());
        } catch (IllegalAccessException e) {
            throw new IllegalStateException(e);
        }
    }

    private static Field findDelayedMods() {
        try {
            Field f = BlockProcessor.class.getDeclaredField("delayedSegmentMods");
            f.setAccessible(true);
            return f;
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }
}
