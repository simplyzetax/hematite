package dev.hematite;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_BYTE;
import static java.lang.foreign.ValueLayout.JAVA_FLOAT;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;
import static java.lang.foreign.ValueLayout.JAVA_SHORT;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandle;
import java.net.JarURLConnection;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Optional;
import java.util.stream.LongStream;
import java.util.HashSet;
import java.util.Map;
import java.util.Random;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.LongAdder;
import java.util.logging.Level;
import java.util.logging.Logger;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.TextColor;
import net.kyori.adventure.text.format.TextDecoration;
import org.bukkit.Bukkit;
import org.bukkit.Chunk;
import org.bukkit.Location;
import org.bukkit.Material;
import org.bukkit.World;
import org.bukkit.block.data.BlockData;
import org.bukkit.craftbukkit.CraftWorld;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerRO;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.util.Mth;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.BulkSectionAccess;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.BlockReplacement;
import net.minecraft.world.level.levelgen.feature.OreFeature;
import net.minecraft.world.level.levelgen.structure.templatesystem.AllOfRuleTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.AnyOfRuleTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.BlockMatchTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.BlockStateMatchTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.NotRuleTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.RuleTest;
import net.minecraft.world.level.levelgen.structure.templatesystem.TagMatchTest;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.levelgen.Heightmap;
import org.bukkit.craftbukkit.block.data.CraftBlockData;
import org.bukkit.craftbukkit.generator.CraftChunkData;
import it.unimi.dsi.fastutil.ints.IntArrayList;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.TerrainAdjustment;
import net.minecraft.world.level.levelgen.structure.pools.JigsawJunction;
import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.entity.Player;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;
import org.bukkit.plugin.java.JavaPlugin;

/** Enable per world in bukkit.yml: worlds: <name>: generator: Hematite */
public final class HematitePlugin extends JavaPlugin {
	Generator gen;
	final Ahead ahead = new Ahead();
	final Set<UUID> hidden = new HashSet<>();
	final Map<UUID, Location> lastPos = new HashMap<>(), recentPos = new HashMap<>();
	long lastTime = System.nanoTime(), lastRust, lastNanos, lastVanilla;
	double lastMs;

	@Override
	public ChunkGenerator getDefaultWorldGenerator(String worldName, String id) {
		return gen = new Generator(getLogger(), getDataFolder().toPath());
	}

	@Override
	public void onEnable() {
		getServer().getScheduler().runTaskTimer(this, this::hud, 20, 20);
		getServer().getScheduler().runTaskTimer(this, this::lookAhead, 5, 5);
		var paper = io.papermc.paper.configuration.GlobalConfiguration.get();
		if (paper.chunkSystem.workerThreads <= 0 || paper.chunkLoadingBasic.playerMaxChunkSendRate > 0) {
			getLogger().info("For fast flying, set chunk-system.worker-threads (Paper's default is low: 3 threads on a 15-core Mac) and "
				+ "chunk-loading-basic.player-max-chunk-send-rate / player-max-chunk-load-rate (default 75 / 100, -1 = no limit) in config/paper-global.yml");
		}
	}

	/** Every quarter second: start generating what fast players are about to see. */
	void lookAhead() {
		for (Player p : getServer().getOnlinePlayers()) {
			Location at = p.getLocation(), before = recentPos.put(p.getUniqueId(), at);
			if (before == null || before.getWorld() != at.getWorld()) continue;
			ahead.predict(at.getWorld(), at.getX(), at.getZ(), (at.getX() - before.getX()) * 4, (at.getZ() - before.getZ()) * 4, viewDistance(p));
		}
	}

	static int viewDistance(Player p) {
		return Math.max(2, Math.min(p.getViewDistance(), Math.min(p.getSendViewDistance(), p.getClientViewDistance())));
	}

	/** Chunks the player should see that haven't reached their client yet. */
	static int missing(Player p) {
		int r = viewDistance(p), cx = p.getLocation().getBlockX() >> 4, cz = p.getLocation().getBlockZ() >> 4, n = 0;
		for (int x = cx - r - 2; x <= cx + r + 2; x++) {
			for (int z = cz - r - 2; z <= cz + r + 2; z++) {
				if (Ahead.inView(cx, cz, r, x, z) && !p.isChunkSent(Chunk.getChunkKey(x, z))) n++;
			}
		}
		return n;
	}

	/** Action bar for players with hematite.admin (ops by default), once a second. */
	void hud() {
		if (gen == null) return;
		long now = System.nanoTime(), rust = gen.rustChunks.sum(), nanos = gen.rustNanos.sum() + gen.writeNanos.sum(), vanilla = gen.vanillaChunks.sum();
		double secs = (now - lastTime) / 1e9, rate = (rust - lastRust + vanilla - lastVanilla) / secs;
		if (rust > lastRust) lastMs = (nanos - lastNanos) / 1e6 / (rust - lastRust);
		lastTime = now;
		lastRust = rust;
		lastNanos = nanos;
		lastVanilla = vanilla;
		double mspt = getServer().getAverageTickTime();
		Component sep = Component.text("  \u00b7  ", NamedTextColor.DARK_GRAY);
		for (Player p : getServer().getOnlinePlayers()) {
			Location at = p.getLocation(), before = lastPos.put(p.getUniqueId(), at);
			if (!p.hasPermission("hematite.admin") || hidden.contains(p.getUniqueId())) continue;
			double speed = before != null && before.getWorld() == at.getWorld() ? before.distance(at) / secs : 0;
			Component line = Component.empty().append(Component.text("Hematite \u25b8 ", NamedTextColor.GOLD, TextDecoration.BOLD));
			if (gen.enabled != Boolean.TRUE) line = line.append(gen.status()).append(sep);
			line = line.append(value(String.format("%.0f", rate), " chunks/s", NamedTextColor.WHITE));
			if (gen.enabled == Boolean.TRUE) line = line.append(sep).append(value(String.format("%.1f", lastMs), " ms each", NamedTextColor.WHITE));
			int missing = missing(p);
			line = line.append(sep).append(value(String.valueOf(missing), " missing", missing == 0 ? NamedTextColor.GREEN : missing < 20 ? NamedTextColor.YELLOW : NamedTextColor.RED))
				.append(sep).append(value(String.format("%.1f", mspt), " MSPT", mspt < 25 ? NamedTextColor.GREEN : mspt < 45 ? NamedTextColor.YELLOW : NamedTextColor.RED))
				.append(sep).append(value(String.format("%.0f", speed), " b/s", NamedTextColor.AQUA));
			p.sendActionBar(line);
		}
	}

