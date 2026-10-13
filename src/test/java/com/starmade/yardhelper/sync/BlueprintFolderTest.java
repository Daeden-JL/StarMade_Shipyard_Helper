package com.starmade.yardhelper.sync;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class BlueprintFolderTest {
    private static final long NOW = 1_000_000, QUIET = 5_000, OLD = NOW - 60_000;

    @TempDir
    Path tmp;
    Path dir;
    BlueprintFolder folder;

    @BeforeEach
    void setUp() throws IOException {
        dir = Files.createDirectories(tmp.resolve("blueprints"));
        folder = new BlueprintFolder(dir);
    }

    /** Name to fingerprint, or "busy". */
    private Map<String, String> list() throws IOException {
        Map<String, String> out = new TreeMap<>();
        for (BlueprintFolder.Listed l : folder.list(NOW, QUIET)) {
            assertEquals(l.busy(), l.print().isEmpty(), "a busy blueprint has no fingerprint, any other has one");
            out.put(l.name(), l.busy() ? "busy" : l.print());
        }
        return out;
    }

    @Test
    void listsBlueprintsAndPassesOverWhatTheGamePassesOver() throws IOException {
        Path scout = Blueprints.write(dir, "Scout", "s", OLD);
        Blueprints.write(dir, "exported", "backup", OLD);
        Blueprints.write(dir, "With.Dot", "d", OLD);
        Blueprints.write(dir, "x".repeat(129), "long", OLD);
        Blueprints.write(dir, "x".repeat(128), "fits", OLD);
        Files.delete(Blueprints.write(dir, "NoHeader", "n", OLD).resolve("header.smbph"));
        Files.delete(Blueprints.write(dir, "NoMeta", "n", OLD).resolve("meta.smbpm"));
        Files.write(dir.resolve("bbversion"), new byte[]{1});
        Map<String, String> listed = list();
        assertEquals(List.of("Scout", "x".repeat(128)), List.copyOf(listed.keySet()));
        assertEquals(Fingerprint.of(scout), listed.get("Scout"));
    }

    @Test
    void aBlueprintWrittenAMomentAgoIsBusy() throws IOException {
        Path scout = Blueprints.write(dir, "Scout", "s", OLD);
        assertFalse(list().get("Scout").equals("busy"));
        Files.setLastModifiedTime(scout.resolve("DATA/r.0.0.0.smd3"), java.nio.file.attribute.FileTime.fromMillis(NOW - 1_000));
        assertEquals("busy", list().get("Scout"));
        // Once it has been quiet long enough it is read.
        Blueprints.age(scout, NOW - QUIET - 1);
        assertEquals(Fingerprint.of(scout), list().get("Scout"));
    }

    @Test
    void aBlueprintTheGameIsSavingOverIsBusyAndNeverMissing() throws IOException {
        Blueprints.write(dir, "Scout", "v1", OLD);
        Blueprints.write(dir, "Hauler", "h", OLD);
        assertEquals(2, list().size());
        // The game renames Scout to ScoutTmp...
        Files.move(dir.resolve("Scout"), dir.resolve("ScoutTmp"));
        Map<String, String> first = list();
        assertEquals("busy", first.get("Scout"), "gone for an instant, but it was there a moment ago");
        assertEquals("busy", first.get("ScoutTmp"));
        // ...makes a new, empty Scout and fills it...
        Files.createDirectories(dir.resolve("Scout"));
        assertEquals("busy", list().get("Scout"));
        Blueprints.write(dir, "Scout", "v2", OLD);
        assertEquals("busy", list().get("Scout"), "ScoutTmp is still there");
        // ...and removes ScoutTmp.
        BlueprintFolder.deleteTree(dir.resolve("ScoutTmp"));
        Map<String, String> after = list();
        assertEquals(List.of("Hauler", "Scout"), List.copyOf(after.keySet()));
        assertEquals(Fingerprint.of(dir.resolve("Scout")), after.get("Scout"));
    }

    @Test
    void aNameEndingInTmpIsAnOrdinaryBlueprintWhenNothingElseHasItsBaseName() throws IOException {
        Path lone = Blueprints.write(dir, "RaiderTmp", "r", OLD);
        assertEquals(Map.of("RaiderTmp", Fingerprint.of(lone)), list());
        assertEquals(Map.of("RaiderTmp", Fingerprint.of(lone)), list());
    }

    @Test
    void unchangedBlueprintsAreNotReadAgain() throws IOException {
        Path scout = Blueprints.write(dir, "Scout", "v1", OLD);
        Blueprints.write(dir, "Hauler", "h", OLD);
        String first = list().get("Scout");
        assertEquals(2, folder.hashedCount());
        list();
        list();
        assertEquals(2, folder.hashedCount());
        assertTrue(folder.unchanged("Scout"));
        // A new save: other bytes, a new time.
        Files.write(scout.resolve("header.smbph"), "header v2".getBytes());
        Blueprints.age(scout, OLD + 1);
        assertFalse(folder.unchanged("Scout"));
        String second = list().get("Scout");
        assertEquals(3, folder.hashedCount());
        assertFalse(second.equals(first));
        assertEquals(Fingerprint.of(scout), second);
        // A deleted blueprint is simply not listed.
        BlueprintFolder.deleteTree(scout);
        assertEquals(List.of("Hauler"), List.copyOf(list().keySet()));
        assertNull(folder.stampsOf("Scout"));
    }

    @Test
    void aMissingBlueprintFolderIsAnErrorNotAnEmptyList() {
        BlueprintFolder gone = new BlueprintFolder(tmp.resolve("no such folder"));
        assertThrows(IOException.class, () -> gone.list(NOW, QUIET));
    }

    @Test
    void aNewVersionReplacesTheOldWhichIsKept() throws IOException {
        Path backups = tmp.resolve("work/replaced");
        Blueprints.write(dir, "Scout", "v1", OLD);
        String v1 = Fingerprint.of(dir.resolve("Scout"));
        Path staged = Blueprints.write(tmp.resolve("work/staging"), "Scout", "v2", OLD);
        String v2 = Fingerprint.of(staged);

        Path backup = folder.install(staged, "Scout", backups, 5000);
        assertEquals(v2, Fingerprint.of(dir.resolve("Scout")));
        assertEquals(backups.resolve("Scout.5000"), backup);
        assertEquals(v1, Fingerprint.of(backup));
        assertFalse(Files.exists(staged));

        // A blueprint that was not there before has nothing to back up.
        Path fresh = Blueprints.write(tmp.resolve("work/staging"), "Fresh", "f", OLD);
        assertNull(folder.install(fresh, "Fresh", backups, 6000));
        assertTrue(Files.isRegularFile(dir.resolve("Fresh/header.smbph")));
    }

    @Test
    void aFailedInstallLeavesTheOldBlueprintInPlace() throws IOException {
        Blueprints.write(dir, "Scout", "v1", OLD);
        String v1 = Fingerprint.of(dir.resolve("Scout"));
        Path missing = tmp.resolve("work/staging/Scout");
        assertThrows(IOException.class, () -> folder.install(missing, "Scout", tmp.resolve("work/replaced"), 5000));
        assertEquals(v1, Fingerprint.of(dir.resolve("Scout")));
    }

    @Test
    void anInstallTheGameRejectsIsTakenBackOut() throws IOException {
        Path backups = tmp.resolve("work/replaced"), rejected = tmp.resolve("work/rejected");
        Blueprints.write(dir, "Scout", "v1", OLD);
        String v1 = Fingerprint.of(dir.resolve("Scout"));
        Path staged = Blueprints.write(tmp.resolve("work/staging"), "Scout", "bad", OLD);
        String bad = Fingerprint.of(staged);
        Path backup = folder.install(staged, "Scout", backups, 5000);
        folder.undoInstall("Scout", backup, rejected, 5000);
        assertEquals(v1, Fingerprint.of(dir.resolve("Scout")));
        assertEquals(bad, Fingerprint.of(rejected.resolve("Scout.5000")));
        // A new name that is rejected just goes away again.
        Path fresh = Blueprints.write(tmp.resolve("work/staging"), "Fresh", "f", OLD);
        folder.undoInstall("Fresh", folder.install(fresh, "Fresh", backups, 6000), rejected, 6000);
        assertFalse(Files.exists(dir.resolve("Fresh")));
    }

    @Test
    void onlyTheNewestFewOldVersionsAreKept() throws IOException {
        Path backups = Files.createDirectories(tmp.resolve("replaced"));
        for (long t : new long[]{100, 300, 200, 500, 400}) Blueprints.write(backups, "Scout." + t, "v" + t, OLD);
        Blueprints.write(backups, "Scout Two.100", "other", OLD);
        Blueprints.write(backups, "Scout.notes", "not a version", OLD);
        BlueprintFolder.prune(backups, "Scout", 3);
        try (var left = Files.list(backups)) {
            assertEquals(List.of("Scout Two.100", "Scout.300", "Scout.400", "Scout.500", "Scout.notes"),
                    left.map(p -> p.getFileName().toString()).sorted().toList());
        }
    }

    @Test
    void aNameInOtherLetterCaseIsFound() throws IOException {
        Blueprints.write(dir, "scout", "s", OLD);
        assertEquals("scout", folder.spelledDifferently("Scout"));
        assertNull(folder.spelledDifferently("scout"));
        assertNull(folder.spelledDifferently("Hauler"));
    }
}
