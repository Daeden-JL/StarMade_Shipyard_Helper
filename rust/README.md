# Shipyard (web)

A web page for StarMade designs: start a ship or upload a blueprint, look at it in 3D, edit it, and download it as a `.sment` the game imports. It also tries out a design the way the game runs it, makes ships from 3D model files, and has a chat with a model that builds for you. Same stack as a typical axum + Leptos project: one cargo workspace.

| Crate | Job |
|---|---|
| `yard-core` | Blocks, entities, blueprint files (`.sment`, `.smd3`, header, links), block catalog, block shapes, edit operations with undo, picking, mesher. Builds natively and for wasm32, so server and browser run the same code. |
| `yard-server` | axum: stores designs, applies edit steps to the copy that is saved, serves the catalog with block colours and the built client, and keeps designs in step with a game server's blueprint catalog. |
| `yard-import` | Reads 3D model files (OBJ, STL, PLY, glTF, FBX, Collada, 3MF, 3DS, OFF, AMF, VRML, MagicaVoxel, and zips of them) and cuts them into coloured blocks. No other crate is needed. |
| `yard-agent` | The chat agent: Claude and OpenAI-compatible providers (LM Studio), the tool loop, and the build, link and inspect tools. |
| `yard-ui` | Leptos client built with Trunk: design list, WebGL2 viewport, build panel, import dialog, assistant. |

`tools/` holds two Java programs that run the game's own classes without starting the game:

| Tool | Job |
|---|---|
| `tools/game-verify` | Loads a blueprint the way the game does and reports what it read. `compare.sh` checks that the Rust reader agrees with it, block for block and link for link. |
| `tools/shape-dump` | Exports the geometry and texture layout of every block shape and rotation from the game; `gen_rust.py` turns it into `yard-core/src/shape_data.rs`. |
| `tools/link-rules` | Asks the game's own classes which block types may be linked, and writes the test data the linking rules are checked against. |

## Status

Phases 1 to 4 of 5 are done: reading and viewing blueprints, editing them by hand, linking, and the chat agent. Docked entities, logic and 3D import are built too; see below for what is checked and what is not.

- **Reading.** Game-written blueprints with docked entities and links. Blueprints saved by older game versions (header versions 2 to 4, block data versions 2 to 5, the old link file layout) are converted on upload exactly as the game converts them when it loads them. Checked against the game's own reader on every stock ship and station of 0.304.7: same blocks, same links.
- **Writing.** Files that were not edited come out byte for byte. Edited, new and converted designs were loaded back with the game's import and reading code, and it read exactly the blocks and links the editor held.
- **Editing.** Place, remove, paint (keeps each block's shape, changes its material) and pick, with a brush of up to 32 blocks a side, mirror planes on all three axes, and undo and redo. Wedges, corners, tetras, heptas and slabs are drawn and placed in their real shapes and rotations; mirrored copies are turned to match. Lights, doors and storage are placed switched on, as in the game.
- **Linking.** A fourth tool, and the game's C and V keys: select a block to see what it controls and what controls it, link and unlink single blocks or whole groups. Which pairs may be linked, and what else a link does (a module has one computer, a weapon computer one second weapon, one effect and one light, the ship core takes back a computer that is let go), is the game's own rule set, checked against the game for every pair of block types. Blocks are linked as they are placed, as in the game: computers to the ship core, anything else to the selected block where the two go together.
- **Docked entities.** A whole-design view shows every docked entity in its place, and sets turrets, rotators and rail riders in motion to show how the design moves. Moving parts stop where the game would stop them: they are tested against the other parts of the design with the blocks' real shapes and the game's own margins. Play can loop or run through once.
- **Logic and rails.** The Use tool (key 5, or R over a block) and a list of the design's switches activate buttons, activation modules, gates, doors and lights by the game's rules, and docked riders travel their rails and rotators turn, all from `yard-core/src/sim.rs`, which follows the game's own code. Nothing it does is saved; "Put back as saved" undoes it.
- **3D models.** Upload a model file and choose its size (up to 8192 blocks on the longest side, results up to 40 million blocks), solid or hollow (a hollow shell 1 to 16 blocks thick: the inside cells within that many blocks of the surface, counting a step along an edge or corner as one, are kept too; a thicker wall holds together where the surface slants and gives smoothing room to work), which way is up and which way the nose points. It is cut into blocks and each block takes the nearest basic hull colour. Smoothing is not part of the import any more: use the Smooth tool's "Smooth whole entity" afterwards (the server's import still takes `smooth=true&step=n` for scripts). See `yard-import`.
- **Assistant.** A chat panel with a model picker (local models from LM Studio, and Claude models when the server has a key). The model builds in the open design with tools, and the edits appear as it works. See below.
- **Looks.** Blocks are drawn with the game's own textures, read from the install the server points at, laid out per side and turned with the block as the game does it. Rails, dockers and the other blocks that turn 24 ways show their arrow side where the game shows it. Light bars, light rods, consoles, pipes, ladders, grates, buttons and capsules are drawn as the game's own 3D models, also read from the install.
- **Saving.** Every edit goes to the server at once and is written to disk about a second after the editing pauses. Two browser tabs on the same design cannot overwrite each other: the one that is behind reloads.
- **Game server sync.** Designs and the blueprints in a game server's catalog follow each other, and a design can be linked to a design held in a game shipyard; see below.

