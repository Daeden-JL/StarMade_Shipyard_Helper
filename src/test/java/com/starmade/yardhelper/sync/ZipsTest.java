package com.starmade.yardhelper.sync;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.stream.Stream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ZipsTest {
    @TempDir
    Path dir;

    private Path archive(String... entries) throws IOException {
        Path zip = Files.createTempFile(dir, "made", ".zip");
        try (ZipOutputStream out = new ZipOutputStream(Files.newOutputStream(zip))) {
            for (String entry : entries) {
                out.putNextEntry(new ZipEntry(entry));
                if (!entry.endsWith("/")) out.write(1);
                out.closeEntry();
            }
        }
        return zip;
    }

    @Test
    void aPackedBlueprintUnpacksToTheSameFiles() throws IOException {
        Path folder = Blueprints.write(dir.resolve("game"), "Scout Mk 2", "abc", 1000);
        Files.createDirectories(folder.resolve("ATTACHED_0/DATA"));
        Files.write(folder.resolve("ATTACHED_0/header.smbph"), new byte[]{9});
        Path zip = dir.resolve("out.zip");
        Zips.zip(folder, "Scout Mk 2", Fingerprint.stamps(folder), zip);
        Path top = Zips.unzip(zip, dir.resolve("unpacked"));
        assertEquals("Scout Mk 2", top.getFileName().toString());
        assertEquals(dir.resolve("unpacked").toAbsolutePath().normalize(), top.getParent());
        assertEquals(Fingerprint.of(folder), Fingerprint.of(top));
    }

    @Test
    void pathsThatLeaveTheFolderAreRefused() throws IOException {
        String[][] bad = {
                {"Ship/header.smbph", "../escaped"},
                {"Ship/../../escaped"},
                {"/etc/escaped"},
                {"Ship/a/../../../escaped"},
                {"C:\\escaped"},
                {"..\\escaped"},
                {"Ship\\..\\..\\escaped"},
                {"./Ship/header.smbph"},
                {"Ship//header.smbph"},
        };
        for (String[] entries : bad) {
            Path into = Files.createTempDirectory(dir, "into");
            assertThrows(IOException.class, () -> Zips.unzip(archive(entries), into.resolve("unpacked")), String.join(",", entries));
        }
        // Nothing was written anywhere but under the folders handed in.
        try (Stream<Path> all = Files.walk(dir)) {
            assertFalse(all.anyMatch(p -> p.getFileName().toString().equals("escaped")));
        }
        assertFalse(Files.exists(dir.getParent().resolve("escaped")));
    }

    @Test
    void anArchiveMustHoldExactlyOneFolder() throws IOException {
        assertThrows(IOException.class, () -> Zips.unzip(archive(), dir.resolve("a")));
        assertThrows(IOException.class, () -> Zips.unzip(archive("One/header.smbph", "Two/header.smbph"), dir.resolve("b")));
        assertThrows(IOException.class, () -> Zips.unzip(archive("loose.txt"), dir.resolve("c")));
        Path top = Zips.unzip(archive("One/", "One/header.smbph", "One\\DATA\\r.smd3"), dir.resolve("d"));
        assertTrue(Files.isRegularFile(top.resolve("DATA/r.smd3")), "paths written on Windows are understood");
    }
}
