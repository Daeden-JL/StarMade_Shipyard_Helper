package com.starmade.yardhelper.catalog;

import java.util.Set;

/**
 * What the agent needs to know about one block type. Built from the game's ElementKeyMap
 * on the server, or from BlockConfig.xml offline.
 *
 * @param key            config name such as GREY_HULL
 * @param category       config path such as "General/Hulls/Basic/Grey"
 * @param slab           0 = full block, 1..3 = quarter-height slabs (1 is 3/4, 3 is 1/4)
 * @param individualSides 1, 3 or 6 distinct side textures; more than 1 means rotating the
 *                       block changes what its faces show
 * @param baseId         the full cube this is a shape/slab variant of, or 0
 * @param styleIds       shaped variants of this block (wedge, corner, hepta, tetra)
 * @param slabIds        slab variants of this block
 */
public record BlockDef(
        short id,
        String key,
        String name,
        String category,
        BlockShape shape,
        int slab,
        boolean orientable,
        int individualSides,
        Set<Short> controlling,
        Set<Short> controlledBy,
        short baseId,
        short[] styleIds,
        short[] slabIds,
        boolean placeable,
        boolean deprecated,
        boolean canActivate,
        boolean logic,
        boolean system,
        String description) {

    /** True when this is a shape or slab variant of another block. */
    public boolean isVariant() {
        return baseId != 0;
    }
}
