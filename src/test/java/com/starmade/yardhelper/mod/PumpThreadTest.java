package com.starmade.yardhelper.mod;

import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PumpThreadTest {
    /** A server loop ticking on its own thread until told to stop. */
    private static Thread serverLoop(TickPump pump, AtomicBoolean stop) {
        Thread t = new Thread(() -> {
            while (!stop.get()) {
                GameServerController.tick(pump);
                try {
                    Thread.sleep(5);
                } catch (InterruptedException e) {
                    return;
                }
            }
        }, "test-server-loop");
        t.start();
        return t;
    }

    @Test
    void workRunsOnTheServerThreadAndItsResultComesBack() throws Exception {
        TickPump pump = new TickPump(100);
        PumpThread thread = new PumpThread(pump);
        assertFalse(thread.running(), "not until the server loop has ticked");
        AtomicBoolean stop = new AtomicBoolean();
        Thread loop = serverLoop(pump, stop);
        try {
            AtomicReference<Thread> ranOn = new AtomicReference<>();
            String result = thread.call(() -> {
                ranOn.set(Thread.currentThread());
                return "done";
            }, 5_000);
            assertEquals("done", result);
            assertSame(loop, ranOn.get());
            assertTrue(thread.running());
            // What the work throws reaches the caller as it was thrown.
            IOException thrown = assertThrows(IOException.class, () -> thread.call(() -> {
                throw new IOException("no good");
            }, 5_000));
            assertEquals("no good", thrown.getMessage());
            // And the loop is still ticking after it.
            assertEquals(7, thread.call(() -> 7, 5_000));
        } finally {
            stop.set(true);
            loop.join();
        }
    }

    @Test
    void workTheServerNeverGotToIsCalledOffForGood() throws Exception {
        TickPump pump = new TickPump(100);
        PumpThread thread = new PumpThread(pump);
        AtomicBoolean ran = new AtomicBoolean();
        assertThrows(TimeoutException.class, () -> thread.call(() -> {
            ran.set(true);
            return null;
        }, 50));
        // The server loop wakes up later: the abandoned work must not run now.
        AtomicBoolean stop = new AtomicBoolean();
        Thread loop = serverLoop(pump, stop);
        try {
            assertEquals("later", thread.call(() -> "later", 5_000));
            assertFalse(ran.get());
        } finally {
            stop.set(true);
            loop.join();
        }
    }

    @Test
    void stoppingTheCallerCallsQueuedWorkOff() throws Exception {
        TickPump pump = new TickPump(100);
        PumpThread thread = new PumpThread(pump);
        AtomicBoolean ran = new AtomicBoolean();
        AtomicReference<Throwable> outcome = new AtomicReference<>();
        Thread caller = new Thread(() -> {
            try {
                thread.call(() -> {
                    ran.set(true);
                    return null;
                }, 60_000);
            } catch (Throwable t) {
                outcome.set(t);
            }
        });
        caller.start();
        Thread.sleep(100);
        caller.interrupt();
        caller.join(5_000);
        assertTrue(outcome.get() instanceof InterruptedException, String.valueOf(outcome.get()));
        AtomicBoolean stop = new AtomicBoolean();
        Thread loop = serverLoop(pump, stop);
        try {
            thread.call(() -> null, 5_000);
            assertFalse(ran.get());
        } finally {
            stop.set(true);
            loop.join();
        }
    }
}
