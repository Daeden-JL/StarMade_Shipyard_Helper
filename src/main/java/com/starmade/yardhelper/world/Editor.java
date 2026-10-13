package com.starmade.yardhelper.world;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.catalog.BlockShape;
import com.starmade.yardhelper.shape.Shapes;

import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

/**
 * Everything that changes a design goes through here: bounds and block checks, symmetry,
 * and the undo journal. One editor per session.
 */
public final class Editor {
    /**
     * @param overwritten existing blocks of another type that were replaced, by block name;
     *                    reported so the model notices when it builds over its own work
     */
    public record Result(int changed, int skippedOccupied, int outOfBounds, boolean coreSkipped,
                         Map<String, Integer> overwritten) {
        @Override
        public String toString() {
            StringBuilder sb = new StringBuilder().append(changed).append(" blocks changed");
            if (!overwritten.isEmpty()) {
                sb.append(" (overwrote existing: ");
                overwritten.forEach((name, n) -> sb.append(n).append(" x ").append(name).append(", "));
                sb.setLength(sb.length() - 2);
                sb.append(")");
            }
            if (skippedOccupied > 0) sb.append(", ").append(skippedOccupied).append(" skipped (already occupied)");
            if (outOfBounds > 0) sb.append(", ").append(outOfBounds).append(" skipped (outside the build area)");
            if (coreSkipped) sb.append(", 0,0,0 skipped (the Ship Core is there and cannot be replaced)");
            return sb.toString();
        }
    }

    private final WorldGateway gateway;
    private final BlockCatalog catalog;
    private final Journal journal;
    private final int maxBlocksPerOp;
    private boolean mirrorX, mirrorY, mirrorZ;

    public Editor(WorldGateway gateway, BlockCatalog catalog, Journal journal, int maxBlocksPerOp) {
        this.gateway = gateway;
        this.catalog = catalog;
        this.journal = journal;
        this.maxBlocksPerOp = maxBlocksPerOp;
    }

    public WorldGateway gateway() { return gateway; }
    public BlockCatalog catalog() { return catalog; }
    public Journal journal() { return journal; }
    public int maxBlocksPerOp() { return maxBlocksPerOp; }

    public void setSymmetry(boolean x, boolean y, boolean z) {
        mirrorX = x;
        mirrorY = y;
        mirrorZ = z;
    }

    public String symmetryText() {
        if (!mirrorX && !mirrorY && !mirrorZ) return "off";
        return (mirrorX ? "X (left/right) " : "") + (mirrorY ? "Y (up/down) " : "") + (mirrorZ ? "Z (front/back)" : "");
    }

    /**
     * Places one block type at many positions, mirrored by the active symmetry.
     *
     * @param onlyEmpty leave positions that already hold a block untouched
     */
    public Result place(List<Vec3> positions, Block block, boolean onlyEmpty) {
        BlockDef def = catalog.get(block.type());
        boolean flipsWithMirror = def != null && def.shape() == BlockShape.CUBE;
        Box limit = gateway.call(DesignWorld::buildLimit);

        Map<Vec3, Block> writes = new LinkedHashMap<>();
        int outOfBounds = 0;
        boolean coreSkipped = false;
        for (Vec3 p : positions) {
            for (int m = 0; m < 8; m++) {
                boolean fx = (m & 1) != 0, fy = (m & 2) != 0, fz = (m & 4) != 0;
                if ((fx && !mirrorX) || (fy && !mirrorY) || (fz && !mirrorZ)) continue;
                Vec3 q = new Vec3(fx ? -p.x() : p.x(), fy ? -p.y() : p.y(), fz ? -p.z() : p.z());
                if (q.equals(Vec3.ZERO)) { // never touch the core
                    coreSkipped = true;
                    continue;
                }
                if (writes.containsKey(q)) continue;
                if (!limit.contains(q)) {
                    outOfBounds++;
                    continue;
                }
                Block b = flipsWithMirror ? mirrorFacing(block, fx, fy, fz) : block;
                writes.put(q, b);
            }
        }
        if (writes.size() > maxBlocksPerOp) throw new Shapes.TooLargeException(writes.size(), maxBlocksPerOp);
        return commit(writes, onlyEmpty, outOfBounds, coreSkipped);
    }

    public Result remove(Collection<Vec3> positions) {
        return place(new ArrayList<>(positions), Block.AIR, false);
    }

