import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.PrintStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Proxy;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import it.unimi.dsi.fastutil.longs.Long2IntOpenHashMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectOpenHashMap;
import it.unimi.dsi.fastutil.longs.LongIterator;
import it.unimi.dsi.fastutil.objects.ObjectArrayFIFOQueue;
import it.unimi.dsi.fastutil.shorts.Short2ObjectMap;
import it.unimi.dsi.fastutil.shorts.Short2ObjectOpenHashMap;

import org.schema.common.util.linAlg.Vector3i;
import org.schema.game.common.controller.ElementCountMap;
import org.schema.game.common.controller.SegmentBufferInterface;
import org.schema.game.common.controller.SegmentController;
import org.schema.game.common.controller.SegmentDataManager;
import org.schema.game.common.controller.SegmentProvider;
import org.schema.game.common.controller.Ship;
import org.schema.game.common.controller.elements.EntityIndexScore;
import org.schema.game.common.controller.io.IOFileManager;
import org.schema.game.common.controller.io.SegmentDataIONew;
import org.schema.game.common.controller.io.SegmentHeader;
import org.schema.game.common.controller.io.SegmentRegionFileNew;
import org.schema.game.common.data.VoidSegmentPiece;
import org.schema.game.common.data.element.ControlElementMapper;
import org.schema.game.common.data.element.ElementCollection;
import org.schema.game.common.data.element.ElementInformation;
import org.schema.game.common.data.element.ElementKeyMap;
import org.schema.game.common.data.world.RemoteSegment;
import org.schema.game.common.data.world.SegmentData;
import org.schema.game.common.data.world.SimpleTransformableSendableObject;
import org.schema.game.common.util.FastCopyLongOpenHashSet;
import org.schema.game.server.controller.BluePrintController;
import org.schema.game.server.controller.ServerSegmentProvider;
import org.schema.game.server.data.blueprintnw.BBWirelessLogicMarker;
import org.schema.game.server.data.blueprintnw.BlueprintEntry;
import org.schema.game.server.data.blueprintnw.BlueprintType;
import org.schema.schine.graphicsengine.forms.BoundingBox;
import org.schema.schine.network.StateInterface;
import org.schema.schine.resource.tag.Tag;

/**
 * Loads a StarMade blueprint (.sment or blueprint directory) with the game's own classes and
 * prints one JSON document describing what the game read. See README.md.
 *
 * Must run with the working directory set to a scratch directory that contains the game's
 * "data" folder (run.sh arranges that); the game resolves ./data/config and ./bbtmp relatively.
 */
public final class GameVerify {

    /** Ship core position in blueprint ("file") space. */
    static final int CORE = 16;
    static final short CORE_TYPE = 1;
    static final int OFF = 1 << 20;

    static boolean summaryOnly = false;
    static File blocksDump = null;

    public static void main(String[] args) {
        PrintStream realOut = System.out;
        PrintStream realErr = System.err;
        Map<String, Object> root = new LinkedHashMap<>();
        List<Object> errors = new ArrayList<>();
        List<Object> warnings = new ArrayList<>();
        String input = null;
        boolean strict = false;
        try {
            for (int i = 0; i < args.length; i++) {
                switch (args[i]) {
                    case "--summary": summaryOnly = true; break;
                    case "--strict": strict = true; break;
                    case "--blocks": blocksDump = new File(args[++i]); break;
                    case "-h":
                    case "--help": input = null; i = args.length; break;
                    default:
                        if (args[i].startsWith("--")) throw new IllegalArgumentException("unknown option " + args[i]);
                        input = args[i];
                }
            }
            if (input == null) {
                realErr.println("usage: run.sh [--summary] [--strict] [--blocks out.tsv] <file.sment | blueprint-dir>");
                realErr.println("  --summary   leave out the per-link target lists (counts and hashes stay)");
                realErr.println("  --strict    exit 2 when there are warnings (e.g. header counts differ from the block data)");
                realErr.println("  --blocks f  also write every block as a TSV line to f");
                realErr.println("exit status: 0 ok, 1 the game rejected something or would drop data, 2 warnings with --strict");
                System.exit(64);
            }
            // Everything the game prints goes to a log, stdout stays pure JSON.
            String logPath = System.getProperty("gameverify.log");
            PrintStream log = logPath != null ? new PrintStream(new FileOutputStream(logPath), true, "UTF-8") : realErr;
            System.setOut(log);
            System.setErr(log);

            root.put("tool", "game-verify");
            root.put("input", input);
            run(new File(input), root, errors, warnings);
        } catch (Throwable t) {
            errors.add(describe("harness", t));
        }
        // entity-level errors/warnings bubble up so one look at the top is enough
        Object ents = root.get("entities");
        if (ents instanceof List) {
            for (Object o : (List<?>) ents) {
                Map<?, ?> e = (Map<?, ?>) o;
                for (Object x : (List<?>) e.get("errors")) errors.add(e.get("path") + ": " + x);
                for (Object x : (List<?>) e.get("warnings")) warnings.add(e.get("path") + ": " + x);
            }
        }
        root.put("ok", errors.isEmpty());
        root.put("errors", errors);
        root.put("warnings", warnings);
        StringBuilder sb = new StringBuilder(1 << 16);
        Json.write(sb, root, 0);
        sb.append('\n');
        realOut.print(sb);
        realOut.flush();
        int code = !errors.isEmpty() ? 1 : (strict && !warnings.isEmpty() ? 2 : 0);
        System.exit(code); // the game leaves non-daemon threads behind
    }

    static String describe(String where, Throwable t) {
        StringWriter sw = new StringWriter();
        t.printStackTrace(new PrintWriter(sw));
        String[] lines = sw.toString().split("\n");
        StringBuilder sb = new StringBuilder(where).append(": ").append(t);
        for (int i = 1; i < Math.min(lines.length, 7); i++) sb.append(" | ").append(lines[i].trim());
        return sb.toString();
    }

    // ------------------------------------------------------------------------------------------

