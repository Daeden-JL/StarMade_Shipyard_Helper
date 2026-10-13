package com.starmade.yardhelper.sync;

import java.io.IOException;
import java.nio.file.DirectoryStream;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;

/**
 * The game's blueprint folder as the exchange sees it: which blueprints are there, which
 * of them the game is busy writing, and how one is put in place of another without the
 * game ever seeing half a blueprint. Works on files only and knows nothing of the game.
 * Not thread safe: one thread lists, and the swap runs while that thread waits.
 */
public final class BlueprintFolder {
    public static final String HEADER = "header.smbph";
    public static final String META = "meta.smbpm";
    /** The game keeps its backups of deleted blueprints in this subfolder. */
    private static final String EXPORTED = "exported";
    /** While it saves a blueprint, the game keeps the previous version beside it under this suffix. */
    private static final String TMP = "Tmp";

    /** One blueprint as it is reported to the shipyard. */
    public record Listed(String name, String print, boolean busy) {
    }

    private record Known(List<Fingerprint.Stamp> stamps, String print) {
    }

    private final Path dir;
    /** Fingerprints by blueprint name, good for as long as the files' stamps stay the same. */
    private final Map<String, Known> known = new HashMap<>();
    /** The names of the last listing, to tell a blueprint that is being saved from one that is gone. */
    private Set<String> lastNames = new HashSet<>();
    private int hashed;

    public BlueprintFolder(Path dir) {
        this.dir = dir;
    }

    public Path path(String name) {
        return dir.resolve(name);
    }

    /** How many blueprints have been read in full so far; unchanged ones are not read again. */
    public int hashedCount() {
        return hashed;
    }

