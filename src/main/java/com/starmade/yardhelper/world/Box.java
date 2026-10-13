package com.starmade.yardhelper.world;

/** Inclusive axis-aligned box. */
public record Box(Vec3 min, Vec3 max) {
    public static Box of(Vec3 a, Vec3 b) {
        return new Box(
                new Vec3(Math.min(a.x(), b.x()), Math.min(a.y(), b.y()), Math.min(a.z(), b.z())),
                new Vec3(Math.max(a.x(), b.x()), Math.max(a.y(), b.y()), Math.max(a.z(), b.z())));
    }

    public int sizeX() { return max.x() - min.x() + 1; }
    public int sizeY() { return max.y() - min.y() + 1; }
    public int sizeZ() { return max.z() - min.z() + 1; }

    public long volume() {
        return (long) sizeX() * sizeY() * sizeZ();
    }

    public boolean contains(int x, int y, int z) {
        return x >= min.x() && x <= max.x() && y >= min.y() && y <= max.y() && z >= min.z() && z <= max.z();
    }

    public boolean contains(Vec3 p) {
        return contains(p.x(), p.y(), p.z());
    }

    /** Overlap with another box, or null when they don't touch. */
    public Box intersect(Box o) {
        Vec3 lo = new Vec3(Math.max(min.x(), o.min.x()), Math.max(min.y(), o.min.y()), Math.max(min.z(), o.min.z()));
        Vec3 hi = new Vec3(Math.min(max.x(), o.max.x()), Math.min(max.y(), o.max.y()), Math.min(max.z(), o.max.z()));
        if (lo.x() > hi.x() || lo.y() > hi.y() || lo.z() > hi.z()) return null;
        return new Box(lo, hi);
    }

    @Override
    public String toString() {
        return "[" + min + " .. " + max + "]";
    }
}
