# game-verify

Loads a StarMade blueprint with the game's own classes (0.304.7) and prints, as one JSON
document, what the game read. Use it to check `.sment` files written by `yard-core`
against the game without starting a server.

```sh
rust/tools/game-verify/run.sh [--summary] [--strict] [--blocks out.tsv] <file.sment | blueprint-dir>
```

`run.sh` works from any directory, compiles `GameVerify.java` on first use (into `build/`)
and runs the JVM inside a throwaway `build/work.*` directory. The input is never modified:
a `.sment` is imported into the work directory, a blueprint directory is copied there first
(the game opens region files read-write and rewrites short ones).

| Environment | Default | Meaning |
|---|---|---|
| `STARMADE_DIR` | `$YARD_STARMADE_DIR`, then `~/.smtest2/vol` | install with `data/` and `lib/` |
| `STARMADE_JAR` | `<repo>/libs/StarMade.jar`, then `$STARMADE_DIR/StarMade.jar` | the game jar |
| `JAVA_HOME` | `/opt/homebrew/opt/openjdk@21` | a JDK 21 |
| `GAMEVERIFY_XMX` | `2g` | JVM heap |
| `GAMEVERIFY_KEEP=1` | off | keep the work directory |
| `GAMEVERIFY_ASSERT=0` | on | run the game code without Java assertions, as the live game does |

Exit status: `0` nothing rejected, `1` at least one entry in `errors`, `2` warnings and
`--strict`. Everything the game prints goes to `build/last-run.log`; stdout is only JSON.

## What reads what

Game code (unmodified classes from `StarMade.jar`):

| Step | Game class and method |
|---|---|
| block config | `ElementKeyMap.initializeData`, `Starter.registerSerializableFactories`, `BlockShapeAlgorithm.initialize` |
| `.sment` import (size check, unzip, layout checks, copy into the catalog) | `BlueprintEntry.importFile(File, BluePrintController)` |
| header, meta, docked children, mod mappings | `BlueprintEntry.read()` (what `BluePrintController.loadBlueprint` calls for each catalog entry), plus `readHeader` and `readMeta(file, true, false)` for the raw counts and the manager tag |
| links | `BlueprintEntry.readStructure` → `ControlElementMapper.deserialize`; positions decoded with `ElementCollection.getPosX/Y/Z` |
| region file index | `IOFileManager.getFromFile` → `SegmentRegionFileNew` / `SegmentHeader` |
| segment data | `SegmentDataIONew.requestStatic` → `RemoteSegment.deserialize` (inflate, old-version migration, `SegmentDataIntArray` / `SegmentDataBitMap` / `SegmentDataSingle`) |
| per block | `SegmentData.getType / getOrientation / isActive / getHitpointsByte / getDataAt` |

Harness code (no format knowledge of its own except where noted):

- Stand-ins for a running server, because a real `Ship` needs a `GameServerState`: a `Ship`
  allocated without its constructor, with only a state proxy (clock), the server flag, a UID
  and an empty-`SegmentData` source set. `RemoteSegment.deserialize` asks for nothing else.
- The copy step of `BluePrintController.loadBluePrint` is transcribed, not called (it needs a
  server state): every `*.smd3` in `DATA` is copied to `<entity uid><source name from its
  first dot>` in `listFiles()` order, later files overwriting earlier ones.
- Counting, sorting, hashing, JSON, and the cross-checks listed under "checks".
- Assertions are enabled for `RemoteSegment` only (`-ea:`), so a segment whose stored
  position disagrees with its index slot is reported. The live game runs without assertions
  and would silently use the slot's position.

Not run: the spawn itself (`SegmentControllerOutline.spawn`), `SegmentData.revalidateData`,
physics, manager containers. A blueprint that passes here was parsed by the game's readers;
it was not placed in a sector.

## Output

Top level: `gameVersion`, `inputKind`, `blueprintName`, `positionSpace`, `hash` (definitions),
`entities`, `ok`, `errors`, `warnings` (entity errors and warnings are repeated at the top,
prefixed with the entity path).