    static void run(File input, Map<String, Object> root, List<Object> errors, List<Object> warnings) throws Exception {
        if (!input.exists()) throw new IOException("no such file or directory: " + input);

        try {
            org.schema.game.common.version.Version.loadVersion();
        } catch (Throwable t) {
            // version.txt not linked into the work dir: only cosmetic
        }
        root.put("gameVersion", org.schema.game.common.version.Version.VERSION + "#" + org.schema.game.common.version.Version.BUILD);

        // GAME: block config (./data/config/BlockConfig.xml, BlockTypes.properties)
        ElementKeyMap.initializeData(null);
        // GAME: factories for SERIALIZABLE tags (manager tags contain them); done by Starter at boot
        org.schema.game.common.Starter.registerSerializableFactories();
        // GAME: shape/orientation tables; only the migration of pre-v6 segment data needs them
        try {
            org.schema.game.client.view.cubes.shapes.BlockShapeAlgorithm.initialize();
        } catch (Throwable t) {
            warnings.add(describe("block shape tables failed to initialise; old-format segments cannot be migrated", t));
        }

        File bpRoot = new File("blueprints").getAbsoluteFile();
        Files.createDirectories(bpRoot.toPath());
        BluePrintController controller = new BluePrintController(bpRoot.getPath(), new File(bpRoot, "DATA").getPath());

        String name;
        if (input.isDirectory()) {
            root.put("inputKind", "directory");
            if (!new File(input, "header.smbph").isFile()) {
                throw new IOException("not a blueprint directory (no header.smbph): " + input);
            }
            name = input.getName();
            copyTree(input.toPath(), new File(bpRoot, name).toPath()); // never touch the original: the game opens region files read-write
        } else {
            root.put("inputKind", "sment");
            // GAME: the import an admin upload goes through (size check, unzip to ./bbtmp, layout checks, copy into the catalog)
            name = BlueprintEntry.importFile(input, controller);
        }
        root.put("blueprintName", name);
        root.put("positionSpace", "file space: block coordinates exactly as stored in the blueprint (a ship's core is at 16,16,16; subtract 16 for core-relative)");
        Map<String, Object> hash = new LinkedHashMap<>();
        hash.put("algorithm", "SHA-256, lowercase hex");
        hash.put("order", "blocks sorted ascending by x, then y, then z (signed)");
        hash.put("contentHash", "per block 17 bytes: x,y,z as big-endian i32; type as big-endian u16; orientation u8; active u8 (0 or 1); hp u8");
        hash.put("rawHash", "per block 15 bytes: x,y,z as big-endian i32; then the 3 stored bytes b0,b1,b2 (value = b0 | b1<<8 | b2<<16)");
        hash.put("linksHash", "one record per (controller, type, target), sorted by controller (x,y,z), then type, then target (x,y,z); 26 bytes: controller x,y,z as big-endian i32; type as big-endian u16; target x,y,z as big-endian i32");
        root.put("hash", hash);

        // GAME: what BluePrintController.loadBlueprint does for every catalog entry
        BlueprintEntry entry = new BlueprintEntry(name, controller);
        entry.rootRead = true;
        entry.read(); // header + meta (+ children, recursively) + mod mappings

        List<Object> entities = new ArrayList<>();
        root.put("entities", entities);
        Stubs stubs = new Stubs();
        walk(entry, null, bpRoot, controller, stubs, entities);
    }

    static void walk(BlueprintEntry entry, String parent, File bpRoot, BluePrintController controller, Stubs stubs, List<Object> out) {
        Map<String, Object> e = new LinkedHashMap<>();
        List<Object> errors = new ArrayList<>();
        List<Object> warnings = new ArrayList<>();
        String path = entryPath(entry);
        e.put("path", path);
        e.put("parent", parent == null ? null : rel(bpRoot, parent));
        out.add(e);
        try {
            describeEntity(entry, path, bpRoot, controller, stubs, e, errors, warnings, out.size() - 1);
        } catch (Throwable t) {
            errors.add(describe("entity", t));
        }
        e.put("errors", errors);
        e.put("warnings", warnings);
        List<BlueprintEntry> childs = entry.getChilds();
        if (childs != null) {
            for (BlueprintEntry c : childs) walk(c, path, bpRoot, controller, stubs, out);
        }
    }

    /** The directory name the entry was constructed with (BlueprintEntry.getName() has dots stripped). */
    static String entryPath(BlueprintEntry entry) {
        try {
            File header = (File) get(BlueprintEntry.class, entry, "header");
            return header.getParentFile().getPath();
        } catch (Throwable t) {
            return entry.getName();
        }
    }

