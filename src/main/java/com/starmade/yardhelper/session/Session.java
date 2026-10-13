package com.starmade.yardhelper.session;

import com.starmade.yardhelper.agent.Conversation;
import com.starmade.yardhelper.world.Editor;

import java.util.concurrent.atomic.AtomicBoolean;

/** One player's conversation with the agent about one design. */
public final class Session {
    public final String player;
    public final String designId;
    public final Editor editor;
    public final Conversation conversation = new Conversation();
    /** Set while a request is running; a player has one request at a time. */
    public final AtomicBoolean busy = new AtomicBoolean();
    /** Set by /yard stop; the running request ends at its next step. */
    public final AtomicBoolean cancel = new AtomicBoolean();
    public volatile long lastActivityMillis = System.currentTimeMillis();

    public Session(String player, String designId, Editor editor) {
        this.player = player;
        this.designId = designId;
        this.editor = editor;
    }

    public void touch() {
        lastActivityMillis = System.currentTimeMillis();
    }
}
