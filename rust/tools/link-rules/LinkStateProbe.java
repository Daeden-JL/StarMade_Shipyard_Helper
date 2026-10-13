import java.io.*;
import java.lang.reflect.*;
import java.util.*;
import org.schema.game.common.data.element.*;
import org.schema.game.common.data.SegmentPiece;
import org.schema.game.common.controller.*;
import org.schema.schine.network.StateInterface;
import org.schema.common.util.linAlg.Vector3i;

public class LinkStateProbe {
    static Object unsafe; static Method allocate;
    static Object alloc(Class<?> c) throws Exception { return allocate.invoke(unsafe, c); }
    static void set(Class<?> declaring, Object target, String field, Object value) throws Exception {
        Field f = declaring.getDeclaredField(field); f.setAccessible(true); f.set(target, value);
    }
    static Map<Long, Short> world = new LinkedHashMap<>();
    static Map<Long, String> names = new LinkedHashMap<>();
    static Object dflt(Method method) {
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
    }
    static long put(String name, int x, int type) {
        long idx = ElementCollection.getIndex(x, 16, 16);
        world.put(idx, (short) type); names.put(idx, name); return idx;
    }
    static ControlElementMap cem; static PrintStream out;
    static void v(long from, long to) {
        short t = world.get(to);
        boolean was = cem.isControlling(from, to, t);
        cem.switchControllerForElement(from, to, t);
        out.println("  V " + names.get(from) + " -> " + names.get(to) + (was ? "   (was linked: toggled OFF)" : ""));
    }
    static void dump(String title) {
        out.println(title);
        TreeMap<String, String> lines = new TreeMap<>();
        for (long c : cem.getControllingMap().keySet()) {
            StringBuilder sb = new StringBuilder();
            for (short ty : cem.getControllingMap().get(c).keySet()) {
                for (long e4 : cem.getControllingMap().get(c).get(ty)) {
                    long pos = ElementCollection.getPosIndexFrom4(e4);
                    sb.append(names.get(pos)).append("[key type ").append(ty).append(", type in index4 ").append(ElementCollection.getType(e4)).append("] ");
                }
            }
            if (sb.length() > 0) lines.put(names.get(c), sb.toString());
        }
        for (Map.Entry<String, String> e : lines.entrySet()) out.println("     " + e.getKey() + " controls: " + e.getValue());
        if (lines.isEmpty()) out.println("     (no links)");
    }
    public static void main(String[] args) throws Exception {
        out = System.out;
        PrintStream log = new PrintStream(new FileOutputStream("game2.log"));
        System.setOut(log); System.setErr(log);
        ElementKeyMap.initializeData(null);
        Class<?> u = Class.forName("sun.misc.Unsafe");
        Field f = u.getDeclaredField("theUnsafe"); f.setAccessible(true);
        unsafe = f.get(null); allocate = u.getMethod("allocateInstance", Class.class);
        StateInterface state = (StateInterface) Proxy.newProxyInstance(LinkStateProbe.class.getClassLoader(), new Class<?>[]{StateInterface.class}, (p, m, a) -> {
            switch (m.getName()) {
                case "isSynched": return true;
                case "toString": return "ProbeState";
                case "hashCode": return System.identityHashCode(p);
                case "equals": return p == a[0];
                case "getUpdateTime": return System.currentTimeMillis();
            }
            return dflt(m);
        });
        SegmentBufferInterface buffer = (SegmentBufferInterface) Proxy.newProxyInstance(LinkStateProbe.class.getClassLoader(), new Class<?>[]{SegmentBufferInterface.class}, (p, m, a) -> {
            if (m.getName().equals("getPointUnsave")) {
                long idx = a[0] instanceof Long ? (Long) a[0] : ElementCollection.getIndex((Vector3i) a[0]);
                Short t = world.get(idx);
                SegmentPiece sp = new SegmentPiece();
                sp.setType(t == null ? (short) 0 : t);
                return sp;
            }
            switch (m.getName()) { case "toString": return "ProbeBuffer"; case "hashCode": return System.identityHashCode(p); case "equals": return p == a[0]; }
            return dflt(m);
        });
        Ship s = (Ship) alloc(Ship.class);
        set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "state", state);
        set(org.schema.game.common.data.world.SimpleTransformableSendableObject.class, s, "onServer", Boolean.TRUE);
        set(SegmentController.class, s, "segmentBuffer", buffer);
        Object net;
        try {
            net = new org.schema.game.network.objects.NetworkShip(state, s);
            out.println("network object: real NetworkShip constructor");
        } catch (Throwable t) {
            net = alloc(org.schema.game.network.objects.NetworkShip.class);
            org.schema.game.network.objects.NetworkSegmentController nsc = (org.schema.game.network.objects.NetworkSegmentController) net;
            nsc.controlledByBuffer = new org.schema.game.network.objects.remote.RemoteControlModBuffer(nsc);
            out.println("network object: allocated without constructor (" + t + ")");
        }
        {
            Object buf = ((org.schema.game.network.objects.NetworkSegmentController) net).controlledByBuffer;
            Class<?> c = buf.getClass(); Field of = null;
            while (c != null && of == null) { try { of = c.getDeclaredField("observer"); } catch (NoSuchFieldException e) { c = c.getSuperclass(); } }
            of.setAccessible(true);
            Object obs = Proxy.newProxyInstance(LinkStateProbe.class.getClassLoader(), new Class<?>[]{of.getType()}, (p, m, a) -> dflt(m));
            of.set(buf, obs);
        }
        set(SendableSegmentController.class, s, "networkEntity", net);
        cem = new ControlElementMap();
        cem.setSendableSegmentController(s);
        set(ControlElementMap.class, cem, "needsControllerUpdates", Boolean.FALSE); // no ManagerContainer callbacks in this probe
        out.println("needsControllerUpdates forced false: ManagerContainer side effects (slave/effect/light uniqueness) are NOT exercised here");

