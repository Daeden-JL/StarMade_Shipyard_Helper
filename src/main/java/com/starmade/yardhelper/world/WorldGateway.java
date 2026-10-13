package com.starmade.yardhelper.world;

import java.util.List;
import java.util.function.Function;

/**
 * Hands work to the thread that owns the design. The agent runs on worker threads; in the
 * game every read and write has to happen on the server tick thread, a bounded amount per
 * tick. Offline the gateway just runs things inline.
 */
public interface WorldGateway {
    /** Runs a short operation against the design and returns its result. */
    <T> T call(Function<DesignWorld, T> operation) throws WorldUnavailableException;

    /**
     * Applies block writes, possibly spread over several ticks, and returns what was there
     * before each write (same order) so the caller can journal it.
     *
     * @param onlyEmpty skip positions that already hold a block
     */
    List<Block> apply(List<BlockWrite> writes, boolean onlyEmpty) throws WorldUnavailableException;

    record BlockWrite(Vec3 pos, Block block) {}
}
