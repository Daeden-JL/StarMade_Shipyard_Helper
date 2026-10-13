package com.starmade.yardhelper.sync;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class FingerprintTest {
    @TempDir
    Path dir;

    /** The same three files the shipyard's own test fingerprints, with the value it expects. */
    @Test
    void matchesTheShipyardsValue() throws IOException {
        Path folder = dir.resolve("X");
        Files.createDirectories(folder.resolve("DATA"));
        Files.write(folder.resolve("header.smbph"), new byte[]{1, 2, 3});
        Files.write(folder.resolve("DATA/r.smd3"), new byte[0]);
        Files.write(folder.resolve("meta.smbpm"), new byte[]{0, 0, 0, 5, 1});
        assertEquals("7776a573a470708d2175ed74d051ad3aed710301d9bd9ea032ba566af8f290f6", Fingerprint.of(folder));
        // The folder's own name is no part of it.
        Files.move(folder, dir.resolve("Renamed"));
        assertEquals("7776a573a470708d2175ed74d051ad3aed710301d9bd9ea032ba566af8f290f6", Fingerprint.of(dir.resolve("Renamed")));
    }

    @Test
    void filesAreTakenInByteOrderOfTheirWholePath() throws IOException {
        // '-' sorts before '/', so "DATA-old/z" comes before "DATA/a" although a walk
        // of the folders visits DATA first.
        Path folder = dir.resolve("S");
        Files.createDirectories(folder.resolve("DATA"));
        Files.createDirectories(folder.resolve("DATA-old"));
        Files.write(folder.resolve("DATA/a"), new byte[]{1});
        Files.write(folder.resolve("DATA-old/z"), new byte[]{2});
        Files.write(folder.resolve("header.smbph"), new byte[]{3});
        List<String> order = new ArrayList<>();
        for (Fingerprint.Stamp s : Fingerprint.stamps(folder)) order.add(s.path());
        assertEquals(List.of("DATA-old/z", "DATA/a", "header.smbph"), order);
        // Upper case before lower case, as bytes have it.
        assertTrue(Fingerprint.comparePaths("Zeta", "alpha") < 0);
        // UTF-8 order, not Java's UTF-16 order: U+FF5E (three bytes, EF..) comes before
        // U+1F600 (four bytes, F0..), while as Java chars the surrogate D83D is smaller.
        String wide = "\uFF5E", emoji = "\uD83D\uDE00";
        assertTrue(wide.compareTo(emoji) > 0);
        assertTrue(Fingerprint.comparePaths(wide, emoji) < 0);
    }

    @Test
    void anyChangeOfNameLengthOrContentShows() throws IOException {
        Path a = Blueprints.write(dir, "A", "one", 1000);
        String print = Fingerprint.of(a);
        assertEquals(64, print.length());
        assertEquals(print, Fingerprint.of(Blueprints.write(dir, "B", "one", 99_000)), "times are not part of it");
        assertNotEquals(print, Fingerprint.of(Blueprints.write(dir, "C", "two", 1000)));
        Files.write(a.resolve("extra"), new byte[0]);
        assertNotEquals(print, Fingerprint.of(a));
    }

    @Test
    void aFileThatChangedSinceItWasListedIsAnError() throws IOException {
        Path a = Blueprints.write(dir, "A", "one", 1000);
        List<Fingerprint.Stamp> stamps = Fingerprint.stamps(a);
        Files.write(a.resolve("header.smbph"), "longer than before".getBytes());
        assertThrows(IOException.class, () -> Fingerprint.of(a, stamps));
        Files.delete(a.resolve("meta.smbpm"));
        assertThrows(IOException.class, () -> Fingerprint.of(a, stamps));
    }
}
