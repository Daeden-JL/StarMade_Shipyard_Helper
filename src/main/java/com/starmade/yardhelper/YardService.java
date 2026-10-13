package com.starmade.yardhelper;

import com.starmade.yardhelper.agent.AgentLoop;
import com.starmade.yardhelper.agent.ContentFilter;
import com.starmade.yardhelper.agent.Guides;
import com.starmade.yardhelper.agent.LlmClient;
import com.starmade.yardhelper.agent.ToolContext;
import com.starmade.yardhelper.agent.ToolRegistry;
import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.session.Session;
import com.starmade.yardhelper.session.SessionManager;
import com.starmade.yardhelper.world.Editor;
import com.starmade.yardhelper.world.Journal;
import com.starmade.yardhelper.world.WorldGateway;
import com.starmade.yardhelper.world.WorldUnavailableException;

import java.io.IOException;
import java.net.http.HttpTimeoutException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Predicate;
import java.util.function.Supplier;

/**
 * The game-independent heart of the mod: takes a player's message, finds or creates their
 * session, and runs the agent on a worker thread. The game adapter (or the offline
 * harness) supplies who is asking, which design, and how to reach it.
 */
public final class YardService {
    /** Sends chat lines to one player. Must be safe to call from any thread. */
    public interface Messenger {
        void send(String text);
    }

    private final YardConfig config;
    private final LlmClient llm;
    private final Supplier<BlockCatalog> catalog;
    private final Guides guides = Guides.load();
    private final ToolRegistry tools = ToolRegistry.standard();
    private final SessionManager sessions = new SessionManager();
    private final ContentFilter filter;
    private final AgentLoop loop;
    private final Semaphore modelSlots;
    private final AtomicInteger waiting = new AtomicInteger();
    private final ExecutorService workers = Executors.newCachedThreadPool(r -> {
        Thread t = new Thread(r, "ShipyardHelper-agent");
        t.setDaemon(true);
        return t;
    });

    public YardService(YardConfig config, LlmClient llm, Supplier<BlockCatalog> catalog) {
        this.config = config;
        this.llm = llm;
        this.catalog = catalog;
        this.filter = new ContentFilter(llm, config.filterEnabled);
        String prompt = Guides.resource("prompts/system.md");
        this.loop = new AgentLoop(llm, tools, prompt == null ? "You help players build ships." : prompt,
                config.agentMaxSteps, config.agentContextChars,
                new LlmClient.Options(config.llmMaxTokens, config.llmReasoningEffort), config.agentTrace);
        this.modelSlots = new Semaphore(config.llmMaxConcurrent, true);
    }

    public SessionManager sessions() {
        return sessions;
    }

    /**
     * Handles a player's request. Returns immediately; replies arrive through the messenger.
     *
     * @param designId      stable identity of the design the player is editing
     * @param gateway       creates access to that design, used when a new session starts
     * @param holderPresent whether a given other player is still inside this design
     */
    public void submit(String player, String designId, String text, Supplier<WorldGateway> gateway,
                       Predicate<String> holderPresent, Messenger out) {
        SessionManager.Acquired got = sessions.acquire(player, designId, holderPresent, id -> new Session(player, id,
                new Editor(gateway.get(), catalog.get(), new Journal(config.journalMaxSteps), config.agentMaxBlocksPerOperation)));
        if (got.session() == null) {
            out.send(got.heldBy() + " is working on this design with the assistant. Only one player per design at a time.");
            return;
        }
        Session session = got.session();
        if (!session.busy.compareAndSet(false, true)) {
            out.send("Still working on your last request. Use /yard stop to cancel it.");
            return;
        }
        session.cancel.set(false);
        session.touch();
        workers.execute(() -> runRequest(session, text, out));
    }

