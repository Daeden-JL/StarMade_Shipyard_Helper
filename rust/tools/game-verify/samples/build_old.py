#!/usr/bin/env python3
"""Hand-builds blueprints whose region file mixes every segment version and encoding, so the
old-version rules can be checked against the game on inputs the stock blueprints never contain.

    build_old.py [out-dir]     writes old-versions.sment, old-versions-unasserted.sment
                               and three inputs the game refuses or misreads

No game code is involved. See "Old segment versions" in ../README.md.
"""
import os
import shutil
import struct
import sys
import zipfile
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_min import header, logic, meta_min  # noqa: E402

# One block type per BlockStyle (0 normal .. 6 normal24), then ids the 0.304.7 config does not
# define: 7 and 88 (gaps), 1792 (first id past the table), 2047 (largest 11-bit id).
TYPES = [5, 208, 209, 93, 214, 210, 114, 7, 88, 1792, 2047]
HP8 = [0, 1, 2, 63, 64, 126, 127, 128, 129, 200, 254, 255]
HP7 = [0, 1, 63, 126, 127]
ORI5 = [0, 1, 7, 8, 15, 16, 23, 24, 31]


def v2_values():
    """Version 2 blocks: 11-bit type, 8-bit hp, inverted active bit, 4-bit orientation."""
    out = []
    for t in TYPES:
        for hp in HP8:
            for inactive in (0, 1):
                for ori in range(16):
                    out.append(t | (hp << 11) | (inactive << 19) | (ori << 20))
    return out


def current_values():
    out = []
    for t in TYPES:
        for hp in HP7:
            for active in (0, 1):
                for ori in ORI5:
                    out.append(t | (hp << 11) | (active << 18) | (ori << 19))
    return out


def int_array(version, values, big_endian=False, first=0):
    raw = bytearray(32768 * 3)
    for n, v in enumerate(values):
        i = (first + n) * 3
        if big_endian:
            raw[i], raw[i + 1], raw[i + 2] = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
        else:
            raw[i], raw[i + 1], raw[i + 2] = v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF
    comp = zlib.compress(bytes(raw))
    return version, b"\x01" + struct.pack(">i", len(comp)) + comp


def single(version, value):
    return version, b"\x03" + struct.pack(">i", value)


def bitmap(version, palette, pick):
    """pick(i) -> palette index of block i."""
    shift = 5
    while len(palette) > 1 << (32 >> shift):
        shift -= 1
    words = [0] * (32768 >> shift)
    low = (1 << shift) - 1
    for i in range(32768):
        words[i >> shift] |= pick(i) << ((i & low) << (5 - shift))
    body = struct.pack(">ii", shift, len(palette)) + struct.pack(">%di" % len(palette), *palette)
    return version, b"\x04" + body + struct.pack(">%dI" % len(words), *words)


def region(segments, file_version=6):
    """segments: {(sx, sy, sz): (version, payload from the data-type byte on)}"""
    table = bytearray(4096 * 4)
    body = b""
    for slot, (seg, (version, payload)) in enumerate(sorted(segments.items())):
        rec = struct.pack(">bq3i", version, 1500000000000, seg[0] * 32, seg[1] * 32, seg[2] * 32) + payload
        assert len(rec) < 49152, len(rec)
        idx = ((seg[2] + 8) & 15) * 256 + ((seg[1] + 8) & 15) * 16 + ((seg[0] + 8) & 15)
        struct.pack_into(">hH", table, idx * 4, slot + 1, len(rec))
        body += rec + b"\x00" * (49152 - len(rec))
    return bytes((file_version, 0, 0, 0)) + bytes(table) + body


def logic_disk(tag, controllers, count_in_tag=False):
    """controllers: [((x, y, z), [(type, [(x, y, z), ...]), ...]), ...] written as stored."""
    out = struct.pack(">i", 0)
    out += struct.pack(">i", len(controllers)) if count_in_tag else struct.pack(">ii", tag, len(controllers))
    for pos, groups in controllers:
        out += struct.pack(">hhhi", *pos, len(groups))
        for block_type, targets in groups:
            out += struct.pack(">hi", block_type, len(targets))
            for t in targets:
                out += struct.pack(">hhh", *t)
    return out


def logic_network(tag, controllers, big=(True, False, True), median=(3, -2, 40)):
    out = struct.pack(">iii", 0, tag, len(controllers))
    for pos, groups in controllers:
        out += struct.pack(">hhhi", *pos, len(groups))
        for block_type, targets in groups:
            out += struct.pack(">hi???hhh", block_type, len(targets), *big, *median)
            for t in targets:
                for axis in range(3):
                    out += struct.pack(">h" if big[axis] else ">b", t[axis] - median[axis])
    return out


