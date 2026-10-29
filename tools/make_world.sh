#!/bin/sh
# Create a world in the Shooting star profile with Distant Horizons LODs pregenerated.
# usage: tools/make_world.sh <world name> <seed or text> [radius in chunks, default 1024]
set -e
cd "$(dirname "$0")/.."
SAVES=${SAVES:-"$HOME/Library/Application Support/ModrinthApp/profiles/Shooting star/saves"}
NAME=$1
SEED=$(python3 -I -c '
import re, struct, sys
s = sys.argv[1].strip()
if re.fullmatch(r"[+-]?\d+", s) and -2**63 <= int(s) < 2**63:
	print(int(s))
else:
	b = s.encode("utf-16-be")
	h = 0
	for c in struct.unpack(">%dH" % (len(b) // 2), b):
		h = (h * 31 + c) & 0xFFFFFFFF
	print(h - (1 << 32) if h >= 1 << 31 else h)
' "$2")
DIR="$SAVES/$(printf %s "$NAME" | tr -c 'A-Za-z0-9_-' _)"
cargo build --release -q
python3 -I tools/new_world.py "$SAVES/ende" "$DIR" "$NAME" "$SEED" $(./target/release/hematite spawn "$SEED")
mkdir -p "$DIR/dimensions/minecraft/overworld/data"
./target/release/hematite export "$SEED" "$DIR/dimensions/minecraft/overworld/data/DistantHorizons.sqlite" "${3:-1024}"
echo "done: \"$NAME\" (seed $SEED) in $DIR"
