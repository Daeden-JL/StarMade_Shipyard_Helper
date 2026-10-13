#!/bin/sh
# package.sh <yard-server binary> <os-arch label> <built client dir> <out dir>
#
# Makes shipyard-<version>-<label>.tar.gz in <out dir>: the server binary, the
# built client under web/dist, a start script and a short README. The version
# is the workspace version in Cargo.toml.
set -eu
bin=$1; label=$2; dist=$3; out=$4
here=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -n 1)
name="shipyard-$version-$label"
stage=$(mktemp -d)
mkdir -p "$stage/$name/web" "$out"
cp "$bin" "$stage/$name/yard-server"
chmod +x "$stage/$name/yard-server"
cp -R "$dist" "$stage/$name/web/dist"
cat > "$stage/$name/start.sh" <<'SH'
#!/bin/sh
# Starts the web shipyard from this folder.
#   ./start.sh /path/to/StarMade            serves http://127.0.0.1:4280 to this machine
#   ./start.sh /path/to/StarMade 0.0.0.0:4280   serves the whole network
# The first argument is the game install: the folder holding data/ and version.txt.
cd "$(dirname "$0")"
export YARD_STARMADE_DIR="${1:-${YARD_STARMADE_DIR:-starmade}}"
export YARD_BIND="${2:-${YARD_BIND:-127.0.0.1:4280}}"
export YARD_STATIC_DIR=web/dist
export YARD_DATA_DIR="${YARD_DATA_DIR:-data}"
if [ ! -f "$YARD_STARMADE_DIR/version.txt" ]; then
  echo "No StarMade install at '$YARD_STARMADE_DIR'. Give the folder that holds data/ and version.txt." >&2
  exit 1
fi
exec ./yard-server
SH
chmod +x "$stage/$name/start.sh"
cat > "$stage/$name/README.txt" <<TXT
StarMade Shipyard Helper: web shipyard $version ($label)

    ./start.sh /path/to/StarMade

then open http://127.0.0.1:4280. The path is the game install that holds
data/ and version.txt; the shipyard reads block definitions, textures and
models from it. Designs are kept in data/ next to this file.

To serve other machines: ./start.sh /path/to/StarMade 0.0.0.0:4280
To let a game server sync its blueprints with it, install the mod jar on
that server and set sync.url in the mod's config to this machine's address.

Settings (environment variables), the chat assistant and the sync rules:
https://github.com/Daeden-JL/StarMade_Shipyard_Helper/blob/main/rust/README.md
TXT
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
rm -rf "$stage"
ls -l "$out/$name.tar.gz"
