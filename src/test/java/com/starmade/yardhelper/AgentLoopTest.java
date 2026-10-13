package com.starmade.yardhelper;

import com.starmade.yardhelper.agent.AgentLoop;
import com.starmade.yardhelper.agent.Conversation;
import com.starmade.yardhelper.agent.Guides;
import com.starmade.yardhelper.agent.LlmClient;
import com.starmade.yardhelper.agent.ToolContext;
import com.starmade.yardhelper.agent.ToolRegistry;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.world.DirectGateway;
import com.starmade.yardhelper.world.Editor;
import com.starmade.yardhelper.world.InMemoryDesignWorld;
import com.starmade.yardhelper.world.Journal;
import com.starmade.yardhelper.world.Vec3;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class AgentLoopTest {
    private InMemoryDesignWorld world;
    private ToolContext ctx;
    private ScriptedLlm llm;
    private final List<String> toolLog = new ArrayList<>();

    private final AgentLoop.Listener listener = new AgentLoop.Listener() {
        @Override
        public void note(String text) {}

        @Override
        public void tool(String name, String result) {
            toolLog.add(name + ": " + result);
        }
    };

    @BeforeEach
    void setUp() {
        world = InMemoryDesignWorld.withCore(TestCatalog.CORE, 50);
        BlockCatalog catalog = TestCatalog.create();
        Editor editor = new Editor(new DirectGateway(world), catalog, new Journal(10), 100_000);
        editor.journal().begin("test");
        ctx = new ToolContext(editor, catalog, Guides.load());
        llm = new ScriptedLlm();
    }

    private String run(String text) throws Exception {
        return new AgentLoop(llm, ToolRegistry.standard(), "system", 10, 100_000, new LlmClient.Options(0, ""), false)
                .run(new Conversation(), text, ctx, listener, () -> false);
    }

    @Test
    void runsToolCallsThenReturnsTheFinalAnswer() throws Exception {
        llm.call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[-2,-1,-5],\"to\":[2,1,5],\"mode\":\"hollow\"}")
                .say("Built a small hull.");
        assertEquals("Built a small hull.", run("build a hull"));
        assertEquals(TestCatalog.HULL, world.get(2, 1, 5).type());
        assertTrue(world.get(1, 0, 3).isAir(), "interior should be hollow");
        assertEquals(TestCatalog.CORE, world.get(0, 0, 0).type());
    }

    @Test
    void weaponCanBePlacedAndLinkedByBox() throws Exception {
        llm.call("fill", "{\"block\":\"Cannon Barrel\",\"from\":[1,0,2],\"to\":[1,0,8]}")
                .call("place_blocks", "{\"blocks\":[{\"pos\":[1,0,1],\"block\":\"Cannon Computer\"}]}")
                .call("link", "{\"controller\":[1,0,1],\"from\":[1,0,2],\"to\":[1,0,8]}")
                .say("Cannon ready.");
        run("add a cannon");
        assertEquals(7, world.linksFrom(new Vec3(1, 0, 1)).size());
    }

    @Test
    void badArgumentsComeBackAsErrorsTheModelCanRead() throws Exception {
        llm.call("fill", "{\"block\":\"Unobtainium\",\"from\":[0,0,1],\"to\":[0,0,2]}")
                .call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":\"oops\",\"to\":[0,0,2]}")
                .call("no_such_tool", "{}")
                .call("fill", "not json")
                .say("ok");
        run("x");
        assertTrue(toolLog.get(0).contains("Error: Unknown block"), toolLog.get(0));
        assertTrue(toolLog.get(1).contains("must be a position"), toolLog.get(1));
        assertTrue(toolLog.get(2).contains("no tool named"), toolLog.get(2));
        assertTrue(toolLog.get(3).contains("not valid JSON"), toolLog.get(3));
        assertEquals(1, world.blockCount());
    }

    @Test
    void deprecatedBlocksAreRefused() throws Exception {
        llm.call("fill", "{\"block\":\"Power (Old)\",\"from\":[0,0,1],\"to\":[0,0,2]}").say("ok");
        run("x");
        assertTrue(toolLog.get(0).contains("cannot be placed"), toolLog.get(0));
    }

    @Test
    void shapeVariantIsResolvedFromTheBaseBlock() throws Exception {
        llm.call("fill", "{\"block\":\"Grey Basic Armor\",\"shape\":\"wedge\",\"from\":[0,0,1],\"to\":[0,0,1]}").say("ok");
        run("x");
        assertEquals(TestCatalog.HULL_WEDGE, world.get(0, 0, 1).type());
    }

    @Test
    void airCarvesBlocksAway() throws Exception {
        llm.call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[1,0,0],\"to\":[5,0,0]}")
                .call("fill", "{\"block\":\"air\",\"from\":[2,0,0],\"to\":[4,0,0]}")
                .say("ok");
        run("x");
        assertEquals(3, world.blockCount()); // core + two hull ends
    }

    @Test
    void scanShowsTheModelWhatItBuilt() throws Exception {
        llm.call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[-1,0,1],\"to\":[1,0,2]}")
                .call("scan", "{\"from\":[-1,0,0],\"to\":[1,0,2]}")
                .say("ok");
        run("x");
        String scan = llm.lastToolResult();
        // rows run from the front (z=2) down to z=0, where only the core sits
        assertTrue(scan.contains("AAA\nAAA\n.@.\n"), scan);
        assertTrue(scan.contains("A Grey Basic Armor"), scan);
    }

    @Test
    void cutOffReplyIsNudgedInsteadOfEndingTheRequest() throws Exception {
        llm.cutOff()
                .call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[1,0,0],\"to\":[2,0,0]}")
                .say("done");
        assertEquals("done", run("x"));
        assertEquals(3, world.blockCount());
    }

    @Test
    void announcingAPlanWithoutActingIsNudgedToAct() throws Exception {
        llm.say("I will build a sleek grey fighter.")
                .call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[1,0,0],\"to\":[2,0,0]}")
                .say("Built a two-block fighter.");
        assertEquals("Built a two-block fighter.", run("x"));
        assertEquals(3, world.blockCount());
    }

    @Test
    void questionsAndFinishedReportsEndTheTurn() throws Exception {
        llm.say("How long should it be? I'll match whatever you say?");
        assertEquals("How long should it be? I'll match whatever you say?", run("x"));
        llm.say("Built a 20 block fighter with one cannon.");
        assertEquals("Built a 20 block fighter with one cannon.", run("y"));
    }

    @Test
    void linkWithOnlyOneCornerRunsFromTheController() throws Exception {
        llm.call("fill", "{\"block\":\"Cannon Barrel\",\"from\":[0,0,2],\"to\":[0,0,4]}")
                .call("place_blocks", "{\"blocks\":[{\"pos\":[0,0,1],\"block\":\"Cannon Computer\"}]}")
                .call("link", "{\"controller\":[0,0,1],\"to\":[0,0,4]}")
                .say("ok");
        run("x");
        assertEquals(3, world.linksFrom(new Vec3(0, 0, 1)).size());
    }

    @Test
    void sameFailingCallOverAndOverEndsTheRequest() throws Exception {
        for (int i = 0; i < 8; i++) llm.call("link", "{\"controller\":[0,0,1]}");
        String reply = run("x");
        assertTrue(reply.contains("got stuck"), reply);
        assertEquals(3, toolLog.size(), "the fourth identical failure ends the request");
        assertTrue(toolLog.get(1).contains("already made this exact call"), toolLog.get(1));
    }

    @Test
    void stopsAtTheStepLimit() throws Exception {
        for (int i = 0; i < 10; i++) llm.call("design_info", "{}");
        assertTrue(run("x").contains("step limit"));
    }

    @Test
    void cancelAnswersPendingCallsWithoutRunningThem() throws Exception {
        llm.call("fill", "{\"block\":\"Grey Basic Armor\",\"from\":[1,0,0],\"to\":[5,0,0]}");
        boolean[] cancelled = {false};
        AgentLoop loop = new AgentLoop(llm, ToolRegistry.standard(), "system", 10, 100_000, new LlmClient.Options(0, ""), false);
        String reply = loop.run(new Conversation(), "x", ctx, new AgentLoop.Listener() {
            @Override
            public void note(String text) {}

            @Override
            public void tool(String name, String result) {
                cancelled[0] = true;
            }
        }, () -> cancelled[0]);
        assertEquals("Stopped.", reply);
    }
}
