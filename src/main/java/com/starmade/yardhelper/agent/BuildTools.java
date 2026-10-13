package com.starmade.yardhelper.agent;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.shape.Shapes;
import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Editor;
import com.starmade.yardhelper.world.Vec3;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Tools that place, change and remove blocks, and link them. */
final class BuildTools {
    private static final int MAX_EXPLICIT_BLOCKS = 300;
    private static final String BLOCK_DESC = "Block name or id (use find_blocks). \"air\" removes blocks.";
    private static final String SHAPE_DESC = "Shape variant of the block (default cube).";
    private static final String FACING_DESC = "Which way the block's front face points. Only matters for blocks with distinct side textures.";

    private BuildTools() {}

    static List<Tool> all() {
        List<Tool> tools = new ArrayList<>();

        tools.add(new Tool("fill",
                "Fill a box-shaped region with one block. Modes: solid, hollow (only the six faces, 1 thick) "
                        + "or frame (only the twelve edges). The main tool for hulls, decks, walls and rooms.",
                blockParams(Schema.object()
                        .vec("from", "One corner of the box.")
                        .vec("to", "Opposite corner of the box (inclusive)."))
                        .choice("mode", "Default solid.", "solid", "hollow", "frame")
                        .bool("only_empty", "If true, keep blocks that are already there (default false).")
                        .require("block", "from", "to").build(),
                (args, ctx) -> {
                    Block block = BlockResolver.resolve(args, "block", ctx, true);
                    Shapes.Fill mode = switch (args.strOr("mode", "solid").toLowerCase()) {
                        case "hollow" -> Shapes.Fill.HOLLOW;
                        case "frame" -> Shapes.Fill.FRAME;
                        default -> Shapes.Fill.SOLID;
                    };
                    List<Vec3> positions = Shapes.box(args.box("from", "to"), mode, ctx.editor().maxBlocksPerOp());
                    return describe(ctx, block, ctx.editor().place(positions, block, args.boolOr("only_empty", false)));
                }));

        tools.add(new Tool("line",
                "Place a straight one-block-thick line of blocks between two points (any direction).",
                blockParams(Schema.object()
                        .vec("from", "Start of the line.")
                        .vec("to", "End of the line."))
                        .require("block", "from", "to").build(),
                (args, ctx) -> {
                    Block block = BlockResolver.resolve(args, "block", ctx, true);
                    List<Vec3> positions = Shapes.line(args.vec("from"), args.vec("to"));
                    return describe(ctx, block, ctx.editor().place(positions, block, false));
                }));

        tools.add(new Tool("ellipsoid",
                "Place a sphere or ellipsoid. Radii count blocks from the centre block outward, so radius 3 spans 7 blocks.",
                blockParams(Schema.object()
                        .vec("center", "Centre block.")
                        .vec("radii", "Radius along x, y and z."))
                        .bool("hollow", "Only the outer shell (default false).")
                        .bool("only_empty", "If true, keep blocks that are already there (default false).")
                        .require("block", "center", "radii").build(),
                (args, ctx) -> {
                    Block block = BlockResolver.resolve(args, "block", ctx, true);
                    Vec3 r = args.vec("radii");
                    if (r.x() < 0 || r.y() < 0 || r.z() < 0) throw new ToolException("radii must not be negative.");
                    List<Vec3> positions = Shapes.ellipsoid(args.vec("center"), r.x(), r.y(), r.z(),
                            args.boolOr("hollow", false), ctx.editor().maxBlocksPerOp());
                    return describe(ctx, block, ctx.editor().place(positions, block, args.boolOr("only_empty", false)));
                }));

        tools.add(new Tool("cylinder",
                "Place a cylinder, cone or tapered tube along an axis: engines, nacelles, barrels, noses, fuselages. "
                        + "It starts at 'start' and extends 'length' blocks toward the positive axis direction.",
                blockParams(Schema.object()
                        .vec("start", "Centre of the first cross-section.")
                        .choice("axis", "Axis the cylinder runs along.", "x", "y", "z")
                        .integer("length", "Number of blocks along the axis.")
                        .integer("radius", "Cross-section radius in blocks from the centre block.")
                        .integer("radius2", "Optional second radius for an elliptical cross-section "
                                + "(axis x: radius=y, radius2=z; axis y: radius=x, radius2=z; axis z: radius=x, radius2=y)."))
                        .number("end_scale", "Radius multiplier at the far end: 1 = cylinder (default), 0 = cone, 0.5 = taper to half.")
                        .bool("hollow", "Only the outer shell (default false).")
                        .bool("only_empty", "If true, keep blocks that are already there (default false).")
                        .require("block", "start", "axis", "length", "radius").build(),
                (args, ctx) -> {
                    Block block = BlockResolver.resolve(args, "block", ctx, true);
                    Shapes.Axis axis = switch (args.str("axis").toLowerCase()) {
                        case "x" -> Shapes.Axis.X;
                        case "y" -> Shapes.Axis.Y;
                        case "z" -> Shapes.Axis.Z;
                        default -> throw new ToolException("axis must be x, y or z.");
                    };
                    int length = args.integer("length");
                    int ra = args.integer("radius");
                    int rb = args.intOr("radius2", ra);
                    if (length < 1 || ra < 0 || rb < 0) throw new ToolException("length must be at least 1 and radii not negative.");
                    double endScale = Math.max(0, Math.min(4, args.numberOr("end_scale", 1.0)));
                    List<Vec3> positions = Shapes.cylinder(args.vec("start"), axis, length, ra, rb, endScale,
                            args.boolOr("hollow", false), ctx.editor().maxBlocksPerOp());
                    return describe(ctx, block, ctx.editor().place(positions, block, args.boolOr("only_empty", false)));
                }));

        ObjectNode item = new ObjectMapper().createObjectNode();
        item.put("type", "object");
        ObjectNode itemProps = item.putObject("properties");
        itemProps.set("pos", Schema.vecItems());
        itemProps.putObject("block").put("type", "string");
        itemProps.putObject("shape").put("type", "string");
        itemProps.putObject("facing").put("type", "string");
        itemProps.putObject("orientation").put("type", "integer");
        item.putArray("required").add("pos").add("block");
        tools.add(new Tool("place_blocks",
                "Place individual blocks, each with its own position and type. For details such as computers, "
                        + "lights and single decorative blocks; use fill and the shape tools for anything large. At most "
                        + MAX_EXPLICIT_BLOCKS + " per call.",
                Schema.object()
                        .array("blocks", "Blocks to place: {pos:[x,y,z], block, shape?, facing?}.", item)
                        .require("blocks").build(),
                BuildTools::placeBlocks));

        tools.add(new Tool("replace",
                "Swap every block of one type inside a box for another block, e.g. to recolour a hull.",
                Schema.object()
                        .vec("from", "One corner of the box.")
                        .vec("to", "Opposite corner of the box.")
                        .str("old_block", "Block to replace (name or id).")
                        .str("new_block", "Block to put in its place (name or id), or \"air\" to remove.")
                        .choice("facing", FACING_DESC, "front", "back", "top", "bottom", "right", "left")
                        .require("from", "to", "old_block", "new_block").build(),
                (args, ctx) -> {
                    short oldType = BlockResolver.lookup(args.str("old_block").trim(), ctx.catalog()).id();
                    Block to = BlockResolver.resolve(args, "new_block", ctx, true);
                    Editor.Result r = ctx.editor().replace(args.box("from", "to"), oldType, to);
                    return r.changed() + " blocks of " + ctx.catalog().name(oldType) + " replaced with "
                            + (to.isAir() ? "nothing (removed)" : ctx.catalog().name(to.type())) + ".";
                }));

        tools.add(new Tool("remove",
                "Remove all blocks inside a box. The core cannot be removed.",
                Schema.object()
                        .vec("from", "One corner of the box.")
                        .vec("to", "Opposite corner of the box.")
                        .require("from", "to").build(),
                (args, ctx) -> {
                    List<Vec3> positions = Shapes.box(args.box("from", "to"), Shapes.Fill.SOLID, ctx.editor().maxBlocksPerOp());
                    return ctx.editor().remove(positions).changed() + " blocks removed.";
                }));

        tools.add(new Tool("set_symmetry",
                "Turn mirroring on or off. While on, everything you place or remove is mirrored across the plane "
                        + "through the core: x mirrors left/right, y mirrors up/down, z mirrors front/back. "
                        + "Ships are almost always x-symmetric, so build one side and let this do the other.",
                Schema.object()
                        .bool("x", "Mirror left/right.")
                        .bool("y", "Mirror up/down.")
                        .bool("z", "Mirror front/back.")
                        .build(),
                (args, ctx) -> {
                    ctx.editor().setSymmetry(args.boolOr("x", false), args.boolOr("y", false), args.boolOr("z", false));
                    return "Symmetry: " + ctx.editor().symmetryText();
                }));

        tools.add(new Tool("link",
                "Link a controller block (weapon computer, logic block, etc.) to the blocks it controls. "
                        + "Give either explicit target positions or a box; with a box, every block in it that the "
                        + "controller can control is linked. Systems do nothing until linked.",
                Schema.object()
                        .vec("controller", "Position of the controlling block.")
                        .array("targets", "Target positions.", Schema.vecItems())
                        .vec("from", "One corner of a box of targets (alternative to targets).")
                        .vec("to", "Opposite corner of the box.")
                        .require("controller").build(),
                (args, ctx) -> ctx.editor().link(args.vec("controller"), targets(args, ctx, true))));

        tools.add(new Tool("unlink",
                "Remove links from a controller block to targets (explicit positions or everything in a box).",
                Schema.object()
                        .vec("controller", "Position of the controlling block.")
                        .array("targets", "Target positions.", Schema.vecItems())
                        .vec("from", "One corner of a box of targets (alternative to targets).")
                        .vec("to", "Opposite corner of the box.")
                        .require("controller").build(),
                (args, ctx) -> ctx.editor().unlink(args.vec("controller"), targets(args, ctx, false)) + " links removed."));

        return tools;
    }

