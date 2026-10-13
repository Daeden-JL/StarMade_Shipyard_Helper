package com.starmade.yardhelper.sync;

/** Which blueprint names take part in the exchange with the shipyard. */
public final class Names {
    /** Longest name the shipyard accepts from a game. */
    public static final int MAX_LENGTH = 128;

    private Names() {
    }

    /**
     * The game's own rule for ship names (EntityRequest.isShipNameValid): letters, digits,
     * '_' and '-', spaces after the first character, no space at the end. A dot, which
     * the game strips from the name but not from the folder, is not allowed.
     */
    public static boolean valid(String name) {
        if (name == null || name.isEmpty() || name.length() > MAX_LENGTH) return false;
        if (name.charAt(name.length() - 1) == ' ') return false;
        for (int i = 0; i < name.length(); i++) {
            char c = name.charAt(i);
            boolean plain = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' || c == '-';
            if (!plain && !(c == ' ' && i > 0)) return false;
        }
        return true;
    }
}
