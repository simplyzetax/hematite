#!/bin/sh
# Compare Rust chunks block-by-block with Mojang's generator (noise, biomes, surface, carvers).
# usage: tools/verify/check.sh <seed> [chunk count]
set -e
cd "$(dirname "$0")/../.."
M="$HOME/Library/Application Support/ModrinthApp/meta"
V=26.3-0.20.1
OUT=target/verify
mkdir -p "$OUT"
CP=$(python3 -c "
import json, sys
j = json.load(open(sys.argv[1]))
libs = [sys.argv[2] + '/libraries/' + l['downloads']['artifact']['path'] for l in j['libraries'] if 'artifact' in l.get('downloads', {})]
print(':'.join(libs + [sys.argv[3]]))" "$M/versions/$V/$V.json" "$M" "$M/versions/$V/$V.jar")
javac -cp "$CP" -d "$OUT" tools/verify/DumpChunks.java 2>&1 | grep -v '^Note' || true
cargo build --release -q
COORDS=$(python3 -c "import random; r = random.Random(1); print(' '.join(f'{r.randint(-3000, 3000)} {r.randint(-3000, 3000)}' for _ in range(${2:-100})))")
java -cp "$CP:$OUT" DumpChunks "$1" "$OUT/java_chunks.bin" $COORDS > "$OUT/java.log" 2>&1
./target/release/hematite dump-chunks "$1" "$OUT/rust_chunks.bin" $COORDS 2>/dev/null
python3 tools/verify/cmp_chunks.py "$OUT"
