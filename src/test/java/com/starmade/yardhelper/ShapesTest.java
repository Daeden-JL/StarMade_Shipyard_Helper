package com.starmade.yardhelper;

import com.starmade.yardhelper.shape.Shapes;
import com.starmade.yardhelper.world.Box;
import com.starmade.yardhelper.world.Vec3;
import org.junit.jupiter.api.Test;

import java.util.HashSet;
import java.util.List;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ShapesTest {
    private static final int MAX = 1_000_000;

    @Test
    void solidBoxHasEveryBlock() {
        assertEquals(60, Shapes.box(Box.of(new Vec3(0, 0, 0), new Vec3(2, 3, 4)), Shapes.Fill.SOLID, MAX).size());
    }

    @Test
    void hollowBoxIsOneThickShell() {
        List<Vec3> shell = Shapes.box(Box.of(new Vec3(0, 0, 0), new Vec3(4, 4, 4)), Shapes.Fill.HOLLOW, MAX);
        assertEquals(125 - 27, shell.size());
        assertFalse(shell.contains(new Vec3(2, 2, 2)));
    }

    @Test
    void frameBoxIsOnlyEdges() {
        List<Vec3> frame = Shapes.box(Box.of(new Vec3(0, 0, 0), new Vec3(4, 4, 4)), Shapes.Fill.FRAME, MAX);
        // 12 edges of 5 blocks sharing 8 corners
        assertEquals(12 * 5 - 8 * 2, frame.size());
        assertFalse(frame.contains(new Vec3(2, 2, 0)));
    }

    @Test
    void boxCornersMayBeGivenInAnyOrder() {
        assertEquals(Box.of(new Vec3(0, 0, 0), new Vec3(2, 3, 4)), Box.of(new Vec3(2, 0, 4), new Vec3(0, 3, 0)));
    }

    @Test
    void oversizedShapeIsRefused() {
        assertThrows(Shapes.TooLargeException.class,
                () -> Shapes.box(Box.of(new Vec3(0, 0, 0), new Vec3(99, 99, 99)), Shapes.Fill.SOLID, 1000));
    }

    @Test
    void lineIsGapFreeAndIncludesBothEnds() {
        List<Vec3> line = Shapes.line(new Vec3(0, 0, 0), new Vec3(10, 4, -3));
        assertEquals(11, line.size());
        assertEquals(new Vec3(0, 0, 0), line.get(0));
        assertEquals(new Vec3(10, 4, -3), line.get(line.size() - 1));
        for (int i = 1; i < line.size(); i++) {
            Vec3 a = line.get(i - 1), b = line.get(i);
            assertTrue(Math.abs(a.x() - b.x()) <= 1 && Math.abs(a.y() - b.y()) <= 1 && Math.abs(a.z() - b.z()) <= 1);
        }
    }

    @Test
    void sphereSpansTwiceRadiusPlusOneAndIsSymmetric() {
        Set<Vec3> s = new HashSet<>(Shapes.ellipsoid(new Vec3(0, 0, 0), 3, 3, 3, false, MAX));
        assertTrue(s.contains(new Vec3(3, 0, 0)));
        assertFalse(s.contains(new Vec3(4, 0, 0)));
        assertFalse(s.contains(new Vec3(3, 3, 3)));
        for (Vec3 p : s) assertTrue(s.contains(new Vec3(-p.x(), -p.y(), -p.z())));
    }

    @Test
    void hollowSphereHasNoInterior() {
        Set<Vec3> s = new HashSet<>(Shapes.ellipsoid(new Vec3(0, 0, 0), 5, 5, 5, true, MAX));
        assertFalse(s.contains(new Vec3(0, 0, 0)));
        assertTrue(s.contains(new Vec3(5, 0, 0)));
    }

    @Test
    void cylinderRunsAlongItsAxisFromStart() {
        Set<Vec3> c = new HashSet<>(Shapes.cylinder(new Vec3(0, 0, 10), Shapes.Axis.Z, 5, 2, 2, 1.0, false, MAX));
        assertTrue(c.contains(new Vec3(0, 0, 10)));
        assertTrue(c.contains(new Vec3(2, 0, 14)));
        assertFalse(c.contains(new Vec3(0, 0, 15)));
        assertFalse(c.contains(new Vec3(0, 0, 9)));
    }

    @Test
    void coneNarrowsToAPoint() {
        Set<Vec3> c = new HashSet<>(Shapes.cylinder(new Vec3(0, 0, 0), Shapes.Axis.Z, 6, 3, 3, 0.0, false, MAX));
        assertTrue(c.contains(new Vec3(3, 0, 0)));
        assertTrue(c.contains(new Vec3(0, 0, 5)));
        assertFalse(c.contains(new Vec3(1, 0, 5)));
    }
}
