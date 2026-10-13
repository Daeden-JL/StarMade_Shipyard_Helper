package com.starmade.yardhelper.catalog;

import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;

/** All block types the game knows, with name search and variant lookup. */
public final class BlockCatalog {
    public static final short CORE_ID = 1;

    private final Map<Short, BlockDef> byId = new TreeMap<>();
    private final Map<String, BlockDef> byKey = new HashMap<>();
    private final Map<String, BlockDef> byName = new HashMap<>();

    public BlockCatalog(Collection<BlockDef> defs) {
        for (BlockDef d : defs) {
            byId.put(d.id(), d);
            byKey.put(d.key().toUpperCase(Locale.ROOT), d);
            // Several deprecated blocks share names with live ones; the live one wins.
            String name = d.name().toLowerCase(Locale.ROOT);
            BlockDef existing = byName.get(name);
            if (existing == null || (existing.deprecated() && !d.deprecated())) byName.put(name, d);
        }
    }

    public BlockDef get(int id) {
        return byId.get((short) id);
    }

    /** Looks up by config key such as GREY_HULL (case-insensitive). */
    public BlockDef byKey(String key) {
        return byKey.get(key.trim().toUpperCase(Locale.ROOT).replace(' ', '_'));
    }

    /** Looks up by exact display name (case-insensitive). */
    public BlockDef byName(String name) {
        return byName.get(name.trim().toLowerCase(Locale.ROOT));
    }

    public int size() {
        return byId.size();
    }

    public Collection<BlockDef> all() {
        return byId.values();
    }

    public String name(int id) {
        BlockDef d = get(id);
        return d == null ? "unknown#" + id : d.name();
    }

    /** Blocks the agent may place: known, placeable and not deprecated. */
    public boolean isUsable(int id) {
        BlockDef d = get(id);
        return d != null && d.placeable() && !d.deprecated();
    }

    /** The variant of a block (or of its base cube) with the given shape, or null. */
    public BlockDef variant(int id, BlockShape shape) {
        BlockDef base = baseOf(id);
        if (base == null) return null;
        if (shape == BlockShape.CUBE) return base;
        for (short styleId : base.styleIds()) {
            BlockDef v = get(styleId);
            if (v != null && v.shape() == shape) return v;
        }
        return null;
    }

    /** The full cube a variant derives from; the block itself when it is not a variant. */
    public BlockDef baseOf(int id) {
        BlockDef d = get(id);
        if (d == null) return null;
        BlockDef base = d.isVariant() ? get(d.baseId()) : d;
        return base == null ? d : base;
    }

    /** Whether the game allows linking a controller block type to a target block type. */
    public boolean canLink(int controllerType, int targetType) {
        BlockDef c = get(controllerType);
        BlockDef t = get(targetType);
        if (c == null || t == null) return false;
        return c.controlling().contains(t.id()) || t.controlledBy().contains(c.id());
    }

    /**
     * Finds blocks by words in their name, config key or category. Every query word has to
     * match somewhere. Shape and slab variants are left out unless the query asks for them
     * (they are reachable through the base block's variant ids).
     */
    public List<BlockDef> search(String query, int limit) {
        String[] words = query.toLowerCase(Locale.ROOT).trim().split("[^a-z0-9]+");
        boolean wantsVariant = false;
        for (String w : words) {
            if (w.equals("wedge") || w.equals("corner") || w.equals("tetra") || w.equals("hepta")
                    || w.equals("slab") || w.equals("penta")) {
                wantsVariant = true;
            }
        }
        record Hit(BlockDef def, int score) {}
        List<Hit> hits = new ArrayList<>();
        for (BlockDef d : byId.values()) {
            if (!d.placeable() || d.deprecated()) continue;
            if (d.isVariant() && !wantsVariant) continue;
            String name = d.name().toLowerCase(Locale.ROOT);
            String rest = (d.key() + " " + d.category()).toLowerCase(Locale.ROOT).replace('_', ' ');
            int score = 0;
            boolean all = true;
            for (String w : words) {
                if (w.isEmpty()) continue;
                if (name.contains(w)) score += 3;
                else if (rest.contains(w)) score += 1;
                else { all = false; break; }
            }
            if (!all || score == 0) continue;
            if (name.equals(query.toLowerCase(Locale.ROOT).trim())) score += 10;
            hits.add(new Hit(d, score));
        }
        hits.sort(Comparator.comparingInt((Hit h) -> -h.score)
                .thenComparingInt(h -> h.def.name().length())
                .thenComparingInt(h -> h.def.id()));
        List<BlockDef> out = new ArrayList<>();
        for (Hit h : hits) {
            if (out.size() >= limit) break;
            out.add(h.def);
        }
        return out;
    }
}