	static Component value(String v, String unit, TextColor color) {
		return Component.text(v, color).append(Component.text(unit, NamedTextColor.GRAY));
	}

	@Override
	public void onDisable() {
		if (gen != null) getLogger().info(gen.summary());
	}

	@Override
	public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
		if (args.length > 0 && args[0].equals("fly")) {
			try {
				World w = getServer().getWorlds().getFirst();
				new Fly(this, w, sender, args.length > 5 && args[5].equals("off") ? null : ahead,
					args.length > 1 ? Double.parseDouble(args[1]) : 33, args.length > 2 ? Integer.parseInt(args[2]) : 30,
					args.length > 3 ? Integer.parseInt(args[3]) : w.getViewDistance(), args.length > 4 ? Integer.parseInt(args[4]) : 1);
				return true;
			} catch (NumberFormatException e) {
				return false;
			}
		}
		if (sender instanceof Player p && !hidden.remove(p.getUniqueId())) hidden.add(p.getUniqueId());
		if (sender instanceof Player p) sender.sendMessage(Component.text("Hematite HUD " + (hidden.contains(p.getUniqueId()) ? "off" : "on"), NamedTextColor.GOLD));
		if (gen != null) sender.sendMessage(Component.text(gen.summary(), NamedTextColor.GRAY));
		return true;
	}
}

/**
 * Rust (Hematite) generates noise, surface and caves, including the terrain vanilla raises or carves around
 * structures; vanilla still does biomes, structures, features, mobs and light.
 * The first chunks are probes: vanilla generates them and Hematite's output is compared block by block. Hematite only
 * takes over if they match, so a different game version or a worldgen datapack falls back to vanilla.
 * Start the server with -Dhematite.verify=true to compare every chunk instead (vanilla generates all of them).
 */
final class Generator extends ChunkGenerator {
	static final int SIZE = 16 * 16 * 384;
	static final int PROBES = 8;
	/** Paper's own noise code (Moonrise) rounds slightly differently from Mojang's, which Hematite matches, so a few
	 *  blocks right at the solid/air threshold can differ. A wrong game version or a worldgen datapack differs in
	 *  thousands of blocks. */
	static final int CLOSE = 64;

	enum Mode { RUST, PROBE, VANILLA }

	/** Per worker thread: the chunk being generated, how, and Hematite's blocks for it. Paper runs a chunk's noise,
	 *  surface and caves steps back to back on one thread. */
	static final class Job {
		long key;
		Mode mode;
		int[] beard; // structure pieces that reshape this chunk, or null
		final short[] ids = new short[SIZE];
		int top; // sections from the bottom that hold anything but air
		// pack(): one section's palette, by Hematite id: the stamp of the section that used it last, its palette index
		int stamp;
		int[] seen = new int[0], slot = new int[0];
		final int[] palette = new int[256];
		final byte[] local = new byte[4096];
	}

	final Logger log;
	final Path dir;
	final boolean verifyAll = Boolean.getBoolean("hematite.verify");
	final ThreadLocal<Job> job = ThreadLocal.withInitial(Job::new);
	final AtomicInteger probesStarted = new AtomicInteger(), probesDone = new AtomicInteger(), probesExact = new AtomicInteger(), probesClose = new AtomicInteger();
	final AtomicInteger structureProbes = new AtomicInteger(), structureExact = new AtomicInteger(), structureClose = new AtomicInteger();
	volatile Boolean enabled; // null while probing
	final LongAdder rustChunks = new LongAdder(), rustNanos = new LongAdder(), writeNanos = new LongAdder(), vanillaChunks = new LongAdder(), structureChunks = new LongAdder();
	volatile Native lib;
	boolean failed;
	/** Paper internals that pack() uses; if they are missing, or pack() builds a wrong section, blocks are set one by one. */
	static final java.lang.reflect.Method CONFIGURATION;
	static final java.lang.reflect.Field PRESETS;
	static {
		java.lang.reflect.Method m = null;
		java.lang.reflect.Field f = null;
		try {
			m = Strategy.class.getDeclaredMethod("getConfigurationForPaletteSize", int.class);
			m.setAccessible(true);
			f = PalettedContainer.class.getDeclaredField("presetValues");
			f.setAccessible(true);
		} catch (ReflectiveOperationException | RuntimeException e) {
			m = null;
		}
		CONFIGURATION = m;
		PRESETS = f;
	}
	volatile boolean packing = CONFIGURATION != null;
	final java.util.concurrent.atomic.AtomicBoolean takeoverTried = new java.util.concurrent.atomic.AtomicBoolean();
	volatile BiomeSource vanillaBiomes; // set once Rust generates biomes
	final LongAdder biomesChecked = new LongAdder(), biomesDiffer = new LongAdder(); // verify mode, in quarts
	final AtomicInteger packChecks = new AtomicInteger(64); // sections still compared block by block after packing
	final int[] storageBits = new int[257]; // by palette size, + 1; 0 = not looked up yet

	Generator(Logger log, Path dir) {
		this.log = log;
		this.dir = dir;
	}

	@Override
	public boolean shouldGenerateNoise(WorldInfo w, Random r, int x, int z) {
		Job j = job.get();
		j.key = key(x, z);
		j.mode = mode(w, x, z, j);
		if (j.mode != Mode.VANILLA) {
			boolean ok;
			long t = System.nanoTime();
			try {
				j.top = lib.generate(x, z, j.ids, j.beard);
				ok = j.top >= 0;
			} catch (Throwable e) {
				ok = false;
			}
			if (j.mode == Mode.RUST) rustNanos.add(System.nanoTime() - t);
			if (!ok) {
				log.warning("Hematite failed on chunk " + x + "," + z + ", using vanilla for it");
				if (j.mode == Mode.PROBE) record(x, z, -1, "");
				j.mode = Mode.VANILLA;
			}
		}
		if (j.mode != Mode.RUST) vanillaChunks.increment();
		return j.mode != Mode.RUST;
	}

	@Override
	public boolean shouldGenerateSurface(WorldInfo w, Random r, int x, int z) {
		return current(x, z) != Mode.RUST;
	}

	@Override
	public boolean shouldGenerateCaves(WorldInfo w, Random r, int x, int z) {
		return current(x, z) != Mode.RUST;
	}

