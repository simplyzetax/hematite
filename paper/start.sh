#!/bin/sh
# Starts the test server in run/ with Hematite as the overworld generator. Extra args go to java,
# e.g. ./start.sh -Dhematite.verify=true to compare every chunk with vanilla.
set -e
cd "$(dirname "$0")/run"
[ -f bukkit.yml ] || printf 'worlds:\n  world:\n    generator: Hematite\n' > bukkit.yml
exec java -Xms2G -Xmx4G --enable-native-access=ALL-UNNAMED "$@" -jar paper.jar --nogui
