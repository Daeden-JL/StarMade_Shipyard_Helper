package com.starmade.yardhelper;

import com.starmade.yardhelper.shape.Shapes;
import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.DirectGateway;
import com.starmade.yardhelper.world.Editor;
import com.starmade.yardhelper.world.InMemoryDesignWorld;
import com.starmade.yardhelper.world.Journal;
import com.starmade.yardhelper.world.Vec3;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class EditorTest {
    private InMemoryDesignWorld world;
    private Editor editor;

    @BeforeEach
    void setUp() {
        world = InMemoryDesignWorld.withCore(TestCatalog.CORE, 20);
        editor = new Editor(new DirectGateway(world), TestCatalog.create(), new Journal(10), 100_000);
        editor.journal().begin("test");
    }

    private List<Vec3> box(int x0, int y0, int z0, int x1, int y1, int z1) {
        return Shapes.box(Box.of(new Vec3(x0, y0, z0), new Vec3(x1, y1, z1)), Shapes.Fill.SOLID, 100_000);
    }

    @Test
    void placingNeverOverwritesTheCore() {
        editor.place(box(-1, -1, -1, 1, 1, 1), Block.of(TestCatalog.HULL), false);
        assertEquals(TestCatalog.CORE, world.get(0, 0, 0).type());
        assertEquals(27, world.blockCount());
    }

    @Test
    void resultSaysWhenTheCoreWasSkippedOrOtherBlocksOverwritten() {
        editor.place(List.of(new Vec3(1, 0, 0)), Block.of(TestCatalog.CANNON_BARREL), false);
        Editor.Result r = editor.place(box(0, 0, 0, 1, 0, 0), Block.of(TestCatalog.HULL), false);
        assertTrue(r.coreSkipped());
        assertEquals(1, r.overwritten().get("Cannon Barrel"));
        assertTrue(r.toString().contains("overwrote existing: 1 x Cannon Barrel"), r.toString());
        assertTrue(r.toString().contains("Ship Core"), r.toString());
    }

    @Test
    void removingNeverRemovesTheCore() {
        editor.remove(box(-1, -1, -1, 1, 1, 1));
        assertEquals(TestCatalog.CORE, world.get(0, 0, 0).type());
    }

    @Test
    void blocksOutsideTheBuildAreaAreSkippedAndCounted() {
        Editor.Result r = editor.place(box(19, 0, 0, 22, 0, 0), Block.of(TestCatalog.HULL), false);
        assertEquals(2, r.changed());
        assertEquals(2, r.outOfBounds());
    }

    @Test
    void onlyEmptyKeepsExistingBlocks() {
        editor.place(List.of(new Vec3(1, 0, 0)), Block.of(TestCatalog.GLASS), false);
        Editor.Result r = editor.place(box(1, 0, 0, 3, 0, 0), Block.of(TestCatalog.HULL), true);
        assertEquals(2, r.changed());
        assertEquals(1, r.skippedOccupied());
        assertEquals(TestCatalog.GLASS, world.get(1, 0, 0).type());
    }

    @Test
    void xSymmetryMirrorsAcrossTheCorePlane() {
        editor.setSymmetry(true, false, false);
        editor.place(List.of(new Vec3(3, 1, 2)), Block.of(TestCatalog.HULL), false);
        assertEquals(TestCatalog.HULL, world.get(-3, 1, 2).type());
        // A block on the mirror plane is placed once.
        Editor.Result r = editor.place(List.of(new Vec3(0, 5, 0)), Block.of(TestCatalog.HULL), false);
        assertEquals(1, r.changed());
    }

    @Test
    void mirroringFlipsCubeFacingAlongTheMirroredAxis() {
        editor.setSymmetry(true, false, false);
        editor.place(List.of(new Vec3(3, 0, 0)), Block.of(TestCatalog.CANNON_COMPUTER, 4), false); // facing right
        assertEquals(4, world.get(3, 0, 0).orientation());
        assertEquals(5, world.get(-3, 0, 0).orientation()); // mirrored copy faces left
    }

    @Test
    void undoRestoresBlocksAndLinks() {
        world.set(5, 0, 0, Block.of(TestCatalog.GLASS));
        editor.place(box(4, 0, 0, 6, 0, 0), Block.of(TestCatalog.CANNON_BARREL), false);
        editor.place(List.of(new Vec3(4, 1, 0)), Block.of(TestCatalog.CANNON_COMPUTER), false);
        editor.link(new Vec3(4, 1, 0), box(4, 0, 0, 6, 0, 0));
        assertEquals(3, world.linksFrom(new Vec3(4, 1, 0)).size());

        assertEquals("test", editor.undo());
        assertEquals(TestCatalog.GLASS, world.get(5, 0, 0).type());
        assertTrue(world.get(4, 0, 0).isAir());
        assertTrue(world.get(4, 1, 0).isAir());
        assertTrue(world.linksFrom(new Vec3(4, 1, 0)).isEmpty());
        assertNull(editor.undo());
    }

    @Test
    void linkRefusesTargetsTheControllerCannotControl() {
        editor.place(List.of(new Vec3(1, 0, 0)), Block.of(TestCatalog.CANNON_COMPUTER), false);
        editor.place(List.of(new Vec3(2, 0, 0)), Block.of(TestCatalog.CANNON_BARREL), false);
        editor.place(List.of(new Vec3(3, 0, 0)), Block.of(TestCatalog.THRUSTER), false);
        String result = editor.link(new Vec3(1, 0, 0), List.of(new Vec3(2, 0, 0), new Vec3(3, 0, 0), new Vec3(9, 9, 9)));
        assertEquals(List.of(new Vec3(2, 0, 0)), world.linksFrom(new Vec3(1, 0, 0)));
        assertTrue(result.contains("Linked 1"), result);
        assertTrue(result.contains("1 targets refused"), result);
        assertTrue(result.contains("1 target positions were empty"), result);
    }

    @Test
    void replaceSwapsOnlyTheNamedType() {
        editor.place(box(1, 0, 0, 3, 0, 0), Block.of(TestCatalog.HULL), false);
        editor.place(List.of(new Vec3(4, 0, 0)), Block.of(TestCatalog.GLASS), false);
        Editor.Result r = editor.replace(Box.of(new Vec3(0, 0, 0), new Vec3(5, 0, 0)), TestCatalog.HULL, Block.of(TestCatalog.THRUSTER));
        assertEquals(3, r.changed());
        assertEquals(TestCatalog.THRUSTER, world.get(2, 0, 0).type());
        assertEquals(TestCatalog.GLASS, world.get(4, 0, 0).type());
        assertEquals(TestCatalog.CORE, world.get(0, 0, 0).type());
    }
}
