package com.starmade.yardhelper.world;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.List;

/**
 * Undo history. Everything the agent does for one player request is one step, so a single
 * undo takes back the whole request.
 */
public final class Journal {
    public record BlockChange(Vec3 pos, Block before, Block after) {}

    public record LinkChange(Vec3 controller, Vec3 target, boolean added) {}

    public static final class Step {
        public final String label;
        public final List<BlockChange> blocks = new ArrayList<>();
        public final List<LinkChange> links = new ArrayList<>();

        Step(String label) {
            this.label = label;
        }

        public boolean isEmpty() {
            return blocks.isEmpty() && links.isEmpty();
        }
    }

    private final Deque<Step> steps = new ArrayDeque<>();
    private final int maxSteps;
    private Step current;

    public Journal(int maxSteps) {
        this.maxSteps = maxSteps;
    }

    public void begin(String label) {
        commit();
        current = new Step(label);
    }

    /** Closes the open step, keeping it only when something changed. */
    public void commit() {
        if (current != null && !current.isEmpty()) {
            steps.push(current);
            while (steps.size() > maxSteps) steps.removeLast();
        }
        current = null;
    }

    public void record(BlockChange change) {
        if (current != null) current.blocks.add(change);
    }

    public void record(LinkChange change) {
        if (current != null) current.links.add(change);
    }

    /** Removes and returns the most recent step, or null when there is nothing to undo. */
    public Step pop() {
        commit();
        return steps.poll();
    }

    public int size() {
        return steps.size();
    }
}
