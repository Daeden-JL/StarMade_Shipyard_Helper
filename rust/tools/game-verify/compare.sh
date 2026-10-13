#!/bin/sh
# Checks that the Rust code and the game read a blueprint the same way.
#
#   ./compare.sh <file.sment> [more.sment ...]
#
# Each argument is a .sment file or a blueprint folder. For each entity it
# compares the block count and the content, raw and link hashes reported by the
# game's own reader (run.sh) with those of the Rust reader (the bphash example).
# Exit 0 when the two agree on every blueprint and the game reported no errors.
# Warnings from the game's reader (old file versions, duplicate region files)
# are shown as notes; with --strict as the first argument they fail the check.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
RUST=$(cd "$HERE/../.." && pwd)
(cd "$RUST" && cargo build -q -p yard-core --example bphash) || exit 2
BPHASH="$RUST/target/debug/examples/bphash"
status=0
STRICT=0
if [ "${1:-}" = "--strict" ]; then
    STRICT=1
    shift
fi
export STRICT
for file in "$@"; do
    game=$("$HERE/run.sh" --summary "$file" 2>/dev/null)
    game_status=$?
    rust=$("$BPHASH" "$file" 2>/dev/null)
    rust_status=$?
    GAME="$game" RUST_OUT="$rust" python3 - "$file" "$game_status" "$rust_status" <<'PY' || status=1
import json, os, sys
name, game_status, rust_status = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
problems, notes = [], []
strict = os.environ.get("STRICT") == "1"
try:
    game = json.loads(os.environ["GAME"])
except ValueError:
    print(f"FAIL  {name}: the game-side reader gave no result (exit {game_status})")
    sys.exit(1)
if rust_status != 0:
    problems.append(f"the Rust reader failed (exit {rust_status})")
# Blocks of types the block config lacks are a property of the file, which both
# readers see alike; the game removes them when the design is loaded.
UNKNOWN = "type id the game's block config does not know"
root = game.get("blueprintName", "")
theirs = {}
for e in game.get("entities", []):
    path = e["path"]
    path = "" if path == root else path[len(root) + 1:] if path.startswith(root + "/") else path
    theirs[path] = (e["totalBlocks"], e["contentHash"], e["rawHash"], e["linksHash"])
    for x in e.get("errors", []):
        (notes if UNKNOWN in x else problems).append(f"{path or 'main'}: game error: {x}")
    notes += [f"{path or 'main'}: game warning: {x}" for x in e.get("warnings", [])]
ours = {}
for line in os.environ["RUST_OUT"].splitlines():
    path, blocks, content, raw, links = line.split("\t")
    ours[path] = (int(blocks), content, raw, links)
for path in sorted(set(theirs) | set(ours)):
    label = path or "main"
    if path not in ours:
        problems.append(f"{label}: only the game found this entity")
    elif path not in theirs:
        problems.append(f"{label}: only Rust found this entity")
    else:
        for what, a, b in zip(("blocks", "block fields", "stored values", "links"), theirs[path], ours[path]):
            if a != b:
                shown = f" (game {a}, Rust {b})" if what == "blocks" else ""
                problems.append(f"{label}: {what} differ{shown}")
total = sum(v[0] for v in theirs.values())
if strict:
    problems += notes
    notes = []
if problems:
    print(f"FAIL  {name}")
else:
    print(f"ok    {name}: {len(theirs)} entities, {total} blocks")
for p in problems:
    print(f"        {p}")
for n in notes:
    print(f"        note: {n}")
sys.exit(1 if problems else 0)
PY
done
exit $status
