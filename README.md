![Hematite](docs/header.png)

**Faster world generation for Paper. Same world, block for block.**

Hematite swaps Minecraft's terrain generator for one written in Rust. It builds exactly the blocks vanilla would, just much faster, and it generates the land fast players are heading into before they get there. Elytra and trident flights stop running into the edge of the world.

![Flying over freshly generated terrain](docs/demo.gif)

## Why Hematite

- **Built for speed.** With two players flying at 200 blocks/s, a new chunk is ready in 2 ms on median instead of 3.9 seconds.
- **Your world stays your world.** Terrain, biomes, caves, aquifers and ores match Mojang's generator block for block. Keep your seed and your existing world: new chunks line up with old ones without seams.
- **One jar, one line of config.** No datapacks, no pregeneration, and nothing to configure in Hematite itself.
- **Safe by default.** Before it takes over, Hematite generates its first chunks, biomes and ore veins both ways and compares them with vanilla on your server. Anything that doesn't match stays vanilla.
- **Looks ahead.** For anyone moving faster than 8 blocks per second, Hematite generates the chunks they'll see in the next 5 seconds.
- **Distant Horizons to the horizon.** Fill the DH Support plugin's database with tens of thousands of distant LODs in minutes, without generating a single real chunk.
- **Live stats.** An admin action bar shows chunks per second, time per chunk, chunks missing around you, MSPT and your speed.

## Performance

Time from a chunk coming into view until it's ready, measured with Hematite's built-in `/hematite fly` benchmark on a 15-core Mac (view distance 16, 12 chunk threads, look-ahead on in both columns):

| Players flying | Vanilla terrain | Hematite |
| --- | --- | --- |
| 2 at 200 blocks/s (elytra and trident), median / 95th percentile | 3,900 / 8,500 ms | 2 / 86 ms |
| 4 at 67 blocks/s, median / 95th percentile | 21–28 / 135–168 ms | 18–20 / 62–70 ms |

Look-ahead alone takes a single player flying 33 blocks/s from 100–160 ms of waiting per chunk to 5–9 ms.

Where the time goes:

- **Terrain:** the Rust engine generates a chunk's terrain in 1.7 ms on one core.
- **Ores and biomes:** ore veins run vanilla's algorithm 1.6x faster, and Rust biomes add about 10% more chunks per second at full load.
- **Limit:** with 3 or more players at 200 blocks/s and view distance 16, the test Mac falls behind either way. Hematite's chunks then arrive 1 to 2 seconds late on median.

## Get started

Needs Paper 26.3 and Java 25 or newer. One jar runs on Linux (x86_64 and ARM), Windows (x86_64) and macOS (Apple Silicon and Intel).

1. Put `hematite.jar` in your `plugins` folder.
2. Make Hematite the overworld generator in `bukkit.yml`:

   ```yaml
   worlds:
     world:
       generator: Hematite
   ```

3. Restart. The console confirms each part as it takes over:

   ```
   Hematite enabled: Rust now generates noise, surface and caves
   Rust now also generates biomes
   faster ore veins for 30 ore features
   ```

That's it.

### For fast flyers

Paper limits each player to 75 new chunks per second by default and uses few threads for chunks. Lift both in `config/paper-global.yml`:

```yaml
chunk-loading-basic:
  player-max-chunk-load-rate: -1.0
  player-max-chunk-send-rate: -1.0
chunk-system:
  worker-threads: 6
```

At elytra and trident speeds, the server's movement checks pull players back with "moved too quickly". Turn them off with `/gamerule minecraft:elytra_movement_check false` and `/gamerule minecraft:player_movement_check false`, or raise `moved-too-quickly-multiplier` in `spigot.yml`.

Optional Java flags:

- `--enable-native-access=ALL-UNNAMED` hides a warning about native code.
- On Java 26 and later, `--enable-final-field-mutation=ALL-UNNAMED` hides a warning about handing biomes to Rust. Java 25 doesn't know this option and won't start with it.

## Commands

| Command | What it does |
| --- | --- |
| `/hematite` | Turns the action bar on or off and shows totals |
| `/hematite fly <blocks/s> <seconds> [view distance] [players] [on/off]` | Simulates players flying over new terrain, with look-ahead on or off, and reports how fast chunks were ready |

Both need `hematite.admin`, which ops have by default.

## Good to know

