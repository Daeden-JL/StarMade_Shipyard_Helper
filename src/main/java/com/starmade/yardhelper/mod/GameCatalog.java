package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.catalog.BlockShape;
import it.unimi.dsi.fastutil.shorts.ShortSet;
import org.schema.game.common.data.element.ElementInformation;
import org.schema.game.common.data.element.ElementKeyMap;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** Builds the block catalog from the running game, so modded blocks are included. */
final class GameCatalog {
    private static volatile BlockCatalog cached;

    private GameCatalog() {}

    /** Loaded on first use: the game's block config is not ready when mods are enabled. */
    static BlockCatalog get() {
        BlockCatalog c = cached;
        if (c == null) {
            synchronized (GameCatalog.class) {
                c = cached;
                if (c == null) cached = c = load();
            }
        }
        return c;
    }

    private static BlockCatalog load() {
        Map<Short, String> keys = new HashMap<>();
        if (ElementKeyMap.properties != null) {
            for (String key : ElementKeyMap.properties.stringPropertyNames()) {
                try {
                    keys.put(Short.parseShort(ElementKeyMap.properties.getProperty(key).trim()), key);
                } catch (NumberFormatException ignored) {
                    // not a block id line
                }
            }
        }
        List<BlockDef> defs = new ArrayList<>();
        for (ElementInformation info : ElementKeyMap.getInfoArray()) {
            if (info == null) continue;
            short id = info.getId();
            String name = info.getName() == null ? "" : info.getName();
            defs.add(new BlockDef(
                    id,
                    keys.getOrDefault(id, name.toUpperCase().replace(' ', '_')),
                    name,
                    category(info),
                    BlockShape.fromStyle(info.getBlockStyle().ordinal()),
                    info.slab,
                    info.orientatable,
                    info.individualSides,
                    toSet(info.controlling),
                    toSet(info.controlledBy),
                    (short) info.sourceReference,
                    info.styleIds == null ? new short[0] : info.styleIds,
                    info.slabIds == null ? new short[0] : info.slabIds,
                    info.placable,
                    info.deprecated,
                    info.canActivate,
                    info.signal,
                    info.systemBlock,
                    info.getDescription() == null ? "" : info.getDescription()));
        }
        return new BlockCatalog(defs);
    }

    /** "Element.General.Hulls.Standard.Grey" becomes "General/Hulls/Standard/Grey", as in the XML. */
    private static String category(ElementInformation info) {
        if (info.getType() == null) return "";
        String path = info.getType().getFullPathRecursive();
        if (path == null) return "";
        if (path.startsWith("Element.")) path = path.substring("Element.".length());
        return path.replace('.', '/');
    }

    private static Set<Short> toSet(ShortSet s) {
        Set<Short> out = new HashSet<>();
        if (s != null) for (short v : s.toShortArray()) out.add(v);
        return out;
    }
}
