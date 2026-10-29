#!/bin/sh
# Exit non-zero unless two DH databases hold identical LOD rows (timestamps ignored).
# usage: tools/verify/same_db.sh <a.sqlite> <b.sqlite>
cols="DetailLevel, PosX, PosZ, MinY, DataChecksum, Data, ColumnGenerationStep, ColumnWorldCompressionMode, Mapping, NorthAdjData, SouthAdjData, EastAdjData, WestAdjData"
n=$(sqlite3 "$1" "ATTACH '$2' AS b; SELECT (SELECT count(*) FROM main.FullData), (SELECT count(*) FROM b.FullData), (SELECT count(*) FROM (SELECT $cols FROM main.FullData EXCEPT SELECT $cols FROM b.FullData));")
echo "rows a|b|differing: $n"
[ "${n##*|}" = 0 ] && [ "$(echo "$n" | cut -d'|' -f1)" = "$(echo "$n" | cut -d'|' -f2)" ]