Not built yet: docking and undocking existing entities by hand (new turrets can be cut out), editing in the whole-design view, build templates, accounts, renaming designs by hand.

Known limits:

- Nobody has looked at a design through a game client yet. What was checked: the game's own import and file-reading code, run headless, and one design made here that a running test server listed in its catalog and spawned from its console.
- Textures on the slanted faces of wedges and the like are approximate. Models are lit like blocks; the glow maps of consoles and buttons are not used.
- Docked entities are listed by their folders, so a leftover `ATTACHED_n` folder the game would ignore still shows up in the entity list. The whole-design view goes by the parent's dock list, as the game does, and leaves such a folder out.
- Blueprints from before 2017 (`.smd2`, header versions 0 and 1) are refused.
- A design stays in server memory while it is being edited and for 30 minutes after.

## Editing

The controls follow the game's build mode where a browser allows it.

There are seven tools. The chosen one stays chosen until you pick another; nothing else switches it. The page starts on Pick, which changes nothing in the design.

| Tool | Key | Left click | Right click |
|---|---|---|---|
| Pick | 1 | Makes the block under the pointer the selected block, with its rotation, and picks that block (outlined in yellow): the rotation arrows (or , and .) then turn it in the design, and choosing another block (in the list or the shape buttons) puts that one in its place. The panel's "Turn all of this type" then turns every block of the picked block's type in the entity on screen to face the way the picked one does, in one step that Undo takes back. Escape lets go | Nothing |
| Paint | 2 | Gives blocks the selected block's material; each keeps its shape. Four modes, chosen in the panel: a single block, a brush (a box as big as the brush size, dug in from the face you click; it paints the blocks in the box with an open face, or all of them with "Inside blocks too" ticked; with "Armor and lights only" ticked it leaves every block that is not armour, glass or a light as it is) and a fill can (the touching blocks of the clicked block's material over the whole surface it reaches; with "Use brush size" ticked, as many blocks deep below the surface as the brush size along the face clicked; with "All touching" ticked, every touching block of the material all the way through; up to 200,000, with the mirror planes honoured; the hover text says how many it would take), and swap (click a block under "Blocks in this entity" on the left to choose the material to swap out, then the new block in the block list: every block of that material in the entity on screen changes, each keeping its shape and turn, in one step that Undo takes back; with "Whole design" ticked the swap goes through the ship and everything docked to it, one step per entity, so Undo takes back the entity on screen's part and the others are undone from their own view) | Nothing |
| Place / Remove | 3 | Places the selected block, as in the game. The panel's "Remove block type" removes every block of the type marked in "Blocks in this entity" on the left from the entity on screen, in one step that Undo takes back (never the core) | Removes, as in the game |
| Link | 4 | Selects the block: its links are shown. The panel's "Show unlinked" marks in orange the blocks of the entity on screen that should be linked and are not: modules that only one kind of computer can run (cannon barrels, missile tubes, beam, salvage, astrotech, push pulse and tractor modules, effect modules, gate, transporter and shipyard modules) with no computer, and computers that run none of their modules; the marks follow every edit until "Hide unlinked" | Links the block to the selected one, or unlinks it; with Shift, its whole group |
| Use | 5 | Presses, flips or opens the block as the game does; nothing in the design changes | Nothing |
| Smooth | 6 | Bevels the outer edges of steps inside the brush with wedges (and corner pieces where two edges meet, tetras where three do, heptas at inside corners), for steps up to the chosen height (1 to 4 blocks). "Smooth whole entity" in the panel does one pass over every block of the entity on screen, as one step that Undo takes back; press it again after raising the step height. "Smooth the inside too" (on by default) also smooths rooms sealed inside the hull and fills the notches on the inner face of a one-block hull with wedges. "Sharpen edges" (off by default) carries a slope on through the whole block its edge ends against: where a row of wedges (made by the pass, or placed by hand) ends at a block that is not a step, that block is cut the same way, and so on along the edge while both sides of the edge face open air, so a chamfer runs to the end of the edge instead of stopping at a square block; where three slopes meet at a tip, the tip becomes a tetra. Off, a slope ends wherever the next block has nothing to step down to. A pass leaves blocks that already have a shape alone; "Smooth again from scratch" turns every wedge, corner, hepta and tetra of hull blocks back into a whole block and smooths once, as one step (shapes placed by hand in hull blocks are redone too; notch wedges stay, since one may be what seals the hull). Pressing it a second time can tidy a few hundred more blocks on a big ship, where the first pass's notch wedges change what the next one sees | Turns wedges, corners, tetras and heptas in the brush back into whole blocks |
| Move core | 8 | Choosing the tool starts choosing the core's new place and the way it faces (see below): click a block, or Shift-click the space in front of it, and choose where the nose and top point; on a turret the dock goes with the core, and Preview shows it turning about the new line; Enter confirms, Escape cancels, and choosing another tool calls it off | Nothing |
| Make turret | 7 | Click two blocks to mark opposite corners of a part, then move any corner or the rail docker with W A S D E Q; with "Make turret" in the panel (or Enter) it is cut out and docked back on a turret axis as its own entity (see below) | Leaves the blocks of the box in the brush on the ship (red), or, on a red block, takes them back in; with Shift, the whole section of touching blocks |

| Action | How | In the game |
|---|---|---|
| Pick a block with any tool | Middle click | Middle click in advanced build mode |
| Swap place and remove | Checkbox under the tools | Setting "Switch build/remove mouse buttons" |
| Move the view | W A S D, E up, Q down, Shift to go faster | The same keys |
| Mouse look | With "Mouse look on double-click" ticked (under the tools, off by default, remembered by the browser), double-click the view: the mouse turns the camera where it stands, a cross marks the middle, clicks use the tool on the block under it, and Esc lets the mouse go. While ticked, a click waits a quarter second to see whether a second follows | Mouse look in build mode |
| Rotate the block to place | Period, comma to go back, or the arrows in the panel | Period, or the wheel in advanced build mode |
| Select a block with any tool | C over it; again, or Esc, lets go | C |
| Link or unlink with any tool | V over a block; Shift+V for its whole group | V; Left Shift+V |
| Undo, redo | Ctrl+Z, Ctrl+Y (Cmd on a Mac; Ctrl+Shift+Z also redoes) | Ctrl+Z, Ctrl+Y |
| Rotate, pan, zoom the view | Drag, right-drag or shift-drag, scroll | Mouse look; no equivalent |

Where it differs, and why:

- The game acts when a button goes down. Here a click acts when the button comes up without the pointer having moved, so that dragging still turns and pans the view. While the right button is held with the Place / Remove tool, the outline shows what letting go will remove.
- The wheel zooms. In the game it scrolls the hotbar, and Shift+wheel zooms.
- The number keys choose tools. In the game they choose hotbar slots; there is no hotbar here yet.
- The view moves 5 blocks a second as in the game only up close. From far away it moves faster, in proportion to the distance.

Both mouse buttons use the brush size and the mirror planes, as in the game. Those two are set in a small panel floating over the top right of the view, which folds away to one line saying the brush size and how many mirror planes are on (the browser remembers whether it is open). The size has a slider per axis, one (All) that sets the three together, and "Reset size" for a single block. While the size or shape is changed, the brush shows in the view for a moment, where the pointer last was over the design (or in the middle of the view), with its size and shape named at the top. The brush's shape is a cube, a cylinder or a sphere, filling the box the size sets; a cylinder stands along the direction you click into. Placing, removing, the paint brush and smoothing use the shape; the Make turret tool's brush is always a box. Slabs and blocks with a front turn to face away from the surface you click unless you choose a direction. A mirror plane's position is in blocks; a value ending in .5 puts it between two rows.

### Linking

Select a block with the Link tool or with C. Its links are drawn through the hull, in the game's colours: the selected block orange, what it controls purple, a weapon computer's second weapon blue, its effect green, its light yellow-green. What controls the selected block is pink, with a line to it; the game does not show that. The panel lists both, and each controller in the list can be clicked to select it in turn.

Right-click or V on another block links it to the selected one, or unlinks it if it is linked. With Shift the whole group of touching blocks of that type goes the way the clicked block goes (rail track only as far as it points the same way). The status line says what was linked and what gave way.

The rules are the game's (`yard-core/src/links.rs`, sources in `tools/link-rules/README.md`):

- Whether a pair may be linked depends on both blocks and on whether the entity is a ship or a station. A pair the game refuses is refused here, by the browser and again by the server.
- A block that is not a logic block, an inventory, rail track, a sensor input or a display has one controller. Linked from another, it leaves the first. The game treats a weapon module found under two computers as cheating and unlinks it when the ship is loaded.
- A weapon computer has one place each for a second weapon, an effect computer and a light. Filling a place empties it first. On a ship a computer that is let go answers to the core again, and one that is taken leaves the core.
- A block placed while another is selected is linked to it if the game would link the two. Computers and the other blocks the ship core controls are linked to the core instead, which is what puts a weapon on the pilot's bar.

Where it differs from the game: Shift takes the whole touching group for every pair, where the game, for a few pairs, takes only the clicked block and its six neighbours. Links made in the game stay as they are even where they break a rule; only new links are checked.

### The whole design, and docked parts in motion

A design with docked entities has a "Whole design" entry above its entity list. It shows every entity where the design docks it: turrets on their axes, wings and doors on their rails, and what is docked to those in turn. It is for looking; blocks and links are edited in the entities themselves, which the panel's Edit buttons open.

The panel lists the docked parts with how each can move, and a slider for each that can:

| Docked to | Moves | Slider |
|---|---|---|
| Rail Turret Axis | Turns freely about the line through the axis block and the docker. A docker that faces front or back on its own entity makes a turret that never turns, as in the game | -180 to 180 degrees |
| Rail Rotator (either kind) | Quarter turns about its arrow side, clockwise or counter-clockwise by kind | 0 to 360 degrees |
| Rail, Pickup Rail, Rail Load and the like | Carried from block to block along each block's arrow, round corners too, keeping the way it faces | Blocks back and ahead |

Play sets everything going at once: turrets sweep from side to side, rotators go round a quarter turn at a time, rail riders travel their track and back. "As saved" puts every part back where the blueprint has it. Nothing here is saved; it is a way to see how a design moves.

Placement is the game's own (`RailRelation.getBlockTransform`, ported in `yard-core/src/dock.rs`): the docker rests in the cell in front of the rail block's arrow side, arrow side to arrow side, arrows pointing the same way, then the turn a turret axis has given the part and the distance a rail has carried it, both of which the blueprint records. The rail and docker are taken from the dock record, as the game takes them, not from the blocks now in those cells. The turn for each of the 576 pairs of rail and docker orientations is tested against a table taken from the game's classes. The motion was read off what real blueprints record (`cargo run -p yard-core --example turns -- <folder of .sment files>`, over the 1,107 docks in a game server's 210 blueprints): the turn kept for a turret axis is about the docker's arrow-side axis in 191 of the 201 that have a measurable axis, the turn kept for a rotator is about the rotator's own in all 120, and most of what a rail has carried sits a whole number of blocks along the rail's arrow.

