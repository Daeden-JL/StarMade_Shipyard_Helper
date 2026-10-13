#!/usr/bin/env python3
"""Standalone reader for StarMade blueprint block data, including old segment versions.

Reimplements, without any game code, what the game's RemoteSegment.deserialize does to
segments whose version byte is below 6 (see "Old segment versions" in ../README.md), and
prints the same per-entity totalBlocks / rawHash / contentHash as game-verify, so the two
can be compared. It also reads logic.smbpl the way ControlElementMapper.deserialize does
(all three formats) and prints the same link totals and linksHash:

    old_segments.py <blueprint-dir | file.sment> [--compare game-verify.json] [--explain-links]

Block styles and the set of valid type ids come from the game's data files
(data/config/BlockConfig.xml and BlockTypes.properties under $STARMADE_DIR, default
~/.smtest2/vol); nothing is read from the jar.
"""
import hashlib
import io
import json
import os
import re
import struct
import sys
import zipfile
import zlib

SEGMENT_SLOT = 49152
REGION_HEADER = 4 + 4096 * 4
BLOCKS = 32768

# BlockStyle ids from BlockConfig.xml: 0 normal, 1 wedge, 2 corner, 3 sprite, 4 tetra,
# 5 hepta ("penta" in the XML comment), 6 normal24.
# Version 2 stored 4 orientation bits and used the active bit as a fifth for these styles.
# The modulus is BlockShapeAlgorithm.algorithms[style - 1].length - 1, which is 24 for all four.
FIVE_BIT_ORIENTATION_MODULUS = {2: 24, 4: 24, 5: 24, 6: 24}


