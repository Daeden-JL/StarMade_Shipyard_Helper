package com.starmade.yardhelper.agent;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.catalog.BlockShape;
import com.starmade.yardhelper.world.Block;

import java.util.List;
import java.util.Locale;
import java.util.stream.Collectors;

/** Turns the model's description of a block ("Grey Standard Armor", wedge, facing up) into a Block. */
final class BlockResolver {
    private static final String[] FACINGS = {"front", "back", "top", "bottom", "right", "left"};

    private BlockResolver() {}

    static Block resolve(Args args, String field, ToolContext ctx, boolean allowAir) throws ToolException {
        String text = args.str(field).trim();
        String lower = text.toLowerCase(Locale.ROOT);
        if (lower.equals("air") || lower.equals("empty") || lower.equals("none")) {
            if (!allowAir) throw new ToolException("\"" + field + "\" cannot be air here.");
            return Block.AIR;
        }
        BlockDef def = lookup(text, ctx.catalog());

        String shapeText = args.strOr("shape", "").toLowerCase(Locale.ROOT);
        if (!shapeText.isEmpty() && !shapeText.equals("cube")) {
            BlockShape shape = switch (shapeText) {
                case "wedge" -> BlockShape.WEDGE;
                case "corner" -> BlockShape.CORNER;
                case "tetra" -> BlockShape.TETRA;
                case "hepta" -> BlockShape.HEPTA;
                default -> throw new ToolException("Unknown shape \"" + shapeText + "\". Use cube, wedge, corner, tetra or hepta.");
            };
            BlockDef variant = ctx.catalog().variant(def.id(), shape);
            if (variant == null) throw new ToolException(def.name() + " has no " + shapeText + " variant.");
            def = variant;
        }

        int orientation = 0;
        if (args.has("facing")) {
            String facing = args.strOr("facing", "").toLowerCase(Locale.ROOT);
            orientation = -1;
            for (int i = 0; i < FACINGS.length; i++) if (FACINGS[i].equals(facing)) orientation = i;
            if (orientation < 0) throw new ToolException("Unknown facing \"" + facing + "\". Use front, back, top, bottom, right or left.");
        } else if (args.has("orientation")) {
            orientation = args.integer("orientation");
            if (orientation < 0 || orientation > 23) throw new ToolException("orientation must be 0..23.");
        }
        return new Block(def.id(), (byte) orientation, false);
    }

    static BlockDef lookup(String text, BlockCatalog catalog) throws ToolException {
        BlockDef def = null;
        if (text.matches("\\d+")) {
            def = catalog.get(Integer.parseInt(text));
            if (def == null) throw new ToolException("There is no block with id " + text + ". Use find_blocks to search.");
        }
        if (def == null) def = catalog.byName(text);
        if (def == null) def = catalog.byKey(text);
        if (def == null) {
            List<BlockDef> hits = catalog.search(text, 6);
            if (hits.size() == 1) {
                def = hits.get(0);
            } else if (hits.isEmpty()) {
                throw new ToolException("Unknown block \"" + text + "\". Use find_blocks to search by keyword.");
            } else {
                throw new ToolException("\"" + text + "\" is ambiguous. Did you mean: "
                        + hits.stream().map(h -> h.name() + " (id " + h.id() + ")").collect(Collectors.joining(", "))
                        + "? Use the exact name or id.");
            }
        }
        if (!def.placeable() || def.deprecated()) {
            throw new ToolException(def.name() + " cannot be placed (deprecated or not a buildable block).");
        }
        return def;
    }
}
