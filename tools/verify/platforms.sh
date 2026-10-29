#!/bin/sh
# Check that Linux (x86_64, ARM) and Windows builds generate byte-identical chunks to this machine's build.
# Runs them in Docker, Windows under Wine. Needs zig, cargo-zigbuild and Docker.
# usage: tools/verify/platforms.sh [seed]
set -e
cd "$(dirname "$0")/../.."
OUT=target/verify/platforms
mkdir -p "$OUT"
SEED=${1:-12345}
JAR=${HEMATITE_JAR:-$HOME/Library/Application Support/ModrinthApp/meta/versions/26.3-0.20.1/26.3-0.20.1.jar}
python3 -c "import random; r = random.Random(7); print(' '.join(f'{r.randint(-5000, 5000)} {r.randint(-5000, 5000)}' for _ in range(100)))" > "$OUT/coords"

cargo build -q --release
xargs ./target/release/hematite dump-chunks "$SEED" "$OUT/here.bin" < "$OUT/coords" > /dev/null 2>&1
RUSTFLAGS="-C target-cpu=x86-64-v2" cargo-zigbuild build -q --release --bin hematite --target x86_64-unknown-linux-gnu.2.17
RUSTFLAGS="" cargo-zigbuild build -q --release --bin hematite --target aarch64-unknown-linux-gnu.2.17
RUSTFLAGS="-C target-cpu=x86-64-v2" cargo-zigbuild build -q --release --bin hematite --target x86_64-pc-windows-gnu

run() { # <docker platform> <script>
	docker run --rm --platform "linux/$1" -e SEED="$SEED" -e HEMATITE_JAR=/jar/game.jar -v "$JAR:/jar/game.jar:ro" -v "$PWD/target:/t:ro" -v "$PWD/$OUT:/out" debian:trixie-slim sh -c "$2"
}
run arm64 '/t/aarch64-unknown-linux-gnu/release/hematite dump-chunks $SEED /out/linux-aarch64.bin $(cat /out/coords) > /dev/null 2>&1'
run amd64 '/t/x86_64-unknown-linux-gnu/release/hematite dump-chunks $SEED /out/linux-x86_64.bin $(cat /out/coords) > /dev/null 2>&1'
run amd64 'apt-get update -qq > /dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends wine64 > /dev/null 2>&1
	W=$(command -v wine64 || command -v wine || echo /usr/lib/wine/wine64)
	HEMATITE_JAR="Z:\jar\game.jar" WINEDEBUG=-all $W /t/x86_64-pc-windows-gnu/release/hematite.exe dump-chunks $SEED "Z:\out\windows-x86_64.bin" $(cat /out/coords) > /dev/null 2>&1'

fail=0
for p in linux-aarch64 linux-x86_64 windows-x86_64; do
	if cmp -s "$OUT/here.bin" "$OUT/$p.bin"; then echo "$p: identical"; else echo "$p: DIFFERS"; fail=1; fi
done
exit $fail