Per entity (root first, then every docked child, depth first):

| Field | Meaning |
|---|---|
| `path`, `parent` | directory relative to the catalog, e.g. `Name/ATTACHED_0` |
| `docking` | children only: `kind` `rail` (with the game's rail `Tag` dumped) or `legacy` (`dockingPos`, `dockingSize`, `dockingStyle`, `dockingOrientation`) |
| `header` | version, saved-in game version, `entityType` + ordinal, `classification` + ordinal, `bounds`, `hasScore`, file size |
| `entityType`, `classification`, `bounds` | copies of the header values |
| `headerCounts` | `{typeId: n}` exactly as stored (types the block config does not know are dropped by the game's reader) |
| `headerCountsAfterGameConversion` | present when `ElementCountMap.convertNonPlaceableBlocks()` changed the counts (the game prices shaped blocks as their base block) |
| `meta` | version, `railUID`, `children`, `dockerPoints`, cargo, wireless markers, whether an AI tag and a manager tag exist |
| `regionFiles` | per region file: source name, shadowed duplicates, region coordinate, version, segments, blocks |
| `segments`, `emptySegments`, `segmentDataTypes` | how many segments were stored and in which encoding |
| `blockCounts`, `totalBlocks`, `blockBounds` | counted from the block data the game decoded |
| `boundsFromBlocks`, `boundsMatchBlocks` | the header box the game would write for these blocks |
| `countsMatchHeader`, `coreAt16` | checks |
| `links` | `[{controller:[x,y,z], type:n, targets:[[x,y,z],…]}]`, sorted; with `--summary` `targetCount` replaces `targets` |
| `linkControllers`, `linkGroups`, `linkTargets`, `linksHash` | link totals |
| `contentHash`, `rawHash` | see below |

All positions are **file space**: the coordinates stored in the blueprint, where a ship's
core is at `16,16,16`. Subtract 16 for core-relative ("design") coordinates. Header bounds
are the exception: the game stores them core-relative.

### Hashes

SHA-256, lowercase hex, over fixed-size big-endian records. Blocks are sorted by `x`, then
`y`, then `z` (signed).

- `contentHash`: per block 17 bytes: `x y z` as i32, `type` as u16, `orientation` u8,
  `active` u8 (0/1), `hp` u8.
- `rawHash`: per block 15 bytes: `x y z` as i32, then the three stored bytes `b0 b1 b2`
  (`value = b0 | b1<<8 | b2<<16`).
- `linksHash`: one 26-byte record per (controller, type, target), sorted by controller
  `(x,y,z)`, type, target `(x,y,z)`: controller `x y z` as i32, `type` as u16, target `x y z`
  as i32.

`--blocks out.tsv` writes the same sorted block list as text for diffing.

### Errors and warnings

`errors`: an exception from game code, or data the game would drop or refuse: unknown block
type ids (removed by `SegmentData.revalidateData`), inverted header box (`INVALID AABB` on
spawn), missing `DATA`, a region file name the game cannot address, an index entry pointing
outside the file, a ship without a core at `16,16,16`.

`warnings`: things the game tolerates but an export should not contain: header counts that
differ from the block data, a header box that does not contain the blocks, links whose
controller or target has no block or a block of another type, several `DATA` files for one
region, a stored segment without blocks, old file versions.

## Limitations

- Reader paths only; nothing is spawned (see "Not run").
- A blueprint passing here can still be refused by server settings (`isOkWithConfig`, size
  limits, upload size `ALLOWED_UNPACKED_FILE_UPLOAD_IN_MB`).
- Mod block id translation (`modmappings.smbmm`) is read but not applied; the game applies
  it during spawn.
- Coordinates must be within ±1,048,575.
- Only `.smd3` data. Pre-2017 `.smd2` catalogs go through the game's own migration in
  `importFile`, which is untested here.

## Samples

`samples/` holds small fixtures, the scripts that hand-build them, and a reader that uses
no game code:

- `minimal-core.sment`: core-only ship, smallest files the game accepts (5-byte meta).
- `plain-core.sment`: same, with the meta sections the game's writer emits for an undocked ship.
- `compressed-segments.sment`: region file written by the game's own `SegmentDataIONew.write`
  with one `BITMAP` and one `SINGLE` segment.
- `old-versions.sment`: one region file (file version byte 2) with segments of version 2, 3,
  4, 5 and 6, plus version 4 and 6 `SINGLE`/`BITMAP` segments. The version 2 segment holds
  every orientation, both active states and twelve hit point values for one block of each
  block style and four unknown type ids. Exit 1 is expected (unknown types).
- `old-versions-unasserted.sment`: `SINGLE`/`BITMAP` segments with version 3 and 5. Run with
  `GAMEVERIFY_ASSERT=0`; with assertions the game's own `assert version == 4` fires.
- `logic-old-disk`, `logic-disk-1026`, `logic-disk-1025`, `logic-network-1`,
  `logic-network-2` (`.sment`): the same links in each `logic.smbpl` format, with unknown
  types, an empty group, a repeated target, a 16-bit wrap, one type in two groups and a
  controller stored twice.
- `build_min.py`, `build_old.py`: write the hand-built fixtures (`build_old.py <dir>` also
  writes three inputs the game refuses or misreads).
- `old_segments.py <blueprint-dir | file.sment> [--compare game.json] [--explain-links]`:
  standalone Python reader implementing the rules under "Old segment versions" and "Old
  logic files". It takes block styles from `BlockConfig.xml`, not from the jar.
- `vectors.json`: what the game read (blocks, hashes, link totals) for 15 stock blueprints
  (95 entities) and the samples, with the command for each. `old_segments.py` reproduces
  every value in it.

## Format notes (from the game's readers)

All integers big-endian (`DataInputStream`). `UTF` is Java modified UTF-8 with a u16 length.

### `.sment`

A zip with **exactly one** top-level directory; that directory must contain something other
than `header.smbph`. Anything else is refused by `BlueprintEntry.importFile`. The directory
name becomes the catalog name (`_0`, `_1`… appended if taken; dots are stripped from the
display name).

### `header.smbph`

```text
i32  version                5 (below 2: chunk16 migration; below 4: old power flag)
UTF  game version           only when version >= 5; free text, e.g. "0.304.7_20260628_193755"
i32  entity type            BlueprintType ordinal: 0 SHIP, 1 SHOP, 2 SPACE_STATION, 3 MANAGED_ASTEROID, 4 ASTEROID, 5 PLANET
i32  classification         BlueprintClassification ordinal (version >= 3); 0 NONE for ships, 9 NONE_STATION
f32  x6                     bounding box min xyz, max xyz; core-relative; the game writes (min block - 1, max block + 2)
i32  n                      element counts
n x (i16 type, i32 count)   types unknown to the block config are skipped on read
u8   hasScore               tail, part 1
if hasScore: i16 score version (1), then 9 f64 (version 0: 8 f64)
```

The bytes after the element counts are therefore `00`, or `01 00 01` plus 72 bytes of
`EntityIndexScore`. The box must not be inverted (spawn throws `INVALID AABB`).

Smallest header for a new core-only ship (72 bytes):

```text
00 00 00 05                                      version 5
00 17 "0.304.7_20260628_193755"                  UTF game version
00 00 00 00                                      SHIP
00 00 00 00                                      NONE
bf 80 00 00  bf 80 00 00  bf 80 00 00            min -1 -1 -1
40 00 00 00  40 00 00 00  40 00 00 00            max  2  2  2
00 00 00 01  00 01 00 00 00 01                   one entry: type 1 x 1
00                                               no score
```

### `logic.smbpl`

```text
i32  structure version      0
i32  tag                    -1026 = -(1024 + 2): uncompressed disk format, no chunk16 shift
i32  controllers
per controller: i16 x, y, z; i32 groups
  per group:    i16 type; i32 n; n x (i16 x, y, z)
```

Empty file for a ship without links: `00 00 00 00 ff ff fb fe 00 00 00 00`. A tag `>= 0` is
the oldest format (the value is the controller count, positions get +8); `-1 … -1024` is a
network format with extra fields. Positions are file space and limited to i16.

### `meta.smbpm`

```text
i32  version                5 (below 4: chunk16, positions get +8; below 2: no rail UID block in section 4)
then sections, each introduced by one byte, in any order, until 01 or 02:

03  legacy docking          i32 n; n x (UTF child name, i32 x y z dock position, f32 x y z size, i16 style, i8 orientation)
06  rail docker blocks      u8 exists; if exists: i32 n; n x (i32 x y z, i16 type, i8 orientation, u8 active, i8 hp)
07  cargo                   u8 exists; if exists: i32 n; n x (i64 position index, f64 capacity)
04  rails                   f32 x6 total box of the rail root;
                            UTF rail UID; i32 n; n x (UTF marking, i64 marker position, i64 from position)   wireless logic
                            i32 n; n x (UTF child name, i32 tag size, tag bytes)                             rail-docked children
05  AI configuration        i32 tag size, tag bytes
02  manager container tag   a Tag running to the end of the file; ends the meta
01  finish                  ends the meta
```

Any other section byte throws `Unknown data type`; a file that ends without `01` or `02`
throws `EOFException`. Child names are paths below the catalog (`Name/ATTACHED_0`); each
child directory is a complete blueprint. A position index is
`(z & 0xFFFF) << 32 | (y & 0xFFFF) << 16 | (x & 0xFFFF)`.

The game's writer always emits `03, 06, 07, 04`, then `05` for ships and stations, then `02`.

Smallest meta the reader accepts, and enough for an undocked new ship (spawn code checks
the AI and manager tags for null and builds defaults):

```text
00 00 00 05 01
```

The same with every section the game writes for an undocked ship, minus the two tags (49 bytes):

```text
00 00 00 05                 version
03 00 00 00 00              no legacy docks
06 00                       no rail docker blocks
07 00                       no cargo
04 (24 x 00)                rail root box
   00 00                    rail UID ""
   00 00 00 00              no wireless markers
   00 00 00 00              no rail-docked children
01                          finish
```

With the 5-byte form `railUID` is null and "had cargo byte" is false; only NPC faction
presets look at the latter. A ship meant to be spawned **onto a rail** needs section `06`
with its rail docker block, otherwise the game reports "no docker".

### `DATA/*.smd3`

```text
u8   version (6), 3 x 00
4096 x (i16 offset, u16 size)     index; entry for segment (sx,sy,sz) is ((sz+8)&15)*256 + ((sy+8)&15)*16 + ((sx+8)&15)
slots of 49152 bytes from byte 16388; slot number = offset - 1
```

- `offset == 0`: never written. `size == 0` with an offset: known empty; the slot may still
  hold stale data and must be ignored. **The index is authoritative**: the game never scans
  slots, and (without assertions) ignores the position stored inside the record. The stock
  `blueprints-stations/shop/Trade Outpost` shows why: 19 live segments (29,887 blocks, equal
  to its header counts) and 38 emptied ones whose slots still hold 26,433 old blocks.
- File name: only the part from the first dot matters (`<anything>.<rx>.<ry>.<rz>.smd3`);
  the game renames the files to the new entity's UID on spawn. Two files for the same region
  both get copied and the directory listing order decides which survives, so write one file
  per region. A name with more than four dots gets its extra dots replaced by `_` first.
- Region `r` holds segments `r*16 - 8 … r*16 + 7` per axis.

Segment record:

```text
u8   version          6
i64  last changed     ms; values in the future are replaced
i32  x, y, z          segment position in blocks (multiples of 32)
u8   data type        1 INT_ARRAY, 2 empty, 3 SINGLE, 4 BITMAP, 5 SINGLE_SIDE_EDGE (planets)
type 1: i32 zip size; zlib of 32768 x 3 bytes; block i = x + y*32 + z*1024; value = b0 | b1<<8 | b2<<16
type 3: i32 value                      every block of the segment has this value
type 4: i32 shift (3, 4 or 5); i32 n (<= 16); n x i32 palette values;
        (32768 >> shift) x i32 words; bits per block = 32 >> shift;
        palette index of block i = (word[i >> shift] >> ((i & ((1 << shift) - 1)) << (5 - shift))) & ((1 << bits) - 1)
```

Block value: bits 0–10 type, 11–17 hit points (0–127), 18 active, 19–23 orientation. Game
files contain hit point values below 127 for undamaged blocks (e.g. 126, 124), so keep the
stored value instead of resetting it.

Types 3 and 4 are not zlib-compressed. The server converts a segment to `BITMAP` when it was
migrated from a pre-v6 save and holds at most 16 distinct block values, so they can appear
in blueprints of old ships; all blueprints shipped with 0.304.7 use type 1 only.

## Old segment versions

The version is the first byte of each **segment record**. The version byte at the start of
the region file is never read for `.smd3` (ten stock files say 2 and hold version 6
segments). A file may mix versions; each segment is converted on its own
(`samples/old-versions.sment`). The record header (version, timestamp, position, data type)
and the region index are the same for every version.

In the 0.304.7 install: 6,253 segments are version 2, 278 are version 3, 133 are version 6;
none are 0, 1, 4 or 5.

Everything below is `RemoteSegment.deserialize`. It picks a decoder by version from
`SegmentSerializationBuffers.createDummies` (0 `SegmentDataOld`, 1 `SegmentDataRepairAll`,
2 `SegmentData3Byte`, 3 to 6 `SegmentDataIntArray`), inflates into it, then calls
`migrateTo(version, next)` once per step until version 6. A segment therefore passes through
every later step in order.

| Version | On disk (data type 1) | Steps applied |
|---|---|---|
| 6 | zlib of 32768 x 3 bytes, value little-endian | none |
| 5 | same as 6 | none (`SegmentDataIONew.checkVersionSkip` turns 5 into 6 for data type 1) |
| 4 | same as 6 | hit point reset |
| 3 | same as 6 | hit point reset |
| 2 | zlib of 32768 x 3 bytes, value **big-endian**, other bit layout | field conversion, then hit point reset |
| 1, 0 | zlib of 4096 x 3 bytes (16-block chunks) | not valid in `.smd3`, see below |

`valid(type)` is `ElementKeyMap.isValidType`: the id has an entry in the block config
(1,516 ids in 0.304.7, largest 1791). It depends on the config in use, so a server with
custom blocks converts the same file differently. `style(type)` is the `<BlockStyle>` number of that
entry: 0 normal, 1 wedge, 2 corner, 3 sprite, 4 tetra, 5 hepta, 6 normal24.

**Hit point reset** (`SegmentDataIntArray.migrateTo(4, …)` → `repairAll`), for every cell:

```text
if valid(type):  hp = 127          # bits 11..17 all set
else:            unchanged
```

**Version 2 field conversion** (`SegmentData3Byte.migrateTo` and `convertOrient`), for every
one of the 32768 cells, including empty ones:

```text
V      = b0 << 16 | b1 << 8 | b2          # b0 is the first byte on disk
type   = V & 0x7FF                        # bits 0..10
hp8    = (V >> 11) & 0xFF                 # bits 11..18, 0..255
active = ((V >> 19) & 1) == 0             # bit 19, stored inverted
ori    = (V >> 20) & 0xF                  # bits 20..23, 0..15

if valid(type) and style(type) in {2, 4, 5, 6}:
    ori = (ori + (0 if active else 16)) % 24
    # 24 = BlockShapeAlgorithm.algorithms[style - 1].length - 1; the same for all four styles

new = type | min(127, hp8) << 11 | (1 if active else 0) << 18 | ori << 19
# then the hit point reset above
```

- The active flag is copied as well as being used as the fifth orientation bit, so a corner,
  tetra, hepta or normal24 block with orientation 16 to 23 ends up inactive.
- Normal, wedge and sprite blocks keep their four orientation bits unchanged.
- Blocks of an unknown type keep orientation and get `min(127, hp8)`.
- No type id is remapped or removed and slabs get no special treatment (versions 2 to 5).
- The index order (`x + y*32 + z*1024`) and the cell count are the same as today.
- An empty cell (`V == 0`) becomes `0x040000`: type 0 with the active bit. The hashes skip
  type 0. Air must be recognised by type, not by value: current game files are full of such
  cells (`0x040000`, and leftovers such as `0x03f800` where a block was removed).
- If the stream does not inflate to exactly 98,304 bytes the game logs a
  `SegmentInflaterException` and carries on with whatever was inflated. Treat it as an error.

**Other data types** (not zlib, see "DATA/*.smd3"):

| Data type | Version 6 | Version 5 | Version 4 and 3 | Version 2, 1, 0 |
|---|---|---|---|---|
| 3 `SINGLE`, 4 `BITMAP` | values as stored | values as stored | expand to 32768 values, then hit point reset | the game throws (`SegmentDataWriteException`); the segment does not load |

Version 3 and 5 with these types trip `assert (version == 4)` when assertions are on; the
live game has them off and reads the values as in the table.

**Versions 1 and 0** are 16-block-chunk formats from before `.smd3`. Old `.smd2` catalogs are
converted by `Chunk32Migration`, which enters the chain at version 2, so the game never
writes them into `.smd3`. If one is put there anyway the game does not refuse it: it keeps
the first 12,288 inflated bytes and treats them as the first 4,096 cells of a 32-block
segment, which misplaces real 16-block data. A reader should refuse both.

- Version 1 (`SegmentDataRepairAll`): bit layout as version 2. Step to version 2: fields
  copied unchanged, then `hp8 = 127` for the type ids in `SegmentDataRepairAll.raisedHP`
  (list in `old_segments.py`). Confirmed against the game with `build_old.py`'s `bad-v1`.
- Version 0 (`SegmentDataOld`): big-endian, type bits 0..10, hit points bits 11..19 (9
  bits), active bit 20 inverted, orientation bits 21..23. Step to version 1
  (`SegmentDataOld.migrateTo`): unknown types are dropped; non-orientable normal blocks get
  their default orientation; type 56 gets orientation 3; sprites get orientation 2 and
  active; other shaped blocks are looked up in `SegmentDataOld.algorithmsOld` by shape class
  and re-encoded as orientation 0..15 plus the active bit. Read from the code only: not
  reimplemented and not compared.

### Proof

`samples/old_segments.py` implements the rules above without game code. For every entity it
gives the same `totalBlocks`, `rawHash` and `contentHash` as the game on:

- stock, version 2: Tradestation Gamma, Tradestation Beta, Tradestation Eta, Piratestation
  Alpha, Piratestation Beta, Derelict Alpha, Derelict Delta, Derelict Eta (under
  `blueprints-stations` / `blueprints-npc`), and Kukri and Refueling Platform (under
  `server-database/world0/npcFactions`);
- stock, version 3: Scud, Small Refueling Platform, Research Station (under
  `server-database/world0/npcFactions`; `blueprints*` has version 3 only as bare terrain
  region files without a header);
- stock, version 6: Trade Outpost, Isanth Type-PNR-25-B;
- `samples/old-versions.sment` and, with `GAMEVERIFY_ASSERT=0`,
  `samples/old-versions-unasserted.sment`.

The stock version 2 data exercises the big-endian layout, the inverted active bit, the hit
point reset, `+16` for corners and normal24 blocks, and wedges and sprites being left
alone. It contains no inactive tetra or hepta block, no orientation that wraps past 24 and
no unknown type with more than 127 hit points; those three cases, version 4, and version 5
are covered only by the synthetic sample.

Stock stations exit with status 1 because they hold blocks of type ids the current block
config no longer defines (7, 88, 289, 420, 421 …). The hashes include those blocks; the game
removes them when the segment is validated after loading (`SegmentData.revalidateData`).

## Old logic files

`BlueprintEntry.readStructure` reads the i32 structure version (ignored), then
`ControlElementMapper.deserialize` decides the format from the next i32, the tag:

| Tag | Format | Position shift |
|---|---|---|
| `>= 0` | oldest: the tag is the controller count, then the disk layout | +8 on every coordinate |
| `< -1024` (today `-1026`) | disk layout after an i32 controller count | none |
| `-1` | network layout | +8 |
| `-2 … -1024` | network layout | none |

The shift depends only on the tag, never on the header or meta version.

```text
disk layout:     per controller: i16 x, y, z; i32 groups
                 per group:      i16 type; i32 n; n x (i16 x, y, z)
network layout:  per controller: i16 x, y, z; i32 groups
                 per group:      i16 type; i32 n; u8 bigX, bigY, bigZ; i16 medianX, medianY, medianZ;
                                 n x (per axis: i16 if big else i8, plus that axis' median)

for each stored controller C, in file order:
    c = wrap16(C + shift)                       # per coordinate, Java (short)
    for each stored group (type, targets):
        if not exists(type): skip the group     # ControlElementMapper.putAll
        links[c][type] = links[c][type] ∪ { wrap16(t + shift) for t in targets }
```

`exists(type)` is `ElementKeyMap.exists`: `type > 0` and defined in the block config.

That one check is the only rule that drops anything, and it applies to every format,
`-1026` included:

- A group whose **controlled type** is unknown is dropped whole.
- A controller appears in the map only once a group of it is kept, so a controller whose
  groups are all dropped disappears.
- A kept group with no targets stays (an empty group), and so does its controller.
- Targets are a set per (controller, type): repeats collapse, and groups of the same type
  merge, also when the same controller is stored twice.
- The reader never looks at the block data. It does not check that the controller block
  exists, what type it is, or that a target block exists or has the recorded type, and it
  remaps no type id.
- Nothing is rewritten on disk; the map is copied into the spawned entity as read
  (`ControlElementMapper.setFromMap`).

What the running game does with such links after the spawn was not run here. In
`ControlElementMap` the only pruning found is tied to a block being removed
(`onRemoveElement`); no pass that checks links against blocks on load was found.

### Worked examples

`samples/old_segments.py <dir> --explain-links` prints every stored group with the verdict.

`blueprints-stations/neutral/Derelict Eta`, main entity: tag 10, so shift +8 and ten
controllers. Five of them have a single group of type 88, an id the 0.304.7 config does not
define, so those groups are dropped and the controllers vanish: stored (8,32,151),
(-41,1,-51), (-41,1,-24), (57,1,-24), (57,1,-51). The other five have a group of type 290
(defined) and survive at (16,9,181), (-51,9,133), (-35,5,152), (83,9,133), (67,5,152) with
21 + 6 + 9 + 6 + 9 = 51 targets. Their controller blocks are type 289, which the config does
not define either; that does not matter to the reader. `ATTACHED_0` keeps all six
controllers, two of them at positions with no block and only empty groups.

`blueprints-stations/pirate/Piratestation Alpha`, main entity: tag 37. Thirteen controllers
have only a type 88 group and one has only an (empty) type 477 group; all fourteen
disappear. 23 controllers, 32 groups and 1,428 targets remain. Over the whole blueprint
(15 entities) 81 stored controllers become 61; the dropped groups are of types 88, 416,
417, 476 and 477.

Both blueprints, and the thirteen others in `vectors.json`, give the same link controllers,
groups, targets and `linksHash` in `old_segments.py` and in the game. The network layout and
tag `-1025` do not occur in stock blueprints and are covered by the `logic-*` samples.
