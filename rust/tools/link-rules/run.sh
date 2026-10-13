#!/bin/sh
# Asks the game's own classes which block types may be linked, and writes the
# test data the Rust port of the rules is checked against:
#   crates/yard-core/tests/data/link_pairs_ship.txt and link_pairs_station.txt
# Environment as for tools/game-verify/run.sh: STARMADE_DIR, STARMADE_JAR, JAVA_HOME.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21}"
: "${STARMADE_DIR:=${YARD_STARMADE_DIR:-$HOME/.smtest2/vol}}"
if [ -z "${STARMADE_JAR:-}" ]; then
    if [ -f "$REPO/libs/StarMade.jar" ]; then STARMADE_JAR="$REPO/libs/StarMade.jar"; else STARMADE_JAR="$STARMADE_DIR/StarMade.jar"; fi
fi
CP="$STARMADE_JAR:$STARMADE_DIR/lib/*"
BUILD="$HERE/build"
rm -rf "$BUILD"
mkdir -p "$BUILD/classes" "$BUILD/out/work"
"$JAVA_HOME/bin/javac" -nowarn -Xlint:none -d "$BUILD/classes" -cp "$CP" "$HERE/LinkProbe.java" "$HERE/LinkStateProbe.java" >&2
# The game reads ./data/config from the working directory; it gets a copy to itself.
mkdir -p "$BUILD/out/work/data"
cp -R "$STARMADE_DIR/data/config" "$BUILD/out/work/data/config"
cd "$BUILD/out/work"
"$JAVA_HOME/bin/java" -cp "$BUILD/classes:$CP" LinkProbe container "$HERE/pairs.txt" matrix > "$BUILD/out/container.log" 2>&1 || {
    tail -20 "$BUILD/out/container.log" >&2
    exit 1
}
python3 "$HERE/pairs_to_fixture.py" "$BUILD/out" "$HERE/../../crates/yard-core/tests/data"
echo "link-rules: wrote the test data; module keys are in $BUILD/out/module_keys_*.txt" >&2
