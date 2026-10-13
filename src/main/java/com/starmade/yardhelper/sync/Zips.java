package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import java.util.zip.ZipOutputStream;

/** Packing a blueprint folder into the file the shipyard takes, and unpacking one it gives. */
public final class Zips {
    /** A file from the shipyard may unpack to this much and no more. */
    static final long MAX_UNPACKED = 4L * 1024 * 1024 * 1024;
    static final int MAX_ENTRIES = 100_000;

    private Zips() {
    }

    /** Writes the listed files of a folder into a zip under one top folder called {@code name}. */
    public static void zip(Path folder, String name, List<Fingerprint.Stamp> stamps, Path out) throws IOException {
        try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(out))) {
            // Block data is already compressed; anything more than a light pass is wasted.
            zip.setLevel(Deflater.BEST_SPEED);
            for (Fingerprint.Stamp stamp : stamps) {
                zip.putNextEntry(new ZipEntry(name + "/" + stamp.path()));
                Files.copy(folder.resolve(stamp.path()), zip);
                zip.closeEntry();
            }
        }
    }

    /**
     * Unpacks a zip that holds exactly one top folder and returns that folder. Nothing is
     * written outside {@code into}: an entry that tries is refused, and so is the file.
     */
    public static Path unzip(Path zipFile, Path into) throws IOException {
        Files.createDirectories(into);
        Path root = into.toAbsolutePath().normalize();
        String top = null;
        long total = 0;
        int entries = 0;
        byte[] buffer = new byte[1 << 16];
        try (ZipInputStream zip = new ZipInputStream(Files.newInputStream(zipFile))) {
            ZipEntry entry;
            while ((entry = zip.getNextEntry()) != null) {
                if (++entries > MAX_ENTRIES) throw new IOException("too many files in the archive");
                String name = entry.getName().replace('\\', '/');
                if (name.startsWith("/") || name.contains(":")) throw new IOException("unsafe path in the archive: " + entry.getName());
                String[] parts = name.split("/");
                for (String part : parts) {
                    if (part.isEmpty() && !name.endsWith("/")) throw new IOException("unsafe path in the archive: " + entry.getName());
                    if (part.equals("..") || part.equals(".")) throw new IOException("unsafe path in the archive: " + entry.getName());
                }
                if (parts.length == 0 || parts[0].isEmpty()) throw new IOException("unsafe path in the archive: " + entry.getName());
                if (top == null) top = parts[0];
                else if (!top.equals(parts[0])) throw new IOException("the archive holds more than one folder");
                Path target = root.resolve(name).normalize();
                if (!target.startsWith(root)) throw new IOException("unsafe path in the archive: " + entry.getName());
                if (entry.isDirectory()) {
                    Files.createDirectories(target);
                    continue;
                }
                if (parts.length < 2) throw new IOException("the archive has a file outside its folder: " + entry.getName());
                Files.createDirectories(target.getParent());
                try (OutputStream out = Files.newOutputStream(target)) {
                    int n;
                    while ((n = zip.read(buffer)) > 0) {
                        total += n;
                        if (total > MAX_UNPACKED) throw new IOException("the archive unpacks to more than 4 GB");
                        out.write(buffer, 0, n);
                    }
                }
            }
        }
        if (top == null) throw new IOException("the archive is empty");
        return root.resolve(top);
    }
}
