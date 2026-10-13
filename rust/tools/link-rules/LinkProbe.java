import java.io.*;
import java.lang.reflect.*;
import java.util.*;
import org.schema.game.common.data.element.*;
import org.schema.game.common.controller.*;
import org.schema.game.common.controller.elements.*;
import org.schema.schine.network.StateInterface;
import api.utils.element.Blocks;

public class LinkProbe {
    static Object unsafe; static Method allocate; static org.w3c.dom.Document behaviorDoc;
    static Object alloc(Class<?> c) throws Exception { return allocate.invoke(unsafe, c); }
    static void set(Class<?> declaring, Object target, String field, Object value) throws Exception {
        Field f = declaring.getDeclaredField(field); f.setAccessible(true); f.set(target, value);
    }
    static InvocationHandler handler(String name) {
        return (proxy, method, a) -> {
            switch (method.getName()) {
                case "getUpdateTime": return System.currentTimeMillis();
                case "toString": return name;
                case "getBlockBehaviorConfig": return behaviorDoc;
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
    static void prep(SegmentController s) throws Exception {
        Object rc = alloc(org.schema.game.common.controller.rails.RailController.class);
        Field f = SegmentController.class.getDeclaredField("railController"); f.setAccessible(true); f.set(s, rc);
    }
    static String nm(int id) { return ElementKeyMap.isValidType((short) id) ? ElementKeyMap.getInfo((short) id).getName() : "?"; }

    public static void main(String[] args) throws Exception {
        PrintStream out = System.out;
        PrintStream log = new PrintStream(new FileOutputStream("game.log"));
        System.setOut(log); System.setErr(log);
        ElementKeyMap.initializeData(null);
        String mode = args.length > 0 ? args[0] : "flags";
        if (mode.equals("blocks")) {
            for (String n : new String[]{"COMMAND_MODULE","SHIPYARD_COMPUTER","BOBBY_AI_MODULE","WARP_GATE_COMPUTER","REACTOR_POWER","THRUSTER_MODULE","LOGIC_BEAM","STORAGE","LOCK_BOX","FACTORY_MANAGER"}) {
                try { Blocks b = Blocks.valueOf(n); out.println(n + "=" + b.getId() + " (" + nm(b.getId()) + ")"); } catch (Throwable t) { out.println(n + " ERR " + t); }
            }
            out.println("NORMAL24.id=" + org.schema.game.client.view.cubes.shapes.BlockStyle.NORMAL24.id);
            out.println("factoryKeySet=" + ElementKeyMap.getFactorykeyset());
            out.println("lightTypes=" + ElementKeyMap.lightTypes.size() + " doorTypes=" + ElementKeyMap.doorTypes + " inventoryTypes=" + ElementKeyMap.inventoryTypes);
            out.println("chamberAnyTypes=" + ElementKeyMap.chamberAnyTypes.size());
            out.println("signalArray=" + Arrays.toString(ElementKeyMap.signalArray));
            return;
        }
        if (mode.equals("flags")) {
            out.println("id\tname\tsignal\tcanActivate\tlight\tdoor\tsensorInput\tinvType\tfactory\trailTrack\trailRot\trailTurret\tmain\tsupport\teffect\tchamber\tdeprecated\tblockStyle\thasLod\tmultiCtl\tcontrolling\tcontrolledBy");
            for (short t : ElementKeyMap.typeList()) {
                ElementInformation i = ElementKeyMap.getInfo(t);
                Method si = ElementInformation.class.getDeclaredMethod("isSensorInput"); si.setAccessible(true);
                out.println(t + "\t" + i.getName() + "\t" + i.isSignal() + "\t" + i.canActivate() + "\t" + i.isLightSource() + "\t" + i.isDoor() + "\t" + i.sensorInput + "\t" + i.inventoryType + "\t" + (i.getFactory() != null)
                    + "\t" + i.isRailTrack() + "\t" + i.isRailRotator() + "\t" + i.isRailTurret() + "\t" + i.isMainCombinationControllerB() + "\t" + i.isSupportCombinationControllerB() + "\t" + i.isEffectCombinationController()
                    + "\t" + i.isReactorChamberAny() + "\t" + i.isDeprecated() + "\t" + i.getBlockStyle() + "\t" + i.hasLod() + "\t" + i.isMultiControlled()
                    + "\t" + new TreeSet<Short>(i.getControlling()) + "\t" + new TreeSet<Short>(i.getControlledBy()));
            }
            return;
        }
        if (mode.equals("pairs")) {
            // args: file with "from to" per line; prints game predicates
            BufferedReader br = new BufferedReader(new FileReader(args[1]));
            String line;
            out.println("from\tfromName\tto\ttoName\tctlByContains\tcontrolsAll\tcombiAny\tEI.canBeControlled\tgate2");
            while ((line = br.readLine()) != null) {
                line = line.trim(); if (line.isEmpty() || line.startsWith("#")) continue;
                String[] p = line.split("\\s+");
                short a = Short.parseShort(p[0]), b = Short.parseShort(p[1]);
                ElementInformation ai = ElementKeyMap.getInfo(a), bi = ElementKeyMap.getInfo(b);
                boolean c1 = bi.getControlledBy().contains(a), c2 = ai.controlsAll(), c3 = ai.isCombiConnectAny(b), c4 = ElementInformation.canBeControlled(a, b);
                out.println(a + "\t" + ai.getName() + "\t" + b + "\t" + bi.getName() + "\t" + c1 + "\t" + c2 + "\t" + c3 + "\t" + c4 + "\t" + (c1 || c2 || c3 || c4));
            }
            return;
        }
        if (mode.equals("container")) {
            Class<?> u = Class.forName("sun.misc.Unsafe");
            Field f = u.getDeclaredField("theUnsafe"); f.setAccessible(true);
            unsafe = f.get(null); allocate = u.getMethod("allocateInstance", Class.class);
            behaviorDoc = javax.xml.parsers.DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(new File("./data/config/blockBehaviorConfig.xml"));
            StateInterface state = (StateInterface) Proxy.newProxyInstance(LinkProbe.class.getClassLoader(),
                new Class<?>[]{StateInterface.class, org.schema.game.client.data.GameStateInterface.class}, handler("ProbeState"));
            for (String kind : new String[]{"ship", "station"}) {
                try {
                    Object mc = null;
                    java.util.List<Integer> placeholders = new java.util.ArrayList<>();
                    ElementInformation[] arr = ElementKeyMap.getInfoArray();
                    for (int attempt = 0; attempt < 40 && mc == null; attempt++) {
                        try {
                            if (kind.equals("ship")) {
                                Ship s = (Ship) alloc(Ship.class);
                                set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "state", state);
                                set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "onServer", Boolean.TRUE);
                                prep(s);
                                mc = new ShipManagerContainer(state, s);
                            } else {
                                SpaceStation s = (SpaceStation) alloc(SpaceStation.class);
                                set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "state", state);
                                set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "onServer", Boolean.TRUE);
                                prep(s);
                                mc = new SpaceStationManagerContainer(state, s);
                            }
                        } catch (NullPointerException npe) {
                            String msg = String.valueOf(npe.getMessage());
                            java.util.regex.Matcher mm = java.util.regex.Pattern.compile("REQUESTED TYPE (\\d+) IS NULL").matcher(msg);
                            if (!mm.find()) throw npe;
                            int id = Integer.parseInt(mm.group(1));
                            placeholders.add(id);
                            arr[id] = (ElementInformation) alloc(ElementInformation.class);
                        }
                    }
                    for (int id : placeholders) arr[id] = null; // restore: these ids do not exist in the config
                    out.println(kind + " placeholders needed for missing types: " + placeholders);
                    ManagerContainer<?> m = (ManagerContainer<?>) mc;
                    StringBuilder sb = new StringBuilder();
                    int n = 0;
                    for (int t = 1; t <= ElementKeyMap.highestType; t++) {
                        if (ElementKeyMap.isValidType((short) t) && m.getModulesMap().containsKey((short) t)) { sb.append(t).append(' '); n++; }
                    }
                    out.println(kind + " modulesMap keys (" + n + "): " + sb);
                    out.println(kind + " special: all=" + m.getModulesMap().containsKey((short) 32767) + " signal=" + m.getModulesMap().containsKey((short) 30000) + " railTrack=" + m.getModulesMap().containsKey((short) 29999) + " railInv=" + m.getModulesMap().containsKey((short) 29998));
                    {
                        StringBuilder nc = new StringBuilder(); StringBuilder dec = new StringBuilder(); int ch = 0;
                        for (int t = 1; t <= ElementKeyMap.highestType; t++) {
                            if (!ElementKeyMap.isValidType((short) t) || !m.getModulesMap().containsKey((short) t)) continue;
                            ElementInformation bi = ElementKeyMap.getInfo((short) t);
                            if (bi.isReactorChamberAny()) ch++; else nc.append(t).append(' ');
                            if (!ElementInformation.canBeControlled((short) 405, (short) t)) dec.append(t).append(":").append(bi.getName()).append("; ");
                        }
                        out.println(kind + " module keys that are reactor chambers: " + ch + " of chamberAnyTypes " + ElementKeyMap.chamberAnyTypes.size());
                        out.println(kind + " module keys that are NOT chambers: " + nc);
                        out.println(kind + " targets where a logic block (405) links ONLY because of modulesMap: " + dec);
                        // exhaustive: all pairs where modulesMap decides
                        int total = 0, legal = 0, legalNoMod = 0;
                        short[] types = ElementKeyMap.typeList();
                        java.util.Map<String,Integer> byReason = new java.util.TreeMap<>();
                        StringBuilder nonSig = new StringBuilder();
                        for (short a : types) for (short b : types) {
                            ElementInformation ai = ElementKeyMap.getInfo(a), bi = ElementKeyMap.getInfo(b);
                            boolean cbc = ElementInformation.canBeControlled(a, b);
                            boolean g1 = m.canBeControlled(a, b);
                            boolean g2 = bi.getControlledBy().contains(a) || ai.controlsAll() || ai.isCombiConnectAny(b) || cbc;
                            total++;
                            if (g1 && g2) { legal++; if (cbc) legalNoMod++; else if (!ai.controlsAll()) nonSig.append(a).append("->").append(b).append(' '); }
                        }
                        out.println(kind + " exhaustive: pairs=" + total + " legal=" + legal + " legal via EI.canBeControlled alone=" + legalNoMod);
                        out.println(kind + " legal pairs that need modulesMap and whose controller is not a logic block: " + nonSig);
                    }
                    if (args.length > 2 && args[2].equals("matrix")) {
                        try (PrintWriter pw = new PrintWriter(new FileWriter("../legal_pairs_" + kind + ".tsv")); PrintWriter kw = new PrintWriter(new FileWriter("../module_keys_" + kind + ".txt"))) {
                            short[] types = ElementKeyMap.typeList(); java.util.Arrays.sort(types);
                            pw.println("# from\tto\tvia_EI_canBeControlled   (all legal pairs: ManagerContainer.canBeControlled && (controlledBy||controlsAll||isCombiConnectAny||EI.canBeControlled))");
                            for (short a : types) for (short b : types) {
                                ElementInformation ai = ElementKeyMap.getInfo(a), bi = ElementKeyMap.getInfo(b);
                                boolean cbc = ElementInformation.canBeControlled(a, b);
                                boolean g1 = m.canBeControlled(a, b);
                                boolean g2 = bi.getControlledBy().contains(a) || ai.controlsAll() || ai.isCombiConnectAny(b) || cbc;
                                if (g1 && g2) pw.println(a + "\t" + b + "\t" + (cbc ? 1 : 0));
                            }
                            for (short t : types) if (m.getModulesMap().containsKey(t)) kw.println(t + "\t" + ElementKeyMap.getInfo(t).getName() + (ElementKeyMap.getInfo(t).isReactorChamberAny() ? "\tchamber" : ""));
                        }
                    }
                    if (args.length > 1) {
                        BufferedReader br = new BufferedReader(new FileReader(args[1]));
                        String line;
                        while ((line = br.readLine()) != null) {
                            line = line.trim(); if (line.isEmpty() || line.startsWith("#")) continue;
                            String[] p = line.split("\\s+");
                            short a = Short.parseShort(p[0]), b = Short.parseShort(p[1]);
                            ElementInformation ai = ElementKeyMap.getInfo(a), bi = ElementKeyMap.getInfo(b);
                            boolean g1 = m.canBeControlled(a, b);
                            boolean g2 = bi.getControlledBy().contains(a) || ai.controlsAll() || ai.isCombiConnectAny(b) || ElementInformation.canBeControlled(a, b);
                            out.println(kind + "\t" + a + "\t" + ai.getName() + "\t" + b + "\t" + bi.getName() + "\tMC.canBeControlled=" + g1 + "\tgate2=" + g2 + "\tcan_link=" + (g1 && g2));
                        }
                    }
                } catch (Throwable t) {
                    out.println(kind + " FAILED: " + t);
                    StringWriter sw = new StringWriter(); t.printStackTrace(new PrintWriter(sw));
                    String[] ls = sw.toString().split("\n");
                    for (int i = 0; i < Math.min(ls.length, 25); i++) out.println("   " + ls[i]);
                }
            }
        }
    }
}
