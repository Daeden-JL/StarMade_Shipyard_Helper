package com.starmade.yardhelper.agent;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Vec3;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.stream.Collectors;

/** Tools that read the design and the block catalog. */
final class InspectTools {
    private static final int MAX_SLICE_SPAN = 80;
    private static final int MAX_SLICES = 24;
    private static final String SYMBOLS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    private InspectTools() {}

    static List<Tool> all() {
        List<Tool> tools = new ArrayList<>();

        tools.add(new Tool("design_info",
                "Summary of the current design: size, bounds, block counts by type, build area and symmetry. Call this first.",
                Schema.object().build(),
                (args, ctx) -> designInfo(ctx)));

        tools.add(new Tool("scan",
                "Look at part of the design as layered ASCII maps, one map per layer along an axis. "
                        + "Use it to check your work. Each map is at most 80x80 blocks; ask for a smaller region if needed.",
                Schema.object()
                        .vec("from", "One corner of the region.")
                        .vec("to", "Opposite corner of the region.")
                        .choice("axis", "Axis the layers are stacked along. y (default) gives top-down maps.", "x", "y", "z")
                        .require("from", "to").build(),
                InspectTools::scan));

        tools.add(new Tool("get_block",
                "What is at one position: block, orientation, and what it is linked to.",
                Schema.object().vec("pos", "Position to look at.").require("pos").build(),
                InspectTools::getBlock));

        tools.add(new Tool("find_blocks",
                "Search the block catalog by keywords, e.g. \"grey standard armor\", \"cannon\", \"rail\", \"light\". "
                        + "Returns ids, exact names and available shape variants.",
                Schema.object()
                        .str("query", "Keywords; every word must match the name or category.")
                        .integer("limit", "Maximum results (default 15).")
                        .require("query").build(),
                InspectTools::findBlocks));

        tools.add(new Tool("block_info",
                "Details of one block type: description, what it controls or is controlled by, shape and slab variants.",
                Schema.object().str("block", "Block name or id.").require("block").build(),
                InspectTools::blockInfo));

        tools.add(new Tool("read_guide",
                "Read notes on game mechanics before building systems.",
                Schema.object().str("topic", "Guide topic.").require("topic").build(),
                (args, ctx) -> {
                    String text = ctx.guides().get(args.str("topic").trim().toLowerCase());
                    return text != null ? text : "No such guide. Topics: " + ctx.guides().topics();
                }));

        return tools;
    }

    private static String designInfo(ToolContext ctx) {
        BlockCatalog catalog = ctx.catalog();
        return ctx.editor().gateway().call(w -> {
            StringBuilder sb = new StringBuilder();
            Box b = w.bounds();
            sb.append("Core at 0,0,0. +Z is forward, +Y is up, +X is right.\n");
            if (b == null) {
                sb.append("The design is empty.\n");
            } else {
                sb.append("Blocks: ").append(w.blockCount()).append("\n");
                sb.append("Bounds: ").append(b).append(" (").append(b.sizeX()).append(" wide, ")
                        .append(b.sizeY()).append(" tall, ").append(b.sizeZ()).append(" long)\n");
                Map<Short, Integer> counts = new TreeMap<>();
                w.forEachBlock(b, (x, y, z, block) -> counts.merge(block.type(), 1, Integer::sum));
                sb.append("By type:\n");
                counts.entrySet().stream()
                        .sorted(Map.Entry.<Short, Integer>comparingByValue(Comparator.reverseOrder()))
                        .limit(25)
                        .forEach(e -> sb.append("  ").append(e.getValue()).append(" x ")
                                .append(catalog.name(e.getKey())).append(" (id ").append(e.getKey()).append(")\n"));
                if (counts.size() > 25) sb.append("  ... and ").append(counts.size() - 25).append(" more types\n");
            }
            sb.append("Build area: ").append(w.buildLimit()).append("\n");
            sb.append("Symmetry: ").append(ctx.editor().symmetryText()).append("\n");
            return sb.toString();
        });
    }

