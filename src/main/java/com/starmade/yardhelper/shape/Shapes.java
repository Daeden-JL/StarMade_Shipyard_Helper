package com.starmade.yardhelper.shape;

import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Vec3;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Turns shape primitives into block positions. The model describes shapes; this code does
 * the voxel math, so results are exact no matter how well the model can count.
 */
public final class Shapes {
    private Shapes() {}

    public enum Fill { SOLID, HOLLOW, FRAME }

    public enum Axis { X, Y, Z }

    /** Thrown when a shape would produce more blocks than allowed. */
    public static final class TooLargeException extends RuntimeException {
        public TooLargeException(long blocks, long max) {
            super("Shape needs about " + blocks + " blocks, more than the limit of " + max
                    + " per operation. Build it in smaller pieces.");
        }
    }

    public static List<Vec3> box(Box b, Fill fill, int maxBlocks) {
        if (fill == Fill.SOLID && b.volume() > maxBlocks) throw new TooLargeException(b.volume(), maxBlocks);
        List<Vec3> out = new ArrayList<>();
        Vec3 lo = b.min(), hi = b.max();
        for (int y = lo.y(); y <= hi.y(); y++) {
            for (int z = lo.z(); z <= hi.z(); z++) {
                for (int x = lo.x(); x <= hi.x(); x++) {
                    int onFaces = (x == lo.x() || x == hi.x() ? 1 : 0)
                            + (y == lo.y() || y == hi.y() ? 1 : 0)
                            + (z == lo.z() || z == hi.z() ? 1 : 0);
                    boolean keep = switch (fill) {
                        case SOLID -> true;
                        case HOLLOW -> onFaces >= 1;
                        case FRAME -> onFaces >= 2;
                    };
                    if (keep) {
                        out.add(new Vec3(x, y, z));
                        if (out.size() > maxBlocks) throw new TooLargeException(out.size(), maxBlocks);
                    }
                }
            }
        }
        return out;
    }

    /** A gap-free straight line of blocks between two points. */
    public static List<Vec3> line(Vec3 a, Vec3 b) {
        int dx = b.x() - a.x(), dy = b.y() - a.y(), dz = b.z() - a.z();
        int steps = Math.max(Math.abs(dx), Math.max(Math.abs(dy), Math.abs(dz)));
        Set<Vec3> out = new LinkedHashSet<>();
        if (steps == 0) {
            out.add(a);
            return new ArrayList<>(out);
        }
        for (int i = 0; i <= steps; i++) {
            double t = (double) i / steps;
            out.add(new Vec3(
                    (int) Math.round(a.x() + dx * t),
                    (int) Math.round(a.y() + dy * t),
                    (int) Math.round(a.z() + dz * t)));
        }
        return new ArrayList<>(out);
    }

    /**
     * An ellipsoid centred on a block. Radii are in blocks from the centre block to the
     * outermost block on each axis, so radius 3 gives a 7-block span.
     */
    public static List<Vec3> ellipsoid(Vec3 c, int rx, int ry, int rz, boolean hollow, int maxBlocks) {
        long estimate = (long) (4.19 * (rx + 0.5) * (ry + 0.5) * (rz + 0.5));
        if (!hollow && estimate > maxBlocks) throw new TooLargeException(estimate, maxBlocks);
        Set<Vec3> solid = new HashSet<>();
        for (int y = -ry; y <= ry; y++)
            for (int z = -rz; z <= rz; z++)
                for (int x = -rx; x <= rx; x++)
                    if (inEllipse(x, rx) + inEllipse(y, ry) + inEllipse(z, rz) <= 1.0) {
                        solid.add(new Vec3(c.x() + x, c.y() + y, c.z() + z));
                    }
        return finish(solid, hollow, maxBlocks);
    }

    /**
     * A cylinder, cone or tapered tube along an axis. It starts at {@code start} and runs
     * {@code length} blocks in the positive axis direction. The cross-section is an ellipse
     * with radii (ra, rb) on the two other axes in X,Y,Z order, scaled linearly to
     * {@code endScale} at the far end (1 = cylinder, 0 = cone).
     */
    public static List<Vec3> cylinder(Vec3 start, Axis axis, int length, int ra, int rb, double endScale,
                                      boolean hollow, int maxBlocks) {
        Set<Vec3> solid = new HashSet<>();
        for (int i = 0; i < length; i++) {
            double t = length == 1 ? 0 : (double) i / (length - 1);
            double scale = 1 + (endScale - 1) * t;
            double sa = ra * scale, sb = rb * scale;
            int ia = (int) Math.ceil(sa), ib = (int) Math.ceil(sb);
            for (int a = -ia; a <= ia; a++) {
                for (int b = -ib; b <= ib; b++) {
                    if (inEllipse(a, sa) + inEllipse(b, sb) > 1.0) continue;
                    solid.add(switch (axis) {
                        case X -> new Vec3(start.x() + i, start.y() + a, start.z() + b);
                        case Y -> new Vec3(start.x() + a, start.y() + i, start.z() + b);
                        case Z -> new Vec3(start.x() + a, start.y() + b, start.z() + i);
                    });
                }
            }
            if (solid.size() > (long) maxBlocks * 4) throw new TooLargeException(solid.size(), maxBlocks);
        }
        return finish(solid, hollow, maxBlocks);
    }

    /** Adds mirror images across the planes through the core. */
    public static List<Vec3> mirror(List<Vec3> positions, boolean mirrorX, boolean mirrorY, boolean mirrorZ) {
        if (!mirrorX && !mirrorY && !mirrorZ) return positions;
        Set<Vec3> out = new LinkedHashSet<>(positions);
        if (mirrorX) for (Vec3 p : new ArrayList<>(out)) out.add(new Vec3(-p.x(), p.y(), p.z()));
        if (mirrorY) for (Vec3 p : new ArrayList<>(out)) out.add(new Vec3(p.x(), -p.y(), p.z()));
        if (mirrorZ) for (Vec3 p : new ArrayList<>(out)) out.add(new Vec3(p.x(), p.y(), -p.z()));
        return new ArrayList<>(out);
    }

    /** Keeps only blocks that have at least one of their six neighbours outside the set. */
    public static Set<Vec3> shell(Set<Vec3> solid) {
        Set<Vec3> out = new HashSet<>();
        for (Vec3 p : solid) {
            if (!solid.contains(p.plus(1, 0, 0)) || !solid.contains(p.plus(-1, 0, 0))
                    || !solid.contains(p.plus(0, 1, 0)) || !solid.contains(p.plus(0, -1, 0))
                    || !solid.contains(p.plus(0, 0, 1)) || !solid.contains(p.plus(0, 0, -1))) {
                out.add(p);
            }
        }
        return out;
    }

    private static List<Vec3> finish(Set<Vec3> solid, boolean hollow, int maxBlocks) {
        Set<Vec3> result = hollow ? shell(solid) : solid;
        if (result.size() > maxBlocks) throw new TooLargeException(result.size(), maxBlocks);
        List<Vec3> out = new ArrayList<>(result);
        // Stable order (bottom-up) so builds look deliberate and tests are deterministic.
        out.sort((p, q) -> p.y() != q.y() ? Integer.compare(p.y(), q.y())
                : p.z() != q.z() ? Integer.compare(p.z(), q.z()) : Integer.compare(p.x(), q.x()));
        return out;
    }

    /** Squared normalised distance along one axis; a radius of 0 only admits the centre. */
    private static double inEllipse(int d, double r) {
        if (r < 0.5) return d == 0 ? 0 : 2;
        double n = d / (r + 0.5);
        return n * n;
    }
}
