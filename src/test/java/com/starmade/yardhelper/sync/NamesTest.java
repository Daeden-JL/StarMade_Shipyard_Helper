package com.starmade.yardhelper.sync;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NamesTest {
    @Test
    void followsTheGamesRuleForShipNames() {
        for (String ok : new String[]{"Scout", "Isanth Type-PNR-25-B", "a", "my_ship 2", "-x-", "9 lives"}) {
            assertTrue(Names.valid(ok), ok);
            // The game's own check, EntityRequest.isShipNameValid, written out.
            assertTrue(ok.matches("[a-zA-Z0-9_-]+[ a-zA-Z0-9_-]*") && !ok.endsWith(" "), ok);
        }
        for (String bad : new String[]{"", " lead", "trail ", "With.Dot", "a/b", "a\\b", "..", "caf\u00e9", "tab\there", "semi;colon"}) {
            assertFalse(Names.valid(bad), bad);
        }
        assertFalse(Names.valid(null));
    }

    @Test
    void stopsAtTheLengthTheShipyardTakes() {
        assertTrue(Names.valid("x".repeat(128)));
        assertFalse(Names.valid("x".repeat(129)));
    }
}