    private static Schema blockParams(Schema s) {
        return s.str("block", BLOCK_DESC)
                .choice("shape", SHAPE_DESC, "cube", "wedge", "corner", "tetra", "hepta")
                .choice("facing", FACING_DESC, "front", "back", "top", "bottom", "right", "left");
    }

    private static String describe(ToolContext ctx, Block block, Editor.Result r) {
        return (block.isAir() ? "Removed: " : ctx.catalog().name(block.type()) + ": ") + r;
    }

    private static String placeBlocks(Args args, ToolContext ctx) throws ToolException {
        JsonNode list = args.raw("blocks");
        if (!list.isArray() || list.isEmpty()) throw new ToolException("\"blocks\" must be a non-empty array.");
        if (list.size() > MAX_EXPLICIT_BLOCKS) {
            throw new ToolException("At most " + MAX_EXPLICIT_BLOCKS + " blocks per call; use fill or a shape tool for large areas.");
        }
        // Group by block so each distinct block is one editor operation.
        Map<Block, List<Vec3>> groups = new LinkedHashMap<>();
        for (JsonNode n : list) {
            Args item = new Args(n);
            groups.computeIfAbsent(BlockResolver.resolve(item, "block", ctx, true), k -> new ArrayList<>()).add(item.vec("pos"));
        }
        StringBuilder sb = new StringBuilder();
        for (Map.Entry<Block, List<Vec3>> e : groups.entrySet()) {
            Editor.Result r = ctx.editor().place(e.getValue(), e.getKey(), false);
            sb.append(describe(ctx, e.getKey(), r)).append('\n');
        }
        return sb.toString();
    }