	@Override
	public void generateNoise(WorldInfo w, Random r, int x, int z, ChunkData data) {
		if (current(x, z) != Mode.RUST) return;
		long t = System.nanoTime();
		Job j = job.get();
		short[] ids = j.ids;
		BlockState[] states = lib.states;
		// Straight into the sections: ChunkData.setBlock takes a lock and updates two heightmaps per block.
		ChunkAccess chunk = ((CraftChunkData) data).getHandle();
		LevelChunkSection[] sections = chunk.getSections();
		for (int s = 0; s < j.top; s++) {
			LevelChunkSection section = sections[s];
			if (packing && pack(section, j, s)) continue;
			for (int bz = 0; bz < 16; bz++) {
				for (int bx = 0; bx < 16; bx++) {
					int base = (bz * 16 + bx) * 384 + s * 16;
					for (int y = 0; y < 16; y++) {
						int id = ids[base + y];
						if (id != 0) section.setBlockState(bx, y, bz, states[id], false);
					}
				}
			}
		}
		// same result as updating them block by block: the top block that counts, plus one
		Heightmap.primeHeightmaps(chunk, chunk.getPersistedStatus().heightmapsAfter());
		writeNanos.add(System.nanoTime() - t);
		rustChunks.increment();
	}

	/**
	 * Once Hematite is on: hands vanilla's chunk biome step Hematite's biomes, if they match vanilla's for a sample of
	 * chunks. The generator's biome source is a final field, so this needs deep reflection; if the JVM refuses, vanilla
	 * keeps generating biomes.
	 */
	void useRustBiomes(ServerLevel level) {
		try {
			net.minecraft.world.level.chunk.ChunkGenerator g = level.getChunkSource().getGenerator();
			BiomeSource vanilla = g.getBiomeSource();
			var registry = level.registryAccess().lookupOrThrow(Registries.BIOME);
			@SuppressWarnings("unchecked")
			Holder<Biome>[] holders = new Holder[lib.biomeNames.length];
			for (int i = 0; i < holders.length; i++) holders[i] = registry.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(lib.biomeNames[i])));
			BiomeResolver v = vanilla.createUncachedResolver(level.getChunkSource().randomState());
			short[] grid = new short[4 * 96 * 4];
			Random r = new Random(level.getSeed());
			int diff = 0, n = 0;
			for (int k = 0; k < 16; k++) {
				int cx = r.nextInt(-2000, 2000), cz = r.nextInt(-2000, 2000);
				if (!lib.biomes(cx, cz, grid)) throw new IllegalStateException("Hematite failed on the biomes of chunk " + cx + "," + cz);
				for (int i = 0; i < grid.length; i++, n++) {
					if (holders[grid[i]] != v.getNoiseBiome(cx * 4 + i / 96 % 4, i % 96 - 16, cz * 4 + i / 384)) diff++;
				}
			}
			// Paper's noise code rounds slightly differently from Mojang's, which Hematite matches, so about 0.3% of quarts
			// near biome borders differ; a datapack or another game version differs in far more
			if (diff * 100 > n) {
				log.info("biomes: " + diff + " of " + n + " sampled differ from vanilla, vanilla keeps generating them");
				return;
			}
			var f = net.minecraft.world.level.chunk.ChunkGenerator.class.getDeclaredField("biomeSource");
			f.setAccessible(true);
			f.set(g, new RustBiomes(vanilla, lib, holders));
			vanillaBiomes = vanilla;
			log.info("Rust now also generates biomes (" + diff + " of " + n + " sampled differ from vanilla)");
		} catch (Throwable e) {
			log.log(Level.WARNING, "cannot hand biomes to Rust, vanilla keeps generating them", e);
		}
	}

	/**
	 * Fills a fresh (all air) section from Hematite's ids in one go, the way Paper loads a section from disk, instead of
	 * setting 4096 blocks one at a time. False if it can't, and then the caller sets them one by one.
	 */
	boolean pack(LevelChunkSection section, Job j, int s) {
		BlockState[] states = lib.states;
		short[] ids = j.ids;
		if (j.seen.length < states.length) {
			j.seen = new int[states.length];
			j.slot = new int[states.length];
		}
		int[] seen = j.seen, slot = j.slot, ours = j.palette;
		byte[] local = j.local;
		int stamp = ++j.stamp, n = 0;
		for (int bz = 0; bz < 16; bz++) {
			for (int bx = 0; bx < 16; bx++) {
				int base = (bz * 16 + bx) * 384 + s * 16, at = bz << 4 | bx;
				for (int y = 0; y < 16; y++) {
					int id = ids[base + y];
					if (seen[id] != stamp) {
						if (n == 256) return false;
						seen[id] = stamp;
						slot[id] = n;
						ours[n++] = id;
					}
					local[y << 8 | at] = (byte) slot[id];
				}
			}
		}
		if (n == 1 && j.palette[0] == 0) return true; // all air, like the section already is
		try {
			PalettedContainer<BlockState> container = section.getStates();
			int bits = storageBits[n] - 1;
			if (bits < 0) storageBits[n] = (bits = ((net.minecraft.world.level.chunk.Configuration) CONFIGURATION.invoke(container.strategy, n)).bitsInStorage()) + 1;
			Optional<LongStream> storage = Optional.empty();
			if (bits > 0) {
				int per = 64 / bits, i = 0; // entries don't straddle longs
				long[] longs = new long[(4096 + per - 1) / per];
				for (int l = 0; l < longs.length; l++) {
					long v = 0;
					for (int k = 0, end = Math.min(per, 4096 - i); k < end; k++) v |= (long) (local[i++] & 0xFF) << k * bits;
					longs[l] = v;
				}
				storage = Optional.of(LongStream.of(longs));
			}
			List<BlockState> palette = new ArrayList<>(n);
			for (int k = 0; k < n; k++) palette.add(states[j.palette[k]]);
			@SuppressWarnings("unchecked")
			PalettedContainer<BlockState> built = PalettedContainer.unpack(container.strategy, new PalettedContainerRO.PackedData<>(palette, storage),
				Blocks.AIR.defaultBlockState(), (BlockState[]) PRESETS.get(container)).getOrThrow();
			if (bits > 4 || packChecks.get() > 0 && packChecks.getAndDecrement() > 0) { // rare wider palettes always
				for (int i = 0; i < 4096; i++) {
					int x = i & 15, y = i >> 8, z = i >> 4 & 15;
					if (built.get(x, y, z) != states[ids[(z * 16 + x) * 384 + s * 16 + y]]) throw new IllegalStateException("packed section differs at " + x + " " + y + " " + z);
				}
			}
			container.data = built.data;
			section.recalcBlockCounts();
			return true;
		} catch (Throwable e) {
			if (packing) log.log(Level.WARNING, "cannot pack sections, setting blocks one by one", e);
			packing = false;
			return false;
		}
	}

	/** Runs after vanilla carvers: for probes, the chunk now holds vanilla's noise + surface + caves. */
	@Override
	public void generateCaves(WorldInfo w, Random r, int x, int z, ChunkData data) {
		if (current(x, z) != Mode.PROBE) return;
		short[] ids = job.get().ids;
		Material[] materials = lib.materials;
		int diff = 0;
		String first = "";
		for (int bz = 0; bz < 16; bz++) {
			for (int bx = 0; bx < 16; bx++) {
				int base = (bz * 16 + bx) * 384;
				for (int y = 0; y < 384; y++) {
					Material vanilla = data.getType(bx, y - 64, bz), ours = materials[ids[base + y] & 0xFFFF];
					if (vanilla != ours && diff++ < 10) first += " [" + (x * 16 + bx) + " " + (y - 64) + " " + (z * 16 + bz) + ": vanilla " + vanilla + ", Hematite " + ours + "]";
				}
			}
		}
		BiomeSource vb = vanillaBiomes;
		if (verifyAll && vb != null) {
			// the biomes the chunk got (Hematite's) against vanilla's
			ChunkAccess chunk = ((CraftChunkData) data).getHandle();
			BiomeResolver v = vb.createUncachedResolver(((CraftWorld) w).getHandle().getChunkSource().randomState());
			for (int qz = 0; qz < 4; qz++) {
				for (int qx = 0; qx < 4; qx++) {
					for (int qy = -16; qy < 80; qy++) {
						biomesChecked.increment();
						if (chunk.getNoiseBiome(x * 4 + qx, qy, z * 4 + qz) != v.getNoiseBiome(x * 4 + qx, qy, z * 4 + qz)) biomesDiffer.increment();
					}
				}
			}
		}
		record(x, z, diff, first);
	}

	@Override
	public boolean shouldGenerateStructures() {
		return true;
	}

	@Override
	public boolean shouldGenerateDecorations() {
		return true;
	}

	@Override
	public boolean shouldGenerateMobs() {
		return true;
	}

	Component status() {
		if (verifyAll) return Component.text("verify " + probesClose.get() + "/" + probesDone.get() + " match", NamedTextColor.YELLOW);
		Boolean e = enabled;
		if (e == null && !failed) return Component.text("checking " + probesDone.get() + "/" + PROBES + " chunks vs vanilla", NamedTextColor.YELLOW);
		return e == Boolean.TRUE ? Component.text("on", NamedTextColor.GREEN) : Component.text("off, vanilla generation", NamedTextColor.RED);
	}

	String summary() {
		long n = rustChunks.sum();
		return n == 0 ? vanillaChunks.sum() + " chunks generated by vanilla, none by Hematite yet"
			: String.format("%d chunks by Hematite: %.2f ms in Rust + %.2f ms writing blocks per chunk; %d by vanilla; %d near structures", n, rustNanos.sum() / 1e6 / n, writeNanos.sum() / 1e6 / n, vanillaChunks.sum(), structureChunks.sum());
	}

	Mode current(int x, int z) {
		Job j = job.get();
		if (j.key != key(x, z)) throw new IllegalStateException("chunk " + x + "," + z + " was not started on this thread");
		return j.mode;
	}

	Mode mode(WorldInfo w, int x, int z, Job j) {
		if (w.getEnvironment() != World.Environment.NORMAL || w.getMinHeight() != -64 || w.getMaxHeight() != 320) return Mode.VANILLA;
		if (lib == null && !load(w.getSeed())) return Mode.VANILLA;
		try {
			j.beard = beard(((CraftWorld) w).getHandle(), x, z);
		} catch (Throwable e) {
			if (!warnedInternals) log.log(Level.WARNING, "cannot read structures, using vanilla generation", e);
			warnedInternals = true;
			return Mode.VANILLA;
		}
		if (j.beard != null) structureChunks.increment();
		Boolean e = enabled;
		if ((verifyAll || e == Boolean.TRUE) && takeoverTried.compareAndSet(false, true)) {
			useRustBiomes(((CraftWorld) w).getHandle());
			try {
				FastOre.install(((CraftWorld) w).getHandle(), log, verifyAll ? Integer.MAX_VALUE : 64, verifyAll);
			} catch (LinkageError err) { // another server version without vanilla's ore classes
				log.log(Level.WARNING, "cannot speed up ores, vanilla keeps placing them", err);
			}
		}
		if (verifyAll) return Mode.PROBE;
		if (e != null) return e ? Mode.RUST : Mode.VANILLA;
		return probesStarted.getAndIncrement() < PROBES ? Mode.PROBE : Mode.VANILLA;
	}

	/**
	 * The structure pieces that make vanilla raise or carve terrain around villages, ancient cities, trial chambers and
	 * others, as ints for Rust (Beard::from_ints there), or null if this chunk has none. Vanilla's own
	 * Beardifier.forStructuresInChunk picks them, so they come in the same order Rust sums them in; the chunks it reads
	 * are always generated far enough by now, so it doesn't wait. Reads private fields: if they change, this throws and
	 * chunks stay vanilla.
	 */
	@SuppressWarnings("unchecked")
	static int[] beard(ServerLevel level, int cx, int cz) throws ReflectiveOperationException {
		Beardifier b = Beardifier.forStructuresInChunk(level.structureManager(), new ChunkPos(cx, cz));
		BoundingBox box = (BoundingBox) field(Beardifier.class, "affectedBox").get(b);
		if (box == null) return null;
		var rigids = (java.util.List<Beardifier.Rigid>) field(Beardifier.class, "pieces").get(b);
		IntArrayList out = IntArrayList.of(box.minX(), box.minY(), box.minZ(), box.maxX(), box.maxY(), box.maxZ(), rigids.size());
		for (var r : rigids) rigid(out, r.box(), r.terrainAdjustment(), r.groundLevelDelta());
		for (var j : (java.util.List<JigsawJunction>) field(Beardifier.class, "junctions").get(b)) out.addAll(IntArrayList.of(j.getSourceX(), j.getSourceGroundY(), j.getSourceZ()));
		return out.toIntArray();
	}

	static java.lang.reflect.Field field(Class<?> c, String name) throws NoSuchFieldException {
		var f = c.getDeclaredField(name);
		f.setAccessible(true);
		return f;
	}

	static void rigid(IntArrayList out, BoundingBox b, TerrainAdjustment adjust, int groundLevelDelta) {
		out.addAll(IntArrayList.of(b.minX(), b.minY(), b.minZ(), b.maxX(), b.maxY(), b.maxZ(), switch (adjust) {
			case NONE -> 0;
			case BURY -> 1;
			case BEARD_THIN -> 2;
			case BEARD_BOX -> 3;
			case ENCAPSULATE -> 4;
		}, groundLevelDelta));
	}

	boolean warnedInternals;

	/** diff: blocks that differ from vanilla, or -1 if Hematite failed. */
	void record(int x, int z, int diff, String first) {
		if (diff == 0) probesExact.incrementAndGet();
		if (diff >= 0 && diff <= CLOSE) probesClose.incrementAndGet();
		if (job.get().beard != null) {
			structureProbes.incrementAndGet();
			if (diff == 0) structureExact.incrementAndGet();
			if (diff >= 0 && diff <= CLOSE) structureClose.incrementAndGet();
		}
		int done = probesDone.incrementAndGet();
		if (verifyAll) {
			if (diff < 0 || diff > CLOSE) log.warning("verify: chunk " + x + "," + z + (job.get().beard != null ? " (near a structure)" : "") + (diff < 0 ? " failed in Hematite" : " differs in " + diff + " blocks" + first));
			if (done % 100 == 0) log.info("verify: " + probesClose.get() + "/" + done + " chunks within " + CLOSE + " blocks of vanilla, " + probesExact.get() + " exact; near structures "
				+ structureClose.get() + "/" + structureProbes.get() + " within, " + structureExact.get() + " exact"
				+ (vanillaBiomes == null ? "; biomes by vanilla" : "; biomes by Hematite, " + biomesDiffer.sum() + " of " + biomesChecked.sum() + " quarts differ")
				+ "; ore veins: " + FastOre.differ.sum() + " of " + FastOre.checked.sum() + " differ from vanilla's code");
			return;
		}
		log.info("probe " + done + "/" + PROBES + ": chunk " + x + "," + z + (diff == 0 ? " matches vanilla" : diff < 0 ? " failed in Hematite" : " differs from vanilla in " + diff + " blocks" + (diff <= CLOSE ? " (fine)" : first)));
		if (done == PROBES) {
			enabled = probesClose.get() == PROBES;
			log.info(enabled ? "Hematite enabled: Rust now generates noise, surface and caves" : "Hematite disabled: it does not match this server's world generation, using vanilla");
		}
	}

	synchronized boolean load(long seed) {
		if (lib != null || failed) return lib != null;
		try {
			long t = System.nanoTime();
			lib = new Native(extractLibrary(), seed, serverJar());
			log.info("Hematite loaded in " + (System.nanoTime() - t) / 1_000_000 + " ms, checking " + PROBES + " chunks against vanilla first");
		} catch (Throwable e) {
			failed = true;
			log.log(Level.SEVERE, "Hematite could not start, using vanilla generation", e);
		}
		return lib != null;
	}

	Path extractLibrary() throws Exception {
		String name = System.mapLibraryName("hematite");
		try (var in = Generator.class.getResourceAsStream("/natives/" + platform() + "/" + name)) {
			if (in == null) throw new IllegalStateException("the plugin jar has no engine for " + platform());
			Files.createDirectories(dir);
			Path p = dir.resolve(name);
			Files.copy(in, p, StandardCopyOption.REPLACE_EXISTING);
			return p;
		}
	}

	/** Where the jar keeps the engine for this system: natives/<os>-<arch>/, e.g. natives/linux-x86_64/libhematite.so */
	static String platform() {
		String os = System.getProperty("os.name").toLowerCase(java.util.Locale.ROOT), arch = System.getProperty("os.arch");
		return (os.startsWith("windows") ? "windows" : os.startsWith("mac") ? "macos" : "linux") + "-" + switch (arch) {
			case "amd64", "x86_64" -> "x86_64";
			case "aarch64", "arm64" -> "aarch64";
			default -> arch;
		};
	}

	/** The jar holding this server's vanilla worldgen data. */
	static Path serverJar() throws Exception {
		var url = Bukkit.getServer().getClass().getClassLoader().getResource("data/minecraft/worldgen/noise_settings/overworld.json");
		return Path.of(((JarURLConnection) url.openConnection()).getJarFileURL().toURI());
	}

	static long key(int x, int z) {
		return ((long) x << 32) | (z & 0xFFFFFFFFL);
	}
}