    static void describeEntity(BlueprintEntry entry, String absPath, File bpRoot, BluePrintController controller, Stubs stubs,
            Map<String, Object> e, List<Object> errors, List<Object> warnings, int index) throws Exception {
        File dir = new File(absPath);
        String rel = rel(bpRoot, absPath);
        e.put("path", rel);

        // ---- docking (how the parent's meta describes this child) ----
        if (entry.railTag != null || entry.getDockingPos() != null) {
            Map<String, Object> d = new LinkedHashMap<>();
            d.put("kind", entry.railDock ? "rail" : "legacy");
            if (entry.getDockingPos() != null) {
                d.put("dockingPos", v3(entry.getDockingPos()));
                d.put("dockingSize", new double[] {entry.getDockingSize().x, entry.getDockingSize().y, entry.getDockingSize().z});
                d.put("dockingStyle", (int) entry.getDockingStyle());
                d.put("dockingOrientation", (int) entry.getDockingOrientation());
            }
            if (entry.railTag != null) d.put("railTag", tag(entry.railTag, 0));
            e.put("docking", d);
        }

        // ---- header: GAME BlueprintEntry.readHeader on a fresh entry, so counts are exactly as stored ----
        File headerFile = new File(dir, "header.smbph");
        BlueprintEntry raw = new BlueprintEntry(rel, controller);
        raw.readHeader(headerFile);
        Map<String, Object> h = new LinkedHashMap<>();
        h.put("version", raw.getHeaderVersion());
        h.put("savedInGameVersion", get(BlueprintEntry.class, raw, "blueprintSavedInGameVersion"));
        BlueprintType type = raw.getEntityType();
        h.put("entityType", type.name());
        h.put("entityTypeOrdinal", type.ordinal());
        h.put("classification", raw.getClassification().name());
        h.put("classificationOrdinal", raw.getClassification().ordinal());
        BoundingBox bb = raw.getBb();
        Map<String, Object> bounds = new LinkedHashMap<>();
        bounds.put("min", new double[] {bb.min.x, bb.min.y, bb.min.z});
        bounds.put("max", new double[] {bb.max.x, bb.max.y, bb.max.z});
        h.put("bounds", bounds);
        EntityIndexScore score = raw.getScore();
        h.put("hasScore", score != null);
        h.put("fileSize", headerFile.length());
        e.put("header", h);
        e.put("entityType", type.name());
        e.put("classification", raw.getClassification().name());
        e.put("bounds", bounds);
        if (!(bb.min.x <= bb.max.x && bb.min.y <= bb.max.y && bb.min.z <= bb.max.z)) {
            errors.add("header bounding box is inverted; BluePrintController.loadBluePrint throws 'INVALID AABB' on spawn");
        }
        TreeMap<Integer, Integer> headerCounts = counts(raw.getElementMap());
        e.put("headerCounts", strKeys(headerCounts));
        TreeMap<Integer, Integer> converted = counts(entry.getElementMap()); // after read(): convertNonPlaceableBlocks()
        if (!converted.equals(headerCounts)) e.put("headerCountsAfterGameConversion", strKeys(converted));
        e.put("price", entry.getPrice());
        e.put("mass", entry.getMass());

        // ---- meta: already parsed by entry.read(); also make the game parse the manager tag like a spawn does ----
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("version", entry.metaVersionRead);
        m.put("fileSize", new File(dir, "meta.smbpm").length());
        m.put("railUID", entry.getRailUID());
        List<Object> childNames = new ArrayList<>();
        if (entry.getChilds() != null) {
            for (BlueprintEntry c : entry.getChilds()) {
                childNames.add(rel(bpRoot, entryPath(c)));
            }
        }
        m.put("children", childNames);
        Long2ObjectOpenHashMap<VoidSegmentPiece> dockers = entry.getDockerPoints();
        List<Object> dockerList = new ArrayList<>();
        if (dockers != null) {
            for (VoidSegmentPiece p : dockers.values()) {
                Map<String, Object> dp = new LinkedHashMap<>();
                dp.put("pos", v3(p.voidPos));
                dp.put("type", (int) p.getType());
                dp.put("orientation", (int) p.getOrientation());
                dp.put("active", p.isActive());
                dp.put("hp", (int) p.getHitpointsByte());
                dockerList.add(dp);
            }
        }
        m.put("dockerPoints", dockerList);
        m.put("hadCargoByte", entry.hadCargoByteRead());
        m.put("cargoCapacity", entry.getCapacitySingle());
        List<Object> wl = new ArrayList<>();
        for (BBWirelessLogicMarker w : entry.wirelessToOwnRail) {
            Map<String, Object> wm = new LinkedHashMap<>();
            wm.put("marking", w.marking);
            wm.put("from", idx(w.fromLocation));
            wm.put("marker", idx(w.markerLocation));
            wl.add(wm);
        }
        m.put("wirelessToOwnRail", wl);
        m.put("aiTag", entry.getAiTag() != null);
        try {
            BlueprintEntry mt = new BlueprintEntry(rel, controller);
            mt.readMeta(new File(dir, "meta.smbpm"), true, false); // GAME: segManager=true parses the manager container tag
            Tag mgr = mt.getManagerTag();
            m.put("managerTag", mgr != null);
            if (mgr != null) m.put("managerTagName", mgr.getName());
        } catch (Throwable t) {
            errors.add(describe("meta manager tag", t));
        }
        e.put("meta", m);

        // ---- links: GAME BlueprintEntry.readStructure -> ControlElementMapper.deserialize ----
        File logicFile = new File(dir, "logic.smbpl");
        ControlElementMapper map = null;
        if (logicFile.isFile()) {
            map = entry.readStructure(true);
        } else {
            errors.add("logic.smbpl missing");
        }

        // ---- block data: GAME region file + segment deserialisation ----
        Blocks blocks = new Blocks();
        readBlocks(entry, dir, type, stubs, index, e, blocks, errors, warnings);
        blocks.index();

        TreeMap<Integer, Integer> blockCounts = new TreeMap<>();
        int invalid = 0;
        int[] mn = {Integer.MAX_VALUE, Integer.MAX_VALUE, Integer.MAX_VALUE};
        int[] mx = {Integer.MIN_VALUE, Integer.MIN_VALUE, Integer.MIN_VALUE};
        for (int i = 0; i < blocks.n; i++) {
            int t = blocks.type(i);
            blockCounts.merge(t, 1, Integer::sum);
            if (!ElementKeyMap.isValidType((short) t)) invalid++;
            int x = blocks.x(i), y = blocks.y(i), z = blocks.z(i);
            mn[0] = Math.min(mn[0], x); mn[1] = Math.min(mn[1], y); mn[2] = Math.min(mn[2], z);
            mx[0] = Math.max(mx[0], x); mx[1] = Math.max(mx[1], y); mx[2] = Math.max(mx[2], z);
        }
        e.put("blockCounts", strKeys(blockCounts));
        e.put("totalBlocks", blocks.n);
        if (blocks.n > 0) {
            Map<String, Object> bbm = new LinkedHashMap<>();
            bbm.put("min", mn);
            bbm.put("max", mx);
            e.put("blockBounds", bbm);
            // Header boxes are core-relative (file position - 16) and the game writes (min - 1, max + 2).
            double[] exMin = {mn[0] - CORE - 1, mn[1] - CORE - 1, mn[2] - CORE - 1};
            double[] exMax = {mx[0] - CORE + 2, mx[1] - CORE + 2, mx[2] - CORE + 2};
            Map<String, Object> ex = new LinkedHashMap<>();
            ex.put("min", exMin);
            ex.put("max", exMax);
            e.put("boundsFromBlocks", ex);
            boolean same = exMin[0] == bb.min.x && exMin[1] == bb.min.y && exMin[2] == bb.min.z && exMax[0] == bb.max.x && exMax[1] == bb.max.y && exMax[2] == bb.max.z;
            e.put("boundsMatchBlocks", same);
            boolean covers = bb.min.x <= mn[0] - CORE && bb.min.y <= mn[1] - CORE && bb.min.z <= mn[2] - CORE
                    && bb.max.x >= mx[0] - CORE + 1 && bb.max.y >= mx[1] - CORE + 1 && bb.max.z >= mx[2] - CORE + 1;
            if (!covers) {
                warnings.add("header bounding box " + Arrays.toString((double[]) bounds.get("min")) + ".." + Arrays.toString((double[]) bounds.get("max"))
                        + " does not contain the blocks (a box written by the game would be " + Arrays.toString(exMin) + ".." + Arrays.toString(exMax) + ")");
            }
        }
        if (invalid > 0) errors.add(invalid + " blocks have a type id the game's block config does not know");
        boolean match = blockCounts.equals(headerCounts);
        e.put("countsMatchHeader", match);
        if (!match) warnings.add("block counts read from segment data differ from header counts: " + diff(headerCounts, blockCounts));
        if (type == BlueprintType.SHIP) {
            int ci = blocks.find(CORE, CORE, CORE);
            boolean core = ci >= 0 && blocks.type(ci) == CORE_TYPE;
            e.put("coreAt16", core);
            if (!core) errors.add("ship has no core block (type 1) at 16,16,16");
            if (blockCounts.getOrDefault((int) CORE_TYPE, 0) > 1) warnings.add("more than one core block");
        }
        MessageDigest content = MessageDigest.getInstance("SHA-256");
        MessageDigest rawHash = MessageDigest.getInstance("SHA-256");
        blocks.hash(content, rawHash);
        e.put("contentHash", hex(content.digest()));
        e.put("rawHash", hex(rawHash.digest()));

        // ---- links out, cross-checked against the blocks ----
        if (map != null) {
            links(map, blocks, e, errors, warnings);
        }
        if (blocksDump != null) blocks.dump(blocksDump, rel, index > 0);
    }

