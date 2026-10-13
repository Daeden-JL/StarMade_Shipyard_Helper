#!/bin/sh
# Regenerates rust/crates/yard-core/data/shapes.json from a StarMade install.
#
#   ./run.sh [starmade_dir] [output.json]
#
# starmade_dir defaults to $STARMADE_DIR, then ~/.smtest2/vol. The install is
# only read: the config files the game classes open are copied to a temporary
# working directory and the JVM runs there.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
SM=${1:-${STARMADE_DIR:-$HOME/.smtest2/vol}}
OUT=${2:-$HERE/../../crates/yard-core/data/shapes.json}
JAVA_HOME=${JAVA_HOME:-/opt/homebrew/opt/openjdk@21}
JAVA=$JAVA_HOME/bin/java
JAVAC=$JAVA_HOME/bin/javac

[ -f "$SM/StarMade.jar" ] || { echo "no StarMade.jar in $SM" >&2; exit 1; }
CP="$SM/StarMade.jar:$SM/lib/*"

mkdir -p "$HERE/build" "$(dirname "$OUT")"
OUT=$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")
"$JAVAC" -nowarn -cp "$CP" -d "$HERE/build" "$HERE/ShapeDump.java"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/shape-dump.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/data/textures"
cp -R "$SM/data/config" "$WORK/data/config"
cp "$SM"/data/textures/texOrder*.config "$WORK/data/textures/"
# EngineSettings lists the texture pack directories while its class loads; names are enough.
(cd "$SM/data/textures" && find block -type d -maxdepth 2) | while read -r d; do mkdir -p "$WORK/data/textures/$d"; done
[ -f "$SM/version.txt" ] && cp "$SM/version.txt" "$WORK/version.txt"
for d in customBlockConfig customBlockBehaviorConfig customEffectConfig customFactionConfig; do
    [ -d "$SM/$d" ] && cp -R "$SM/$d" "$WORK/$d"
done

cd "$WORK"
"$JAVA" -Djava.awt.headless=true -cp "$HERE/build:$CP" ShapeDump "$OUT" > "$WORK/run.log" 2>&1 || {
    tail -40 "$WORK/run.log" >&2
    exit 1
}
grep -E '^(wrote|PROBLEM)' "$WORK/run.log" || tail -5 "$WORK/run.log"