/**
 * Hematite's biomes for vanilla's chunk biome step, which otherwise samples six climate noises at every quart. They
 * match vanilla's, which everything else (structures, /locate, possible biomes and so feature order) keeps using.
 */
final class RustBiomes extends BiomeSource {
	final BiomeSource vanilla;
	final Native lib;
	final Holder<Biome>[] holders; // by Hematite biome id

	RustBiomes(BiomeSource vanilla, Native lib, Holder<Biome>[] holders) {
		this.vanilla = vanilla;
		this.lib = lib;
		this.holders = holders;
	}

	@Override
	protected com.mojang.serialization.MapCodec<? extends BiomeSource> codec() {
		throw new UnsupportedOperationException("Hematite's biome source is never saved");
	}

	@Override
	protected java.util.stream.Stream<Holder<Biome>> collectPossibleBiomes() {
		return vanilla.possibleBiomes().stream();
	}

	@Override
	public BiomeResolver createResolver(Climate.Sampler s) {
		return vanilla.createResolver(s);
	}

	/** qx, qy, qz: the chunk's lowest quart; nx, ny, nz: its size in quarts. */
	@Override
	public BiomeResolver createResolverForChunk(Climate.Sampler s, int qx, int qy, int qz, int nx, int ny, int nz) {
		short[] grid = new short[4 * 96 * 4];
		try {
			if (nx != 4 || ny != 96 || nz != 4 || qy != -16 || !lib.biomes(qx >> 2, qz >> 2, grid)) return vanilla.createResolverForChunk(s, qx, qy, qz, nx, ny, nz);
		} catch (Throwable e) {
			return vanilla.createResolverForChunk(s, qx, qy, qz, nx, ny, nz);
		}
		return (x, y, z) -> {
			int ix = x - qx, iy = y - qy, iz = z - qz;
			return ix >= 0 && ix < 4 && iy >= 0 && iy < 96 && iz >= 0 && iz < 4 ? holders[grid[(iz * 4 + ix) * 96 + iy]] : vanilla.createResolver(s).getNoiseBiome(x, y, z);
		};
	}
}

