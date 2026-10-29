#!/bin/sh
# Builds the Rust engine and the plugin into run/plugins/hematite.jar.
#   paper/build.sh         engine for this machine only
#   paper/build.sh --all   engines for macOS, Linux and Windows (needs zig and cargo-zigbuild)
# The first run downloads Paper into run/ (its libraries are the compile classpath).
set -e
cd "$(dirname "$0")"
PAPER_BUILD=230
if [ ! -f run/paper.jar ]; then
	mkdir -p run
	read -r url sha <<EOF
$(curl -sf "https://fill.papermc.io/v3/projects/paper/versions/26.3/builds/$PAPER_BUILD" | python3 -c "import json,sys; d=json.load(sys.stdin)['downloads']['server:default']; print(d['url'], d['checksums']['sha256'])")
EOF
	curl -sfL -o run/paper.jar "$url"
	echo "$sha  run/paper.jar" | shasum -a 256 -c -
	(cd run && java -Dpaperclip.patchonly=true -jar paper.jar)
fi

rm -rf build && mkdir -p build/classes run/plugins

# engine <rust target> <natives dir> <library file> [cpu]: a baseline CPU, not this machine's (.cargo/config.toml)
engine() {
	case $1 in *apple*) cargo=cargo ;; *) cargo=cargo-zigbuild ;; esac
	RUSTFLAGS="${4:+-C target-cpu=$4}" $cargo rustc -q --manifest-path ../Cargo.toml --lib --profile plugin --crate-type cdylib --target "$1"
	mkdir -p "build/classes/natives/$2"
	cp "../target/${1%.2.17}/plugin/$3" "build/classes/natives/$2/"
	echo "engine: $2"
}
mac_arm() { engine aarch64-apple-darwin macos-aarch64 libhematite.dylib apple-m1; }
mac_x86() { engine x86_64-apple-darwin macos-x86_64 libhematite.dylib x86-64-v2; }
linux_x86() { engine x86_64-unknown-linux-gnu.2.17 linux-x86_64 libhematite.so x86-64-v2; } # glibc 2.17: distros since about 2013
linux_arm() { engine aarch64-unknown-linux-gnu.2.17 linux-aarch64 libhematite.so; }
windows_x86() { engine x86_64-pc-windows-gnu windows-x86_64 hematite.dll x86-64-v2; }
if [ "$1" = --all ]; then
	mac_arm; mac_x86; linux_x86; linux_arm; windows_x86
else
	case "$(uname -s)-$(uname -m)" in
		Darwin-arm64) mac_arm ;;
		Darwin-x86_64) mac_x86 ;;
		Linux-x86_64) linux_x86 ;;
		Linux-aarch64) linux_arm ;;
		*) windows_x86 ;;
	esac
fi

CP=$(find run/libraries run/versions -name '*.jar' | tr '\n' ':')
javac --release 25 -cp "$CP" -d build/classes src/dev/hematite/*.java
# VERSION=1.2.0 paper/build.sh stamps that version into the plugin (CI does)
if [ -n "$VERSION" ]; then sed "s/^version: .*/version: $VERSION/" src/plugin.yml > build/classes/plugin.yml; else cp src/plugin.yml build/classes/; fi
jar cf run/plugins/hematite.jar.tmp -C build/classes . && mv run/plugins/hematite.jar.tmp run/plugins/hematite.jar # a running test server keeps the old file
echo "built run/plugins/hematite.jar"
