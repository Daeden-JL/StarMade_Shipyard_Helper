package com.starmade.yardhelper;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.catalog.BlockShape;

import java.util.List;
import java.util.Set;

/** A handful of blocks shaped like the real config, so tests need no game files. */
public final class TestCatalog {
    public static final short CORE = 1, HULL = 598, HULL_WEDGE = 599, CANNON_COMPUTER = 6, CANNON_BARREL = 16,
            THRUSTER = 8, OLD_POWER = 2, GLASS = 63;

    private TestCatalog() {}

    public static BlockCatalog create() {
        return new BlockCatalog(List.of(
                def(CORE, "SHIP_CORE", "Ship Core", "Ship", BlockShape.CUBE, 0, new short[0], Set.of(), Set.of(), false),
                def(HULL, "GREY_HULL", "Grey Basic Armor", "General/Hulls/Basic/Grey", BlockShape.CUBE, 0,
                        new short[]{HULL_WEDGE}, Set.of(), Set.of(), false),
                def(HULL_WEDGE, "GREY_HULL_WEDGE", "Grey Basic Armor Wedge", "General/Hulls/Basic/Grey", BlockShape.WEDGE, HULL,
                        new short[0], Set.of(), Set.of(), false),
                def(CANNON_COMPUTER, "CANNON_COMPUTER", "Cannon Computer", "Ship/Weapons", BlockShape.CUBE, 0,
                        new short[0], Set.of(CANNON_BARREL), Set.of(CORE), false),
                def(CANNON_BARREL, "CANNON_BARREL", "Cannon Barrel", "Ship/Weapons", BlockShape.CUBE, 0,
                        new short[0], Set.of(), Set.of(CANNON_COMPUTER), false),
                def(THRUSTER, "THRUSTER_MODULE", "Thruster Module", "Ship", BlockShape.CUBE, 0,
                        new short[0], Set.of(), Set.of(), false),
                def(OLD_POWER, "POWER_REACTOR", "Power (Old)", "General/Power", BlockShape.CUBE, 0,
                        new short[0], Set.of(), Set.of(), true),
                def(GLASS, "GLASS", "Glass", "General", BlockShape.CUBE, 0, new short[0], Set.of(), Set.of(), false)));
    }

    private static BlockDef def(short id, String key, String name, String category, BlockShape shape, int baseId,
                                short[] styleIds, Set<Short> controlling, Set<Short> controlledBy, boolean deprecated) {
        return new BlockDef(id, key, name, category, shape, 0, false, 6, controlling, controlledBy, (short) baseId,
                styleIds, new short[0], true, deprecated, false, false, false, "");
    }
}