/**
 * Vanilla's ore veins (OreFeature.doPlace, as compiled in Paper 26.3) with the same arithmetic, random numbers, block
 * reads and writes in the same order, minus work that doesn't change the result: build height and write zone looked up
 * per section instead of per block through interfaces, one division per row of a blob instead of per block, a plain
 * bit array instead of a BitSet. Swapped into the feature registry for every plain OreFeature; vanilla's place() still
 * picks where veins go. The first veins also run vanilla's code on the same blocks and compare; one mismatch and
 * vanilla places all ores again.
 */
final class FastOre extends OreFeature {
	static final AtomicInteger checks = new AtomicInteger();
	static final LongAdder checked = new LongAdder(), differ = new LongAdder();
	static volatile boolean broken, verify;
	static Logger log;
	static final Direction[] DIRECTIONS = Direction.values();

	final BlockReplacement[] targets;
	/** Every target's test looks at the block state alone, so it can be looked up per state. */
	final boolean pure;
	/** ... and no random numbers or neighbor blocks are read per block, so blocks can be placed in any order. */
	final boolean anyOrder;

	FastOre(OreFeature o) {
		super(o.targetStates(), o.size(), o.discardChanceOnAirExposure());
		targets = targetStates().toArray(new BlockReplacement[0]);
		pure = targets.length <= 32 && Arrays.stream(targets).allMatch(t -> pure(t.target()));
		anyOrder = pure && discardChanceOnAirExposure() <= 0.0F;
	}

