package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.attribute.BasicFileAttributes;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;

/**
 * The fingerprint that tells the shipyard whether a blueprint changed. Both sides compute
 * it the same way: SHA-256 over every file in the blueprint's folder, in unsigned byte
 * order of the UTF-8 path relative to that folder ('/' between parts), each file as its
 * path, a zero byte, its length as eight big-endian bytes, and its contents.
 */
public final class Fingerprint {
    /**
     * One file of a blueprint: where it is, how big, when it was last written, and which
     * file on disk it is (empty where the file system does not say). Two listings with
     * equal stamps are taken to be the same bytes; the fingerprint itself uses only the
     * path and the contents.
     */
    public record Stamp(String path, long size, long modified, String id) {
    }

    private Fingerprint() {
    }

    /** Orders paths by their UTF-8 bytes, read as unsigned. */
    public static int comparePaths(String a, String b) {
        return Arrays.compareUnsigned(a.getBytes(StandardCharsets.UTF_8), b.getBytes(StandardCharsets.UTF_8));
    }

    /** Every file under the folder, in the order the fingerprint takes them. */
    public static List<Stamp> stamps(Path folder) throws IOException {
        List<Stamp> out = new ArrayList<>();
        Files.walkFileTree(folder, new SimpleFileVisitor<>() {
            @Override
            public FileVisitResult visitFile(Path file, BasicFileAttributes attrs) {
                if (attrs.isRegularFile()) {
                    String relative = folder.relativize(file).toString().replace('\\', '/');
                    Object key = attrs.fileKey();
                    out.add(new Stamp(relative, attrs.size(), attrs.lastModifiedTime().toMillis(), key == null ? "" : key.toString()));
                }
                return FileVisitResult.CONTINUE;
            }
        });
        out.sort((x, y) -> comparePaths(x.path(), y.path()));
        return out;
    }

    /** The fingerprint of a folder as it is right now. */
    public static String of(Path folder) throws IOException {
        return of(folder, stamps(folder));
    }

    /**
     * The fingerprint of the listed files.
     *
     * @throws IOException if a file is gone or no longer has the listed size, which means
     *                     the folder is being written to
     */
    public static String of(Path folder, List<Stamp> stamps) throws IOException {
        MessageDigest hash;
        try {
            hash = MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
        byte[] buffer = new byte[1 << 16];
        for (Stamp stamp : stamps) {
            hash.update(stamp.path().getBytes(StandardCharsets.UTF_8));
            hash.update((byte) 0);
            for (int shift = 56; shift >= 0; shift -= 8) hash.update((byte) (stamp.size() >>> shift));
            long left = stamp.size();
            try (InputStream in = Files.newInputStream(folder.resolve(stamp.path()))) {
                int n;
                while ((n = in.read(buffer)) > 0) {
                    left -= n;
                    if (left < 0) break;
                    hash.update(buffer, 0, n);
                }
            }
            if (left != 0) throw new IOException(stamp.path() + " changed size while it was read");
        }
        return HexFormat.of().formatHex(hash.digest());
    }
}
