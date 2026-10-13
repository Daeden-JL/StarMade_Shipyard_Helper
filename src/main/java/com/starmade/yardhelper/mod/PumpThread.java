package com.starmade.yardhelper.mod;

import com.starmade.yardhelper.sync.SyncAgent;

import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicInteger;

/** Hands work to the server tick thread through the mod's tick pump and waits for the result. */
final class PumpThread implements SyncAgent.ServerThread {
    private static final int QUEUED = 0, STARTED = 1, GIVEN_UP = 2;

    private final TickPump pump;

    PumpThread(TickPump pump) {
        this.pump = pump;
    }

    @Override
    public boolean running() {
        return pump.isRunning();
    }

    @Override
    public <T> T call(Callable<T> work, long timeoutMillis) throws Exception {
        CompletableFuture<T> result = new CompletableFuture<>();
        AtomicInteger state = new AtomicInteger(QUEUED);
        pump.submit(budget -> {
            // If the caller stopped waiting, the work must not run behind its back.
            if (state.compareAndSet(QUEUED, STARTED)) {
                try {
                    result.complete(work.call());
                } catch (Throwable t) {
                    result.completeExceptionally(t);
                }
            }
            return -1;
        });
        try {
            try {
                return result.get(timeoutMillis, TimeUnit.MILLISECONDS);
            } catch (TimeoutException e) {
                if (state.compareAndSet(QUEUED, GIVEN_UP)) throw new TimeoutException("the server did not get to it in " + timeoutMillis / 1000 + " s");
                // Already under way on the server thread: see it through.
                return result.get();
            } catch (InterruptedException e) {
                // The agent is being stopped. Work still in the queue is called off;
                // work already under way finishes by itself.
                state.compareAndSet(QUEUED, GIVEN_UP);
                throw e;
            }
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof Exception x) throw x;
            throw e;
        }
    }
}
