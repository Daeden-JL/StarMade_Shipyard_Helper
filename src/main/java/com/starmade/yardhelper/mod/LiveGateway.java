package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.world.Block;
import com.starmade.yardhelper.world.DesignWorld;
import com.starmade.yardhelper.world.Vec3;
import com.starmade.yardhelper.world.WorldGateway;
import com.starmade.yardhelper.world.WorldUnavailableException;
import org.schema.game.common.controller.SegmentController;
import org.schema.game.server.data.GameServerState;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.function.Function;

/**
 * Bridges agent worker threads to one design on the server tick thread. Before any work it
 * re-checks that the design is still an editable shipyard design and the player is still
 * online, so a session can never outlive what it was allowed to touch.
 */
public final class LiveGateway implements WorldGateway {
    private static final long CALL_TIMEOUT_SECONDS = 60;
    /** Ticks to wait for the game to apply queued writes before giving up on a batch. */
    private static final int MAX_SETTLE_TICKS = 200;

    private final LiveDesignWorld world;
    private final String playerName;
    private final TickPump pump;

    public LiveGateway(LiveDesignWorld world, String playerName, TickPump pump) {
        this.world = world;
        this.playerName = playerName;
        this.pump = pump;
    }

    @Override
    public <T> T call(Function<DesignWorld, T> operation) {
        CompletableFuture<T> done = new CompletableFuture<>();
        pump.submit(budget -> {
            try {
                check();
                done.complete(operation.apply(world));
            } catch (Throwable t) {
                done.completeExceptionally(t);
            }
            return -1;
        });
        return await(done, CALL_TIMEOUT_SECONDS);
    }

    @Override
    public List<Block> apply(List<BlockWrite> writes, boolean onlyEmpty) {
        CompletableFuture<List<Block>> done = new CompletableFuture<>();
        List<Block> before = new ArrayList<>(writes.size());
        pump.submit(new TickPump.Job() {
            int next;
            int settleTicks;

            @Override
            public int step(int budget) {
                try {
                    check();
                    if (next < writes.size()) {
                        // Let the game apply the previous batch first so it never has more
                        // than one tick's worth of our writes queued.
                        if (world.pendingWrites() > 0) return 1;
                        int end = Math.min(writes.size(), next + budget);
                        int used = end - next;
                        for (; next < end; next++) {
                            BlockWrite w = writes.get(next);
                            Vec3 p = w.pos();
                            Block was = world.get(p.x(), p.y(), p.z());
                            before.add(was);
                            if (onlyEmpty && !was.isAir()) continue;
                            if (!was.equals(w.block())) world.set(p.x(), p.y(), p.z(), w.block());
                        }
                        return used;
                    }
                    // All queued; finish once the game has applied them, so the next read
                    // by the agent sees the result.
                    if (world.pendingWrites() > 0 && settleTicks++ < MAX_SETTLE_TICKS) return 1;
                    done.complete(before);
                } catch (Throwable t) {
                    done.completeExceptionally(t);
                }
                return -1;
            }
        });
        // Generous: large builds are spread over many ticks.
        return await(done, CALL_TIMEOUT_SECONDS + writes.size() / 200);
    }

    /** Tick thread. Throws when the session must not touch the design any more. */
    private void check() {
        SegmentController entity = world.entity();
        if (!entity.isVirtualBlueprint()) {
            throw new WorldUnavailableException("the design was turned into a real ship or unloaded.");
        }
        GameServerState state = GameServerState.instance;
        if (state == null || state.getSegmentControllersByName().get(entity.getUniqueIdentifier()) != entity) {
            throw new WorldUnavailableException("the design is no longer loaded in the shipyard.");
        }
        if (!state.getPlayerStatesByName().containsKey(playerName)) {
            throw new WorldUnavailableException("you left the server.");
        }
    }

    private static <T> T await(CompletableFuture<T> done, long timeoutSeconds) {
        try {
            return done.get(timeoutSeconds, TimeUnit.SECONDS);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException re) throw re;
            if (cause instanceof Error err) throw err;
            throw new IllegalStateException(cause);
        } catch (TimeoutException e) {
            throw new WorldUnavailableException("the server did not get to the edit in time (is it lagging?).");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new WorldUnavailableException("the request was interrupted.");
        }
    }
}
