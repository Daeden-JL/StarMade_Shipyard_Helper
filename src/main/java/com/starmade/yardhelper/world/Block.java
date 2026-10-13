package com.starmade.yardhelper.world;

/** One block's state. Type 0 is empty space. */
public record Block(short type, byte orientation, boolean active) {
    public static final Block AIR = new Block((short) 0, (byte) 0, false);

    public static Block of(int type) {
        return new Block((short) type, (byte) 0, false);
    }

    public static Block of(int type, int orientation) {
        return new Block((short) type, (byte) orientation, false);
    }

    public boolean isAir() {
        return type == 0;
    }
}