    // ------------------------------------------------------------------------------------------
    // Block data

    static final Pattern REGION = Pattern.compile("^(.*)\\.(-?\\d+)\\.(-?\\d+)\\.(-?\\d+)\\.smd3$");

    static void readBlocks(BlueprintEntry entry, File dir, BlueprintType type, Stubs stubs, int index,
            Map<String, Object> e, Blocks blocks, List<Object> errors, List<Object> warnings) throws Exception {
        List<Object> regionOut = new ArrayList<>();
        e.put("regionFiles", regionOut);
        if (!entry.existRawData()) {
            errors.add("no DATA directory; BluePrintController.loadBluePrint throws 'no raw DATA dir'");
            return;
        }
        // Same file selection as BluePrintController.loadBluePrint (mine, transcribed: the original needs a GameServerState).
        File[] rawTest = entry.getRawBlockData();
        File[] rawFiles = null;
        if (rawTest != null) {
            for (File f : rawTest) {
                if (f.isDirectory()) rawFiles = f.listFiles();
            }
        }
        if (rawFiles == null) rawFiles = rawTest;
        if (rawFiles == null) rawFiles = new File[0];

        // The game copies every .smd3 into the server database as <ENTITY UID><everything from the first dot of the
        // source name>, in listFiles() order, overwriting earlier files that map to the same name.
        String uid = "ENTITY_SHIP_gameverify" + index;
        File db = new File("db" + index).getAbsoluteFile();
        Files.createDirectories(db.toPath());
        Map<String, List<String>> targets = new LinkedHashMap<>();
        int smd3 = 0;
        for (File f : rawFiles) {
            if (f.isDirectory()) {
                warnings.add("directory inside DATA is ignored by the game: " + f.getName());
                continue;
            }
            if (!f.getName().toLowerCase(Locale.ENGLISH).endsWith(".smd3")) {
                warnings.add("non-.smd3 file inside DATA is ignored by the game: " + f.getName());
                continue;
            }
            smd3++;
            String noPoints = BlueprintEntry.removePoints(f.getName()); // GAME
            String target = uid + noPoints.substring(noPoints.indexOf('.'));
            Files.copy(f.toPath(), new File(db, target).toPath(), StandardCopyOption.REPLACE_EXISTING);
            targets.computeIfAbsent(target, k -> new ArrayList<>()).add(f.getName());
        }
        e.put("expectedNonEmptySegments", smd3);
        if (smd3 == 0) warnings.add("no .smd3 files in DATA (the game logs 'has no DATA files' and spawns an empty entity)");

        TreeMap<String, Integer> dataTypes = new TreeMap<>();
        int segments = 0, emptySegments = 0;
        ByteBuffer buf = ByteBuffer.allocate(1 << 20);
        for (Map.Entry<String, List<String>> t : targets.entrySet()) {
            Map<String, Object> r = new LinkedHashMap<>();
            regionOut.add(r);
            List<String> sources = t.getValue();
            r.put("file", sources.get(sources.size() - 1));
            if (sources.size() > 1) {
                r.put("shadowedFiles", new ArrayList<Object>(sources.subList(0, sources.size() - 1)));
                warnings.add("several DATA files map to the same region on spawn " + sources + "; the game keeps whichever its directory listing returns last (here: "
                        + sources.get(sources.size() - 1) + ")");
            }
            File f = new File(db, t.getKey());
            r.put("size", f.length());
            Matcher mm = REGION.matcher(t.getKey());
            if (!mm.matches() || !mm.group(1).equals(uid)) {
                errors.add("region file name does not end in .<x>.<y>.<z>.smd3 and would never be found by the game: " + sources);
                continue;
            }
            int rx = Integer.parseInt(mm.group(2)), ry = Integer.parseInt(mm.group(3)), rz = Integer.parseInt(mm.group(4));
            r.put("region", new int[] {rx, ry, rz});
            if (f.length() < SegmentHeader.HEADER_SIZE) {
                errors.add("region file shorter than its " + SegmentHeader.HEADER_SIZE + "-byte header (the game would overwrite it with an empty header): " + sources);
                continue;
            }
            if ((f.length() - SegmentHeader.HEADER_SIZE) % SegmentHeader.SEGMENT_SECTOR != 0) {
                warnings.add("region file length is not header + n*49152: " + f.length() + " " + sources);
            }
            int rSegs = 0, rEmpty = 0, rBlocks = 0;
            SegmentRegionFileNew rf = null;
            try {
                rf = (SegmentRegionFileNew) IOFileManager.getFromFile(f); // GAME: parses the offset/size table
                r.put("version", rf.getVersion());
                if (rf.getVersion() != SegmentDataIONew.VERSION) warnings.add("region file version " + rf.getVersion() + " (current is " + SegmentDataIONew.VERSION + ")");
                SegmentHeader head = rf.getHeader();
                long slots = (f.length() - SegmentHeader.HEADER_SIZE) / SegmentHeader.SEGMENT_SECTOR;
                for (int lz = 0; lz < 16; lz++) {
                    for (int ly = 0; ly < 16; ly++) {
                        for (int lx = 0; lx < 16; lx++) {
                            int x = (rx * 16 + lx - 8) * 32, y = (ry * 16 + ly - 8) * 32, z = (rz * 16 + lz - 8) * 32;
                            if (head.isNoData(x, y, z)) continue; // offset 0: never written
                            if (head.isEmpty(x, y, z)) { // size 0: known empty
                                rEmpty++;
                                continue;
                            }
                            int size = head.getSize(x, y, z);
                            int slot = SegmentHeader.convertToDataOffset(head.getOffset(x, y, z));
                            if (slot < 0 || slot >= slots || size > SegmentHeader.SEGMENT_SECTOR) {
                                errors.add("segment " + x + "," + y + "," + z + " points outside the file (slot " + slot + " of " + slots + ", size " + size + ")");
                                continue;
                            }
                            RemoteSegment seg = new RemoteSegment(stubs.ship(uid));
                            seg.compressionCheck = false; // keep the data exactly as stored
                            int res;
                            try {
                                // GAME: reads the slot and runs RemoteSegment.deserialize (inflate + decode)
                                res = SegmentDataIONew.requestStatic(x, y, z, uid, buf, db.getPath() + File.separator, seg);
                            } catch (Throwable th) {
                                errors.add(describe("segment " + x + "," + y + "," + z + " in " + sources, th));
                                continue;
                            }
                            if (res != 0) {
                                errors.add("segment " + x + "," + y + "," + z + ": game returned read code " + res);
                                continue;
                            }
                            rSegs++;
                            SegmentData data = seg.getSegmentData();
                            if (data == null) {
                                dataTypes.merge("EMPTY_MARKER", 1, Integer::sum);
                                continue;
                            }
                            dataTypes.merge(data.getDataType().name(), 1, Integer::sum);
                            int before = blocks.n;
                            for (int i = 0; i < 32768; i++) {
                                short ty = data.getType(i); // GAME accessors from here on
                                if (ty == 0) continue;
                                blocks.add(x + SegmentData.getPosXFromIndex(i), y + SegmentData.getPosYFromIndex(i), z + SegmentData.getPosZFromIndex(i),
                                        ty & 0xFFFF, data.getOrientation(i) & 0xFF, data.isActive(i), data.getHitpointsByte(i) & 0xFF, data.getDataAt(i) & 0xFFFFFF);
                            }
                            rBlocks += blocks.n - before;
                            if (blocks.n == before) warnings.add("segment " + x + "," + y + "," + z + " is stored with data but contains no blocks");
                        }
                    }
                }
            } catch (Throwable th) {
                errors.add(describe("region file " + sources, th));
            } finally {
                if (rf != null) {
                    try { rf.close(); } catch (Throwable ignored) { }
                }
            }
            r.put("segments", rSegs);
            r.put("emptySegments", rEmpty);
            r.put("blocks", rBlocks);
            segments += rSegs;
            emptySegments += rEmpty;
        }
        e.put("segments", segments);
        e.put("emptySegments", emptySegments);
        e.put("segmentDataTypes", dataTypes);
    }

