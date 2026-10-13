package com.starmade.yardhelper.world;

/**
 * Integer block position in design space: the ship core is (0,0,0), +Z is forward,
 * +Y is up and +X is right (the game's FRONT/TOP/RIGHT directions).
 */
public record Vec3(int x, int y, int z) {
    public static final Vec3 ZERO = new Vec3(0, 0, 0);

    public Vec3 plus(int dx, int dy, int dz) {
        return new Vec3(x + dx, y + dy, z + dz);
    }

    public Vec3 plus(Vec3 o) {
        return new Vec3(x + o.x, y + o.y, z + o.z);
    }

    @Override
    public String toString() {
        return x + "," + y + "," + z;
    }
}