Limits: a part is shown passing through whatever is in its way, where the game would stop it. A part carried onto a rotator is not turned by it. Rotators always turn a quarter here; in the game, linked activation modules can set other angles. Docks of the kind the game had before rails are not placed.

## The assistant

`yard-agent` talks to a model and gives it tools: inspect (`design_info`, `find_blocks`, `scan_region`, `list_links`, guides), inside the hull (`inside_space`, `fill_inside`, `find_outlines`, `find_groups`), build (`fill`, `line`, `ellipsoid`, `cylinder`, `hull`, `place_blocks`, `replace`, `remove`, `mirror`, `copy_region`) and systems (`place_system`, `link`, `unlink`, `undo`). Every edit is one undoable step under the game's linking rules, applied to the design the server holds, and the browser follows it live. The server keeps one conversation per design and runs the model in the background; the browser asks `GET /api/designs/{id}/chat?from=N` for what has happened.

The model's key stays on the server. Set `YARD_ANTHROPIC_KEY` (or `ANTHROPIC_API_KEY`) for Claude, and `YARD_LOCAL_MODEL_URL` (default `http://127.0.0.1:1234/v1`) for LM Studio or any OpenAI-compatible server; in production each server admin supplies their own key. Without a key the picker lists the local models and says why Claude is missing.

