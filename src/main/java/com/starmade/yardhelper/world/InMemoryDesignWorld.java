package com.starmade.yardhelper.world;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** Pure-Java design used by unit tests and the offline harness. */
public final class InMemoryDesignWorld implements DesignWorld {
    private final Map<Vec3, Block> blocks = new HashMap<>();
    private final Map<Vec3, Set<Vec3>> links = new HashMap<>();
    private final Box limit;

    public InMemoryDesignWorld(Box limit) {
        this.limit = limit;
    }

    /** A design holding only a ship core, like a freshly created shipyard design. */
    public static InMemoryDesignWorld withCore(short coreType, int halfExtent) {
        InMemoryDesignWorld w = new InMemoryDesignWorld(
                new Box(new Vec3(-halfExtent, -halfExtent, -halfExtent), new Vec3(halfExtent, halfExtent, halfExtent)));
        w.set(0, 0, 0, new Block(coreType, (byte) 0, false));
        return w;
    }

    @Override
    public Block get(int x, int y, int z) {
        return blocks.getOrDefault(new Vec3(x, y, z), Block.AIR);
    }

    @Override
    public void set(int x, int y, int z, Block block) {
        Vec3 p = new Vec3(x, y, z);
        if (block.isAir()) {
            blocks.remove(p);
            // The game drops links when either end is removed.
            links.remove(p);
            links.values().forEach(targets -> targets.remove(p));
        } else {
            blocks.put(p, block);
        }
    }

    @Override
    public Box bounds() {
        if (blocks.isEmpty()) return null;
        int x0 = Integer.MAX_VALUE, y0 = Integer.MAX_VALUE, z0 = Integer.MAX_VALUE;
        int x1 = Integer.MIN_VALUE, y1 = Integer.MIN_VALUE, z1 = Integer.MIN_VALUE;
        for (Vec3 p : blocks.keySet()) {
            x0 = Math.min(x0, p.x()); y0 = Math.min(y0, p.y()); z0 = Math.min(z0, p.z());
            x1 = Math.max(x1, p.x()); y1 = Math.max(y1, p.y()); z1 = Math.max(z1, p.z());
        }
        return new Box(new Vec3(x0, y0, z0), new Vec3(x1, y1, z1));
    }

    @Override
    public int blockCount() {
        return blocks.size();
    }

    @Override
    public Box buildLimit() {
        return limit;
    }

    @Override
    public void link(Vec3 controller, Vec3 target) {
        links.computeIfAbsent(controller, k -> new LinkedHashSet<>()).add(target);
    }

    @Override
    public void unlink(Vec3 controller, Vec3 target) {
        Set<Vec3> targets = links.get(controller);
        if (targets != null) targets.remove(target);
    }

    @Override
    public List<Vec3> linksFrom(Vec3 controller) {
        Set<Vec3> targets = links.get(controller);
        return targets == null ? List.of() : new ArrayList<>(targets);
    }

    @Override
    public void forEachBlock(Box region, BlockVisitor visitor) {
        if (region.volume() < blocks.size()) {
            for (int y = region.min().y(); y <= region.max().y(); y++)
                for (int z = region.min().z(); z <= region.max().z(); z++)
                    for (int x = region.min().x(); x <= region.max().x(); x++) {
                        Block b = blocks.get(new Vec3(x, y, z));
                        if (b != null) visitor.visit(x, y, z, b);
                    }
        } else {
            for (Map.Entry<Vec3, Block> e : blocks.entrySet()) {
                Vec3 p = e.getKey();
                if (region.contains(p)) visitor.visit(p.x(), p.y(), p.z(), e.getValue());
            }
        }
    }
}
