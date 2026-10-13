package com.starmade.yardhelper.catalog;

/** Block geometry, in the order of the game's BlockStyle ordinals (0..6). */
public enum BlockShape {
    CUBE, WEDGE, CORNER, SPRITE, TETRA, HEPTA, RAIL;

    public static BlockShape fromStyle(int style) {
        BlockShape[] all = values();
        return style >= 0 && style < all.length ? all[style] : CUBE;
    }
}