The tools tell the inside of a hull from the outside (`yard-core/src/interior.rs`): empty cells are outside if they connect through faces to the space beyond the entity's bounds, and the rest are enclosed. A shell with openings (thruster ports, an open hangar) would let the outside in, so the tools try counting gaps up to 2, 4, 6 and 8 blocks wide as closed and take the narrowest that encloses much more room; a wider one would also close the hollows of the outer surface. `inside_space` reports the enclosed room section by section, `fill_inside` fills it (optionally a set number of blocks packed against one side), `only_where: "inside"` keeps `fill`, `line`, `cylinder`, `ellipsoid` and `hull` to it, `get_block` says whether an empty cell is inside or outside, and `scan_region` draws enclosed empty cells as `:`. `find_outlines` finds shapes a player drew in one block on a side of the ship (circles on the stern marking engines): it looks through the outer layers on that side for the one with the most closed rings of that block, and gives each ring's box behind it; with `fill_with` and `blocks_each` it fills that many blocks inside the hull behind every ring in one step. `find_groups` lists the connected groups of a block anywhere. On a 1.65 million block ship (417 x 110 x 1056) finding the inside takes about 50 ms per seal width.

Checked: the loop, the tools and both wire formats against scripted and mock servers, and one real run on a local model (`google/gemma-4-26b-a4b-qat`) that built a hull in the open design. Not checked: the Claude path against the real API (no key was available), and how well any model builds; the prompt is untuned.

## Game server sync

The designs here and the blueprints in a game server's catalog follow each other:

- A blueprint saved in the game shows up as a design.
- A design made or edited here shows up in the game's catalog, about 20 seconds after the last edit. "Sync now", under the sync box of a design that is waiting, sends it at the game's next call instead (`POST /api/designs/<id>/sync/now`).
- Whichever side changed gives its version to the other. If both changed, the game's version wins the name and the edits made here are kept as a separate design called `<name> shipyard copy`.
- A blueprint deleted in the game is not sent back. Its design stays here, marked "Deleted in the game"; ticking its "Sync with the game server" box sends it to the game afresh. If the blueprint turns up in the game again by itself, the two carry on as before.
- Unticking that box keeps a design out of the exchange in both directions.
- Nothing is ever deleted in the game from here.

