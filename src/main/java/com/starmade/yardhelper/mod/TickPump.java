package com.starmade.yardhelper.mod;

import java.util.ArrayDeque;
import java.util.Deque;

/**
 * Runs queued work on the server tick thread, a bounded amount per tick. Game state may
 * only be touched from that thread, and too much work in one tick stalls the server (which
 * its watchdog treats as a hang).
 */
public final class TickPump {
    /** A unit of work that may span several ticks. */
    public interface Job {
        /**
         * Does some work on the tick thread.
         *
         * @param budget block operations still allowed this tick
         * @return operations used, or -1 when the job is finished
         */
        int step(int budget);
    }

    private final Deque<Job> jobs = new ArrayDeque<>();
    private final int budgetPerTick;
    private volatile Thread serverThread;

    public TickPump(int budgetPerTick) {
        this.budgetPerTick = budgetPerTick;
    }

    public void submit(Job job) {
        synchronized (jobs) {
            jobs.add(job);
        }
    }

    /**
     * Called every tick by a StarRunnable. In a single-player game the client loop ticks
     * runnables too, so the pump binds to the first thread that calls it from the server
     * controller and ignores every other caller.
     */
    public void tick() {
        Thread current = Thread.currentThread();
        if (serverThread == null) {
            if (!calledFromServerLoop()) return;
            serverThread = current;
        } else if (serverThread != current) {
            return;
        }

        int budget = budgetPerTick;
        int pending;
        synchronized (jobs) {
            pending = jobs.size();
        }
        // Round-robin so one player's large build cannot starve another's.
        for (int i = 0; i < pending && budget > 0; i++) {
            Job job;
            synchronized (jobs) {
                job = jobs.poll();
            }
            if (job == null) break;
            int used;
            try {
                used = job.step(budget);
            } catch (Throwable t) {
                // Jobs report their own failures; anything reaching here is a bug. Drop the
                // job rather than let an exception escape into the server loop.
                System.err.println("[Shipyard Helper] Tick job failed: " + t);
                t.printStackTrace();
                continue;
            }
            if (used >= 0) {
                budget -= Math.max(1, used);
                synchronized (jobs) {
                    jobs.add(job);
                }
            }
        }
    }

    /** Whether the server loop has started calling the pump, so queued work will run. */
    public boolean isRunning() {
        return serverThread != null;
    }

    public boolean isServerThread() {
        return Thread.currentThread() == serverThread;
    }

    private static boolean calledFromServerLoop() {
        for (StackTraceElement frame : Thread.currentThread().getStackTrace()) {
            if (frame.getClassName().endsWith("GameServerController")) return true;
        }
        return false;
    }
}
