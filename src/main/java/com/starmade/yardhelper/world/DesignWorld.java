package com.starmade.yardhelper.world;

import java.util.List;

/**
 * One editable design entity (a ship or turret inside a shipyard design), in design space
 * (see {@link Vec3}). Implementations are not thread safe: call them only from the thread
 * that owns the design, which is what {@link WorldGateway} is for.
 */
public interface DesignWorld {
    Block get(int x, int y, int z);

    /** Writes a block; {@link Block#AIR} removes. */
    void set(int x, int y, int z, Block block);

    /** Bounds of all non-empty blocks, or null when the design is empty. */
    Box bounds();

    int blockCount();

    /** Region the agent is allowed to build in. */
    Box buildLimit();

    /** Links a controller block (computer, logic block...) to a target block. */
    void link(Vec3 controller, Vec3 target);

    void unlink(Vec3 controller, Vec3 target);

    List<Vec3> linksFrom(Vec3 controller);

    /** Visits every non-empty block inside the region. */
    void forEachBlock(Box region, BlockVisitor visitor);

    interface BlockVisitor {
        void visit(int x, int y, int z, Block block);
    }
}