    /**
     * Every blueprint in the folder. A blueprint the game is in the middle of writing is
     * listed as busy and never left out, because the shipyard takes a missing name to mean
     * the blueprint was deleted.
     *
     * @param now         the current time in milliseconds
     * @param quietMillis a blueprint with a file written more recently than this is busy
     * @throws IOException if the folder itself cannot be read; no list is better than a wrong one
     */
    public List<Listed> list(long now, long quietMillis) throws IOException {
        Set<String> folders = new TreeSet<>();
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(dir)) {
            for (Path entry : stream) {
                String name = entry.getFileName().toString();
                if (Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS) && !name.equals(EXPORTED) && Names.valid(name)) {
                    folders.add(name);
                }
            }
        }
        // Saving over a blueprint, the game renames <name> to <name>Tmp, writes <name>
        // afresh and then deletes <name>Tmp. While both exist, neither is to be trusted.
        // For an instant only <name>Tmp exists: if <name> was there a moment ago it is
        // still reported, as busy, so that it is not taken for deleted.
        Set<String> saving = new HashSet<>();
        for (String name : folders) {
            if (!name.endsWith(TMP) || name.length() == TMP.length()) continue;
            String base = name.substring(0, name.length() - TMP.length());
            if (folders.contains(base) || lastNames.contains(base)) {
                saving.add(name);
                saving.add(base);
            }
        }
        Set<String> reported = new TreeSet<>(folders);
        reported.addAll(saving);

        List<Listed> out = new ArrayList<>();
        Set<String> names = new HashSet<>();
        for (String name : reported) {
            if (saving.contains(name)) {
                // Whatever was known about it is out of date once the save is through.
                known.remove(name);
                out.add(new Listed(name, "", true));
                names.add(name);
                continue;
            }
            Path folder = dir.resolve(name);
            // The game itself passes over a folder without these two files.
            if (!Files.isRegularFile(folder.resolve(HEADER)) || !Files.isRegularFile(folder.resolve(META))) {
                known.remove(name);
                continue;
            }
            names.add(name);
            String print = read(name, folder, now, quietMillis);
            out.add(print == null ? new Listed(name, "", true) : new Listed(name, print, false));
        }
        known.keySet().retainAll(names);
        lastNames = names;
        return out;
    }

    /** The fingerprint of one blueprint, or null if it is being written to. */
    private String read(String name, Path folder, long now, long quietMillis) {
        try {
            List<Fingerprint.Stamp> stamps = Fingerprint.stamps(folder);
            for (Fingerprint.Stamp stamp : stamps) {
                if (stamp.modified() > now - quietMillis) return null;
            }
            Known before = known.get(name);
            if (before != null && before.stamps().equals(stamps)) return before.print();
            String print = Fingerprint.of(folder, stamps);
            hashed++;
            // A file added, grown or rewritten while the rest was being read.
            if (!Fingerprint.stamps(folder).equals(stamps)) return null;
            known.put(name, new Known(stamps, print));
            return print;
        } catch (IOException e) {
            // Files vanishing or changing under the read: the game is at work on it.
            return null;
        }
    }

    /** The files the fingerprint of a listed blueprint was taken from, or null if it has none. */
    public List<Fingerprint.Stamp> stampsOf(String name) {
        Known k = known.get(name);
        return k == null ? null : k.stamps();
    }

    /** Whether a blueprint's files are still exactly the ones its fingerprint was taken from. */
    public boolean unchanged(String name) {
        Known k = known.get(name);
        if (k == null) return false;
        try {
            return Fingerprint.stamps(dir.resolve(name)).equals(k.stamps());
        } catch (IOException e) {
            return false;
        }
    }

    /** Forgets a blueprint's fingerprint, so the next listing reads it again. */
    public void forget(String name) {
        known.remove(name);
    }

    /**
     * The folder whose name equals {@code name} except for upper and lower case, if there
     * is one. The game finds docked entities by the exact spelling of the blueprint's
     * folder, so a blueprint cannot be written under a spelling that is already taken.
     */
    public String spelledDifferently(String name) throws IOException {
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(dir)) {
            for (Path entry : stream) {
                String other = entry.getFileName().toString();
                if (other.equalsIgnoreCase(name) && !other.equals(name)) return other;
            }
        }
        return null;
    }

    /**
     * Puts a complete blueprint folder in place under {@code name}. A blueprint already
     * there is moved to {@code backups} first, and moved back if the new one cannot be
     * put in. Both steps are renames, so the game sees the old blueprint or the new one.
     *
     * @return where the previous version went, or null if there was none
     */
    public Path install(Path staged, String name, Path backups, long now) throws IOException {
        Path target = dir.resolve(name);
        Path backup = null;
        if (Files.exists(target, LinkOption.NOFOLLOW_LINKS)) {
            Files.createDirectories(backups);
            backup = backups.resolve(name + "." + now);
            Files.move(target, backup, StandardCopyOption.ATOMIC_MOVE);
        }
        try {
            Files.move(staged, target, StandardCopyOption.ATOMIC_MOVE);
        } catch (IOException | RuntimeException e) {
            if (backup != null) Files.move(backup, target, StandardCopyOption.ATOMIC_MOVE);
            throw e;
        }
        known.remove(name);
        return backup;
    }

    /**
     * Takes back an install the game would not accept: the new folder goes to
     * {@code rejected} and the previous version, if there was one, returns to its place.
     */
    public void undoInstall(String name, Path backup, Path rejected, long now) throws IOException {
        Path target = dir.resolve(name);
        if (Files.exists(target, LinkOption.NOFOLLOW_LINKS)) {
            Files.createDirectories(rejected);
            Files.move(target, rejected.resolve(name + "." + now), StandardCopyOption.ATOMIC_MOVE);
        }
        if (backup != null) Files.move(backup, target, StandardCopyOption.ATOMIC_MOVE);
        known.remove(name);
    }

    /** Deletes all but the newest {@code keep} versions of {@code name} in a folder of set-aside blueprints. */
    public static void prune(Path folder, String name, int keep) throws IOException {
        if (!Files.isDirectory(folder)) return;
        List<Path> versions = new ArrayList<>();
        try (DirectoryStream<Path> stream = Files.newDirectoryStream(folder)) {
            for (Path entry : stream) {
                if (versionTime(entry.getFileName().toString(), name) >= 0) versions.add(entry);
            }
        }
        versions.sort(Comparator.comparingLong((Path p) -> versionTime(p.getFileName().toString(), name)).reversed());
        for (int i = keep; i < versions.size(); i++) deleteTree(versions.get(i));
    }

    /** The time in {@code <name>.<millis>}, or -1 if the file name is not a version of that blueprint. */
    private static long versionTime(String fileName, String name) {
        if (!fileName.startsWith(name + ".")) return -1;
        try {
            return Long.parseLong(fileName.substring(name.length() + 1));
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    public static void deleteTree(Path root) throws IOException {
        if (!Files.exists(root, LinkOption.NOFOLLOW_LINKS)) return;
        Files.walkFileTree(root, new SimpleFileVisitor<>() {
            @Override
            public FileVisitResult visitFile(Path file, BasicFileAttributes attrs) throws IOException {
                Files.delete(file);
                return FileVisitResult.CONTINUE;
            }

            @Override
            public FileVisitResult postVisitDirectory(Path d, IOException e) throws IOException {
                Files.delete(d);
                return FileVisitResult.CONTINUE;
            }
        });
    }
}
