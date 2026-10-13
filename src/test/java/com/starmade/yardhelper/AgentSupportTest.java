package com.starmade.yardhelper;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.starmade.yardhelper.agent.ContentFilter;
import com.starmade.yardhelper.agent.Conversation;
import com.starmade.yardhelper.agent.ToolRegistry;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.catalog.BlockConfigParser;
import com.starmade.yardhelper.catalog.BlockDef;
import com.starmade.yardhelper.catalog.BlockShape;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AgentSupportTest {
    @Test
    void searchNeedsEveryWordAndHidesVariantsUnlessAsked() {
        BlockCatalog c = TestCatalog.create();
        assertEquals("Grey Basic Armor", c.search("grey armor", 5).get(0).name());
        assertEquals(1, c.search("grey armor", 5).size());
        assertEquals("Grey Basic Armor Wedge", c.search("grey armor wedge", 5).get(0).name());
        assertTrue(c.search("cannon thruster", 5).isEmpty());
        assertTrue(c.search("power", 5).isEmpty(), "deprecated blocks are not offered");
    }

    @Test
    void linkRulesFollowTheCatalog() {
        BlockCatalog c = TestCatalog.create();
        assertTrue(c.canLink(TestCatalog.CANNON_COMPUTER, TestCatalog.CANNON_BARREL));
        assertFalse(c.canLink(TestCatalog.CANNON_COMPUTER, TestCatalog.THRUSTER));
        assertFalse(c.canLink(TestCatalog.CANNON_BARREL, TestCatalog.CANNON_COMPUTER));
    }

    @Test
    void conversationTrimsOldToolOutputBeforeDroppingTurns() {
        Conversation c = new Conversation();
        ObjectMapper json = new ObjectMapper();
        c.addUser("first request");
        c.addAssistant(json.createObjectNode().put("role", "assistant").put("content", ""));
        c.addToolResult("call_0", "x".repeat(5000));
        c.addUser("second request");
        c.addToolResult("call_1", "y".repeat(500));

        List<ObjectNode> sent = c.forRequest("system", 2000);
        String all = sent.toString();
        assertTrue(all.contains("first request"), "old turn survives once its tool output is trimmed");
        assertFalse(all.contains("xxxxx"));
        assertTrue(all.contains("yyyyy"), "current turn is never trimmed");
        assertEquals("system", sent.get(0).path("role").asText());

        sent = c.forRequest("system", 700);
        assertFalse(sent.toString().contains("first request"), "oldest turn is dropped when still too large");
        assertTrue(sent.toString().contains("second request"));
    }

    @Test
    void obviousRequestsAreBlockedWithoutAskingTheModel() throws Exception {
        ScriptedLlm llm = new ScriptedLlm();
        ContentFilter filter = new ContentFilter(llm, true);
        assertFalse(filter.allows("build a giant penis ship"));
        assertTrue(llm.requests.isEmpty());
    }

    @Test
    void modelVerdictDecidesTheRest() throws Exception {
        ScriptedLlm llm = new ScriptedLlm().say("ALLOW").say("BLOCK");
        ContentFilter filter = new ContentFilter(llm, true);
        assertTrue(filter.allows("build a star destroyer with turbolasers"));
        assertFalse(filter.allows("something the judge dislikes"));
    }

    @Test
    void wordsThatMerelyContainFlaggedLettersAreNotBlocked() throws Exception {
        ScriptedLlm llm = new ScriptedLlm().say("ALLOW").say("ALLOW");
        ContentFilter filter = new ContentFilter(llm, true);
        assertTrue(filter.allows("add a cockpit and document the layout"));
        assertTrue(filter.allows("a peacock themed yacht"));
    }

    @Test
    void toolDefinitionsAreWellFormed() {
        var defs = ToolRegistry.standard().definitions();
        assertTrue(defs.size() >= 14);
        for (var d : defs) {
            assertEquals("function", d.path("type").asText());
            assertFalse(d.path("function").path("name").asText().isEmpty());
            assertFalse(d.path("function").path("description").asText().isEmpty());
            assertEquals("object", d.path("function").path("parameters").path("type").asText());
            for (var req : d.path("function").path("parameters").path("required")) {
                assertTrue(d.path("function").path("parameters").path("properties").has(req.asText()),
                        d.path("function").path("name").asText() + " requires undeclared " + req.asText());
            }
        }
    }

    /** Runs only on machines with a StarMade install; checks the real config parses as expected. */
    @Test
    void realBlockConfigParses() throws Exception {
        Path dir = Paths.get(System.getProperty("user.home"), ".smtest2", "vol", "data", "config");
        Assumptions.assumeTrue(Files.exists(dir.resolve("BlockConfig.xml")));
        BlockCatalog c = BlockConfigParser.load(dir);
        assertTrue(c.size() > 1000);
        assertEquals("Ship Core", c.get(BlockCatalog.CORE_ID).name());
        BlockDef hull = c.byName("Grey Basic Armor");
        assertNotNull(hull);
        assertEquals("General/Hulls/Basic/Grey", hull.category());
        assertEquals(BlockShape.WEDGE, c.variant(hull.id(), BlockShape.WEDGE).shape());
        assertEquals(hull.id(), c.variant(hull.id(), BlockShape.WEDGE).baseId());
        assertTrue(c.canLink(c.byName("Cannon Computer").id(), c.byName("Cannon Barrel").id()));
        assertEquals("Grey Standard Armor", c.search("grey standard armor", 3).get(0).name());
    }
}