The game only looks at its blueprint folder when it starts, and only then gives new folders a catalog entry. So the game's side is handled by the mod in this repository (`../src`, package `sync`), which runs inside the game server: it lists the blueprint folder, sends and fetches `.sment` files over HTTP, and tells the game about each change through the game's own catalog calls. The mod only needs to reach this server's address; the two can be on different machines. The mod's settings are described in the mod's README.

How it works:

- Each side's copy is known by a fingerprint: SHA-256 over the blueprint folder's files in path order, each as path, a zero byte, the length as eight big-endian bytes, and the contents.
- This server remembers, per design, the pair of fingerprints that last belonged together. A side whose copy no longer has its half has changed. The agent keeps no state.
- Every few seconds the agent posts the game's list to `/api/sync/report` and gets back what to upload (`PUT /api/sync/blueprints`) and what to download (`GET /api/sync/designs/<id>/file`, then `POST /api/sync/done`).
- A blueprint the game is in the middle of saving is reported as busy and left alone. A blueprint missing from a report counts as deleted, so the agent sends no report at all if it cannot read the folder.

### Designs in game shipyards

A design can also be linked to a design held in a shipyard in the game: "Link to a shipyard design..." under the sync box lists the shipyards the game reports, with the designs each holds. "Send this one" makes this design replace the shipyard's; "Take the shipyard's" brings the shipyard's design here (what this design held is kept as `<name> shipyard copy` if it differs); "New design in this shipyard" makes one in a free shipyard. From then on the two follow each other, the same way a design and its blueprint do.