    /** Growable block table: positions packed into a sortable key, decoded fields from the game's accessors. */
    static final class Blocks {
        long[] key = new long[1 << 12];
        int[] dec = new int[1 << 12];
        int[] raw = new int[1 << 12];
        int n;
        Long2IntOpenHashMap byPos;
        int[] order;

        void add(int x, int y, int z, int type, int orientation, boolean active, int hp, int rawValue) {
            if (Math.abs(x) >= OFF || Math.abs(y) >= OFF || Math.abs(z) >= OFF) throw new IllegalStateException("coordinate out of range: " + x + "," + y + "," + z);
            if (n == key.length) {
                key = Arrays.copyOf(key, n * 2);
                dec = Arrays.copyOf(dec, n * 2);
                raw = Arrays.copyOf(raw, n * 2);
            }
            key[n] = pack(x, y, z);
            dec[n] = (type << 16) | (orientation << 8) | (hp << 1) | (active ? 1 : 0);
            raw[n] = rawValue;
            n++;
        }

        static long pack(int x, int y, int z) {
            return ((long) (x + OFF) << 42) | ((long) (y + OFF) << 21) | (long) (z + OFF);
        }

        int x(int i) { return (int) (key[i] >>> 42) - OFF; }
        int y(int i) { return (int) ((key[i] >>> 21) & 0x1FFFFF) - OFF; }
        int z(int i) { return (int) (key[i] & 0x1FFFFF) - OFF; }
        int type(int i) { return dec[i] >>> 16; }
        int orientation(int i) { return (dec[i] >>> 8) & 0xFF; }
        int hp(int i) { return (dec[i] >>> 1) & 0x7F; }
        boolean active(int i) { return (dec[i] & 1) != 0; }