	static boolean pure(RuleTest t) {
		Class<?> c = t.getClass();
		if (c == TagMatchTest.class || c == BlockMatchTest.class
			|| c == BlockStateMatchTest.class) return true;
		try {
			if (c == NotRuleTest.class) return pure((RuleTest) Generator.field(c, "rule").get(t));
			if (c == AnyOfRuleTest.class || c == AllOfRuleTest.class) {
				return Arrays.stream((RuleTest[]) Generator.field(c, "rulesArray").get(t)).allMatch(FastOre::pure);
			}
		} catch (ReflectiveOperationException | RuntimeException e) {
			// can't look inside, so treat it as impure
		}
		return false;
	}

	/** checks: veins to compare with vanilla's code first. */
	static void install(ServerLevel level, Logger log, int checks, boolean verify) {
		try {
			var value = Holder.Reference.class.getDeclaredField("value");
			value.setAccessible(true);
			FastOre.log = log;
			FastOre.verify = verify;
			FastOre.checks.set(checks);
			int n = 0;
			for (var h : level.registryAccess().lookupOrThrow(Registries.FEATURE).listElements().toList()) {
				if (h.value().getClass() == OreFeature.class) {
					value.set(h, new FastOre((OreFeature) h.value()));
					n++;
				}
			}
			log.info("faster ore veins for " + n + " ore features");
		} catch (Throwable e) {
			log.log(Level.WARNING, "cannot speed up ores, vanilla keeps placing them", e);
		}
	}

	@Override
	protected boolean doPlace(WorldGenLevel level, RandomSource random, double x0, double x1, double z0, double z1, double y0, double y1, int minX, int minY, int minZ, int w, int h) {
		if (broken || level.getClass() != WorldGenRegion.class) return super.doPlace(level, random, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h);
		if (checks.get() > 0 && checks.getAndDecrement() > 0) {
			checked.increment();
			if (!same((WorldGenRegion) level, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h)) {
				differ.increment();
				if (!verify) {
					broken = true;
					log.warning("ore vein at " + minX + " " + minY + " " + minZ + " differs from vanilla's, vanilla places ores again");
					return super.doPlace(level, random, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h);
				}
			}
		}
		return fast(level, random, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h);
	}

	boolean fast(WorldGenLevel level, RandomSource random, double x0, double x1, double z0, double z1, double y0, double y1, int minX, int minY, int minZ, int w, int h) {
		int size = size();
		// blobs along the vein: x, y, z, radius
		double[] s = new double[size * 4];
		for (int i = 0; i < size; i++) {
			float t = (float) i / (float) size;
			double d = random.nextDouble() * size / 16.0;
			s[i * 4] = Mth.lerp(t, x0, x1);
			s[i * 4 + 1] = Mth.lerp(t, y0, y1);
			s[i * 4 + 2] = Mth.lerp(t, z0, z1);
			s[i * 4 + 3] = ((Mth.sin((float) Math.PI * t) + 1.0F) * d + 1.0) / 2.0;
		}
		// drop blobs inside others
		for (int i = 0; i < size - 1; i++) {
			if (s[i * 4 + 3] <= 0.0) continue;
			for (int j = i + 1; j < size; j++) {
				if (s[j * 4 + 3] <= 0.0) continue;
				double dx = s[i * 4] - s[j * 4], dy = s[i * 4 + 1] - s[j * 4 + 1], dz = s[i * 4 + 2] - s[j * 4 + 2], dr = s[i * 4 + 3] - s[j * 4 + 3];
				if (dr * dr > dx * dx + dy * dy + dz * dz) {
					if (dr > 0.0) s[j * 4 + 3] = -1.0;
					else s[i * 4 + 3] = -1.0;
				}
			}
		}
		try (var access = new BulkSectionAccess(level)) {
			Vein v = new Vein(level, access, random);
			long[] rows = anyOrder ? rows(s, minX, minY, minZ, w, h, level.getMinY(), level.getMaxY()) : null;
			if (rows != null) {
				// each block once, a section at a time
				for (int zs = minZ >> 4; zs <= minZ + 63 >> 4; zs++) {
					long zm = -1L << Math.max(0, zs * 16 - minZ) & -1L >>> 63 - Math.min(63, zs * 16 + 15 - minZ);
					for (int x = 0; x < w; x++) {
						for (int y = 0; y < h; y++) {
							for (long m = rows[x + y * w] & zm; m != 0; m &= m - 1) v.block(minX + x, minY + y, minZ + Long.numberOfTrailingZeros(m));
						}
					}
				}
			} else {
				ordered(v, s, minX, minY, minZ, w, h, level.getMinY(), level.getMaxY());
			}
			return v.count > 0;
		}
	}