    private static String scan(Args args, ToolContext ctx) throws ToolException {
        Box region = args.box("from", "to");
        String axis = args.strOr("axis", "y").toLowerCase();
        // (layer, row, column) in terms of the chosen axis. Rows run top to bottom on the
        // page, so they count down along the axis that should read as "up" or "forward".
        int layers, rows, cols;
        switch (axis) {
            case "x" -> { layers = region.sizeX(); rows = region.sizeY(); cols = region.sizeZ(); }
            case "z" -> { layers = region.sizeZ(); rows = region.sizeY(); cols = region.sizeX(); }
            default -> { axis = "y"; layers = region.sizeY(); rows = region.sizeZ(); cols = region.sizeX(); }
        }
        if (rows > MAX_SLICE_SPAN || cols > MAX_SLICE_SPAN) {
            throw new ToolException("Region is " + cols + "x" + rows + " per layer; the limit is "
                    + MAX_SLICE_SPAN + "x" + MAX_SLICE_SPAN + ". Scan a smaller region.");
        }
        final String ax = axis;
        BlockCatalog catalog = ctx.catalog();

        return ctx.editor().gateway().call(w -> {
            Map<Short, Character> legend = new LinkedHashMap<>();
            List<String> maps = new ArrayList<>();
            for (int l = 0; l < layers; l++) {
                StringBuilder map = new StringBuilder();
                for (int r = 0; r < rows; r++) {
                    for (int c = 0; c < cols; c++) {
                        int x, y, z;
                        switch (ax) {
                            case "x" -> { x = region.min().x() + l; y = region.max().y() - r; z = region.min().z() + c; }
                            case "z" -> { z = region.min().z() + l; y = region.max().y() - r; x = region.min().x() + c; }
                            default -> { y = region.min().y() + l; z = region.max().z() - r; x = region.min().x() + c; }
                        }
                        Block b = w.get(x, y, z);
                        if (b.isAir()) {
                            map.append('.');
                        } else if (b.type() == BlockCatalog.CORE_ID) {
                            map.append('@');
                        } else {
                            Character sym = legend.get(b.type());
                            if (sym == null) {
                                sym = legend.size() < SYMBOLS.length() ? SYMBOLS.charAt(legend.size()) : '#';
                                legend.put(b.type(), sym);
                            }
                            map.append(sym);
                        }
                    }
                    map.append('\n');
                }
                maps.add(map.toString());
            }

            StringBuilder sb = new StringBuilder();
            String rowsDesc, colsDesc;
            switch (ax) {
                case "x" -> { rowsDesc = "rows y " + region.max().y() + " down to " + region.min().y();
                              colsDesc = "columns z " + region.min().z() + " to " + region.max().z(); }
                case "z" -> { rowsDesc = "rows y " + region.max().y() + " down to " + region.min().y();
                              colsDesc = "columns x " + region.min().x() + " to " + region.max().x(); }
                default -> { rowsDesc = "rows z " + region.max().z() + " (front) down to " + region.min().z();
                             colsDesc = "columns x " + region.min().x() + " to " + region.max().x(); }
            }
            sb.append("Layers along ").append(ax).append("; ").append(rowsDesc).append("; ").append(colsDesc).append(".\n");
            int base = switch (ax) { case "x" -> region.min().x(); case "z" -> region.min().z(); default -> region.min().y(); };
            int shown = 0;
            String empty = maps.isEmpty() ? "" : maps.get(0).replaceAll("[^\\n]", ".");
            for (int l = 0; l < layers; ) {
                int end = l;
                while (end + 1 < layers && maps.get(end + 1).equals(maps.get(l))) end++;
                String label = ax + "=" + (base + l) + (end > l ? ".." + (base + end) : "");
                if (maps.get(l).equals(empty)) {
                    sb.append(label).append(": empty\n");
                } else if (shown >= MAX_SLICES) {
                    sb.append(label).append(": (not shown, scan fewer layers)\n");
                } else {
                    sb.append(label).append(":\n").append(maps.get(l));
                    shown++;
                }
                l = end + 1;
            }
            if (legend.isEmpty()) {
                sb.append("No blocks in this region.\n");
            } else {
                sb.append("Legend: . empty, @ core");
                legend.forEach((type, sym) -> sb.append(", ").append(sym).append(' ').append(catalog.name(type)));
                sb.append('\n');
            }
            return sb.toString();
        });
    }

