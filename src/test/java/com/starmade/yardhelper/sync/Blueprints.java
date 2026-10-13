package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import java.util.stream.Stream;

/** Blueprint folders for tests: the files the exchange looks at, with made-up contents. */
final class Blueprints {
    private Blueprints() {
    }

    /** Writes a blueprint whose files were last touched at {@code modified}. */
    static Path write(Path dir, String name, String contents, long modified) throws IOException {
        Path folder = dir.resolve(name);
        Files.createDirectories(folder.resolve("DATA"));
        Files.write(folder.resolve("header.smbph"), ("header " + contents).getBytes());
        Files.write(folder.resolve("meta.smbpm"), new byte[]{0, 0, 0, 5, 1});
        Files.write(folder.resolve("logic.smbpl"), new byte[12]);
        Files.write(folder.resolve("DATA").resolve("r.0.0.0.smd3"), ("blocks " + contents).getBytes());
        age(folder, modified);
        return folder;
    }

    /** Sets the time of every file in a folder. */
    static void age(Path folder, long modified) throws IOException {
        try (Stream<Path> files = Files.walk(folder)) {
            for (Path p : (Iterable<Path>) files::iterator) {
                if (Files.isRegularFile(p)) Files.setLastModifiedTime(p, FileTime.fromMillis(modified));
            }
        }
    }
}
