package com.starmade.yardhelper.agent;

import com.starmade.yardhelper.catalog.BlockCatalog;
import com.starmade.yardhelper.world.Editor;

/** What tools get to work with: one session's design and nothing else. */
public record ToolContext(Editor editor, BlockCatalog catalog, Guides guides) {}