        void index() {
            byPos = new Long2IntOpenHashMap(Math.max(16, n));
            byPos.defaultReturnValue(-1);
            for (int i = 0; i < n; i++) {
                if (byPos.put(key[i], i) != -1) throw new IllegalStateException("two blocks at " + x(i) + "," + y(i) + "," + z(i));
            }
            long[] sorted = Arrays.copyOf(key, n);
            Arrays.sort(sorted);
            order = new int[n];
            for (int i = 0; i < n; i++) order[i] = byPos.get(sorted[i]);
        }

        int find(int x, int y, int z) {
            if (Math.abs(x) >= OFF || Math.abs(y) >= OFF || Math.abs(z) >= OFF) return -1;
            return byPos.get(pack(x, y, z));
        }

        void hash(MessageDigest content, MessageDigest rawHash) {
            ByteBuffer a = ByteBuffer.allocate(17 * 4096);
            ByteBuffer b = ByteBuffer.allocate(15 * 4096);
            for (int k = 0; k < n; k++) {
                int i = order[k];
                a.putInt(x(i)).putInt(y(i)).putInt(z(i)).putShort((short) type(i)).put((byte) orientation(i)).put((byte) (active(i) ? 1 : 0)).put((byte) hp(i));
                b.putInt(x(i)).putInt(y(i)).putInt(z(i)).put((byte) raw[i]).put((byte) (raw[i] >> 8)).put((byte) (raw[i] >> 16));
                if (!a.hasRemaining()) {
                    content.update(a.array(), 0, a.position());
                    a.clear();
                    rawHash.update(b.array(), 0, b.position());
                    b.clear();
                }
            }
            content.update(a.array(), 0, a.position());
            rawHash.update(b.array(), 0, b.position());
        }