def load_block_styles(starmade_dir):
    """type id -> BlockStyle id for every block the config defines (= the valid types)."""
    config = os.path.join(starmade_dir, "data", "config")
    ids = {}
    with open(os.path.join(config, "BlockTypes.properties"), encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            name, value = line.split("=", 1)
            ids[name.strip()] = int(value.strip())
    with open(os.path.join(config, "BlockConfig.xml"), encoding="utf-8", errors="replace") as f:
        xml = f.read()
    styles = {}
    for m in re.finditer(r'<Block\s[^>]*\btype="([^"]+)"[^>]*>(.*?)</Block>', xml, re.S):
        style = re.search(r"<BlockStyle>\s*(\d+)", m.group(2))
        if m.group(1) in ids:
            styles[ids[m.group(1)]] = int(style.group(1)) if style else 0
    return styles


def convert_v2(b0, b1, b2, styles):
    """Version 2 block (three bytes, big-endian) -> current 24-bit value."""
    v = (b0 << 16) | (b1 << 8) | b2
    block_type = v & 0x7FF
    hp = min(127, (v >> 11) & 0xFF)          # 8 bits stored
    active = ((v >> 19) & 1) == 0            # stored inverted
    orientation = (v >> 20) & 0xF            # 4 bits stored
    style = styles.get(block_type)           # None: type unknown to the block config
    if style is not None:
        modulus = FIVE_BIT_ORIENTATION_MODULUS.get(style)
        if modulus is not None:
            orientation = (orientation + (0 if active else 16)) % modulus
        hp = 127                             # the version 4 step, applied after
    return block_type | (hp << 11) | ((1 if active else 0) << 18) | (orientation << 19)


# SegmentDataRepairAll.raisedHP: block types whose hit points the version 1 -> 2 step sets to 127.
RAISED_HP_V1 = frozenset([
    591, 598, 599, 600, 601, 602, 263, 311, 320, 402, 401, 608, 609, 610, 611, 612, 271, 319, 328, 384, 376, 507, 508, 509,
    510, 511, 603, 604, 605, 606, 607, 264, 312, 321, 377, 369, 593, 594, 595, 596, 597, 628, 629, 630, 631, 632, 270, 318,
    327, 383, 375, 522, 523, 524, 525, 526, 633, 634, 635, 636, 637, 431, 432, 433, 434, 435, 517, 518, 519, 520, 521, 638,
    639, 640, 641, 642, 265, 313, 322, 378, 370, 512, 513, 514, 515, 516, 613, 614, 615, 616, 617, 266, 314, 323, 379, 371,
    537, 538, 539, 540, 541, 618, 620, 619, 621, 622, 267, 315, 324, 380, 372, 532, 533, 534, 535, 536, 623, 624, 625, 626,
    627, 268, 316, 325, 381, 373, 527, 528, 529, 530, 531, 643, 644, 645, 646, 647, 269, 317, 326, 382, 374, 63, 329, 330,
    368, 367, 289, 7, 122, 588, 479, 418, 419, 420, 421, 424, 425, 422, 423, 460, 461, 462, 463, 464, 465, 466, 467, 476,
    477, 123, 120, 331, 654, 655, 6, 16, 38, 32, 414, 415, 416, 417, 4, 24, 39, 30, 46, 40, 54, 48, 334, 335, 332, 333,
    344, 345, 3, 478, 22, 15, 47, 544, 211, 113, 347])


def decode_v1(stream, styles):
    """What the game literally does with a version 1 segment in a .smd3 file.

    Version 1 is a 16x16x16 chunk format (4096 blocks, same bit layout as version 2). The
    game keeps the first 12288 inflated bytes and treats them as the first 4096 cells of a
    32x32x32 segment, so positions are wrong for real 16-block data. Shown for completeness;
    a reader should refuse versions 0 and 1 in .smd3 files.
    """
    raw = zlib.decompressobj().decompress(stream, 4096 * 3)
    if len(raw) != 4096 * 3:
        raise ValueError("version 1 segment inflates to %d bytes" % len(raw))
    out = [convert_v2(0, 0, 0, styles)] * BLOCKS
    for n in range(4096):
        b0, b1, b2 = raw[n * 3:n * 3 + 3]
        v = (b0 << 16) | (b1 << 8) | b2
        if (v & 0x7FF) in RAISED_HP_V1:
            v = (v & ~(0xFF << 11)) | (127 << 11)
            b0, b1, b2 = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
        out[n] = convert_v2(b0, b1, b2, styles)
    return out


def convert_v3_v4(value, styles):
    """Version 3 and 4 use the current layout; known block types get full hit points."""
    if (value & 0x7FF) in styles:
        return (value & ~(0x7F << 11)) | (127 << 11)
    return value


def decode_segment(record, styles):
    """One segment record -> list of 32768 current-format values (or None when empty)."""
    version = record[0]
    data_type = record[21]
    if data_type == 2:
        return None
    if data_type == 1:
        size = struct.unpack_from(">i", record, 22)[0]
        if version == 5:
            version = 6                      # SegmentDataIONew.checkVersionSkip
        if version == 1:
            return decode_v1(record[26:26 + size], styles)
        raw = zlib.decompress(record[26:26 + size])
        if len(raw) != BLOCKS * 3:
            raise ValueError("segment inflates to %d bytes" % len(raw))
        if version >= 6:
            return [raw[i] | (raw[i + 1] << 8) | (raw[i + 2] << 16) for i in range(0, BLOCKS * 3, 3)]
        if version in (3, 4):
            return [convert_v3_v4(raw[i] | (raw[i + 1] << 8) | (raw[i + 2] << 16), styles) for i in range(0, BLOCKS * 3, 3)]
        if version == 2:
            cache = {}
            out = []
            for i in range(0, BLOCKS * 3, 3):
                key = raw[i:i + 3]
                value = cache.get(key)
                if value is None:
                    value = cache[key] = convert_v2(key[0], key[1], key[2], styles)
                out.append(value)
            return out
        raise ValueError("segment version %d is not reimplemented (16-block chunk era)" % version)
    if data_type == 3:                       # one value for every block
        value = struct.unpack_from(">i", record, 22)[0] & 0xFFFFFF
        values = [value] * BLOCKS
    elif data_type == 4:                     # palette of up to 16 values
        shift, count = struct.unpack_from(">ii", record, 22)
        palette = struct.unpack_from(">%di" % count, record, 30)
        words = struct.unpack_from(">%dI" % (BLOCKS >> shift), record, 30 + 4 * count)
        bits = 32 >> shift
        mask = (1 << bits) - 1
        low = (1 << shift) - 1
        values = [palette[(words[i >> shift] >> ((i & low) << (5 - shift))) & mask] & 0xFFFFFF for i in range(BLOCKS)]
    else:
        raise ValueError("segment data type %d" % data_type)
    if version < 3:
        raise ValueError("compressed segment with version %d: the game throws" % version)
    if version in (3, 4):
        values = [convert_v3_v4(v, styles) for v in values]
    return values


def read_entity(files, styles):
    """files: {region file name: bytes} of one DATA folder -> {(x, y, z): value}."""
    by_suffix = {}
    for name, data in files:                 # later files replace earlier ones, as on spawn
        if name.lower().endswith(".smd3"):
            by_suffix[name[name.index("."):]] = data
    blocks = {}
    for suffix, data in by_suffix.items():
        rx, ry, rz = (int(n) for n in suffix[1:-len(".smd3")].split("."))
        for index in range(4096):
            offset, size = struct.unpack_from(">hH", data, 4 + index * 4)
            if offset == 0 or size == 0:
                continue
            start = REGION_HEADER + (offset - 1) * SEGMENT_SLOT
            values = decode_segment(data[start:start + size], styles)
            if values is None:
                continue
            sx = (rx * 16 + (index & 15) - 8) * 32
            sy = (ry * 16 + ((index >> 4) & 15) - 8) * 32
            sz = (rz * 16 + (index >> 8) - 8) * 32
            for i, value in enumerate(values):
                if value & 0x7FF:
                    blocks[(sx + (i & 31), sy + ((i >> 5) & 31), sz + (i >> 10))] = value
    return blocks


def hashes(blocks):
    raw = hashlib.sha256()
    content = hashlib.sha256()
    for pos in sorted(blocks):
        v = blocks[pos]
        head = struct.pack(">iii", *pos)
        raw.update(head + bytes((v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF)))
        content.update(head + struct.pack(">HBBB", v & 0x7FF, (v >> 19) & 0x1F, (v >> 18) & 1, (v >> 11) & 0x7F))
    return raw.hexdigest(), content.hexdigest()


def s16(v):
    """Java (short) cast."""
    v &= 0xFFFF
    return v - 0x10000 if v >= 0x8000 else v


def read_links(data, styles):
    """logic.smbpl -> (links, report).

    links: {controller (x, y, z): {type: set of target (x, y, z)}}, as the game holds them.
    report: one entry per stored group, saying whether the game kept it.
    """
    pos = [4]                                # i32 structure version, read and ignored

    def take(fmt):
        value = struct.unpack_from(">" + fmt, data, pos[0])
        pos[0] += struct.calcsize(">" + fmt)
        return value if len(value) > 1 else value[0]

    tag = take("i")
    if tag >= 0:                             # oldest format: the tag is the controller count
        fmt, shift, controllers = "old disk", 8, tag
    elif tag < -1024:                        # -1026 is what the game writes today
        fmt, shift, controllers = "disk", 0, take("i")
    else:                                    # -1 .. -1024: network encoding
        fmt, shift, controllers = "network", (8 if -tag < 2 else 0), take("i")
    links = {}
    report = []
    for _ in range(controllers):
        x, y, z = take("hhh")
        controller = (s16(x + shift), s16(y + shift), s16(z + shift))
        for _ in range(take("i")):
            block_type = take("h")
            count = take("i")
            if fmt == "network":
                big = take("???")
                median = take("hhh")
                targets = []
                for _ in range(count):
                    t = [s16(s16(take("h" if big[a] else "b") + median[a]) + shift) for a in range(3)]
                    targets.append(tuple(t))
            else:
                flat = take("%dh" % (count * 3)) if count else ()
                targets = [(s16(flat[i] + shift), s16(flat[i + 1] + shift), s16(flat[i + 2] + shift)) for i in range(0, count * 3, 3)]
            kept = block_type in styles      # ControlElementMapper.putAll: ElementKeyMap.exists(type)
            report.append({"stored": [x, y, z], "controller": list(controller), "type": block_type, "targets": count, "kept": kept})
            if kept:
                links.setdefault(controller, {}).setdefault(block_type, set()).update(targets)
    return links, {"format": fmt, "tag": tag, "shift": shift, "storedControllers": controllers, "groups": report}


def links_summary(links):
    md = hashlib.sha256()
    targets = 0
    for controller in sorted(links):
        for block_type in sorted(links[controller]):
            for target in sorted(links[controller][block_type]):
                md.update(struct.pack(">iiiHiii", *controller, block_type, *target))
                targets += 1
    return {"linkControllers": len(links), "linkGroups": sum(len(g) for g in links.values()), "linkTargets": targets, "linksHash": md.hexdigest()}


def entities_of_dir(root):
    """Yields (path relative to the blueprint's parent, [(file name, bytes)]) for the root and every ATTACHED_n."""
    parent = os.path.dirname(os.path.abspath(root))

    def walk(d):
        data_dir = os.path.join(d, "DATA")
        files = []
        if os.path.isdir(data_dir):
            for name in os.listdir(data_dir):
                p = os.path.join(data_dir, name)
                if os.path.isfile(p):
                    with open(p, "rb") as f:
                        files.append((name, f.read()))
        logic_path = os.path.join(d, "logic.smbpl")
        logic_data = open(logic_path, "rb").read() if os.path.isfile(logic_path) else None
        yield os.path.relpath(d, parent).replace(os.sep, "/"), files, logic_data
        children = [n for n in os.listdir(d) if re.fullmatch(r"ATTACHED_\d+", n) and os.path.isdir(os.path.join(d, n))]
        for name in sorted(children, key=lambda n: int(n.split("_")[1])):
            yield from walk(os.path.join(d, name))

    yield from walk(os.path.abspath(root))


def entities_of_sment(path):
    with zipfile.ZipFile(path) as z:
        names = [n for n in z.namelist() if not n.endswith("/")]
        dirs = sorted({n[:-len("/header.smbph")] for n in names if n.endswith("/header.smbph")},
                      key=lambda d: [int(p.split("_")[1]) if re.fullmatch(r"ATTACHED_\d+", p) else -1 for p in d.split("/")])
        for d in dirs:
            prefix = d + "/DATA/"
            logic_data = z.read(d + "/logic.smbpl") if d + "/logic.smbpl" in names else None
            yield d, [(n[len(prefix):], z.read(n)) for n in names if n.startswith(prefix) and "/" not in n[len(prefix):]], logic_data


def main(argv):
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 64
    target = argv[1]
    compare = argv[argv.index("--compare") + 1] if "--compare" in argv else None
    styles = load_block_styles(os.environ.get("STARMADE_DIR", os.path.expanduser("~/.smtest2/vol")))
    source = entities_of_dir(target) if os.path.isdir(target) else entities_of_sment(target)
    out = []
    explain = "--explain-links" in argv
    for path, files, logic_data in source:
        versions = {}
        for name, data in files:
            if not name.lower().endswith(".smd3"):
                continue
            for index in range(4096):
                offset, size = struct.unpack_from(">hH", data, 4 + index * 4)
                if offset and size:
                    v = data[REGION_HEADER + (offset - 1) * SEGMENT_SLOT]
                    versions[v] = versions.get(v, 0) + 1
        blocks = read_entity(files, styles)
        raw, content = hashes(blocks)
        entity = {"path": path, "segmentVersions": {str(k): v for k, v in sorted(versions.items())},
                  "totalBlocks": len(blocks), "rawHash": raw, "contentHash": content}
        if logic_data is not None:
            links, report = read_links(logic_data, styles)
            entity.update(links_summary(links))
            entity["logicFormat"] = {k: report[k] for k in ("format", "tag", "shift", "storedControllers")}
            if explain:
                for g in report["groups"]:
                    at = blocks.get(tuple(g["controller"]))
                    g["blockAtController"] = (at & 0x7FF) if at is not None else None
                entity["storedGroups"] = report["groups"]
        out.append(entity)
    result = {"tool": "old_segments.py", "validTypes": len(styles), "entities": out}
    status = 0
    if compare:
        with open(compare, encoding="utf-8") as f:
            game = {e["path"]: e for e in json.load(f)["entities"]}
        for e in out:
            g = game.get(e["path"])
            keys = ["totalBlocks", "rawHash", "contentHash"]
            if "linksHash" in e:
                keys += ["linkControllers", "linkGroups", "linkTargets", "linksHash"]
            e["matchesGame"] = bool(g) and all(g.get(k) == e[k] for k in keys)
            if not e["matchesGame"]:
                status = 1
        if len(game) != len(out):
            status = 1
        result["allMatch"] = status == 0
    json.dump(result, sys.stdout, indent=1)
    print()
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv))