	/** Vanilla's order: blob by blob, x, y, z, each block the first time a blob reaches it. */
	void ordered(Vein v, double[] s, int minX, int minY, int minZ, int w, int h, int lo, int hi) {
		long[] seen = new long[(w * h * w >> 6) + 1]; // vanilla's BitSet, same index
		double[] zz = new double[0];
		for (int i = 0; i < size(); i++) {
			double r = s[i * 4 + 3];
			if (r < 0.0) continue;
			double cx = s[i * 4], cy = s[i * 4 + 1], cz = s[i * 4 + 2];
			int ax = Math.max(Mth.floor(cx - r), minX), ay = Math.max(Mth.floor(cy - r), minY), az = Math.max(Mth.floor(cz - r), minZ);
			int bx = Math.max(Mth.floor(cx + r), ax), by = Math.max(Mth.floor(cy + r), ay), bz = Math.max(Mth.floor(cz + r), az);
			if (zz.length <= bz - az) zz = new double[bz - az + 1];
			for (int z = az; z <= bz; z++) {
				double dz = (z + 0.5 - cz) / r;
				zz[z - az] = dz * dz;
			}
			for (int x = ax; x <= bx; x++) {
				double dx = (x + 0.5 - cx) / r, xx = dx * dx;
				if (!(xx < 1.0)) continue;
				for (int y = ay; y <= by; y++) {
					double dy = (y + 0.5 - cy) / r, xy = xx + dy * dy;
					if (!(xy < 1.0) || y < lo || y > hi) continue;
					for (int z = az; z <= bz; z++) {
						if (!(xy + zz[z - az] < 1.0)) continue;
						int k = x - minX + (y - minY) * w + (z - minZ) * w * h;
						if (k >>> 6 >= seen.length) seen = Arrays.copyOf(seen, (k >>> 6) + 1);
						if ((seen[k >>> 6] & 1L << k) != 0) continue;
						seen[k >>> 6] |= 1L << k;
						v.block(x, y, z);
					}
				}
			}
		}
	}

	/** The blocks inside any blob, as bit z - minZ of row x - minX + (y - minY) * w, or null if a blob sticks out of the box. */
	long[] rows(double[] s, int minX, int minY, int minZ, int w, int h, int lo, int hi) {
		long[] rows = new long[w * h];
		for (int i = 0; i < size(); i++) {
			double r = s[i * 4 + 3];
			if (r < 0.0) continue;
			double cx = s[i * 4], cy = s[i * 4 + 1], cz = s[i * 4 + 2];
			int ax = Math.max(Mth.floor(cx - r), minX), ay = Math.max(Mth.floor(cy - r), minY), az = Math.max(Mth.floor(cz - r), minZ);
			int bx = Math.max(Mth.floor(cx + r), ax), by = Math.max(Mth.floor(cy + r), ay), bz = Math.max(Mth.floor(cz + r), az);
			if (bx - minX >= w || by - minY >= h || bz - minZ >= 64) return null;
			for (int x = ax; x <= bx; x++) {
				double dx = (x + 0.5 - cx) / r, xx = dx * dx;
				if (!(xx < 1.0)) continue;
				for (int y = ay; y <= by; y++) {
					double dy = (y + 0.5 - cy) / r, xy = xx + dy * dy;
					if (!(xy < 1.0) || y < lo || y > hi) continue;
					long m = 0;
					for (int z = az; z <= bz; z++) {
						double dz = (z + 0.5 - cz) / r;
						if (xy + dz * dz < 1.0) m |= 1L << z - minZ;
					}
					rows[x - minX + (y - minY) * w] |= m;
				}
			}
		}
		return rows;
	}

	/** One vein's blocks: vanilla's checks for one block, with the section and target matches looked up once. */
	final class Vein {
		final WorldGenLevel level;
		final BulkSectionAccess access;
		final RandomSource random;
		final BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos(), near = new BlockPos.MutableBlockPos();
		final float chance = discardChanceOnAirExposure();
		int count, sx = Integer.MIN_VALUE, sy, sz;
		LevelChunkSection sec;
		final BlockState[] states = new BlockState[8];
		final int[] matches = new int[8];
		int known;

		Vein(WorldGenLevel level, BulkSectionAccess access, RandomSource random) {
			this.level = level;
			this.access = access;
			this.random = random;
		}

		void block(int x, int y, int z) {
			pos.set(x, y, z);
			// the write zone and sections are whole sections
			if (x >> 4 != sx || y >> 4 != sy || z >> 4 != sz) {
				sx = x >> 4;
				sy = y >> 4;
				sz = z >> 4;
				sec = level.ensureCanWrite(pos) ? access.getSection(pos) : null;
			}
			if (sec == null) return;
			int lx = x & 15, ly = y & 15, lz = z & 15;
			BlockState state = sec.getBlockState(lx, ly, lz);
			int m = pure ? matches(state) : -1;
			for (int i = 0; m != 0 && i < targets.length; i++) {
				var t = targets[i];
				if ((pure ? (m >>> i & 1) != 0 : t.target().test(state, pos, random)) && (chance <= 0.0F || !(chance >= 1.0F) && random.nextFloat() >= chance || !nearAir(lx, ly, lz))) {
					sec.setBlockState(lx, ly, lz, t.state(), false);
					count++;
					return;
				}
			}
		}

		/** Bit i: target i's test passes for this state. */
		int matches(BlockState state) {
			for (int i = 0; i < known; i++) if (states[i] == state) return matches[i];
			int m = 0;
			for (int i = 0; i < targets.length; i++) if (targets[i].target().test(state, pos, random)) m |= 1 << i;
			if (known < states.length) {
				states[known] = state;
				matches[known++] = m;
			}
			return m;
		}

		boolean nearAir(int lx, int ly, int lz) {
			for (var d : DIRECTIONS) {
				int x = lx + d.getStepX(), y = ly + d.getStepY(), z = lz + d.getStepZ();
				if (((x | y | z) >>> 4 == 0 ? sec.getBlockState(x, y, z) : access.getBlockState(near.setWithOffset(pos, d))).isAir()) return true;
			}
			return false;
		}
	}

	/** Runs this and vanilla's code on the same blocks with the same random numbers, puts the blocks back, and compares. */
	boolean same(WorldGenRegion level, double x0, double x1, double z0, double z1, double y0, double y1, int minX, int minY, int minZ, int w, int h) {
		long seed = minX * 3129871L ^ minZ * 116129781L ^ minY;
		var a = new XoroshiroRandomSource(seed);
		var b = new XoroshiroRandomSource(seed);
		BlockState[] before = blocks(level, minX, minY, minZ, w, h, null);
		boolean ra = fast(level, a, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h);
		BlockState[] ours = blocks(level, minX, minY, minZ, w, h, before);
		boolean rb = super.doPlace(level, b, x0, x1, z0, z1, y0, y1, minX, minY, minZ, w, h);
		BlockState[] vanilla = blocks(level, minX, minY, minZ, w, h, before);
		return ra == rb && a.nextLong() == b.nextLong() && Arrays.equals(ours, vanilla);
	}