        void dump(File file, String entity, boolean append) throws IOException {
            try (PrintStream ps = new PrintStream(new FileOutputStream(file, append), false, "UTF-8")) {
                if (!append) ps.println("entity\tx\ty\tz\ttype\torientation\tactive\thp\traw");
                for (int k = 0; k < n; k++) {
                    int i = order[k];
                    ps.println(entity + "\t" + x(i) + "\t" + y(i) + "\t" + z(i) + "\t" + type(i) + "\t" + orientation(i) + "\t" + (active(i) ? 1 : 0) + "\t" + hp(i)
                            + "\t" + String.format("%06x", raw[i]));
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------
    // Links

    static void links(ControlElementMapper map, Blocks blocks, Map<String, Object> e, List<Object> errors, List<Object> warnings) throws Exception {
        // controller -> type -> sorted targets, all positions decoded with the game's ElementCollection
        TreeMap<Long, TreeMap<Integer, long[]>> sorted = new TreeMap<>();
        LongIterator keys = map.keySet().iterator();
        while (keys.hasNext()) {
            long ck = keys.nextLong();
            Short2ObjectOpenHashMap<FastCopyLongOpenHashSet> byType = map.get(ck);
            TreeMap<Integer, long[]> types = new TreeMap<>();
            for (Short2ObjectMap.Entry<FastCopyLongOpenHashSet> en : byType.short2ObjectEntrySet()) {
                long[] targets = new long[en.getValue().size()];
                int i = 0;
                LongIterator it = en.getValue().iterator();
                while (it.hasNext()) {
                    long t = it.nextLong();
                    targets[i++] = Blocks.pack(ElementCollection.getPosX(t), ElementCollection.getPosY(t), ElementCollection.getPosZ(t));
                }
                Arrays.sort(targets);
                types.put(en.getShortKey() & 0xFFFF, targets);
            }
            sorted.put(Blocks.pack(ElementCollection.getPosX(ck), ElementCollection.getPosY(ck), ElementCollection.getPosZ(ck)), types);
        }
        MessageDigest md = MessageDigest.getInstance("SHA-256");
        ByteBuffer rec = ByteBuffer.allocate(26);
        List<Object> out = new ArrayList<>();
        int controllers = 0, groups = 0, total = 0, missingController = 0, missingTarget = 0, typeMismatch = 0, emptyGroups = 0;
        for (Map.Entry<Long, TreeMap<Integer, long[]>> c : sorted.entrySet()) {
            controllers++;
            int[] cp = unpack(c.getKey());
            if (blocks.find(cp[0], cp[1], cp[2]) < 0) missingController++;
            for (Map.Entry<Integer, long[]> g : c.getValue().entrySet()) {
                groups++;
                if (g.getValue().length == 0) emptyGroups++;
                Map<String, Object> l = new LinkedHashMap<>();
                l.put("controller", cp);
                l.put("type", g.getKey());
                List<Object> targets = new ArrayList<>();
                for (long t : g.getValue()) {
                    int[] tp = unpack(t);
                    total++;
                    int bi = blocks.find(tp[0], tp[1], tp[2]);
                    if (bi < 0) missingTarget++;
                    else if (blocks.type(bi) != g.getKey()) typeMismatch++;
                    rec.clear();
                    rec.putInt(cp[0]).putInt(cp[1]).putInt(cp[2]).putShort((short) (int) g.getKey()).putInt(tp[0]).putInt(tp[1]).putInt(tp[2]);
                    md.update(rec.array(), 0, 26);
                    if (!summaryOnly) targets.add(tp);
                }
                if (summaryOnly) l.put("targetCount", g.getValue().length);
                else l.put("targets", targets);
                out.add(l);
            }
        }
        e.put("linkControllers", controllers);
        e.put("linkGroups", groups);
        e.put("linkTargets", total);
        e.put("linksHash", hex(md.digest()));
        e.put("links", out);
        if (missingController > 0) warnings.add(missingController + " link controllers sit on positions without a block");
        if (missingTarget > 0) warnings.add(missingTarget + " link targets sit on positions without a block");
        if (typeMismatch > 0) warnings.add(typeMismatch + " link targets have a block of a different type than the link records");
        if (emptyGroups > 0) warnings.add(emptyGroups + " link groups have no targets");
    }

    static int[] unpack(long k) {
        return new int[] {(int) (k >>> 42) - OFF, (int) ((k >>> 21) & 0x1FFFFF) - OFF, (int) (k & 0x1FFFFF) - OFF};
    }

    // ------------------------------------------------------------------------------------------
    // Stand-ins for the parts of a running server that segment deserialisation touches.
    // None of them contain format logic: they only let RemoteSegment.deserialize find a clock,
    // a server flag, an entity UID and a source of empty SegmentData objects.

    static final class Stubs {
        final Object unsafe; // sun.misc.Unsafe, used reflectively: only for allocateInstance
        final java.lang.reflect.Method allocate;
        final StateInterface state;
        final SegmentBufferInterface buffer;
        final SegmentProvider provider;
        final Map<String, Ship> ships = new LinkedHashMap<>();

        Stubs() throws Exception {
            Class<?> u = Class.forName("sun.misc.Unsafe");
            Field f = u.getDeclaredField("theUnsafe");
            f.setAccessible(true);
            unsafe = f.get(null);
            allocate = u.getMethod("allocateInstance", Class.class);
            state = (StateInterface) Proxy.newProxyInstance(GameVerify.class.getClassLoader(), new Class<?>[] {StateInterface.class}, handler("GameVerifyState"));
            buffer = (SegmentBufferInterface) Proxy.newProxyInstance(GameVerify.class.getClassLoader(), new Class<?>[] {SegmentBufferInterface.class},
                    handler("GameVerifySegmentBuffer"));
            SegmentDataManager mgr = (SegmentDataManager) alloc(SegmentDataManager.class);
            set(SegmentDataManager.class, mgr, "state", state);
            set(SegmentDataManager.class, mgr, "unusedSegmentData", new ObjectArrayFIFOQueue<SegmentData>(16));
            provider = (SegmentProvider) alloc(ServerSegmentProvider.class);
            set(SegmentProvider.class, provider, "segmentDataManager", mgr);
        }

        /** Creates an object without running its constructor. */
        Object alloc(Class<?> c) throws Exception {
            return allocate.invoke(unsafe, c);
        }

        Ship ship(String uid) throws Exception {
            Ship s = ships.get(uid);
            if (s == null) {
                s = (Ship) alloc(Ship.class); // no constructor: a real Ship needs a whole GameServerState
                set(SimpleTransformableSendableObject.class, s, "state", state);
                set(SimpleTransformableSendableObject.class, s, "onServer", Boolean.TRUE);
                set(SegmentController.class, s, "uniqueIdentifier", uid);
                set(SegmentController.class, s, "segmentBuffer", buffer);
                set(SegmentController.class, s, "segmentProvider", provider);
                ships.put(uid, s);
            }
            return s;
        }

        static InvocationHandler handler(String name) {
            return (proxy, method, a) -> {
                switch (method.getName()) {
                    case "getUpdateTime": return System.currentTimeMillis();
                    case "toString": return name;
                    case "hashCode": return System.identityHashCode(proxy);
                    case "equals": return proxy == a[0];
                    default:
                }
                Class<?> r = method.getReturnType();
                if (!r.isPrimitive() || r == void.class) return null;
                if (r == boolean.class) return false;
                if (r == long.class) return 0L;
                if (r == int.class) return 0;
                if (r == short.class) return (short) 0;
                if (r == byte.class) return (byte) 0;
                if (r == float.class) return 0f;
                if (r == double.class) return 0d;
                return (char) 0;
            };
        }
    }

    static void set(Class<?> declaring, Object target, String field, Object value) throws Exception {
        Field f = declaring.getDeclaredField(field);
        f.setAccessible(true);
        f.set(target, value);
    }

    static Object get(Class<?> declaring, Object target, String field) throws Exception {
        Field f = declaring.getDeclaredField(field);
        f.setAccessible(true);
        return f.get(target);
    }

    // ------------------------------------------------------------------------------------------
    // Helpers

    static TreeMap<Integer, Integer> counts(ElementCountMap map) {
        TreeMap<Integer, Integer> out = new TreeMap<>();
        if (map == null) return out;
        for (int t = 1; t < 2048; t++) {
            if (!ElementKeyMap.exists(t)) continue;
            int c = map.get((short) t);
            if (c != 0) out.put(t, c);
        }
        return out;
    }

    static Map<String, Object> strKeys(TreeMap<Integer, Integer> in) {
        Map<String, Object> out = new LinkedHashMap<>();
        for (Map.Entry<Integer, Integer> x : in.entrySet()) out.put(String.valueOf(x.getKey()), x.getValue());
        return out;
    }

    static String diff(TreeMap<Integer, Integer> header, TreeMap<Integer, Integer> blocks) {
        StringBuilder sb = new StringBuilder();
        TreeMap<Integer, Integer> all = new TreeMap<>(header);
        for (Integer k : blocks.keySet()) all.putIfAbsent(k, 0);
        int shown = 0;
        for (Integer k : all.keySet()) {
            int a = header.getOrDefault(k, 0), b = blocks.getOrDefault(k, 0);
            if (a == b) continue;
            if (shown++ < 12) {
                ElementInformation info = ElementKeyMap.isValidType(k.shortValue()) ? ElementKeyMap.getInfo(k.shortValue()) : null;
                sb.append(sb.length() > 0 ? "; " : "").append("type ").append(k).append(info != null ? " (" + info.getName() + ")" : "").append(": header ").append(a).append(", data ").append(b);
            }
        }
        if (shown > 12) sb.append("; … ").append(shown - 12).append(" more");
        return sb.toString();
    }

    static String rel(File bpRoot, String absPath) {
        return bpRoot.toPath().relativize(new File(absPath).toPath()).toString().replace(File.separatorChar, '/');
    }

    static int[] v3(Vector3i v) {
        return new int[] {v.x, v.y, v.z};
    }

    static int[] idx(long index) {
        return new int[] {ElementCollection.getPosX(index), ElementCollection.getPosY(index), ElementCollection.getPosZ(index)};
    }

    static String hex(byte[] b) {
        StringBuilder sb = new StringBuilder(b.length * 2);
        for (byte x : b) sb.append(String.format("%02x", x));
        return sb.toString();
    }

    /** Generic dump of the game's Tag tree (used for rail docking tags). */
    static Object tag(Tag t, int depth) {
        Map<String, Object> m = new LinkedHashMap<>();
        if (t.getName() != null) m.put("name", t.getName());
        m.put("type", String.valueOf(t.getType()));
        Object v = t.getValue();
        if (v instanceof Tag[]) {
            List<Object> l = new ArrayList<>();
            for (Tag c : (Tag[]) v) {
                if (c == null || c.getType() == Tag.Type.FINISH) continue;
                l.add(depth > 12 ? "…" : tag(c, depth + 1));
            }
            m.put("value", l);
        } else if (v instanceof byte[]) {
            byte[] b = (byte[]) v;
            m.put("length", b.length);
            m.put("hex", hex(Arrays.copyOf(b, Math.min(b.length, 64))));
        } else if (v instanceof Number || v instanceof Boolean) {
            m.put("value", v);
        } else if (v instanceof Vector3i) {
            m.put("value", v3((Vector3i) v));
        } else {
            m.put("value", String.valueOf(v));
        }
        return m;
    }

    static void copyTree(Path from, Path to) throws IOException {
        Files.walkFileTree(from, new SimpleFileVisitor<Path>() {
            @Override
            public FileVisitResult preVisitDirectory(Path d, BasicFileAttributes a) throws IOException {
                Files.createDirectories(to.resolve(from.relativize(d).toString()));
                return FileVisitResult.CONTINUE;
            }

            @Override
            public FileVisitResult visitFile(Path f, BasicFileAttributes a) throws IOException {
                Files.copy(f, to.resolve(from.relativize(f).toString()), StandardCopyOption.REPLACE_EXISTING);
                return FileVisitResult.CONTINUE;
            }
        });
    }

    /** Minimal JSON writer: maps and lists of maps go one entry per line, number lists stay on one line. */
    static final class Json {
        static void write(StringBuilder sb, Object o, int ind) {
            if (o == null) {
                sb.append("null");
            } else if (o instanceof Map) {
                Map<?, ?> m = (Map<?, ?>) o;
                if (m.isEmpty()) {
                    sb.append("{}");
                    return;
                }
                boolean flat = flat(m.values());
                sb.append('{');
                boolean first = true;
                for (Map.Entry<?, ?> en : m.entrySet()) {
                    if (!first) sb.append(flat ? ", " : ",");
                    first = false;
                    if (!flat) nl(sb, ind + 1);
                    str(sb, String.valueOf(en.getKey()));
                    sb.append(": ");
                    write(sb, en.getValue(), ind + 1);
                }
                if (!flat) nl(sb, ind);
                sb.append('}');
            } else if (o instanceof List) {
                List<?> l = (List<?>) o;
                if (l.isEmpty()) {
                    sb.append("[]");
                    return;
                }
                boolean flat = flat(l);
                sb.append('[');
                boolean first = true;
                for (Object x : l) {
                    if (!first) sb.append(flat ? ", " : ",");
                    first = false;
                    if (!flat) nl(sb, ind + 1);
                    write(sb, x, ind + 1);
                }
                if (!flat) nl(sb, ind);
                sb.append(']');
            } else if (o instanceof int[]) {
                int[] a = (int[]) o;
                sb.append('[');
                for (int i = 0; i < a.length; i++) sb.append(i > 0 ? "," : "").append(a[i]);
                sb.append(']');
            } else if (o instanceof double[]) {
                double[] a = (double[]) o;
                sb.append('[');
                for (int i = 0; i < a.length; i++) {
                    sb.append(i > 0 ? "," : "");
                    num(sb, a[i]);
                }
                sb.append(']');
            } else if (o instanceof Double || o instanceof Float) {
                num(sb, ((Number) o).doubleValue());
            } else if (o instanceof Number || o instanceof Boolean) {
                sb.append(o);
            } else {
                str(sb, String.valueOf(o));
            }
        }

        static boolean flat(Iterable<?> values) {
            int n = 0;
            for (Object v : values) {
                if (v instanceof Map || v instanceof List || v instanceof String) return false;
                n++;
            }
            return n <= 64 || allArrays(values);
        }

        static boolean allArrays(Iterable<?> values) {
            for (Object v : values) {
                if (!(v instanceof int[])) return false;
            }
            return true;
        }

        static void num(StringBuilder sb, double d) {
            if (Double.isNaN(d) || Double.isInfinite(d)) sb.append("null");
            else if (d == Math.rint(d) && Math.abs(d) < 1e15) sb.append((long) d).append(".0");
            else sb.append(d);
        }

        static void nl(StringBuilder sb, int ind) {
            sb.append('\n');
            for (int i = 0; i < ind; i++) sb.append(' ');
        }

        static void str(StringBuilder sb, String s) {
            sb.append('"');
            for (int i = 0; i < s.length(); i++) {
                char c = s.charAt(i);
                switch (c) {
                    case '"': sb.append("\\\""); break;
                    case '\\': sb.append("\\\\"); break;
                    case '\n': sb.append("\\n"); break;
                    case '\r': sb.append("\\r"); break;
                    case '\t': sb.append("\\t"); break;
                    default:
                        if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                        else sb.append(c);
                }
            }
            sb.append('"');
        }
    }
}
