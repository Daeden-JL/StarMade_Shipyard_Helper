#!/bin/sh
# Loads a blueprint with the game's own classes and prints JSON on stdout.
#   run.sh [--summary] [--strict] [--blocks out.tsv] <file.sment | blueprint-dir>
# Environment:
#   STARMADE_DIR   game install with data/ and lib/   (default: $YARD_STARMADE_DIR, then ~/.smtest2/vol)
#   STARMADE_JAR   StarMade.jar                       (default: <repo>/libs/StarMade.jar, then $STARMADE_DIR/StarMade.jar)
#   JAVA_HOME      a JDK 21                           (default: /opt/homebrew/opt/openjdk@21)
#   GAMEVERIFY_XMX heap size                          (default: 2g)
#   GAMEVERIFY_KEEP=1 keeps the scratch work directory and prints its path on stderr
#   GAMEVERIFY_ASSERT=0 runs the game code without assertions, as the live game does
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
START=$(pwd)

: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21}"
: "${STARMADE_DIR:=${YARD_STARMADE_DIR:-$HOME/.smtest2/vol}}"
if [ -z "${STARMADE_JAR:-}" ]; then
    if [ -f "$REPO/libs/StarMade.jar" ]; then STARMADE_JAR="$REPO/libs/StarMade.jar"; else STARMADE_JAR="$STARMADE_DIR/StarMade.jar"; fi
fi
JAVA="$JAVA_HOME/bin/java"
JAVAC="$JAVA_HOME/bin/javac"
[ -x "$JAVAC" ] || { echo "game-verify: no JDK at JAVA_HOME=$JAVA_HOME" >&2; exit 69; }
[ -f "$STARMADE_JAR" ] || { echo "game-verify: StarMade.jar not found ($STARMADE_JAR)" >&2; exit 69; }
[ -d "$STARMADE_DIR/data/config" ] || { echo "game-verify: no game data at STARMADE_DIR=$STARMADE_DIR" >&2; exit 69; }

CP="$STARMADE_JAR:$STARMADE_DIR/lib/*"
BUILD="$HERE/build"
mkdir -p "$BUILD/classes"
if [ ! -f "$BUILD/classes/GameVerify.class" ] || [ "$HERE/GameVerify.java" -nt "$BUILD/classes/GameVerify.class" ]; then
    "$JAVAC" -nowarn -Xlint:none -d "$BUILD/classes" -cp "$CP" "$HERE/GameVerify.java" >&2
fi

# Make the blueprint path and the --blocks file absolute: the JVM runs in a scratch directory.
n=$#
i=0
prev=""
for a in "$@"; do
    i=$((i + 1))
    if [ "$prev" = "--blocks" ] || [ "$i" -eq "$n" ]; then
        case "$a" in /* | -*) ;; *) a="$START/$a" ;; esac
    fi
    prev="$a"
    set -- "$@" "$a"
    shift
done

# The game resolves ./data/config and ./bbtmp from the working directory and opens region
# files read-write, so it gets a throwaway directory with a link to the game data.
WORK=$(mktemp -d "$BUILD/work.XXXXXX")
cleanup() {
    if [ "${GAMEVERIFY_KEEP:-0}" = "1" ]; then echo "game-verify: kept $WORK" >&2; else rm -rf "$WORK"; fi
}
trap cleanup EXIT
ln -s "$STARMADE_DIR/data" "$WORK/data"
[ -f "$STARMADE_DIR/version.txt" ] && ln -s "$STARMADE_DIR/version.txt" "$WORK/version.txt"
LOG="$BUILD/last-run.log"

cd "$WORK"
set +e
if [ "${GAMEVERIFY_ASSERT:-1}" = "0" ]; then EA="-da"; else EA="-ea:org.schema.game.common.data.world.RemoteSegment"; fi
"$JAVA" -Xmx"${GAMEVERIFY_XMX:-2g}" -Djava.awt.headless=true -Dgameverify.log="$LOG" \
    "$EA" \
    -cp "$BUILD/classes:$CP" GameVerify "$@"
STATUS=$?
set -e
if [ "$STATUS" -ne 0 ] && [ "$STATUS" -ne 1 ] && [ "$STATUS" -ne 2 ] && [ "$STATUS" -ne 64 ]; then
    echo "game-verify: JVM exited with $STATUS; game output is in $LOG" >&2
fi
exit "$STATUS"
