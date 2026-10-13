import com.bulletphysics.collision.shapes.ConvexHullShape;
import com.bulletphysics.collision.shapes.ConvexShape;
import com.bulletphysics.linearmath.Transform;
import java.io.File;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import javax.vecmath.Matrix3f;
import javax.vecmath.Vector3f;
import org.schema.common.util.linAlg.Vector3i;
import org.schema.game.client.view.cubes.CubeMeshBufferContainer;
import org.schema.game.client.view.cubes.shapes.AlgorithmParameters;
import org.schema.game.client.view.cubes.shapes.BlockShape;
import org.schema.game.client.view.cubes.shapes.BlockShapeAlgorithm;
import org.schema.game.client.view.cubes.shapes.BlockStyle;
import org.schema.game.client.view.cubes.shapes.HalfBlockArray;
import org.schema.game.client.view.cubes.shapes.orientcube.Oriencube;
import org.schema.game.client.view.cubes.shapes.sprite.SpriteShapeAlgorythm;
import org.schema.game.client.view.cubes.shapes.wedge.WedgeShapeAlgorithm;
import org.schema.game.client.view.shader.CubeMeshQuadsShader13;
import org.schema.game.common.data.element.Element;
import org.schema.game.common.data.element.ElementInformation;
import org.schema.game.common.data.element.ElementKeyMap;
import org.schema.game.common.data.world.SegmentData;
import org.schema.schine.graphicsengine.core.GlUtil;

/**
 * Dumps StarMade's block shape and orientation tables to JSON by calling the
 * game's own classes. See README.md for what is called and what is transcribed.
 *
 * Output coordinates: unit cube 0..1 per axis, x=0 is the -X face (Element.LEFT),
 * x=1 the +X face (Element.RIGHT), y=0 BOTTOM, y=1 TOP, z=0 BACK, z=1 FRONT.
 * That is the game's block-local position plus 0.5.
 */
public class ShapeDump {
    static final double EPS = 1e-5;
    static final String[] SIDE_NAMES = new String[6];
    /** The cube shader's `normals` uniform, entries 1..6 (GlUtil.DIRECTIONSf). */
    static Vector3f[] normals;
    /** The cube shader's `quadPosMark` uniform. */
    static Vector3f[] quadPosMark;

    static final List<String> problems = new ArrayList<>();
    static final Map<String, Object> checks = new LinkedHashMap<>();

    public static void main(String[] args) throws Exception {
        String out = args.length > 0 ? args[0] : "shapes.json";
        Locale.setDefault(Locale.ROOT);
        for (int i = 0; i < 6; i++) {
            SIDE_NAMES[i] = Element.getSideString(i);
        }
        normals = GlUtil.DIRECTIONSf;
        quadPosMark = CubeMeshQuadsShader13.quadPosMark;
        BlockShapeAlgorithm.initialize();
        boolean haveBlocks = true;
        try {
            ElementKeyMap.initializeData(null);
        } catch (Throwable t) {
            haveBlocks = false;
            problems.add("ElementKeyMap could not be initialised: " + t);
        }

        Map<String, Object> root = new LinkedHashMap<>();
        root.put("format", 1);
        root.put("source", source());
        root.put("convention", convention());
        root.put("blockBits", blockBits());
        root.put("sides", sides());
        root.put("styles", styles(haveBlocks));
        root.put("shapes", shapes());
        root.put("cube", cubes());
        root.put("slabs", slabs(haveBlocks));
        root.put("handedness", handedness());
        checks.put("problems", problems);
        root.put("checks", checks);
        root.put("notes", notes());

        StringBuilder sb = new StringBuilder();
        Json.write(sb, root, 0);
        sb.append('\n');
        File f = new File(out);
        if (f.getParentFile() != null) {
            f.getParentFile().mkdirs();
        }
        Files.write(f.toPath(), sb.toString().getBytes(StandardCharsets.UTF_8));
        System.out.println("wrote " + f.getAbsolutePath() + " (" + sb.length() + " bytes), problems: " + problems.size());
        for (String p : problems) {
            System.out.println("PROBLEM: " + p);
        }
        System.exit(0);
    }

    // ------------------------------------------------------------------ meta

    static Map<String, Object> source() {
        Map<String, Object> m = new LinkedHashMap<>();
        String version = "unknown";
        try {
            version = new String(Files.readAllBytes(new File("version.txt").toPath()), StandardCharsets.UTF_8).trim();
        } catch (Exception ignored) {
        }
        m.put("game", "StarMade");
        m.put("gameVersion", version);
        m.put("tool", "rust/tools/shape-dump/ShapeDump.java");
        m.put("classes", Arrays.asList(
                "org.schema.game.client.view.cubes.shapes.BlockShapeAlgorithm (algorithms, createSide, getShape, vertexOrderMap, mirror mappings)",
                "org.schema.game.client.view.cubes.shapes.BlockStyle",
                "org.schema.game.client.view.cubes.shapes.HalfBlockArray",
                "org.schema.game.client.view.cubes.CubeMeshBufferContainer (getOrientationCode3/6/24)",
                "org.schema.game.client.view.shader.CubeMeshQuadsShader13.quadPosMark",
                "org.schema.schine.graphicsengine.core.GlUtil.DIRECTIONSf",
                "org.schema.game.common.data.element.Element, ElementKeyMap, ElementInformation"));
        m.put("transcribed", "The vertex position and texture corner formulas come from data/shader/cube/quads13/cube-3rd.vsh and "
                + "were rewritten in Java (ShapeDump.shaderPos); the game's own Java port of that shader, "
                + "org.schema.game.client.view.Segment2ObjWriter.softwareShader, has the same arithmetic. "
                + "Everything else is read from the game's classes at run time.");
        return m;
    }

