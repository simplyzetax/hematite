#!/bin/sh
# Decode the LODs in a DH Support database with Distant Horizons' own client code and validate every data point.
# usage: tools/verify/check_dhs.sh <plugins/DHSupport/data.sqlite> [row count]
set -e
cd "$(dirname "$0")/../.."
DH="$HOME/Library/Application Support/ModrinthApp/profiles/Shooting star/mods/DistantHorizons-3.3.4-26.3-fabric-neoforge.jar"
OUT=target/verify/dhs
rm -rf "$OUT" && mkdir -p "$OUT"
python3 -I - "$1" "$OUT" "${2:-300}" <<'EOF'
import sqlite3, sys
for x, z, d in sqlite3.connect(sys.argv[1]).execute("select x, z, data from lods order by random() limit ?", (int(sys.argv[3]),)):
    open(f"{sys.argv[2]}/{x}_{z}.bin", "wb").write(d)
EOF
CP="$DH:$(find paper/run/libraries -name '*.jar' | tr '\n' ':')"
java --enable-native-access=ALL-UNNAMED -cp "$CP" tools/verify/CheckDh.java "$OUT"/*.bin | grep -v '^   ' > "$OUT/log"
n=$(wc -l < "$OUT/log" | tr -d ' ')
good=$(grep -c 'cols=4096 .* bad=0 .* leftover=0 ' "$OUT/log" || true)
echo "$good of $n LODs decode to full sections with valid data points"
[ "$good" = "$n" ] && [ "$n" -gt 0 ]