    private static String getBlock(Args args, ToolContext ctx) throws ToolException {
        Vec3 p = args.vec("pos");
        BlockCatalog catalog = ctx.catalog();
        return ctx.editor().gateway().call(w -> {
            Block b = w.get(p.x(), p.y(), p.z());
            if (b.isAir()) return p + " is empty.";
            StringBuilder sb = new StringBuilder(p + ": " + catalog.name(b.type()) + " (id " + b.type()
                    + "), orientation " + b.orientation() + (b.active() ? ", active" : ""));
            List<Vec3> links = w.linksFrom(p);
            if (!links.isEmpty()) {
                Map<Short, Integer> byType = new TreeMap<>();
                for (Vec3 t : links) byType.merge(w.get(t.x(), t.y(), t.z()).type(), 1, Integer::sum);
                sb.append("\nLinked to ").append(links.size()).append(" blocks: ")
                        .append(byType.entrySet().stream()
                                .map(e -> e.getValue() + " x " + catalog.name(e.getKey()))
                                .collect(Collectors.joining(", ")));
                if (links.size() <= 12) sb.append(" at ").append(links);
            }
            return sb.toString();
        });
    }

    private static String findBlocks(Args args, ToolContext ctx) throws ToolException {
        int limit = Math.max(1, Math.min(40, args.intOr("limit", 15)));
        List<BlockDef> hits = ctx.catalog().search(args.str("query"), limit);
        if (hits.isEmpty()) return "No blocks match. Try fewer or different keywords.";
        StringBuilder sb = new StringBuilder();
        for (BlockDef d : hits) {
            sb.append(d.id()).append(" | ").append(d.name()).append(" | ").append(d.category());
            String variants = variants(d, ctx.catalog());
            if (!variants.isEmpty()) sb.append(" | shapes: ").append(variants);
            sb.append('\n');
        }
        return sb.toString();
    }

    private static String blockInfo(Args args, ToolContext ctx) throws ToolException {
        BlockCatalog catalog = ctx.catalog();
        BlockDef d = BlockResolver.lookup(args.str("block").trim(), catalog);
        StringBuilder sb = new StringBuilder();
        sb.append(d.name()).append(" (id ").append(d.id()).append(", ").append(d.key()).append(")\n");
        sb.append("Category: ").append(d.category()).append("\n");
        sb.append("Shape: ").append(d.shape().name().toLowerCase());
        if (d.slab() > 0) sb.append(", slab ").append(4 - d.slab()).append("/4 height");
        sb.append("\n");
        if (d.individualSides() > 1 || d.orientable()) {
            sb.append("Has ").append(d.individualSides()).append(" distinct side textures; set facing to rotate it.\n");
        }
        String variants = variants(d, catalog);
        if (!variants.isEmpty()) sb.append("Shape variants: ").append(variants).append("\n");
        if (d.slabIds().length > 0) {
            sb.append("Slab variants: ");
            for (short id : d.slabIds()) sb.append(catalog.name(id)).append(" (").append(id).append(") ");
            sb.append("\n");
        }
        if (!d.controlling().isEmpty()) sb.append("Links to (controls): ").append(names(d.controlling(), catalog)).append("\n");
        if (!d.controlledBy().isEmpty()) sb.append("Linked from (controlled by): ").append(names(d.controlledBy(), catalog)).append("\n");
        if (d.canActivate()) sb.append("Can be activated (on/off).\n");
        String desc = d.description();
        if (!desc.isEmpty()) sb.append(desc.length() > 500 ? desc.substring(0, 500) + "..." : desc).append("\n");
        return sb.toString();
    }

    private static String variants(BlockDef d, BlockCatalog catalog) {
        StringBuilder sb = new StringBuilder();
        for (short id : d.styleIds()) {
            BlockDef v = catalog.get(id);
            if (v == null) continue;
            if (sb.length() > 0) sb.append(", ");
            sb.append(v.shape().name().toLowerCase()).append(" (").append(id).append(")");
        }
        return sb.toString();
    }

    private static String names(java.util.Set<Short> ids, BlockCatalog catalog) {
        return ids.stream().limit(20).map(id -> catalog.name(id) + " (" + id + ")").collect(Collectors.joining(", "))
                + (ids.size() > 20 ? ", ..." : "");
    }
}
