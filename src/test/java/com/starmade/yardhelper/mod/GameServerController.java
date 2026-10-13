package com.starmade.yardhelper.mod;

/**
 * Stands in for the game's server loop in tests. The tick pump only accepts ticks that
 * come from a class of this name, which is how it tells the server's thread from a
 * client's in a single-player game.
 */
final class GameServerController {
    private GameServerController() {
    }

    static void tick(TickPump pump) {
        pump.tick();
    }
}