        long core = put("core(1)", 16, 1);
        long compA = put("cannonCompA(6)", 20, 6), compB = put("cannonCompB(6)", 21, 6), barrel = put("barrel(16)", 22, 16);
        long act1 = put("act1(405)", 30, 405), act2 = put("act2(405)", 31, 405), act3 = put("act3(405)", 32, 405);
        long light = put("light(55)", 33, 55), door = put("door(122)", 34, 122);
        long st1 = put("storage1(120)", 40, 120), st2 = put("storage2(120)", 41, 120), st3 = put("storage3(120)", 45, 120), cargo = put("cargo(689)", 42, 689);
        long disp = put("display(479)", 43, 479), rail = put("rail(662)", 44, 662), sp1 = put("speedCtl1(672)", 46, 672), sp2 = put("speedCtl2(672)", 47, 672);
        long cmd = put("command(134)", 50, 134), bobby = put("bobby(121)", 51, 121);
        long and1 = put("and(408)", 52, 408);

        out.println("T1 module block linked from a second computer");
        v(compA, barrel); dump("   after A->barrel"); v(compB, barrel); dump("   after B->barrel");
        out.println("T2 toggle");
        v(compB, barrel); dump("   after B->barrel again");
        out.println("T3 logic block from two logic blocks");
        v(act1, act3); v(act2, act3); dump("   result"); v(act1, act3); v(act2, act3);
        out.println("T4 light from two logic blocks");
        v(act1, light); v(act2, light); dump("   result"); v(act2, light);
        out.println("T5 door from two logic blocks");
        v(act1, door); v(act2, door); dump("   result"); v(act1, door); v(act2, door);
        out.println("T6 cargo space: two storages, plus a logic block");
        v(act1, cargo); v(st1, cargo); dump("   after act1->cargo, storage1->cargo"); v(st2, cargo); dump("   after storage2->cargo"); v(act2, cargo); dump("   after act2->cargo"); v(st2, cargo); v(act2, cargo); v(act1, cargo);
        dump("   cleanup check");
        out.println("T7 computer auto-linked from core, then linked as slave of another computer");
        cem.addControllerForElement(core, compA, (short) 6); cem.addControllerForElement(core, compB, (short) 6); dump("   core links added (as the game does on placement)");
        v(compB, compA); dump("   after compB->compA");
        out.println("T8 logic block linked to a computer that the core links");
        v(and1, compB); dump("   after and->compB");
        v(and1, compB); v(compB, compA);
        dump("   cleanup check");
        out.println("T9 display from two logic blocks"); v(act1, disp); v(act2, disp); dump("   result"); v(act1, disp); v(act2, disp);
        out.println("T10 rail from two speed controllers and a logic block"); v(sp1, rail); v(sp2, rail); v(act1, rail); dump("   result"); v(sp1, rail); v(sp2, rail); v(act1, rail);
        out.println("T11 command module does not unlink others; later logic link does");
        v(act1, bobby); v(cmd, bobby); dump("   after act1->bobby, command->bobby"); v(act2, bobby); dump("   after act2->bobby"); v(act2, bobby);
        out.println("T12 storage from two storages"); v(st1, st2); v(st3, st2); dump("   result"); v(st1, st2); v(st3, st2);
        out.println("T13 self link via ControlElementMap.addControllerForElement(from==to)");
        cem.addControllerForElement(act1, act1, (short) 405); dump("   result");
        out.println("T14 removing a block: onRemoveElement(controlled) and onRemoveElement(controller)");
        v(compA, barrel); v(act1, act3); v(act3, light); dump("   before");
        cem.onRemoveElement(barrel, (short) 16); dump("   after onRemoveElement(barrel,16)");
        cem.onRemoveElement(act3, (short) 405); dump("   after onRemoveElement(act3,405)  [act3 was both controlled and controller]");
    }
}
