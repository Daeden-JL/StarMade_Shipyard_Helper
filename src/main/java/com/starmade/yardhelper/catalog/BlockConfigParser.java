package com.starmade.yardhelper.catalog;

import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;

import javax.xml.parsers.DocumentBuilderFactory;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import java.util.Set;

/**
 * Builds a catalog from a StarMade install's data/config directory (BlockConfig.xml plus
 * BlockTypes.properties). Used offline; on the server the catalog comes from the live
 * ElementKeyMap instead, which also knows about modded blocks.
 */
public final class BlockConfigParser {
    private BlockConfigParser() {}

    public static BlockCatalog load(Path configDir) throws IOException {
        Map<String, Short> ids = new HashMap<>();
        Properties props = new Properties();
        try (InputStream in = Files.newInputStream(configDir.resolve("BlockTypes.properties"))) {
            props.load(in);
        }
        for (String key : props.stringPropertyNames()) {
            try {
                ids.put(key, Short.parseShort(props.getProperty(key).trim()));
            } catch (NumberFormatException ignored) {
                // not a block id line
            }
        }

        Document doc;
        try (InputStream in = Files.newInputStream(configDir.resolve("BlockConfig.xml"))) {
            doc = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(in);
        } catch (Exception e) {
            throw new IOException("Cannot parse BlockConfig.xml: " + e.getMessage(), e);
        }
        Element root = firstChild(doc.getDocumentElement(), "Element");
        if (root == null) throw new IOException("BlockConfig.xml has no <Element> section");

        List<BlockDef> defs = new ArrayList<>();
        collect(root, "", ids, defs);
        return new BlockCatalog(defs);
    }

    private static void collect(Element parent, String path, Map<String, Short> ids, List<BlockDef> out) {
        for (Node n = parent.getFirstChild(); n != null; n = n.getNextSibling()) {
            if (!(n instanceof Element e)) continue;
            if (e.getTagName().equals("Block")) {
                BlockDef def = parseBlock(e, path, ids);
                if (def != null) out.add(def);
            } else {
                collect(e, path.isEmpty() ? e.getTagName() : path + "/" + e.getTagName(), ids, out);
            }
        }
    }

    private static BlockDef parseBlock(Element e, String category, Map<String, Short> ids) {
        String key = e.getAttribute("type");
        Short id = ids.get(key);
        if (id == null) return null;
        return new BlockDef(
                id,
                key,
                e.getAttribute("name"),
                category,
                BlockShape.fromStyle(intOf(e, "BlockStyle", 0)),
                intOf(e, "Slab", 0),
                boolOf(e, "Orientation"),
                intOf(e, "IndividualSides", 1),
                elementRefs(e, "Controlling", ids),
                elementRefs(e, "ControlledBy", ids),
                (short) intOf(e, "SourceReference", 0),
                idList(e, "StyleIds"),
                idList(e, "SlabIds"),
                boolOf(e, "Placable"),
                boolOf(e, "Deprecated"),
                boolOf(e, "CanActivate"),
                boolOf(e, "LogicBlock"),
                boolOf(e, "SystemBlock"),
                textOf(e, "Description"));
    }

    private static Set<Short> elementRefs(Element block, String tag, Map<String, Short> ids) {
        Set<Short> out = new HashSet<>();
        Element list = firstChild(block, tag);
        if (list == null) return out;
        for (Node n = list.getFirstChild(); n != null; n = n.getNextSibling()) {
            if (n instanceof Element ref && ref.getTagName().equals("Element")) {
                Short id = ids.get(ref.getTextContent().trim());
                if (id != null) out.add(id);
            }
        }
        return out;
    }

    private static short[] idList(Element block, String tag) {
        String text = textOf(block, tag);
        if (text.isEmpty()) return new short[0];
        String[] parts = text.split(",");
        short[] out = new short[parts.length];
        int n = 0;
        for (String p : parts) {
            try {
                out[n++] = Short.parseShort(p.trim());
            } catch (NumberFormatException ignored) {
                // skip malformed entry
            }
        }
        return n == out.length ? out : java.util.Arrays.copyOf(out, n);
    }

    private static Element firstChild(Element parent, String tag) {
        for (Node n = parent.getFirstChild(); n != null; n = n.getNextSibling()) {
            if (n instanceof Element e && e.getTagName().equals(tag)) return e;
        }
        return null;
    }

    private static String textOf(Element block, String tag) {
        Element e = firstChild(block, tag);
        return e == null ? "" : e.getTextContent().trim();
    }

    private static int intOf(Element block, String tag, int fallback) {
        try {
            return Integer.parseInt(textOf(block, tag));
        } catch (NumberFormatException ex) {
            return fallback;
        }
    }

    private static boolean boolOf(Element block, String tag) {
        return "true".equalsIgnoreCase(textOf(block, tag));
    }
}