- The game only works on shipyards of stations it has loaded, which is while a player is near them, and only on a design that is loaded in its shipyard. A link waits until then and says so under the link.
- A change in the shipyard is picked up once the design has been left alone for 10 seconds and every docked part is in place (the game writes what it holds in memory before the copy is taken, so parts of a large design not in memory are complete in the copy; the game's own "fully loaded" flag is not waited for, because for a large design read from disk it can stay off until the design is unloaded and loaded again). The mod copies it as a blueprint and uploads it; copies are compared by their blocks, facings, links and logic, not by their files, because the game writes the time into every copy.
- A change here goes to the game's catalog first (at once, without the usual wait), and the mod then has the shipyard unload the design, make a new one from the catalog's blueprint under the same name, and remove the old one. That needs one free slot in the shipyard's inventory. The mod does not swap out a design a player is aboard, or one that changed in the shipyard since it was last read: that version comes here first, and the edits made here are kept as a copy.
- The game unloads a design that does not fit its shipyard; it then waits here for a design that fits.
- Unlinking leaves both sides as they are. Switching a design's sync off pauses its link too.
- Which shipyards may be used is the mod's `sync.shipyards` setting: by default only those on stations of the sync owner's faction, since the game itself checks no permissions for shipyard orders given by the server.
- Protocol: reports carry `yards` (each shipyard with its state, designs, loaded design and, for watched ones, the fingerprint of a fresh copy); the answer's `yards` says which designs to watch, which to upload (`PUT /api/sync/yards/take`) and which blueprints to put into a shipyard (confirmed with `POST /api/sync/yards/given`, failures reported to `POST /api/sync/yards/trouble`). The page uses `GET /api/sync/yards` and `GET|POST|DELETE /api/designs/<id>/yard`.

### Sync history

Each time a design and its blueprint in the game come to hold the same thing, the design's file at that moment is kept: the last 20 such versions per design. The open design's panel lists them under "Sync history". Clicking one puts that version on screen to look at, with a bar over the view to go back, restore it, save it as a design of its own, or download it; nothing can be edited while an old version is shown. A version records when it was synced, which way it went (from the game, to the game, or matched when both sides already had it), the design's name, block and entity counts, file size and fingerprint.

- `GET /api/designs/<id>/history` lists the versions, newest first. `current` is set on the one the design is at now; if none is, the design has edits the game has not seen.
- `GET /api/designs/<id>/history/<seq>/file` is that version as a `.sment`.
- `POST /api/designs/<id>/history/<seq>/restore` makes that version the design's contents again. The design keeps its present name, open editors load it again, and the exchange then carries it to the game like any other edit. If the design held edits the game never saw, those are kept as a version first, marked unsynced, so putting a version back never loses work.
- `POST /api/designs/<id>/history/<seq>/copy` stores that version as a new design called `<name> v<seq>`, with syncing switched off.

The files are in `<data>/history/<design id>/`: `<seq>.sment` per version and an `index.json`. A version is a hard link of the stored design where the file system allows, so it takes no room of its own while it is still the current one; the store never writes into a design file, it puts a new one in its place. Designs that were already in step with the game when the history was introduced get their current file as their first version when the server starts.

Names follow the game's rule: letters, digits, spaces, `-` and `_`, no space at either end, at most 48 characters for names made here. No two designs share a name, because the game's catalog has one blueprint per name. Game blueprints whose folder name breaks the rule (a dot, most often) are left out; the game itself mishandles those.

Set `YARD_SYNC_TOKEN` here and the same value in the mod to make the sync endpoints refuse anyone else. Without accounts, anyone who can open this site can still edit every design, and through the sync every blueprint in the game.

## Run

Needs a StarMade install to read block definitions and textures from (they are not shipped here).

```
cd rust
(cd crates/yard-ui && trunk build --release)      # client -> rust/web/dist
YARD_STARMADE_DIR=/path/to/StarMade cargo run -p yard-server
```

Then open http://127.0.0.1:4280.

To reach it from other machines, set `YARD_BIND=0.0.0.0:4280`. On a Mac the application firewall drops incoming connections to a freshly built binary and asks again after every rebuild. For development, leave the server on loopback at port 4281 and run `/usr/bin/python3 scripts/lan-relay.py`, which listens on port 4280 and forwards to it; stop the relay to take the shipyard off the network. Use the system Python as written: the firewall's allow list goes by program, and a Python from Homebrew is a different program.

| Variable | Default | |
|---|---|---|
| `YARD_STARMADE_DIR` | `starmade` | Install containing `data/config` (`BlockConfig.xml`, `BlockTypes.properties`, `mainConfig.xml`), `data/textures/block`, `data/models/lod` and `version.txt`. Its block list decides how old blueprints are converted. |
| `YARD_DATA_DIR` | `data` | Where uploaded designs are kept. |
| `YARD_STATIC_DIR` | `web/dist` | The built client. |
| `YARD_BIND` | `127.0.0.1:4280` | Use `0.0.0.0:4280` to serve other machines. |
| `YARD_SYNC_TOKEN` | none | A secret the game server's sync agent has to send. |
| `YARD_SYNC_QUIET_SECS` | `20` | How long a design has to be left alone before it goes to the game. |

There are no accounts yet: anyone who can reach the server can upload, open and edit every design.

## Trying a 3D model

```
cargo run -p yard-import --release --example voxelize -- model.obj 64 solid
```

prints what was read, the size of the result and some slices; add `smooth` after `solid` or `shell` to smooth it. In the page, "Upload blueprint or 3D model" opens the options for anything that is not a `.sment`.

## Test

```
cargo test                 # core and server
cargo test -p yard-ui      # camera math, brush boxes, model looks
```

Tests that need game files or the block config are skipped on a machine without a StarMade install. Set `STARMADE_DIR`, or they look in `~/.smtest2/vol`.

To check a blueprint against the game itself (needs Java 21 and the install):

```
tools/game-verify/compare.sh some-ship.sment     # Rust reader against the game's reader
tools/game-verify/run.sh --summary some-ship.sment
```

`tools/link-rules/run.sh` asks the game which block types may be linked and rewrites `crates/yard-core/tests/data/link_pairs_*.txt`; the `link_rules` test then holds the Rust rules to it, pair by pair.

The expected hashes in `crates/yard-core/tests/game_vectors.rs` and `tools/game-verify/samples/vectors.json` were produced this way. The largest stock stations are only checked on request:

```
cargo test --release -p yard-core --test game_vectors -- --ignored
```

### Smoothing

Smoothing keeps the model's own surface, not the blocks: for every cell the surface passes through, the pieces of triangles in it are gathered, and if they are one flat sheet (or two meeting at an edge) the plane says which of the cell's 4 by 4 by 4 sample points the model fills. The server then looks for the hull block, of the colour the cell already has, whose real geometry (the game's own shape tables) fills the same points: a wedge, corner, hepta or tetra in whichever of its turns fits. It uses one only where it matches the surface closely (at most 10 of the 64 points differ) and clearly better than the whole block, and only on the blocks the plain result already has, the outer layer: nothing is added in the hollows between steps. Each side of the cell the shape leaves empty must face an empty cell, so a wedge trims an outer edge and is never dug in against a wall as a pit. Slabs are never used. So steep slopes and curves become wedges and corners on their outer edges, gentle slopes and flat faces stay clean stairs and cubes, and cells with three sheets (corners), thin walls and sharp bends stay whole blocks, so sharp edges stay square and thin wings stay whole. Where the model's own slope fits no wedge (a gentle slope of long treads), the edge of each step is beveled instead (where two edges of a step meet at an outward corner, the block is cut by both and becomes the game's corner block; where three meet, at the tip of a hull that steps in every way, three cuts through the middle match no block and the nearest in sample points would be the corner piece, whose square base sticks out past the row of wedges below, so the block is the tetra that is flush with all three rows; where a row of wedges ends at a block whose step below recedes, and the block's own bevel would leave the row's edge square so that it sticks out past the row, that edge is cut too, flush with the row (a row cuts an edge where the neighbour's face toward the block is the triangle a wedge along that edge leaves, which a wedge, a corner piece or a tetra sloping that way has and a hepta or a slab does not), and where that makes all three edges round an open corner cut, the block is a tetra; a block whose own cuts are on the face opposite the one its row slopes at (its bottom edges beveled as the lip of an overhang where the blocks along it slope at the top, which looks like a spike) was shaped the wrong way up, and its cuts are mirrored to the row's face first; this flush-cutting goes round again until nothing changes, since a block cut flush can be what the next one along is to be flush with (a pass also re-cuts a row end an earlier pass left square, where the block is exactly the shape the bevel makes there, though it otherwise leaves shaped blocks alone); where they meet at an inward corner, the block that stands up between the two beveled rows loses its corner and becomes a hepta: decided after the edges, from what the two neighbours became, so it is only where both really slope down toward that corner; a block with two such corners stays whole; the same holds upside down under an overhang, and on walls): a block with open air on two adjacent sides, and a lower tread within the chosen step height across one and down the other, becomes the wedge (or corner) that cuts the edge. Edges of plates standing free (a wing has nothing below its edge to step down to), of taller drops and of the flat sides of boxes are left square; the one-block skin of a hollow hull is beveled like any step. A staircase one block thick, whose blocks touch only at their edges, steps down on both faces; only the face with more room in front of it (looking up to 64 blocks out, past which the hull's inside is the other way) has its edges beveled, and with "Smooth the inside too" the notches of the other face are filled with wedges, so both faces slope. That option also counts rooms sealed inside the hull as open air, but only for blocks the open air outside does not reach: a block of a thin hull is always shaped for its outer face, so the inner bevels never take the blocks the outer face needs (the heptas between corner pieces on an overhang, for one). The slider in the dialog sets the step height (0 turns the bevel off), and the Smooth tool does the same to blocks already in a design. On a 4096 long hull it takes about 25 seconds and 5 GB while it runs (results over 20 million blocks are refused for smoothing).

Under the design's name the sidebar gives its size in blocks, wide (X) × high (Y) × long (Z), with every docked entity at rest where it docks: the space a shipyard must hold. Turrets that turn can sweep a little past it. Each entity's card gives its own size. Both follow every edit.

The sidebar's "Blocks in this entity" lists every block type in the entity on screen (in the whole design in the whole-design view), most used first, with a search box that keeps the rows whose name holds every word typed; it follows every edit. Clicking a row marks those blocks through the hull in pink (up to 3,000 of them), and turns the view to them when there are few, which is how to find a buried ship core; click the row again to stop.

### Moving the core

The Move core tool (8) moves the core of the entity on screen to another cell. The game keeps the core in one fixed cell of every ship's file, so this moves the whole design round it: the core and the block at the chosen cell swap places and everything shifts so the core is at the origin again (blocks, links, docked entities' records, rail dockers, cargo and wireless logic, which the blueprint stores by position). Click a block (Shift-click the space in front of it) or use W A S D E Q (or the arrow keys, Page Up and Page Down) to move the marker, which is drawn through the hull in green with the old core in orange. A green arrow from the marker points the way the core's nose will face, which is the way the ship flies, with a short stalk for its top. If it points the wrong way, "Nose points" and "Top points" (any of the 24 turns) turn the design round the core to match, with or without moving it: every block turns with it (wedges, corners, slabs, rails and dockers take the matching orientation), links, rail dockers, cargo and wireless logic follow, docked entities turn with their rails, and a docked entity that is turned stays where it is docked, turned on its docker. An entity docked by its core rather than a docker cannot be turned on its rail and is refused; Enter or "Confirm" does it, Escape cancels. The movement keys follow the way the view looks.