    static Map<String, Object> convention() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("coordinates", "Unit cube, 0..1 on each axis = game block-local position + 0.5. x=1 is the +X face (Element.RIGHT), "
                + "y=1 the +Y face (TOP), z=1 the +Z face (FRONT).");
        m.put("winding", "faces[].verts are ordered so that cross(v1-v0, v2-v0) points along `normal` (outward) in this right-handed x,y,z frame, "
                + "i.e. counter-clockwise seen from outside. render[].verts keep the game's own emission order instead.");
        m.put("side", "faces[].side / sideId name the cube side the face lies in (Element constants); null for a diagonal face. "
                + "`full` is true when the face is that entire unit square.");
        m.put("sideCoverage", "Per side id 0..5: 2 = the solid covers the whole square, 1 = part of it (a triangle), 0 = nothing.");
        m.put("uv", "render[].verts[].uv is the texture corner the game assigns: u = (ext >> 1) & 1, v = ext & 1, with "
                + "texCoord = (tileColumn + u, tileRow + v) / 16. v grows in the same direction as the tile row index.");
        m.put("orientationIndex", "shapes[style].orientations is indexed by the block's stored 5 bit orientation value. "
                + "The game looks an algorithm up with value % orientationModulus; values up to modulus-1 are listed.");
        return m;
    }

    static Map<String, Object> blockBits() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("typeMask", SegmentData.typeMask);
        m.put("hpMask", SegmentData.hpMask);
        m.put("activeMask", SegmentData.activeMask);
        m.put("orientMask", SegmentData.orientMask);
        m.put("orientationStart", SegmentData.orientationStart);
        m.put("orientationBitCount", SegmentData.orientationBitCount);
        m.put("note", "Current layout (segment version byte 3 and later, SegmentDataIntArray): the 5 orientation bits are used as they are; "
                + "the active bit is not part of the orientation. Segment versions 0..2 used an older layout and are migrated on load "
                + "(SegmentData3Byte.convertOrient: for corner, tetra, hepta and normal24 blocks new = (old4bits + (active ? 0 : 16)) % 24).");
        return m;
    }

    static List<Object> sides() {
        List<Object> l = new ArrayList<>();
        for (int i = 0; i < 6; i++) {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("id", i);
            m.put("name", SIDE_NAMES[i]);
            Vector3i d = Element.DIRECTIONSi[i];
            m.put("dir", new int[]{d.x, d.y, d.z});
            m.put("opposite", Element.getOpposite(i));
            m.put("switchLeftRight", (int) Element.switchLeftRight(i));
            m.put("flag", Element.SIDE_FLAG[i]);
            l.add(m);
        }
        return l;
    }

    static List<Object> styles(boolean haveBlocks) {
        List<Object> l = new ArrayList<>();
        for (BlockStyle s : BlockStyle.values()) {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("id", s.id);
            m.put("name", s.name());
            m.put("realName", s.realName);
            m.put("description", s.desc);
            m.put("cube", s.cube);
            m.put("solidBlockStyle", s.solidBlockStyle);
            m.put("blendedBlockStyle", s.blendedBlockStyle);
            m.put("orientations", s.orientations);
            int maxStored = (1 << SegmentData.orientationBitCount) - 1;
            if (s.id > 0) {
                BlockShapeAlgorithm[] row = BlockShapeAlgorithm.algorithms[s.id - 1];
                m.put("orientationModulus", row.length - 1);
                m.put("maxStoredValue", Math.min(maxStored, row.length - 2));
                m.put("iconAlgorithm", row[row.length - 1].getClass().getSimpleName());
            } else {
                m.put("orientationModulus", null);
                m.put("maxStoredValue", maxStored);
            }
            if (haveBlocks) {
                int count = 0;
                TreeMap<Integer, Integer> slabs = new TreeMap<>();
                TreeMap<Integer, Integer> indiv = new TreeMap<>();
                int lod = 0;
                List<String> examples = new ArrayList<>();
                List<String> several = new ArrayList<>();
                for (short id : sortedIds()) {
                    ElementInformation info = ElementKeyMap.getInfo(id);
                    if (info.getBlockStyle() != s) {
                        continue;
                    }
                    count++;
                    slabs.merge(info.getSlab(), 1, Integer::sum);
                    indiv.merge(info.getIndividualSides(), 1, Integer::sum);
                    if (info.hasLod()) {
                        lod++;
                    }
                    if (examples.size() < 4) {
                        examples.add(id + " " + info.getName());
                    }
                    if (!s.cube && info.getIndividualSides() != 1) {
                        several.add(id + " " + info.getName() + " (individualSides " + info.getIndividualSides() + ")");
                    }
                }
                m.put("blockCount", count);
                m.put("blocksBySlab", strKeys(slabs));
                m.put("blocksByIndividualSides", strKeys(indiv));
                m.put("blocksWithLodModel", lod);
                m.put("examples", examples);
                if (!s.cube) {
                    m.put("blocksWithSeveralTextures", several);
                }
            }
            l.add(m);
        }
        return l;
    }

    static List<Short> sortedIds() {
        List<Short> ids = new ArrayList<>(ElementKeyMap.keySet);
        java.util.Collections.sort(ids);
        return ids;
    }

    static Map<String, Object> strKeys(Map<Integer, Integer> in) {
        Map<String, Object> m = new LinkedHashMap<>();
        for (Map.Entry<Integer, Integer> e : in.entrySet()) {
            m.put(String.valueOf(e.getKey()), e.getValue());
        }
        return m;
    }

    // ---------------------------------------------------------------- shapes

    /** Transcription of the position math in data/shader/cube/quads13/cube-3rd.vsh. Result is centred (-0.5..0.5). */
    static double[] shaderPos(int sid, int vertNumCode, int eleE, int eleEV, int xyScaleManip) {
        double[] n = {normals[sid].x, normals[sid].y, normals[sid].z};
        double[] q = {quadPosMark[sid].x, quadPosMark[sid].y, quadPosMark[sid].z};
        double quarters = vertNumCode * 0.25;
        double edge = eleEV * 0.25;
        double[] p = new double[3];
        for (int a = 0; a < 3; a++) {
            int mm = ((int) (quarters * q[a])) & 1;
            double extra = ((q[a] == 4.0 && xyScaleManip > 0.5) || (q[a] == 2.0 && xyScaleManip < 0.5)) ? edge * (mm > 0 ? 1.0 : -1.0) : 0.0;
            double base = (-0.5 - (Math.abs(n[a]) * -0.5)) + mm - extra;
            p[a] = base + n[a] * 0.5 - eleE * n[a] * 0.25;
        }
        return p;
    }

    static double[] unit(double[] centred) {
        return new double[]{centred[0] + 0.5, centred[1] + 0.5, centred[2] + 0.5};
    }

    static final class RenderFace {
        int emittedForSide;
        List<double[]> pos = new ArrayList<>();
        List<int[]> uv = new ArrayList<>();
        List<Integer> sid = new ArrayList<>();
        int normalMode;
        boolean normalModeUniform = true;

        List<double[]> polygon() {
            List<double[]> poly = new ArrayList<>();
            for (double[] p : pos) {
                if (poly.isEmpty() || !same(poly.get(poly.size() - 1), p)) {
                    poly.add(p);
                }
            }
            while (poly.size() > 1 && same(poly.get(0), poly.get(poly.size() - 1))) {
                poly.remove(poly.size() - 1);
            }
            // a vertex may repeat non-consecutively in a degenerate quad
            List<double[]> distinct = new ArrayList<>();
            for (double[] p : poly) {
                boolean dup = false;
                for (double[] d : distinct) {
                    dup |= same(d, p);
                }
                if (!dup) {
                    distinct.add(p);
                }
            }
            return distinct.size() == poly.size() ? poly : distinct;
        }
    }

    /** What the game's mesher would emit for one block: one quad per side id, via createSide. */
    static List<RenderFace> renderFaces(BlockShapeAlgorithm algo, BlockStyle style, int orientation, int slab) {
        List<RenderFace> faces = new ArrayList<>();
        AlgorithmParameters p = new AlgorithmParameters();
        int sideCount = algo.hasExtraSide() ? 7 : 6;
        for (int sideId = 0; sideId < sideCount; sideId++) {
            RenderFace f = new RenderFace();
            f.emittedForSide = sideId;
            for (short i = 0; i < 4; i++) {
                p.fresh = true;
                p.insideMode = 0;
                algo.createSide(sideId, i, p);
                int[] hb = HalfBlockArray.getHalfBlockConfig(style, i, p.sid, orientation, slab);
                int vertNum = BlockShapeAlgorithm.vertexOrderMap[p.sid][p.vID];
                f.pos.add(unit(shaderPos(p.sid, vertNum, hb[0], hb[1], hb[2])));
                f.uv.add(new int[]{(p.ext >> 1) & 1, p.ext & 1});
                f.sid.add(p.sid);
                if (i == 0) {
                    f.normalMode = p.normalMode;
                } else if (f.normalMode != p.normalMode) {
                    f.normalModeUniform = false;
                }
            }
            faces.add(f);
        }
        return faces;
    }

    static Map<String, Object> shapes() {
        Map<String, Object> all = new LinkedHashMap<>();
        Map<String, Object> volumes = new LinkedHashMap<>();
        Map<String, Object> duplicates = new LinkedHashMap<>();
        int nonPlanar = 0;
        int hullRenderMismatch = 0;
        int hullRenderCompared = 0;
        int renderCcw = 0;
        int renderCw = 0;
        int shadingNormalChecked = 0;
        int shadingNormalMismatch = 0;
        int visMismatch = 0;
        int heptaVisEmpty = 0;
        int mirrorChecked = 0;
        int mirrorMismatch = 0;

        for (BlockStyle style : BlockStyle.values()) {
            if (style.id == 0) {
                continue;
            }
            BlockShapeAlgorithm[] row = BlockShapeAlgorithm.algorithms[style.id - 1];
            int modulus = row.length - 1;
            Map<String, Object> st = new LinkedHashMap<>();
            st.put("styleId", style.id);
            st.put("name", style.realName);
            st.put("orientationModulus", modulus);
            st.put("distinctOrientations", style.orientations);
            List<Object> orientations = new ArrayList<>();
            List<Set<String>> pointSets = new ArrayList<>();
            List<List<double[]>> hulls = new ArrayList<>();
            TreeMap<String, List<Integer>> byVolume = new TreeMap<>();
            Map<String, Integer> firstWithSameGeometry = new LinkedHashMap<>();
            List<String> dupList = new ArrayList<>();

            // 5 stored bits: values above 31 cannot occur (the sprite row is longer than that)
            int listed = Math.min(modulus, 1 << SegmentData.orientationBitCount);
            st.put("listedOrientations", listed);
            for (int o = 0; o < listed; o++) {
                BlockShapeAlgorithm algo = BlockShapeAlgorithm.getAlgo(style.id, (byte) o);
                if (algo != row[o]) {
                    problems.add("getAlgo(" + style.id + "," + o + ") is not algorithms[" + (style.id - 1) + "][" + o + "]");
                }
                Map<String, Object> e = new LinkedHashMap<>();
                e.put("orientation", o);
                e.put("algorithm", algo.getClass().getSimpleName());
                BlockShape ann = algo.getClass().getAnnotation(BlockShape.class);
                e.put("name", ann != null ? ann.name() : null);
                e.put("physical", algo.isPhysical());

                List<RenderFace> render = renderFaces(algo, style, o, 0);
                List<double[]> hull = null;
                if (algo.hasValidShape()) {
                    try {
                        ConvexShape shape = BlockShapeAlgorithm.getShape(style.id, (byte) o);
                        if (shape instanceof ConvexHullShape) {
                            hull = new ArrayList<>();
                            for (Vector3f v : ((ConvexHullShape) shape).getPoints()) {
                                hull.add(new double[]{v.x + 0.5, v.y + 0.5, v.z + 0.5});
                            }
                        }
                    } catch (Throwable t) {
                        hull = null;
                    }
                }
                hulls.add(hull);

                String geometryKey;
                if (hull != null) {
                    Set<String> ps = pointSet(hull);
                    pointSets.add(ps);
                    geometryKey = ps.toString();
                    e.put("hull", hull);
                    List<Face> faces = Geometry.hullFaces(hull);
                    double volume = Geometry.volume(faces);
                    e.put("volume", round(volume));
                    byVolume.computeIfAbsent(String.format("%.4f", volume), k -> new ArrayList<>()).add(o);
                    if (volume <= EPS) {
                        problems.add(style.realName + " " + o + ": hull volume " + volume);
                    }
                    List<Object> fl = new ArrayList<>();
                    int[] coverage = new int[6];
                    for (Face f : faces) {
                        if (!Geometry.planar(f.verts, f.normal)) {
                            nonPlanar++;
                        }
                        Map<String, Object> fm = new LinkedHashMap<>();
                        Integer side = f.side();
                        fm.put("side", side == null ? null : SIDE_NAMES[side]);
                        fm.put("sideId", side);
                        boolean full = side != null && f.verts.size() == 4 && f.allCorners();
                        fm.put("full", full);
                        if (side != null) {
                            coverage[side] = full ? 2 : 1;
                        }
                        fm.put("normal", f.normal);
                        fm.put("verts", f.verts);
                        fl.add(new Json.Inline(fm));
                    }
                    e.put("faces", fl);
                    e.put("sideCoverage", coverage);
                    e.put("description", describe(faces, coverage));

                    // cross-check: hull faces against what the mesher emits
                    Set<String> hullFaceSets = new TreeSet<>();
                    for (Face f : faces) {
                        hullFaceSets.add(pointSet(f.verts).toString());
                    }
                    Set<String> renderFaceSets = new TreeSet<>();
                    for (RenderFace rf : render) {
                        List<double[]> poly = rf.polygon();
                        if (poly.size() < 3) {
                            continue;
                        }
                        renderFaceSets.add(pointSet(poly).toString());
                        Face match = null;
                        for (Face f : faces) {
                            if (pointSet(f.verts).equals(pointSet(poly))) {
                                match = f;
                            }
                        }
                        if (match != null) {
                            double[] n = Geometry.newell(poly);
                            if (Geometry.dot(n, match.normal) > 0) {
                                renderCcw++;
                            } else {
                                renderCw++;
                            }
                            if (rf.normalModeUniform) {
                                double[] sn = shadingNormal(rf);
                                if (sn != null) {
                                    shadingNormalChecked++;
                                    if (Geometry.dot(sn, match.normal) < 0.99) {
                                        shadingNormalMismatch++;
                                        problems.add(style.realName + " " + o + " side " + rf.emittedForSide + ": shading normal "
                                                + Arrays.toString(sn) + " vs geometric " + Arrays.toString(match.normal));
                                    }
                                }
                            }
                        }
                    }
                    hullRenderCompared++;
                    boolean agree = hullFaceSets.equals(renderFaceSets);
                    e.put("renderMatchesHull", agree);
                    if (!agree) {
                        hullRenderMismatch++;
                        problems.add(style.realName + " " + o + " (" + algo.getClass().getSimpleName() + "): mesher faces differ from hull faces");
                    }

                    // cross-check: the game's own list of sides that are completely covered
                    try {
                        int[] vis = algo.getSidesToCheckForVis();
                        e.put("gameSidesToCheckForVis", vis);
                        TreeSet<Integer> fullSides = new TreeSet<>();
                        for (int s = 0; s < 6; s++) {
                            if (coverage[s] == 2) {
                                fullSides.add(s);
                            }
                        }
                        TreeSet<Integer> gameSides = new TreeSet<>();
                        for (int s : vis) {
                            gameSides.add(s);
                        }
                        if (!fullSides.equals(gameSides)) {
                            if (style == BlockStyle.HEPTA && gameSides.isEmpty()) {
                                heptaVisEmpty++;
                            } else {
                                visMismatch++;
                                problems.add(style.realName + " " + o + ": full sides " + fullSides + " but getSidesToCheckForVis " + gameSides);
                            }
                        }
                    } catch (Throwable ignored) {
                    }
                } else {
                    pointSets.add(null);
                    StringBuilder key = new StringBuilder();
                    for (RenderFace rf : render) {
                        key.append(rf.emittedForSide).append(pointList(rf.pos)).append(uvList(rf.uv));
                    }
                    geometryKey = key.toString();
                    e.put("hull", null);
                }
                try {
                    e.put("gameSidesAngled", algo.getSidesAngled());
                } catch (Throwable ignored) {
                }
                try {
                    e.put("gameSidesOpenToAir", algo.getSidesOpenToAir());
                } catch (Throwable ignored) {
                }
                if (algo instanceof WedgeShapeAlgorithm) {
                    byte[] wo = algo.getWedgeOrientation();
                    e.put("wedgeOrientation", wo == null ? null : new int[]{wo[0], wo[1]});
                }
                if (algo instanceof SpriteShapeAlgorythm) {
                    e.put("primary", (int) ((SpriteShapeAlgorythm) algo).getPrimaryOrientation());
                }
                if (algo instanceof Oriencube) {
                    Oriencube oc = (Oriencube) algo;
                    e.put("primary", (int) oc.getOrientCubePrimaryOrientation());
                    e.put("secondary", (int) oc.getOrientCubeSecondaryOrientation());
                }

                if (algo.hasExtraSide()) {
                    e.put("extraSideEmittedWith", algo.getSixthSideOrientation());
                }
                Integer first = firstWithSameGeometry.putIfAbsent(geometryKey, o);
                e.put("sameAs", first);
                if (first != null) {
                    dupList.add(o + "=" + first);
                }

                // the game only fills the mirror tables for the first 24 values of each style
                if (o < BlockShapeAlgorithm.xyMappings[style.id - 1].length - 1) {
                    Map<String, Object> mirror = new LinkedHashMap<>();
                    mirror.put("xy", BlockShapeAlgorithm.xyMappings[style.id - 1][o]);
                    mirror.put("xz", BlockShapeAlgorithm.xzMappings[style.id - 1][o]);
                    mirror.put("yz", BlockShapeAlgorithm.yzMappings[style.id - 1][o]);
                    e.put("mirror", new Json.Inline(mirror));
                } else {
                    e.put("mirror", null);
                }

                List<Object> rl = new ArrayList<>();
                for (RenderFace rf : render) {
                    Map<String, Object> rm = new LinkedHashMap<>();
                    rm.put("emittedForSide", rf.emittedForSide);
                    List<double[]> poly = rf.polygon();
                    rm.put("drawn", poly.size() >= 3);
                    if (rf.emittedForSide < 6) {
                        rm.put("angled", algo.isAngled(rf.emittedForSide));
                    }
                    rm.put("normalMode", rf.normalMode);
                    List<Object> vl = new ArrayList<>();
                    for (int i = 0; i < 4; i++) {
                        Map<String, Object> vm = new LinkedHashMap<>();
                        vm.put("pos", rf.pos.get(i));
                        vm.put("uv", rf.uv.get(i));
                        vl.add(new Json.Inline(vm));
                    }
                    rm.put("verts", vl);
                    rl.add(new Json.Inline(rm));
                }
                e.put("render", rl);
                orientations.add(e);
            }

            // mirror tables against the geometry (solid styles only)
            for (int o = 0; o < listed; o++) {
                List<double[]> hull = hulls.get(o);
                if (hull == null) {
                    continue;
                }
                int[] maps = {BlockShapeAlgorithm.yzMappings[style.id - 1][o], BlockShapeAlgorithm.xzMappings[style.id - 1][o],
                        BlockShapeAlgorithm.xyMappings[style.id - 1][o]};
                String[] names = {"yz", "xz", "xy"};
                for (int axis = 0; axis < 3; axis++) {
                    List<double[]> mirrored = new ArrayList<>();
                    for (double[] p : hull) {
                        double[] q = p.clone();
                        q[axis] = 1.0 - q[axis];
                        mirrored.add(q);
                    }
                    mirrorChecked++;
                    Set<String> target = maps[axis] >= 0 && maps[axis] < pointSets.size() ? pointSets.get(maps[axis]) : null;
                    if (target == null || !target.equals(pointSet(mirrored))) {
                        mirrorMismatch++;
                        problems.add(style.realName + " " + o + ": mirror." + names[axis] + " -> " + maps[axis] + " does not match the mirrored geometry");
                    }
                }
            }

            st.put("orientations", orientations);
            all.put(String.valueOf(style.id), st);
            Map<String, Object> vm = new LinkedHashMap<>();
            for (Map.Entry<String, List<Integer>> en : byVolume.entrySet()) {
                vm.put(en.getKey(), en.getValue());
            }
            volumes.put(style.realName, vm);
            duplicates.put(style.realName, dupList);
        }

        checks.put("hullVolumesByStyle", volumes);
        checks.put("duplicateOrientations", duplicates);
        checks.put("nonPlanarFaces", nonPlanar);
        checks.put("hullVersusMesherCompared", hullRenderCompared);
        checks.put("hullVersusMesherMismatches", hullRenderMismatch);
        checks.put("mesherPolygonsCounterClockwise", renderCcw);
        checks.put("mesherPolygonsClockwise", renderCw);
        checks.put("shadingNormalsChecked", shadingNormalChecked);
        checks.put("shadingNormalMismatches", shadingNormalMismatch);
        checks.put("fullSidesVersusGameVisListMismatches", visMismatch);
        checks.put("heptaOrientationsWhereGameVisListIsEmpty", heptaVisEmpty);
        checks.put("mirrorMappingsChecked", mirrorChecked);
        checks.put("mirrorMappingMismatches", mirrorMismatch);
        return all;
    }

    static String describe(List<Face> faces, int[] coverage) {
        List<String> full = new ArrayList<>();
        List<String> part = new ArrayList<>();
        List<String> open = new ArrayList<>();
        for (int s = 0; s < 6; s++) {
            (coverage[s] == 2 ? full : coverage[s] == 1 ? part : open).add(SIDE_NAMES[s]);
        }
        StringBuilder sb = new StringBuilder();
        sb.append("full faces: ").append(full.isEmpty() ? "none" : String.join(", ", full));
        sb.append("; triangles on: ").append(part.isEmpty() ? "none" : String.join(", ", part));
        sb.append("; nothing on: ").append(open.isEmpty() ? "none" : String.join(", ", open));
        for (Face f : faces) {
            if (f.side() == null) {
                List<String> toward = new ArrayList<>();
                for (int s = 0; s < 6; s++) {
                    Vector3i d = Element.DIRECTIONSi[s];
                    if (f.normal[0] * d.x + f.normal[1] * d.y + f.normal[2] * d.z > EPS) {
                        toward.add(SIDE_NAMES[s] + (d.x != 0 ? (d.x > 0 ? "(+X)" : "(-X)") : d.y != 0 ? (d.y > 0 ? "(+Y)" : "(-Y)") : (d.z > 0 ? "(+Z)" : "(-Z)")));
                    }
                }
                sb.append("; slanted face (").append(f.verts.size() == 3 ? "triangle" : "quad").append(") faces ").append(String.join(" and ", toward));
            }
        }
        return sb.toString();
    }

    /** Decodes the mesher's normalMode the way the shader does: three indices into normals[0..6]. */
    static double[] shadingNormal(RenderFace rf) {
        double[] n = new double[3];
        if (rf.normalMode == 0) {
            int sid = rf.sid.get(0);
            for (int s : rf.sid) {
                if (s != sid) {
                    return null;
                }
            }
            return new double[]{normals[sid].x, normals[sid].y, normals[sid].z};
        }
        int[] idx = {(rf.normalMode >> 6) & 7, (rf.normalMode >> 3) & 7, rf.normalMode & 7};
        for (int i : idx) {
            if (i > 0) {
                n[0] += normals[i - 1].x;
                n[1] += normals[i - 1].y;
                n[2] += normals[i - 1].z;
            }
        }
        double len = Math.sqrt(Geometry.dot(n, n));
        if (len < EPS) {
            return null;
        }
        return new double[]{n[0] / len, n[1] / len, n[2] / len};
    }

    // ----------------------------------------------------------------- cubes

    static Map<String, Object> cubes() {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("textureSlots", "ElementInformation.textureId[0..5] (the six ids in BlockConfig.xml, in order) are slots "
                + "0 FRONT, 1 BACK, 2 TOP, 3 BOTTOM, 4 RIGHT, 5 LEFT of the block itself. "
                + "textureSlotByWorldSide[s] is the slot drawn on the face lying on world side s; "
                + "worldSideBySlot[k] is the world side(s) slot k ends up on.");

        m.put("sixSided", cubeTable(6));
        m.put("threeSided", cubeTable(3));
        Map<String, Object> one = new LinkedHashMap<>();
        one.put("note", "individualSides 1: every face uses slot 0; the orientation only changes texture rotation "
                + "(texOrder[style][orientation % 6]). The mesher does not clamp the value.");
        m.put("oneSided", one);

        // 24 orientation cubes
        List<Object> l24 = new ArrayList<>();
        BlockShapeAlgorithm[] row = BlockShapeAlgorithm.algorithms[BlockStyle.NORMAL24.id - 1];
        for (int o = 0; o < row.length - 1; o++) {
            Oriencube oc = (Oriencube) row[o];
            Map<String, Object> e = new LinkedHashMap<>();
            e.put("orientation", o);
            e.put("algorithm", oc.getClass().getSimpleName());
            int prim = oc.getOrientCubePrimaryOrientation();
            int sec = oc.getOrientCubeSecondaryOrientation();
            e.put("primary", prim);
            e.put("primaryName", SIDE_NAMES[prim]);
            e.put("secondary", sec);
            e.put("secondaryName", SIDE_NAMES[sec]);
            e.put("meshOrientationArg", o / 4);
            int[] slotBySide = new int[6];
            for (int s = 0; s < 6; s++) {
                slotBySide[s] = CubeMeshBufferContainer.getOrientationCode24(s, o / 4);
            }
            e.put("textureSlotByWorldSide", slotBySide);
            e.put("worldSideBySlot", invert(slotBySide));
            e.put("slot0FacesDir", dirOf(firstSide(slotBySide, 0)));
            Vector3i trueP = Element.DIRECTIONSi[Element.switchLeftRight(prim)];
            Vector3i trueS = Element.DIRECTIONSi[Element.switchLeftRight(sec)];
            e.put("primaryDirSwitched", new int[]{trueP.x, trueP.y, trueP.z});
            e.put("secondaryDirSwitched", new int[]{trueS.x, trueS.y, trueS.z});
            e.put("orientationMatrix", matrix(oc.getOrientationMatrix(new Matrix3f())));
            e.put("orientationMatrixSwitched", matrix(oc.getOrientationMatrixSwitched(new Matrix3f())));
            Transform t = oc.getBasicTransform();
            e.put("lodBasis", matrix(t.basis));
            e.put("lodBasisUp", column(t.basis, 1));
            e.put("lodBasisForward", column(t.basis, 2));
            e.put("mirrorAlgorithm", oc.getMirrorAlgo().getClass().getSimpleName());
            l24.add(e);
        }
        Map<String, Object> n24 = new LinkedHashMap<>();
        n24.put("note", "Style 6 (rails, rotators, display modules and similar). The mesher calls getOrientationCode24(side, orientation / 4), "
                + "so the texture slots depend on the primary side only. orientationMatrix columns are right, up (= DIRECTIONS[primary]), "
                + "forward (= DIRECTIONS[secondary]) as the game builds them; the Switched variant swaps LEFT and RIGHT first. "
                + "lodBasis is the rotation the game applies to the block's LoD model (Oriencube.getBasicTransform); rows are listed, "
                + "lodBasisUp/Forward are where the model's +Y and +Z axes end up.");
        n24.put("orientations", l24);
        m.put("normal24", n24);

        // texture corner order per orientation and side for plain cubes
        Map<String, Object> tex = new LinkedHashMap<>();
        tex.put("note", "For plain cubes: texOrder[style][orientation % 6][side] lists, for the four vertices the mesher emits for that side, "
                + "pos (unit cube) and uv (texture corner). NORMAL is used by default and always for three sided blocks (with orientation 0), "
                + "ORIENT when the block has sideTexturesPointToOrientation, AREA4x4 for extended (4x4 tiled) textures.");
        for (BlockShapeAlgorithm.TexOrderStyle ts : BlockShapeAlgorithm.TexOrderStyle.values()) {
            List<Object> byOrientation = new ArrayList<>();
            for (int o = 0; o < 6; o++) {
                List<Object> bySide = new ArrayList<>();
                for (int s = 0; s < 6; s++) {
                    List<Object> verts = new ArrayList<>();
                    for (int i = 0; i < 4; i++) {
                        int ext = ts.map[o][s][i];
                        int vertNum = BlockShapeAlgorithm.vertexOrderMap[s][i];
                        Map<String, Object> vm = new LinkedHashMap<>();
                        vm.put("pos", unit(shaderPos(s, vertNum, 0, 0, 1)));
                        vm.put("uv", new int[]{(ext >> 1) & 1, ext & 1});
                        verts.add(new Json.Inline(vm));
                    }
                    bySide.add(verts);
                }
                byOrientation.add(bySide);
            }
            tex.put(ts.name(), byOrientation);
        }
        m.put("texOrder", tex);
        return m;
    }

    static Map<String, Object> cubeTable(int individualSides) {
        Map<String, Object> m = new LinkedHashMap<>();
        List<Object> l = new ArrayList<>();
        boolean constant = true;
        int[] firstRow = null;
        for (int o = 0; o < 6; o++) {
            int[] slotBySide = new int[6];
            for (int s = 0; s < 6; s++) {
                slotBySide[s] = individualSides == 6 ? CubeMeshBufferContainer.getOrientationCode6(s, o) : CubeMeshBufferContainer.getOrientationCode3(s, o);
            }
            if (firstRow == null) {
                firstRow = slotBySide;
            } else if (!Arrays.equals(firstRow, slotBySide)) {
                constant = false;
            }
            Map<String, Object> e = new LinkedHashMap<>();
            e.put("orientation", o);
            e.put("orientationName", SIDE_NAMES[o]);
            e.put("textureSlotByWorldSide", slotBySide);
            e.put("worldSideBySlot", invert(slotBySide));
            if (individualSides == 6) {
                int[] dirs = new int[18];
                for (int k = 0; k < 6; k++) {
                    int s = firstSide(slotBySide, k);
                    Vector3i d = Element.DIRECTIONSi[s];
                    dirs[k * 3] = d.x;
                    dirs[k * 3 + 1] = d.y;
                    dirs[k * 3 + 2] = d.z;
                }
                e.put("ownSideDirs", "front " + dirStr(dirs, 0) + " back " + dirStr(dirs, 1) + " top " + dirStr(dirs, 2)
                        + " bottom " + dirStr(dirs, 3) + " right-slot " + dirStr(dirs, 4) + " left-slot " + dirStr(dirs, 5));
                e.put("slot0FacesDir", dirOf(firstSide(slotBySide, 0)));
            }
            l.add(e);
        }
        m.put("orientationClamp", "The mesher clamps the stored value to 0..5 for blocks with individualSides " + individualSides + ".");
        m.put("independentOfOrientation", constant);
        m.put("orientations", l);
        return m;
    }

    static String dirStr(int[] d, int k) {
        return "(" + d[k * 3] + "," + d[k * 3 + 1] + "," + d[k * 3 + 2] + ")";
    }

    static int firstSide(int[] slotBySide, int slot) {
        for (int s = 0; s < 6; s++) {
            if (slotBySide[s] == slot) {
                return s;
            }
        }
        return -1;
    }

    static int[] dirOf(int side) {
        if (side < 0) {
            return null;
        }
        Vector3i d = Element.DIRECTIONSi[side];
        return new int[]{d.x, d.y, d.z};
    }

    static List<Object> invert(int[] slotBySide) {
        List<Object> l = new ArrayList<>();
        for (int k = 0; k < 6; k++) {
            List<Integer> where = new ArrayList<>();
            for (int s = 0; s < 6; s++) {
                if (slotBySide[s] == k) {
                    where.add(s);
                }
            }
            l.add(new Json.Inline(where));
        }
        return l;
    }

    static List<Object> matrix(Matrix3f m) {
        List<Object> rows = new ArrayList<>();
        rows.add(new double[]{r(m.m00), r(m.m01), r(m.m02)});
        rows.add(new double[]{r(m.m10), r(m.m11), r(m.m12)});
        rows.add(new double[]{r(m.m20), r(m.m21), r(m.m22)});
        return rows;
    }

    static double[] column(Matrix3f m, int c) {
        Vector3f v = new Vector3f();
        m.getColumn(c, v);
        return new double[]{r(v.x), r(v.y), r(v.z)};
    }

    static double r(float f) {
        return round(f);
    }

    // ----------------------------------------------------------------- slabs

    static Map<String, Object> slabs(boolean haveBlocks) {
        Map<String, Object> m = new LinkedHashMap<>();
        m.put("note", "Slab geometry as the mesher emits it for a cube style block: HalfBlockArray.getHalfBlockConfig moves vertices in "
                + "quarter block steps. box is [minX,minY,minZ,maxX,maxY,maxZ] in unit cube coordinates.");
        List<Object> thickness = new ArrayList<>();
        List<Object> table = new ArrayList<>();
        int notBox = 0;
        int ruleMismatch = 0;
        for (int slab = 0; slab < 4; slab++) {
            Double thick = null;
            for (int o = 0; o < 6; o++) {
                double[] min = {9, 9, 9};
                double[] max = {-9, -9, -9};
                List<double[]> all = new ArrayList<>();
                for (int s = 0; s < 6; s++) {
                    for (int i = 0; i < 4; i++) {
                        int[] hb = HalfBlockArray.getHalfBlockConfig(BlockStyle.NORMAL, i, s, o, slab);
                        double[] p = unit(shaderPos(s, BlockShapeAlgorithm.vertexOrderMap[s][i], hb[0], hb[1], hb[2]));
                        all.add(p);
                        for (int a = 0; a < 3; a++) {
                            min[a] = Math.min(min[a], p[a]);
                            max[a] = Math.max(max[a], p[a]);
                        }
                    }
                }
                boolean isBox = true;
                Set<String> corners = new TreeSet<>();
                for (double[] p : all) {
                    for (int a = 0; a < 3; a++) {
                        isBox &= Math.abs(p[a] - min[a]) < EPS || Math.abs(p[a] - max[a]) < EPS;
                    }
                    corners.add(key(p));
                }
                isBox &= corners.size() == 8;
                if (!isBox) {
                    notBox++;
                    problems.add("slab " + slab + " orientation " + o + ": emitted vertices are not the 8 corners of a box");
                }
                Integer cutSide = null;
                for (int a = 0; a < 3; a++) {
                    if (max[a] < 1 - EPS) {
                        cutSide = sideForAxis(a, true);
                    }
                    if (min[a] > EPS) {
                        cutSide = sideForAxis(a, false);
                    }
                }
                Map<String, Object> e = new LinkedHashMap<>();
                e.put("slab", slab);
                e.put("orientation", o);
                e.put("orientationName", SIDE_NAMES[o]);
                e.put("box", new double[]{min[0], min[1], min[2], max[0], max[1], max[2]});
                if (cutSide != null) {
                    int against = Element.getOpposite(cutSide);
                    e.put("openSide", SIDE_NAMES[cutSide]);
                    e.put("openSideId", cutSide);
                    e.put("sitsAgainst", SIDE_NAMES[against]);
                    e.put("sitsAgainstId", against);
                    int axis = Element.SIGNIFICANT_COORD[cutSide];
                    double t = max[axis] - min[axis];
                    if (thick == null) {
                        thick = t;
                    } else if (Math.abs(thick - t) > EPS) {
                        problems.add("slab " + slab + ": thickness differs between orientations");
                    }
                    int gameRule = Element.switchLeftRight(Element.getOpposite(o));
                    e.put("gameFullSide", gameRule);
                    if (gameRule != against) {
                        ruleMismatch++;
                        problems.add("slab " + slab + " orientation " + o + ": box sits against " + against + " but the game's visibility rule says " + gameRule);
                    }
                } else {
                    e.put("openSide", null);
                    e.put("sitsAgainst", null);
                }
                table.add(new Json.Inline(e));
            }
            Map<String, Object> t = new LinkedHashMap<>();
            t.put("slab", slab);
            t.put("thickness", slab == 0 ? 1.0 : thick);
            thickness.add(new Json.Inline(t));
        }
        m.put("thickness", thickness);
        m.put("byOrientation", table);

        boolean higherAreFull = true;
        for (int o = 6; o < 32; o++) {
            for (int s = 0; s < 6; s++) {
                for (int i = 0; i < 4; i++) {
                    for (int slab = 0; slab < 4; slab++) {
                        int[] hb = HalfBlockArray.getHalfBlockConfig(BlockStyle.NORMAL, i, s, o, slab);
                        higherAreFull &= hb[0] == 0 && hb[1] == 0;
                    }
                }
            }
        }
        m.put("orientationsAbove5DrawFullCube", higherAreFull);

        if (haveBlocks) {
            Map<String, Object> usage = new LinkedHashMap<>();
            List<String> examples = new ArrayList<>();
            int visRuleChecked = 0;
            int visRuleMismatch = 0;
            for (short id : sortedIds()) {
                ElementInformation info = ElementKeyMap.getInfo(id);
                if (info.getSlab() <= 0) {
                    continue;
                }
                String k = info.getBlockStyle().realName + "/slab" + info.getSlab();
                usage.merge(k, 1, (a, b) -> (Integer) a + (Integer) b);
                if (examples.size() < 6) {
                    examples.add(id + " " + info.getName() + " (slab " + info.getSlab() + ", style " + info.getBlockStyle().realName + ")");
                }
                if (info.getBlockStyle() == BlockStyle.NORMAL) {
                    for (int o = 0; o < 6; o++) {
                        try {
                            int[] vis = BlockShapeAlgorithm.getSidesToCheckForVis(info, (byte) o);
                            visRuleChecked++;
                            if (vis.length != 1 || vis[0] != Element.switchLeftRight(Element.getOpposite(o))) {
                                visRuleMismatch++;
                            }
                        } catch (Throwable ignored) {
                        }
                    }
                }
            }
            m.put("blocksByStyleAndSlab", usage);
            m.put("examples", examples);
            checks.put("slabVisRuleChecked", visRuleChecked);
            checks.put("slabVisRuleMismatches", visRuleMismatch);
        }
        checks.put("slabBoxesNotBoxes", notBox);
        checks.put("slabSitsAgainstVersusGameRuleMismatches", ruleMismatch);
        return m;
    }

    static int sideForAxis(int axis, boolean positive) {
        for (int s = 0; s < 6; s++) {
            Vector3i d = Element.DIRECTIONSi[s];
            int c = axis == 0 ? d.x : axis == 1 ? d.y : d.z;
            if (c == (positive ? 1 : -1)) {
                return s;
            }
        }
        throw new IllegalStateException();
    }

    // ------------------------------------------------------------ handedness

    /**
     * For each cube side, checks how the texture is laid on the face: with u to the image's right and v down the
     * image (v grows with the tile row), cross(right, up) equals the outward normal when the texture reads
     * normally in an unmirrored right-handed view.
     */
    static Map<String, Object> handedness() {
        Map<String, Object> m = new LinkedHashMap<>();
        Map<String, Object> per = new LinkedHashMap<>();
        for (BlockShapeAlgorithm.TexOrderStyle ts : BlockShapeAlgorithm.TexOrderStyle.values()) {
            int plus = 0;
            int minus = 0;
            int skew = 0;
            List<String> detail = new ArrayList<>();
            for (int o = 0; o < 6; o++) {
                for (int s = 0; s < 6; s++) {
                    double[][] corner = new double[4][];
                    for (int i = 0; i < 4; i++) {
                        int ext = ts.map[o][s][i];
                        corner[ext & 3] = shaderPos(s, BlockShapeAlgorithm.vertexOrderMap[s][i], 0, 0, 1);
                    }
                    // ext = u*2 + v
                    if (corner[0] == null || corner[1] == null || corner[2] == null || corner[3] == null) {
                        skew++;
                        continue;
                    }
                    double[] right = Geometry.sub(corner[2], corner[0]);
                    double[] down = Geometry.sub(corner[1], corner[0]);
                    double[] up = {-down[0], -down[1], -down[2]};
                    double[] c = Geometry.cross(right, up);
                    double[] n = {normals[s].x, normals[s].y, normals[s].z};
                    double d = Geometry.dot(c, n);
                    if (d > 0.5) {
                        plus++;
                    } else if (d < -0.5) {
                        minus++;
                    } else {
                        skew++;
                    }
                    if (o == 0) {
                        detail.add(SIDE_NAMES[s] + ": image right = " + vec(right) + ", image up = " + vec(up) + (d > 0 ? " (reads normally in a right-handed view)" : " (mirrored in a right-handed view)"));
                    }
                }
            }
            Map<String, Object> e = new LinkedHashMap<>();
            e.put("facesReadingNormallyInRightHandedView", plus);
            e.put("facesMirroredInRightHandedView", minus);
            e.put("facesWithUnusableMapping", skew);
            e.put("orientation0", detail);
            per.put(ts.name(), e);
        }
        m.put("note", "Evidence about display handedness from the cube texture mapping, assuming texture tile row 0 is the top row of the atlas image "
                + "(v grows downward in the image). If faces read normally in a right-handed view the game draws x,y,z right-handed, "
                + "which puts +X on the viewer's left when looking along +Z (FRONT) with +Y up.");
        m.put("byTexOrderStyle", per);
        return m;
    }

    static String vec(double[] v) {
        return "(" + Json.num(v[0]) + "," + Json.num(v[1]) + "," + Json.num(v[2]) + ")";
    }

    // ----------------------------------------------------------------- notes

    static List<String> notes() {
        List<String> n = new ArrayList<>();
        n.add("Side ids: 0 FRONT +Z, 1 BACK -Z, 2 TOP +Y, 3 BOTTOM -Y, 4 RIGHT +X, 5 LEFT -X (Element.DIRECTIONSi). The mesher draws side 4 at +X.");
        n.add("LEFT and RIGHT are swapped in orientation semantics: for cube orientations, slabs and 24-orientation blocks the value named "
                + "RIGHT (4) points to -X and LEFT (5) to +X (the game calls Element.switchLeftRight in these paths). FRONT, BACK, TOP, BOTTOM are as named. "
                + "Wedge, corner, tetra and hepta geometry is given directly, so no swap has to be applied to it.");
        n.add("Lookup is algorithms[style-1][orientation % modulus] in physics and build code, but the mesher indexes algorithms[style-1][orientation] "
                + "without the modulo, where index `modulus` is an icon placeholder. Write only values below the modulus.");
        n.add("Wedges repeat after 12 values, tetra and hepta after 8, sprites after 6; corners and normal24 use all 24. See checks.duplicateOrientations.");
        n.add("Sprites (style 3) are not solid: no hull, two crossed quads drawn from both sides; `render` lists the quads the mesher emits.");
        n.add("normal24 blocks (style 6) are full cubes for geometry; blocks with a LoD model (most rails) are drawn with that model rotated by lodBasis instead.");
        n.add("render[].emittedForSide is the cube side whose visibility triggers the quad; the slanted face of a wedge is emitted for its `angled` side. "
                + "Entries with drawn=false are collapsed quads the game still sends to the GPU.");
        n.add("Only hepta blocks emit a seventh quad (emittedForSide 6, the diagonal face); the game sends it together with the side given in extraSideEmittedWith. "
                + "Wedge, corner and tetra bend one of the six ordinary quads into the slanted face instead.");
        n.add("The mesher clamps the stored orientation to 0..5 for every block with individualSides 3 or 6 before it picks the shape algorithm, "
                + "so the few non-cube blocks listed in styles[].blocksWithSeveralTextures can only be drawn in their first six orientations. "
                + "Non-cube blocks with individualSides 1 use texture slot 0 on every face.");
        n.add("mirror.xy / xz / yz give the orientation value after mirroring across that plane (xy flips Z, xz flips Y, yz flips X), from BlockShapeAlgorithm.xyMappings etc.");
        n.add("Cargo space (id 689) reuses the orientation value as its slab height and block 689/688 have special default orientations; they are not covered here.");
        n.add("Hepta blocks cover three sides completely, but the game's getSidesToCheckForVis is empty for them, so the game never hides a neighbour's face behind a hepta. "
                + "sideCoverage reports the geometry, not that choice.");
        n.add("Algorithm class names are not a reliable description (WedgeTopRight has its full wall on +X, WedgeBottomRight on -X); use faces and description.");
        n.add("Display handedness: tile rows in the block atlas count from the top of the image (checked by hand against t000.png: tile 20 is red, 22 yellow, "
                + "24 blue, matching the Standard Armor blocks), so v grows downward in the image and the `handedness` section applies: "
                + "cube textures read unmirrored only if x,y,z are drawn as a right-handed frame.");
        n.add("Default orientation on placement (ElementInformation.getDefaultOrientation): 2 for sprites and three sided blocks, 14 for normal24, otherwise 0.");
        return n;
    }

    // --------------------------------------------------------------- helpers

    static boolean same(double[] a, double[] b) {
        return Math.abs(a[0] - b[0]) < EPS && Math.abs(a[1] - b[1]) < EPS && Math.abs(a[2] - b[2]) < EPS;
    }

    static String key(double[] p) {
        return Json.num(p[0]) + "," + Json.num(p[1]) + "," + Json.num(p[2]);
    }

    static Set<String> pointSet(Collection<double[]> pts) {
        Set<String> s = new TreeSet<>();
        for (double[] p : pts) {
            s.add(key(p));
        }
        return s;
    }

    static String pointList(List<double[]> pts) {
        StringBuilder sb = new StringBuilder();
        for (double[] p : pts) {
            sb.append('[').append(key(p)).append(']');
        }
        return sb.toString();
    }

    static String uvList(List<int[]> uv) {
        StringBuilder sb = new StringBuilder();
        for (int[] p : uv) {
            sb.append(p[0]).append(p[1]);
        }
        return sb.toString();
    }

    static double round(double d) {
        double r = Math.round(d * 1e6) / 1e6;
        return r == 0.0 ? 0.0 : r;
    }

    // -------------------------------------------------------------- geometry

    static final class Face {
        double[] normal;
        List<double[]> verts;

        Integer side() {
            for (int s = 0; s < 6; s++) {
                Vector3i d = Element.DIRECTIONSi[s];
                if (Math.abs(normal[0] - d.x) < EPS && Math.abs(normal[1] - d.y) < EPS && Math.abs(normal[2] - d.z) < EPS) {
                    int axis = d.x != 0 ? 0 : d.y != 0 ? 1 : 2;
                    double want = (d.x + d.y + d.z) > 0 ? 1.0 : 0.0;
                    for (double[] v : verts) {
                        if (Math.abs(v[axis] - want) > EPS) {
                            return null;
                        }
                    }
                    return s;
                }
            }
            return null;
        }

        boolean allCorners() {
            for (double[] v : verts) {
                for (double c : v) {
                    if (Math.abs(c) > EPS && Math.abs(c - 1) > EPS) {
                        return false;
                    }
                }
            }
            return true;
        }
    }

    static final class Geometry {
        static double[] sub(double[] a, double[] b) {
            return new double[]{a[0] - b[0], a[1] - b[1], a[2] - b[2]};
        }

        static double[] cross(double[] a, double[] b) {
            return new double[]{a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]};
        }

        static double dot(double[] a, double[] b) {
            return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        }

        static double[] normalize(double[] a) {
            double l = Math.sqrt(dot(a, a));
            return new double[]{round(a[0] / l), round(a[1] / l), round(a[2] / l)};
        }

        /** Faces of the convex hull of at most a handful of points, each wound counter-clockwise seen from outside. */
        static List<Face> hullFaces(List<double[]> pts) {
            List<Face> faces = new ArrayList<>();
            Set<String> seen = new TreeSet<>();
            int n = pts.size();
            for (int i = 0; i < n; i++) {
                for (int j = i + 1; j < n; j++) {
                    for (int k = j + 1; k < n; k++) {
                        double[] nrm = cross(sub(pts.get(j), pts.get(i)), sub(pts.get(k), pts.get(i)));
                        if (Math.sqrt(dot(nrm, nrm)) < EPS) {
                            continue;
                        }
                        double d = dot(nrm, pts.get(i));
                        boolean pos = false;
                        boolean neg = false;
                        List<double[]> members = new ArrayList<>();
                        for (double[] p : pts) {
                            double s = dot(nrm, p) - d;
                            if (s > EPS) {
                                pos = true;
                            } else if (s < -EPS) {
                                neg = true;
                            } else {
                                members.add(p);
                            }
                        }
                        if (pos && neg) {
                            continue;
                        }
                        if (pos) {
                            nrm = new double[]{-nrm[0], -nrm[1], -nrm[2]};
                        }
                        String key = pointSet(members).toString();
                        if (!seen.add(key)) {
                            continue;
                        }
                        Face f = new Face();
                        f.normal = normalize(nrm);
                        f.verts = order(members, f.normal);
                        faces.add(f);
                    }
                }
            }
            return faces;
        }

        static List<double[]> order(List<double[]> members, double[] normal) {
            double[] c = new double[3];
            for (double[] p : members) {
                c[0] += p[0] / members.size();
                c[1] += p[1] / members.size();
                c[2] += p[2] / members.size();
            }
            double[] u = sub(members.get(0), c);
            double ul = Math.sqrt(dot(u, u));
            u = new double[]{u[0] / ul, u[1] / ul, u[2] / ul};
            double[] w = cross(normal, u);
            final double[] cu = u;
            final double[] cc = c;
            List<double[]> sorted = new ArrayList<>(members);
            sorted.sort((a, b) -> Double.compare(angle(a, cc, cu, w), angle(b, cc, cu, w)));
            // start at the lexicographically smallest vertex for a stable file
            int best = 0;
            for (int i = 1; i < sorted.size(); i++) {
                if (key(sorted.get(i)).compareTo(key(sorted.get(best))) < 0) {
                    best = i;
                }
            }
            List<double[]> out = new ArrayList<>();
            for (int i = 0; i < sorted.size(); i++) {
                out.add(sorted.get((best + i) % sorted.size()));
            }
            return out;
        }

        static double angle(double[] p, double[] c, double[] u, double[] w) {
            double[] d = sub(p, c);
            double a = Math.atan2(dot(d, w), dot(d, u));
            return a < -1e-9 ? a + 2 * Math.PI : a;
        }

        static double[] newell(List<double[]> poly) {
            double[] n = new double[3];
            for (int i = 0; i < poly.size(); i++) {
                double[] a = poly.get(i);
                double[] b = poly.get((i + 1) % poly.size());
                n[0] += (a[1] - b[1]) * (a[2] + b[2]);
                n[1] += (a[2] - b[2]) * (a[0] + b[0]);
                n[2] += (a[0] - b[0]) * (a[1] + b[1]);
            }
            return n;
        }

        static boolean planar(List<double[]> verts, double[] normal) {
            double d = dot(normal, verts.get(0));
            for (double[] v : verts) {
                if (Math.abs(dot(normal, v) - d) > 1e-4) {
                    return false;
                }
            }
            double[] n = newell(verts);
            return dot(n, normal) > 0;
        }

        static double volume(List<Face> faces) {
            double v = 0;
            for (Face f : faces) {
                double[] a = f.verts.get(0);
                for (int i = 1; i + 1 < f.verts.size(); i++) {
                    v += dot(a, cross(f.verts.get(i), f.verts.get(i + 1))) / 6.0;
                }
            }
            return v;
        }
    }

    // ------------------------------------------------------------------ json

    static final class Json {
        /** Marks a value that should be written on a single line. */
        static final class Inline {
            final Object value;

            Inline(Object value) {
                this.value = value;
            }
        }

        static String num(double d) {
            double r = Math.round(d * 1e6) / 1e6;
            if (r == Math.rint(r)) {
                return String.valueOf((long) r);
            }
            String s = String.format(Locale.ROOT, "%.6f", r);
            s = s.replaceAll("0+$", "");
            return s;
        }

        static void write(StringBuilder sb, Object o, int indent) {
            if (o instanceof Inline) {
                inline(sb, ((Inline) o).value);
            } else if (o instanceof Map) {
                Map<?, ?> m = (Map<?, ?>) o;
                if (m.isEmpty()) {
                    sb.append("{}");
                    return;
                }
                sb.append("{\n");
                int i = 0;
                for (Map.Entry<?, ?> e : m.entrySet()) {
                    pad(sb, indent + 1);
                    str(sb, String.valueOf(e.getKey()));
                    sb.append(": ");
                    write(sb, e.getValue(), indent + 1);
                    if (++i < m.size()) {
                        sb.append(',');
                    }
                    sb.append('\n');
                }
                pad(sb, indent);
                sb.append('}');
            } else if (o instanceof List && !simpleList((List<?>) o)) {
                List<?> l = (List<?>) o;
                sb.append("[\n");
                for (int i = 0; i < l.size(); i++) {
                    pad(sb, indent + 1);
                    write(sb, l.get(i), indent + 1);
                    if (i + 1 < l.size()) {
                        sb.append(',');
                    }
                    sb.append('\n');
                }
                pad(sb, indent);
                sb.append(']');
            } else {
                inline(sb, o);
            }
        }

        static boolean simpleList(List<?> l) {
            for (Object o : l) {
                if (o instanceof Map || o instanceof Inline || o instanceof String) {
                    return false;
                }
                if (o instanceof List && !simpleList((List<?>) o)) {
                    return false;
                }
            }
            return true;
        }

        static void inline(StringBuilder sb, Object o) {
            if (o == null) {
                sb.append("null");
            } else if (o instanceof Inline) {
                inline(sb, ((Inline) o).value);
            } else if (o instanceof String) {
                str(sb, (String) o);
            } else if (o instanceof Boolean) {
                sb.append(o);
            } else if (o instanceof Double || o instanceof Float) {
                sb.append(num(((Number) o).doubleValue()));
            } else if (o instanceof Number) {
                sb.append(o);
            } else if (o instanceof double[]) {
                double[] a = (double[]) o;
                sb.append('[');
                for (int i = 0; i < a.length; i++) {
                    sb.append(i > 0 ? ", " : "").append(num(a[i]));
                }
                sb.append(']');
            } else if (o instanceof int[]) {
                int[] a = (int[]) o;
                sb.append('[');
                for (int i = 0; i < a.length; i++) {
                    sb.append(i > 0 ? ", " : "").append(a[i]);
                }
                sb.append(']');
            } else if (o instanceof Map) {
                sb.append('{');
                int i = 0;
                for (Map.Entry<?, ?> e : ((Map<?, ?>) o).entrySet()) {
                    sb.append(i++ > 0 ? ", " : "");
                    str(sb, String.valueOf(e.getKey()));
                    sb.append(": ");
                    inline(sb, e.getValue());
                }
                sb.append('}');
            } else if (o instanceof Collection) {
                sb.append('[');
                int i = 0;
                for (Object e : (Collection<?>) o) {
                    sb.append(i++ > 0 ? ", " : "");
                    inline(sb, e);
                }
                sb.append(']');
            } else {
                str(sb, String.valueOf(o));
            }
        }

        static void pad(StringBuilder sb, int indent) {
            for (int i = 0; i < indent; i++) {
                sb.append(' ');
            }
        }

        static void str(StringBuilder sb, String s) {
            sb.append('"');
            for (int i = 0; i < s.length(); i++) {
                char c = s.charAt(i);
                switch (c) {
                    case '"':
                        sb.append("\\\"");
                        break;
                    case '\\':
                        sb.append("\\\\");
                        break;
                    case '\n':
                        sb.append("\\n");
                        break;
                    case '\t':
                        sb.append("\\t");
                        break;
                    default:
                        if (c < 0x20) {
                            sb.append(String.format("\\u%04x", (int) c));
                        } else {
                            sb.append(c);
                        }
                }
            }
            sb.append('"');
        }
    }
}