	/** The writable blocks of a vein's box, plus one on the far sides; if put is given, also sets them to put. */
	static BlockState[] blocks(WorldGenRegion level, int minX, int minY, int minZ, int w, int h, BlockState[] put) {
		int y0 = Math.max(minY, level.getMinY()), y1 = Math.min(minY + h, level.getMaxY()), ny = Math.max(0, y1 - y0 + 1);
		BlockState[] out = new BlockState[(w + 1) * (w + 1) * ny];
		var pos = new BlockPos.MutableBlockPos();
		int i = 0;
		for (int x = minX; x <= minX + w; x++) {
			for (int z = minZ; z <= minZ + w; z++, i += ny) {
				if (!level.isWithinWriteZone(pos.set(x, 0, z))) continue;
				ChunkAccess chunk = level.getChunk(x >> 4, z >> 4);
				for (int y = y0; y <= y1; y++) {
					LevelChunkSection s = chunk.getSection(level.getSectionIndex(y));
					int k = i + y - y0;
					out[k] = s.getBlockState(x & 15, y & 15, z & 15);
					if (put != null && put[k] != out[k]) s.setBlockState(x & 15, y & 15, z & 15, put[k], false);
				}
			}
		}
		return out;
	}
}

/** The Rust library, called through Java's foreign function API. */
final class Native {
	static final ThreadLocal<MemorySegment> BUF = ThreadLocal.withInitial(() -> Arena.ofAuto().allocate(JAVA_SHORT, Generator.SIZE));

	final MemorySegment handle;
	final MethodHandle generate, biomes;
	final BlockState[] states;
	final Material[] materials;
	final String[] biomeNames;

	Native(Path library, long seed, Path jar) throws Throwable {
		Linker linker = Linker.nativeLinker();
		SymbolLookup lib = SymbolLookup.libraryLookup(library, Arena.global());
		MethodHandle create = linker.downcallHandle(lib.find("hematite_new").orElseThrow(), FunctionDescriptor.of(ADDRESS, JAVA_LONG, ADDRESS, ADDRESS, ADDRESS));
		MethodHandle palette = linker.downcallHandle(lib.find("hematite_palette").orElseThrow(), FunctionDescriptor.of(JAVA_LONG, ADDRESS, ADDRESS, JAVA_LONG));
		generate = linker.downcallHandle(lib.find("hematite_generate").orElseThrow(), FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, ADDRESS, ADDRESS, JAVA_LONG));
		biomes = linker.downcallHandle(lib.find("hematite_biomes").orElseThrow(), FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, ADDRESS));
		MethodHandle biomeNames = linker.downcallHandle(lib.find("hematite_biome_names").orElseThrow(), FunctionDescriptor.of(JAVA_LONG, ADDRESS, ADDRESS, JAVA_LONG));
		try (Arena a = Arena.ofConfined()) {
			handle = (MemorySegment) create.invokeExact(seed, a.allocateFrom(jar.toString()), a.allocateFrom(JAVA_FLOAT, table(Mth.class, "SIN")),
				a.allocateFrom(JAVA_FLOAT, table(Beardifier.class, "BEARD_KERNEL")));
			if (handle.equals(MemorySegment.NULL)) throw new IllegalStateException("Hematite could not load worldgen data from " + jar + " (details in the console above)");
			MemorySegment buf = a.allocate(1 << 20);
			long n = (long) palette.invokeExact(handle, buf, buf.byteSize());
			if (n > buf.byteSize()) throw new IllegalStateException("palette too large");
			String[] names = new String(buf.asSlice(0, n).toArray(JAVA_BYTE), StandardCharsets.UTF_8).split("\n");
			n = (long) biomeNames.invokeExact(handle, buf, buf.byteSize());
			if (n > buf.byteSize()) throw new IllegalStateException("biome names too large");
			this.biomeNames = new String(buf.asSlice(0, n).toArray(JAVA_BYTE), StandardCharsets.UTF_8).split("\n");
			states = new BlockState[names.length];
			materials = new Material[names.length];
			for (int i = 0; i < names.length; i++) {
				BlockData b = Bukkit.createBlockData(bukkitName(names[i]));
				states[i] = ((CraftBlockData) b).getState();
				materials[i] = b.getMaterial();
			}
		}
	}

	/** A float table as this JVM built it (Mth.SIN, Beardifier.BEARD_KERNEL): Java's Math.sin and Math.pow can differ
	 *  between platforms in the last bit. */
	static float[] table(Class<?> c, String field) throws ReflectiveOperationException {
		var f = c.getDeclaredField(field);
		f.setAccessible(true);
		return (float[]) f.get(null);
	}

	/** Fills out with the chunk's blocks; returns how many sections from the bottom hold anything but air, or -1.
	 *  beard: the chunk's structure pieces (Generator.beard), or null. */
	int generate(int cx, int cz, short[] out, int[] beard) throws Throwable {
		MemorySegment buf = BUF.get();
		int r;
		if (beard == null) {
			r = (int) generate.invokeExact(handle, cx, cz, buf, MemorySegment.NULL, 0L);
		} else {
			try (Arena a = Arena.ofConfined()) {
				r = (int) generate.invokeExact(handle, cx, cz, buf, a.allocateFrom(JAVA_INT, beard), (long) beard.length);
			}
		}
		if (r < 0) return -1;
		MemorySegment.copy(buf, JAVA_SHORT, 0, out, 0, out.length);
		return r;
	}

	/** Fills out (4 * 96 * 4) with the chunk's biome ids, index (z * 4 + x) * 96 + y + 16 in quarts of the chunk. */
	boolean biomes(int cx, int cz, short[] out) throws Throwable {
		MemorySegment buf = BUF.get();
		if ((int) biomes.invokeExact(handle, cx, cz, buf) != 0) return false;
		MemorySegment.copy(buf, JAVA_SHORT, 0, out, 0, out.length);
		return true;
	}

	/** Hematite writes properties as "name|{k:v}{k2:v2}", Bukkit wants "name[k=v,k2=v2]". */
	static String bukkitName(String n) {
		int i = n.indexOf('|');
		return i < 0 ? n : n.substring(0, i) + "[" + n.substring(i + 2, n.length() - 1).replace("}{", ",").replace(':', '=') + "]";
	}
}