On a turret (an entity that hangs from a turret axis) the dock goes with the core unless "The dock goes with the core" is unticked: the rail docker under the core and the turret axis it rests on (a block of the parent), and every turret axis of this entity that a barrel hangs from, with that barrel's docker, shift by as much as the core, each within its own entity. The turret and its barrels keep their places; only the line the turret turns about moves, and the blocks that were in the way take the cells the moving blocks left. It is refused where that would displace a core or a block another dock rests on (moving the core up into a barrel's docker, say). A turret on a rotator or a plain rail, and the main entity, have no dock to follow: there the core alone moves. A turret whose rail docker faces front or back on it is one the game holds still (it only turns a turret whose docker faces up, down or sideways; `RailRelation.isTurretDock` in the game's code); the panel says so (the parts list calls it "Turret axis, but its rail docker faces front or back on it, so the game holds it still"), the Move core panel warns of it while choosing, and Auto-fit refuses it. "Preview" shows the move before it is made: a copy of the design with the core moved appears in the whole-design view, framed on the turret and with Play running so the turret sweeps about its new line, under a banner with Confirm, "Back to choosing" (Escape) and Cancel. Nothing is saved until Confirm; the design cannot be edited meanwhile, and a change that arrives from the game server during the preview is loaded once it is left. "Auto-fit for the widest sweep" (`yard-core/src/pivot.rs`) fits the turret and every section docked to it down the chain (the base, then its barrels, then what hangs from those), one after another: for each it tries every block of the core's row across the turn axis as the pivot, sweeps the section about each in the rig as Play does until it meets the ship or another part, and takes the cell with the widest sweep (the most even one both ways among equals, then the nearest). A cell is passed over when its move would leave a block floating outside the structure: the turret axis beyond the deck's edge, say, or hanging off the hole it left behind. A block counts as floating when it touches no block of its entity and has open space on some side all the way out, so one adrift inside a hull is let be, as is one that floated before. A base's move takes its barrels' axes along, so each barrel is tried after its base has moved. The moves are previewed together; the message says, section by section, how far each turns at its new cell against where it is now, which stay as they are, and which could not be fitted. Confirm makes them all in one step (one "Put back"); Back to choosing keeps them until a cell is chosen by hand, which keeps only this core's move. Rows of more than 200 cells are tried on a grid first and the cells round the best after.

"Clone" (key 9, `yard-core/src/clone.rs`) copies the docked entity on screen, with everything docked to it: the same blocks about its core, links, rail docker, storage, AI settings and docked entities, under a new name (names must be unique among a design's entities, and wireless links go by them; the copy's dock records are written anew, at rest). It goes either in place of another docked entity, chosen in the panel, which is removed with everything docked to it so the copy hangs from the rail it hung from; or onto a new rail block of the kind the original hangs from (a turret axis for a turret), placed on the entity the original hangs from: "choose the cell" brings that entity on screen with the original's rail marked in orange, and a click makes the block clicked the rail, with the copy resting on the face clicked (Shift-click puts the rail in the space in front; the side can also be set in the panel, and W A S D E Q move the cell). The cell in front of the new rail, where the copy's rail docker rests, must be empty. Preview shows the copy in the whole-design view, framed on it, and the notes say if any of its blocks sit where another part has blocks. Confirm makes it, with "Put back" to undo. `POST /api/designs/{id}/clone-docked` does the same (`source` and either `replace` or `parent`, `at`, `facing`).

A turret axis a barrel hangs from follows the core only where, moved, it would still touch a block of the turret; one that would float free stays where it is with its barrel, and the notice says so. The same tool moves a rail by itself: "Move" in its panel lists the turret axis or rotator the entity on screen rests on and each one a docked entity hangs from (not runs of rail, which are moved block by block). Choose the rail's new cell as for the core (for the rail the entity rests on, the cell of this entity that is to rest on it; the orange box marks the rail, or the docker, as it is). With "What hangs from it comes along" the docked entity moves with its rail, docked as it was; without, it stays where it is and its rail docker moves within it to meet the rail. Preview, Confirm and "Put back as it was" work as for the core. Settings the game stores by position that the editor cannot read (storage contents, display text, other block settings) are reset, and the notice says so. It is refused, with a reason and nothing changed, for a design whose files are older than the game's meta version 4 (9 of the 222 dev designs), with old-style docks, or with wireless logic that names this entity by a name another entity shares. Each step is checked with the game's own reader (`tools/game-verify`), not in a running game. Undo cannot follow a change like this, so "Put back as it was" appears until the next edit.

### Making a turret

The Make turret tool cuts a box of blocks out of the entity on screen and docks it back as a turret: its own entity with its own core, on a turret axis. Click two blocks to mark opposite corners, choose which side of the box rests on the ship, and press "Make turret" (or Enter). The corner in hand (white) moves with W A S D E Q; click any other corner of the box, or press Tab, to take it, and Escape to let go so the keys fly the view again. The rail docker (yellow) starts in the middle of the face that rests on the ship; "Move the rail docker" (or Tab past the corners) takes it, and the keys move it about that face. The axis goes in the cell outside the docker (blue) and replaces what is there; the new core takes the cell beyond the docker (green). The blocks that will go are tinted orange (boxes over 2 million cells show only their outline). Right-click blocks in the box to leave them on the ship, as many as the brush size takes (a ridge of the hull the box takes in by the way): they turn red, and right-clicking a red one takes those in the brush back; Shift+right-click does the same to the whole section the block is part of (every block reached from it through blocks touching at a face, an edge or a corner, as far as air or the edge of the box); "Take them all back in" clears the lot. The docker's cell and the new core's always go with the turret. Links wholly inside what goes with it go along; links that cross its edge are cut. "Put back as it was" undoes it. The turret gets a fresh core and no AI settings (set them in the game). It is refused, with a reason, for a box that holds the ship's core, is less than three cells deep from its base, has no blocks, or holds the block another docked entity hangs from. If the cut takes blocks the game may keep settings for (systems, logic, storage, computers), the parent's per-block settings are reset, as for moving the core.

### Deleting a blueprint

"Rename..." under the design's name gives it another name (letters, digits, spaces, - and _, as the game allows; no two designs share one, whatever the capitals). The name is part of the blueprint's file, so the design is loaded again. The game cannot rename a blueprint, so for a design the game has, the blueprint under the old name stays there as it was and is not taken back in, and the design goes to the game as a new blueprint under the new name; delete the old one in the game if it is not wanted. A design the game has cannot change only its capitals, which the game does not tell apart (`POST /api/designs/{id}/rename`).

"Delete design..." under the design's name opens a confirmation (Cancel is the default; Escape also cancels). Deleting closes the design, removes it from the list and from the game exchange, and moves its two files (and its sync history) into a `trash` folder beside the `designs` folder, named with the time, so nothing is erased and an administrator can put them back by hand. The shipyard never deletes in the game: if the game server has a copy, it stays there, and is not brought back here unless it changes there (`DELETE /api/designs/{id}`).