- **What Hematite does:** biomes, terrain shape, surface and caves, including the ground vanilla raises or carves around villages, ancient cities and other structures. Ore veins use vanilla's own algorithm with the same random numbers, minus its overhead. Structures, trees, plants, mobs and light stay vanilla.
- **Which worlds:** Hematite only runs in worlds you set it as the generator for, and only in the overworld. If a worldgen datapack or a different Minecraft version changes generation, the startup check notices and Hematite stays off.
- **Paper's rounding:** Paper rounds a few noise values differently from Mojang's code. So a few chunks differ from a Paper server without Hematite by a handful of blocks, as do about 1 in 2,500 biome cells (4×4×4 blocks) on biome borders.
- **Verify mode:** start the server with `-Dhematite.verify=true` to compare every chunk, biome and ore vein with vanilla. It's slow and meant for testing.

## Distant Horizons

[DH Support](https://gitlab.com/distant-horizons-team/distant-horizons-server-plugin) sends distant terrain to players running Distant Horizons. Normally it loads or generates the 16 real chunks behind each distant section, which is heavy. With Hematite's command-line tool and `tools/dhs_config.py` from this repository (see Building), you can fill its database up front and switch that off:

```sh
# in the server folder; once: put the DH Support jar in plugins, start the server, stop it
python3 tools/dhs_config.py plugins/DHSupport/config.yml   # serve stored LODs only
HEMATITE_JAR=versions/26.3/paper-26.3.jar hematite export-dhs <seed> world plugins/DHSupport/data.sqlite 512
```

- **When to run it:** with the server stopped. `HEMATITE_JAR` points the tool at the game data, and Paper's own server jar works. `/seed` shows the seed.
- **Size and time:** the radius is in chunks around 0,0. 512 chunks (8,192 blocks in every direction) takes about 5 minutes and 530 MB.
- **What's left out:** distant terrain has trees but no structures, and block changes don't update other players' distant terrain.

## Command-line tool

Hematite also works on its own, for example to give a single-player world a huge Distant Horizons render distance:

```sh
cargo build --release

# new world in the Shooting star Modrinth profile, plus LODs for a radius of 1024 chunks
tools/make_world.sh "My World" 8675309 1024

# or step by step
python3 tools/new_world.py "<saves>/ende" "<saves>/horizon" "Horizon" <seed> $(./target/release/hematite spawn <seed>)
./target/release/hematite export <seed> "<saves>/horizon/dimensions/minecraft/overworld/data/DistantHorizons.sqlite" 1024

# top-down map of 12x12 sections from section -6,-6, and a map decoded from an existing DH database
./target/release/hematite render <seed> map.ppm -6 -6 12
./target/release/hematite preview DistantHorizons.sqlite 3 map.ppm
```

- **Game data:** the tool reads worldgen data from the game jar in `HEMATITE_JAR`, any 26.3 client or server jar. By default it uses the Modrinth app's 26.3 install on macOS.
- **Speed:** it exports about 5,500 chunks per second on a 15-core Mac (radius 1024 in roughly 13 minutes).
- **Smaller files:** underground detail hidden below the surface is left out, which makes files about 5x smaller. Set `NO_CULL=1` to keep it.
- **What the LODs show:** trees use vanilla's types and shapes in different positions, snow and ice follow vanilla's rules on top of them, and light is sky light straight down. Structures, ores, lakes and small plants are left out.

## Building and verifying

```sh
cargo build --release   # command-line tool: target/release/hematite
paper/build.sh          # plugin with the engine for this machine: paper/run/plugins/hematite.jar (downloads Paper the first time)
paper/build.sh --all    # plugin with engines for every platform, for release (needs zig and cargo-zigbuild)
paper/start.sh          # test server in paper/run with Hematite
```

Every push to `main` builds the plugin for all platforms on GitHub Actions and, once `MODRINTH_TOKEN` and `MODRINTH_PROJECT_ID` are set up, uploads it to Modrinth as an alpha. Tags like `v1.2.0` upload a release.

Worldgen data (density functions, noises, biomes, surface rules, carvers, trees) is read from the game jar at runtime, so nothing is hardcoded for a version except the algorithms.

```sh
tools/verify/check.sh <seed> 100                   # 100 random chunks, block by block against Mojang's generator
tools/verify/check.sh <seed> 100 structures        # the same for chunks next to structures
tools/verify/same_db.sh a.sqlite b.sqlite          # two exports are identical
tools/verify/check_dhs.sh <DHSupport/data.sqlite>  # DH Support rows decode with Distant Horizons' own code
tools/verify/platforms.sh                          # Linux and Windows builds generate byte-identical chunks (Docker, Wine)
```

These need a JDK and the 26.3 install from the Modrinth app. The other files in `tools/verify` check density values, biomes, temperatures and DH decoding the same way.

## License

MIT, see [LICENSE](LICENSE). The Distant Horizons database scripts in `data/dh_sql` are LGPL-3.0, see [their notice](data/dh_sql/NOTICE).