    private void runRequest(Session session, String text, Messenger out) {
        boolean haveSlot = false;
        try {
            if (!modelSlots.tryAcquire()) {
                out.send("The assistant is busy with other players; you are number " + waiting.incrementAndGet() + " in line.");
                try {
                    modelSlots.acquire();
                } finally {
                    waiting.decrementAndGet();
                }
            }
            haveSlot = true;
            if (session.cancel.get()) {
                out.send("Stopped.");
                return;
            }
            if (!filter.allows(text)) {
                System.err.println("[Shipyard Helper] Refused request from " + session.player + ": " + text);
                out.send(ContentFilter.REFUSAL);
                return;
            }
            session.editor.journal().begin(text.length() > 60 ? text.substring(0, 60) + "..." : text);
            ToolContext ctx = new ToolContext(session.editor, session.editor.catalog(), guides);
            String reply = loop.run(session.conversation, text, ctx, new AgentLoop.Listener() {
                @Override
                public void note(String note) {
                    out.send(note);
                }

                @Override
                public void tool(String name, String result) {
                    if (config.agentShowToolProgress && isBuildTool(name)) {
                        out.send("[" + name + "] " + firstLine(result));
                    }
                }
            }, session.cancel::get);
            out.send(reply);
        } catch (WorldUnavailableException e) {
            out.send("Paused: " + e.getMessage());
        } catch (HttpTimeoutException e) {
            // The endpoint is up; one step just ran past llm.timeoutSeconds.
            System.err.println("[Shipyard Helper] Model step timed out for " + session.player);
            out.send("The model took too long on one step, so I stopped. What is built so far is in place; "
                    + "say continue, or use /yard undo.");
        } catch (IOException e) {
            System.err.println("[Shipyard Helper] Model request failed: " + e);
            out.send("The assistant's model is not reachable right now. Tell an admin: " + e.getMessage());
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } catch (Throwable t) {
            // Nothing may escape a worker: an uncaught Error here would kill the thread silently.
            System.err.println("[Shipyard Helper] Request failed: " + t);
            t.printStackTrace();
            out.send("Something went wrong with that request. Your design is unchanged beyond what was already reported.");
        } finally {
            session.editor.journal().commit();
            session.touch();
            session.busy.set(false);
            if (haveSlot) modelSlots.release();
        }
    }

    public void stop(String player, Messenger out) {
        Session s = sessions.forPlayer(player);
        if (s == null || !s.busy.get()) {
            out.send("Nothing is running.");
            return;
        }
        s.cancel.set(true);
        out.send("Stopping after the current step.");
    }

    public void undo(String player, Messenger out) {
        Session s = sessions.forPlayer(player);
        if (s == null) {
            out.send("Nothing to undo.");
            return;
        }
        if (!s.busy.compareAndSet(false, true)) {
            out.send("A request is still running. Use /yard stop first.");
            return;
        }
        workers.execute(() -> {
            try {
                String label = s.editor.undo();
                out.send(label == null ? "Nothing to undo." : "Undid: " + label);
            } catch (WorldUnavailableException e) {
                out.send("Cannot undo: " + e.getMessage());
            } catch (Throwable t) {
                System.err.println("[Shipyard Helper] Undo failed: " + t);
                t.printStackTrace();
                out.send("Undo failed.");
            } finally {
                s.touch();
                s.busy.set(false);
            }
        });
    }

    /** Forgets the conversation (the design and its undo history stay). */
    public void reset(String player, Messenger out) {
        Session s = sessions.forPlayer(player);
        if (s != null && s.busy.get()) {
            out.send("A request is still running. Use /yard stop first.");
            return;
        }
        if (s != null) s.conversation.clear();
        out.send("Conversation cleared.");
    }

    public void status(String player, Messenger out) {
        Session s = sessions.forPlayer(player);
        if (s == null) {
            out.send("No active session. Enter a shipyard design in build mode and use /yard <what to build>.");
        } else {
            out.send("Session on design " + s.designId + ": " + (s.busy.get() ? "working" : "idle")
                    + ", " + s.editor.journal().size() + " undo steps, symmetry " + s.editor.symmetryText() + ".");
        }
    }

    public void expireIdleSessions() {
        sessions.expireIdle(TimeUnit.MINUTES.toMillis(config.sessionIdleMinutes));
    }

    public void shutdown() {
        sessions.all().forEach(s -> s.cancel.set(true));
        workers.shutdownNow();
    }

    private static boolean isBuildTool(String name) {
        return switch (name) {
            case "fill", "line", "ellipsoid", "cylinder", "place_blocks", "replace", "remove", "link", "unlink" -> true;
            default -> false;
        };
    }

    private static String firstLine(String s) {
        int nl = s.indexOf('\n');
        String line = nl < 0 ? s : s.substring(0, nl);
        return line.length() > 160 ? line.substring(0, 160) + "..." : line;
    }
}