    /** Swaps every block of one type inside a region for another block. */
    public Result replace(Box region, short fromType, Block to) {
        if (region.volume() > (long) maxBlocksPerOp * 8) {
            throw new Shapes.TooLargeException(region.volume(), (long) maxBlocksPerOp * 8);
        }
        List<Vec3> matches = gateway.call(w -> {
            List<Vec3> found = new ArrayList<>();
            w.forEachBlock(region, (x, y, z, b) -> {
                if (b.type() == fromType) found.add(new Vec3(x, y, z));
            });
            return found;
        });
        Map<Vec3, Block> writes = new LinkedHashMap<>();
        for (Vec3 p : matches) {
            if (!p.equals(Vec3.ZERO)) writes.put(p, to);
        }
        if (writes.size() > maxBlocksPerOp) throw new Shapes.TooLargeException(writes.size(), maxBlocksPerOp);
        return commit(writes, false, 0, false);
    }

    /**
     * Links a controller to targets after checking the game allows each pairing. Returns a
     * description of what was linked and what was refused.
     */
    public String link(Vec3 controller, List<Vec3> targets) {
        return gateway.call(w -> {
            Block c = w.get(controller.x(), controller.y(), controller.z());
            if (c.isAir()) return "No block at " + controller + " to link from.";
            int linked = 0, wrongType = 0, empty = 0;
            for (Vec3 t : targets) {
                Block b = w.get(t.x(), t.y(), t.z());
                if (b.isAir()) {
                    empty++;
                } else if (!catalog.canLink(c.type(), b.type())) {
                    wrongType++;
                } else {
                    w.link(controller, t);
                    journal.record(new Journal.LinkChange(controller, t, true));
                    linked++;
                }
            }
            StringBuilder sb = new StringBuilder("Linked " + linked + " blocks to " + catalog.name(c.type())
                    + " at " + controller + ".");
            if (wrongType > 0) sb.append(" ").append(wrongType).append(" targets refused: that block type cannot be controlled by ")
                    .append(catalog.name(c.type())).append(".");
            if (empty > 0) sb.append(" ").append(empty).append(" target positions were empty.");
            return sb.toString();
        });
    }

    public int unlink(Vec3 controller, List<Vec3> targets) {
        return gateway.call(w -> {
            List<Vec3> existing = w.linksFrom(controller);
            int n = 0;
            for (Vec3 t : targets) {
                if (existing.contains(t)) {
                    w.unlink(controller, t);
                    journal.record(new Journal.LinkChange(controller, t, false));
                    n++;
                }
            }
            return n;
        });
    }

    /** Reverts the most recent journal step. Returns its label, or null when there is none. */
    public String undo() {
        Journal.Step step = journal.pop();
        if (step == null) return null;
        // Blocks first: restoring a removed block has to happen before its links come back.
        List<WorldGateway.BlockWrite> writes = new ArrayList<>(step.blocks.size());
        for (int i = step.blocks.size() - 1; i >= 0; i--) {
            Journal.BlockChange c = step.blocks.get(i);
            writes.add(new WorldGateway.BlockWrite(c.pos(), c.before()));
        }
        gateway.apply(writes, false);
        gateway.call(w -> {
            for (int i = step.links.size() - 1; i >= 0; i--) {
                Journal.LinkChange c = step.links.get(i);
                if (c.added()) w.unlink(c.controller(), c.target());
                else w.link(c.controller(), c.target());
            }
            return null;
        });
        return step.label;
    }

    private Result commit(Map<Vec3, Block> writes, boolean onlyEmpty, int outOfBounds, boolean coreSkipped) {
        List<WorldGateway.BlockWrite> list = new ArrayList<>(writes.size());
        writes.forEach((p, b) -> list.add(new WorldGateway.BlockWrite(p, b)));
        List<Block> before = gateway.apply(list, onlyEmpty);
        int changed = 0, skipped = 0;
        Map<String, Integer> overwritten = new TreeMap<>();
        for (int i = 0; i < list.size(); i++) {
            Block was = before.get(i);
            Block now = list.get(i).block();
            if (onlyEmpty && !was.isAir()) {
                skipped++;
            } else if (!was.equals(now)) {
                journal.record(new Journal.BlockChange(list.get(i).pos(), was, now));
                changed++;
                if (!was.isAir() && !now.isAir() && was.type() != now.type()) {
                    overwritten.merge(catalog.name(was.type()), 1, Integer::sum);
                }
            }
        }
        return new Result(changed, skipped, outOfBounds, coreSkipped, overwritten);
    }

    /** Cube facings are 0 front, 1 back, 2 top, 3 bottom, 4 right, 5 left. */
    private static Block mirrorFacing(Block b, boolean fx, boolean fy, boolean fz) {
        int o = b.orientation();
        if (fz && (o == 0 || o == 1)) o ^= 1;
        if (fy && (o == 2 || o == 3)) o ^= 1;
        if (fx && (o == 4 || o == 5)) o ^= 1;
        return o == b.orientation() ? b : new Block(b.type(), (byte) o, b.active());
    }
}
