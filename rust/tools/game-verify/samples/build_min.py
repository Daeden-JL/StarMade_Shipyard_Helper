#!/usr/bin/env python3
"""Hand-builds a core-only StarMade ship blueprint (no game code involved)."""
import os, struct, sys, time, zipfile, zlib, shutil

def utf(s):
    b = s.encode("utf-8")
    return struct.pack(">H", len(b)) + b

def header(counts, bmin, bmax, game_version="0.304.7_20260628_193755", entity_type=0, classification=0):
    out = struct.pack(">i", 5) + utf(game_version)
    out += struct.pack(">ii", entity_type, classification)
    out += struct.pack(">6f", *bmin, *bmax)
    out += struct.pack(">i", len(counts))
    for t, n in sorted(counts.items()):
        out += struct.pack(">hi", t, n)
    out += b"\x00"                      # boolean hasScore = false
    return out

def logic():
    return struct.pack(">iii", 0, -1026, 0)   # structure version 0, disk-format tag, 0 controllers

def meta_min():
    return struct.pack(">i", 5) + b"\x01"     # version 5, FINISH

def meta_plain():
    # what the game's writer emits for a ship with nothing docked, minus the AI and manager tags
    out = struct.pack(">i", 5)
    out += b"\x03" + struct.pack(">i", 0)                 # legacy docks: none
    out += b"\x06\x00"                                    # rail docker blocks: absent
    out += b"\x07\x00"                                    # cargo: absent
    out += b"\x04" + struct.pack(">6f", 0, 0, 0, 0, 0, 0) # rail root total box
    out += utf("") + struct.pack(">i", 0)                 # rail UID, wireless markers
    out += struct.pack(">i", 0)                           # rail-docked children
    out += b"\x01"                                        # FINISH
    return out

def smd3(segments, stored_pos=None, timestamp=None):
    """segments: {(sx,sy,sz) segment index: {(x,y,z) local: 24-bit value}}"""
    table = bytearray(4096 * 4)
    body = b""
    ts = int(time.time() * 1000) if timestamp is None else timestamp
    for slot, (seg, blocks) in enumerate(sorted(segments.items())):
        raw = bytearray(32768 * 3)
        for (x, y, z), v in blocks.items():
            i = (x + y * 32 + z * 1024) * 3
            raw[i], raw[i + 1], raw[i + 2] = v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF
        comp = zlib.compress(bytes(raw))
        pos = stored_pos if stored_pos is not None else tuple(c * 32 for c in seg)
        rec = struct.pack(">bq3i", 6, ts, *pos) + b"\x01" + struct.pack(">i", len(comp)) + comp
        assert len(rec) < 49152
        idx = ((seg[2] + 8) & 15) * 256 + ((seg[1] + 8) & 15) * 16 + ((seg[0] + 8) & 15)
        struct.pack_into(">hH", table, idx * 4, slot + 1, len(rec))
        body += rec + b"\x00" * (49152 - len(rec))
    return b"\x06\x00\x00\x00" + bytes(table) + body

def block(type_, orientation=0, active=False, hp=127):
    return type_ | (hp << 11) | ((1 if active else 0) << 18) | (orientation << 19)

def write(root, name, meta, counts=None, stored_pos=None, bounds=((-1, -1, -1), (2, 2, 2)), blocks=None):
    d = os.path.join(root, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(os.path.join(d, "DATA"))
    blocks = blocks or {(0, 0, 0): {(16, 16, 16): block(1)}}
    open(os.path.join(d, "header.smbph"), "wb").write(header(counts or {1: 1}, *bounds))
    open(os.path.join(d, "logic.smbpl"), "wb").write(logic())
    open(os.path.join(d, "meta.smbpm"), "wb").write(meta)
    open(os.path.join(d, "DATA", name + ".0.0.0.smd3"), "wb").write(smd3(blocks, stored_pos))
    with zipfile.ZipFile(os.path.join(root, name + ".sment"), "w", zipfile.ZIP_DEFLATED) as z:
        for base, _, files in os.walk(d):
            for f in files:
                p = os.path.join(base, f)
                z.write(p, os.path.relpath(p, root))
    return d

if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    write(root, "GV Minimal Core", meta_min())
    write(root, "GV Plain Core", meta_plain())
    # negative cases
    write(root, "GV Bad Count", meta_min(), counts={1: 1, 5: 3})
    write(root, "GV Bad SegPos", meta_min(), stored_pos=(32, 0, 0))
    write(root, "GV No Core", meta_min(), counts={5: 1}, blocks={(0, 0, 0): {(17, 16, 16): block(5)}})
    write(root, "GV Bad Type", meta_min(), counts={1: 1}, blocks={(0, 0, 0): {(16, 16, 16): block(1), (17, 16, 16): block(2040)}})
    for n in ("header.smbph", "logic.smbpl", "meta.smbpm"):
        print(n, open(os.path.join(root, "GV Minimal Core", n), "rb").read().hex(" "))
    print("meta_plain", meta_plain().hex(" "))
