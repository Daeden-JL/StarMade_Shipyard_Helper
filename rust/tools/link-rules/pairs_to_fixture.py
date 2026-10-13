#!/usr/bin/env python3
"""Turns the probe's legal_pairs_<kind>.tsv into the compact test data files:
one line per controller type, `from: to to to`."""
import pathlib
import sys

source, target = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
for kind in ("ship", "station"):
    by = {}
    for line in (source / f"legal_pairs_{kind}.tsv").read_text().splitlines():
        if not line[:1].isdigit():
            continue
        a, b, _ = line.split("\t")
        by.setdefault(int(a), []).append(int(b))
    out = [
        f"# Every pair of block types the game (0.304.7) lets a player link on a {kind}, as its own",
        "# classes answered when asked for all 1516 x 1516 pairs: controller type, then the types it",
        "# may control. Written by the link rule probe; see tools/link-rules/README.md.",
    ]
    out += [f"{a}: {' '.join(str(b) for b in sorted(by[a]))}" for a in sorted(by)]
    (target / f"link_pairs_{kind}.txt").write_text("\n".join(out) + "\n")
    print(kind, len(by), "controller types,", sum(len(v) for v in by.values()), "pairs")