    /** Explicit targets, or every block in the given box (only linkable ones when linking). */
    private static List<Vec3> targets(Args args, ToolContext ctx, boolean linkableOnly) throws ToolException {
        List<Vec3> explicit = args.vecList("targets");
        if (!explicit.isEmpty()) return explicit;
        Vec3 controller = args.vec("controller");
        if (!args.has("from") && !args.has("to")) {
            throw new ToolException("Say which blocks: either \"targets\": [[x,y,z], ...] or a box, e.g. "
                    + "{\"controller\": [0,0,1], \"from\": [0,0,2], \"to\": [0,0,6]}.");
        }
        // With only one corner given, the box runs from the controller to that corner.
        Box region = Box.of(args.has("from") ? args.vec("from") : controller, args.has("to") ? args.vec("to") : controller);
        BlockCatalog catalog = ctx.catalog();
        return ctx.editor().gateway().call(w -> {
            short controllerType = w.get(controller.x(), controller.y(), controller.z()).type();
            List<Vec3> found = new ArrayList<>();
            w.forEachBlock(region, (x, y, z, b) -> {
                Vec3 p = new Vec3(x, y, z);
                if (p.equals(controller)) return;
                if (!linkableOnly || catalog.canLink(controllerType, b.type())) found.add(p);
            });
            return found;
        });
    }
}
