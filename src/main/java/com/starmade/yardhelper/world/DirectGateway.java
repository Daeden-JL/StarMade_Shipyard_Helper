package com.starmade.yardhelper.world;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

/** Gateway for designs with no owning thread (in-memory worlds). */
public final class DirectGateway implements WorldGateway {
    private final DesignWorld world;

    public DirectGateway(DesignWorld world) {
        this.world = world;
    }

    @Override
    public synchronized <T> T call(Function<DesignWorld, T> operation) {
        return operation.apply(world);
    }

    @Override
    public synchronized List<Block> apply(List<BlockWrite> writes, boolean onlyEmpty) {
        List<Block> before = new ArrayList<>(writes.size());
        for (BlockWrite w : writes) {
            Vec3 p = w.pos();
            Block was = world.get(p.x(), p.y(), p.z());
            before.add(was);
            if (onlyEmpty && !was.isAir()) continue;
            world.set(p.x(), p.y(), p.z(), w.block());
        }
        return before;
    }
}