# Stored links for the logic-format samples: a known type, a type the config does not define
# (88), a controller whose only group is dropped, an empty group, a repeated target,
# coordinates that wrap a 16-bit integer once 8 is added, a type split over two groups and a
# controller stored twice.
LINKS = [
    ((16, 16, 16), [(5, [(17, 16, 16), (18, 16, 16), (17, 16, 16)]), (88, [(19, 16, 16)])]),
    ((8, 8, 8), [(88, [(1, 2, 3), (4, 5, 6)])]),
    ((-5, 0, 60), [(6, []), (16, [(-5, 1, 60), (-5, 2, 61)])]),
    ((32764, 10, 10), [(5, [(32764, 11, 10)])]),
    ((0, 0, 0), [(2047, [(1, 1, 1)]), (0, [(2, 2, 2)]), (-3, [(3, 3, 3)])]),
    ((40, 40, 40), [(5, [(41, 40, 40)]), (5, [(42, 40, 40), (41, 40, 40)])]),   # one type in two groups
    ((16, 16, 16), [(5, [(20, 16, 16)])]),                                      # a controller stored twice
]
NETWORK_LINKS = [(pos, [(t, [(min(max(x, -124), 130), min(max(y, -130), 125), z) for x, y, z in targets]) for t, targets in groups])
                 for pos, groups in LINKS]


def write(root, name, segments, file_version=6, logic_bytes=None):
    d = os.path.join(root, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(os.path.join(d, "DATA"))
    open(os.path.join(d, "header.smbph"), "wb").write(header({1: 1}, (-300, -300, -300), (300, 300, 300)))
    open(os.path.join(d, "logic.smbpl"), "wb").write(logic_bytes if logic_bytes is not None else logic())
    open(os.path.join(d, "meta.smbpm"), "wb").write(meta_min())
    open(os.path.join(d, "DATA", name + ".0.0.0.smd3"), "wb").write(region(segments, file_version))
    sment = os.path.join(root, name + ".sment")
    with zipfile.ZipFile(sment, "w", zipfile.ZIP_DEFLATED) as z:
        for base, _, files in os.walk(d):
            for f in sorted(files):
                p = os.path.join(base, f)
                z.write(p, os.path.relpath(p, root))
    shutil.rmtree(d)
    return sment


CORE = 1 | (127 << 11)
core_segment = int_array(6, [CORE], first=16 + 16 * 32 + 16 * 1024)
half = lambda i: 1 if (i & 31) < 16 else 0          # noqa: E731
palette = [0, 5 | (100 << 11), 7 | (90 << 11) | (1 << 18), 209 | (3 << 11) | (17 << 19)]

if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(root, exist_ok=True)
    # Every version the game reads from .smd3, in one region file whose own version byte says 2.
    write(root, "old-versions", {
        (0, 0, 0): core_segment,
        (1, 0, 0): int_array(2, v2_values(), big_endian=True),
        (2, 0, 0): int_array(3, current_values()),
        (3, 0, 0): int_array(4, current_values()),
        (4, 0, 0): int_array(5, current_values()),
        (5, 0, 0): int_array(6, current_values()),
        (0, 1, 0): single(4, 5 | (64 << 11) | (9 << 19)),
        (1, 1, 0): bitmap(4, palette, lambda i: (i >> 3) & 3),
        (4, 1, 0): single(6, 5 | (64 << 11)),
        (5, 1, 0): bitmap(6, palette, lambda i: (i >> 5) & 3),
    }, file_version=2)
    # Compressed encodings with version 3 or 5: an assertion in RemoteSegment says a compressed
    # segment below version 6 must be version 4. The live game runs without assertions and
    # reads them; check this file with GAMEVERIFY_ASSERT=0.
    write(root, "old-versions-unasserted", {
        (0, 0, 0): core_segment,
        (1, 0, 0): single(5, 5 | (64 << 11) | (9 << 19)),
        (2, 0, 0): bitmap(5, palette, lambda i: (i >> 3) & 3),
        (3, 0, 0): single(3, 7 | (64 << 11) | (1 << 18)),
        (4, 0, 0): bitmap(3, palette[:2], half),
        (5, 0, 0): single(3, 5 | (64 << 11) | (1 << 18)),
    })
    # logic.smbpl in each format the reader accepts.
    write(root, "logic-old-disk", {(0, 0, 0): core_segment}, logic_bytes=logic_disk(None, LINKS, count_in_tag=True))
    write(root, "logic-disk-1026", {(0, 0, 0): core_segment}, logic_bytes=logic_disk(-1026, LINKS))
    write(root, "logic-disk-1025", {(0, 0, 0): core_segment}, logic_bytes=logic_disk(-1025, LINKS))
    write(root, "logic-network-1", {(0, 0, 0): core_segment}, logic_bytes=logic_network(-1, NETWORK_LINKS))
    write(root, "logic-network-2", {(0, 0, 0): core_segment}, logic_bytes=logic_network(-2, NETWORK_LINKS))
    # Inputs outside what the game writes.
    write(root, "bad-v2-compressed", {(0, 0, 0): core_segment, (1, 0, 0): single(2, 5 | (64 << 11))})
    write(root, "bad-v1", {(0, 0, 0): core_segment, (1, 0, 0): int_array(1, v2_values(), big_endian=True)})
    write(root, "bad-v0", {(0, 0, 0): core_segment, (1, 0, 0): int_array(0, v2_values(), big_endian=True)})
