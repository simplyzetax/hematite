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
import java.util.HashMap;
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
import net.minecraft.world.level.levelgen.Heightmap;
import org.bukkit.craftbukkit.block.data.CraftBlockData;
import org.bukkit.craftbukkit.generator.CraftChunkData;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.TerrainAdjustment;
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
 * Rust (Hematite) generates noise, surface and caves; vanilla still does biomes, structures, features, mobs and light.
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
		final short[] ids = new short[SIZE];
	}

	final Logger log;
	final Path dir;
	final boolean verifyAll = Boolean.getBoolean("hematite.verify");
	final ThreadLocal<Job> job = ThreadLocal.withInitial(Job::new);
	final AtomicInteger probesStarted = new AtomicInteger(), probesDone = new AtomicInteger(), probesExact = new AtomicInteger(), probesClose = new AtomicInteger();
	volatile Boolean enabled; // null while probing
	final LongAdder rustChunks = new LongAdder(), rustNanos = new LongAdder(), writeNanos = new LongAdder(), vanillaChunks = new LongAdder(), structureChunks = new LongAdder();
	volatile Native lib;
	boolean failed;

	Generator(Logger log, Path dir) {
		this.log = log;
		this.dir = dir;
	}

	@Override
	public boolean shouldGenerateNoise(WorldInfo w, Random r, int x, int z) {
		Job j = job.get();
		j.key = key(x, z);
		j.mode = mode(w, x, z);
		if (j.mode != Mode.VANILLA) {
			boolean ok;
			long t = System.nanoTime();
			try {
				ok = lib.generate(x, z, j.ids);
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
		short[] ids = job.get().ids;
		BlockState[] states = lib.states;
		// Straight into the sections: ChunkData.setBlock takes a lock and updates two heightmaps per block.
		ChunkAccess chunk = ((CraftChunkData) data).getHandle();
		LevelChunkSection[] sections = chunk.getSections();
		for (int s = 0; s < sections.length; s++) {
			LevelChunkSection section = sections[s];
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
		return n == 0 ? vanillaChunks.sum() + " chunks generated by vanilla (" + structureChunks.sum() + " near structures), none by Hematite yet"
			: String.format("%d chunks by Hematite: %.2f ms in Rust + %.2f ms writing blocks per chunk; %d by vanilla (%d of them near structures)", n, rustNanos.sum() / 1e6 / n, writeNanos.sum() / 1e6 / n, vanillaChunks.sum(), structureChunks.sum());
	}

	Mode current(int x, int z) {
		Job j = job.get();
		if (j.key != key(x, z)) throw new IllegalStateException("chunk " + x + "," + z + " was not started on this thread");
		return j.mode;
	}

	Mode mode(WorldInfo w, int x, int z) {
		if (w.getEnvironment() != World.Environment.NORMAL || w.getMinHeight() != -64 || w.getMaxHeight() != 320) return Mode.VANILLA;
		if (lib == null && !load(w.getSeed())) return Mode.VANILLA;
		if (nearTerrainChangingStructure(w, x, z)) {
			structureChunks.increment();
			return Mode.VANILLA;
		}
		if (verifyAll) return Mode.PROBE;
		Boolean e = enabled;
		if (e != null) return e ? Mode.RUST : Mode.VANILLA;
		return probesStarted.getAndIncrement() < PROBES ? Mode.PROBE : Mode.VANILLA;
	}

	/**
	 * Vanilla raises or carves terrain around villages, outposts, ancient cities, trial chambers, strongholds and
	 * trail ruins; Hematite doesn't, so chunks that reference one stay vanilla. Uses Paper internals (the chunk being
	 * generated); if those change, every chunk stays vanilla.
	 */
	boolean nearTerrainChangingStructure(WorldInfo w, int x, int z) {
		try {
			ChunkAccess chunk = ((CraftWorld) w).getHandle().moonrise$getChunkTaskScheduler().chunkHolderManager.getChunkHolder(x, z).getCurrentChunk();
			for (Structure s : chunk.getAllReferences().keySet()) {
				if (s.terrainAdaptation() != TerrainAdjustment.NONE) return true;
			}
			return false;
		} catch (Throwable e) {
			if (!warnedInternals) log.log(Level.WARNING, "cannot read structure references, using vanilla generation", e);
			warnedInternals = true;
			return true;
		}
	}

	boolean warnedInternals;

	/** diff: blocks that differ from vanilla, or -1 if Hematite failed. */
	void record(int x, int z, int diff, String first) {
		if (diff == 0) probesExact.incrementAndGet();
		if (diff >= 0 && diff <= CLOSE) probesClose.incrementAndGet();
		int done = probesDone.incrementAndGet();
		if (verifyAll) {
			if (diff < 0 || diff > CLOSE) log.warning("verify: chunk " + x + "," + z + (diff < 0 ? " failed in Hematite" : " differs in " + diff + " blocks" + first));
			if (done % 100 == 0) log.info("verify: " + probesClose.get() + "/" + done + " chunks within " + CLOSE + " blocks of vanilla, " + probesExact.get() + " exact (" + structureChunks.sum() + " near structures skipped)");
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

/** The Rust library, called through Java's foreign function API. */
final class Native {
	static final ThreadLocal<MemorySegment> BUF = ThreadLocal.withInitial(() -> Arena.ofAuto().allocate(JAVA_SHORT, Generator.SIZE));

	final MemorySegment handle;
	final MethodHandle generate;
	final BlockState[] states;
	final Material[] materials;

	Native(Path library, long seed, Path jar) throws Throwable {
		Linker linker = Linker.nativeLinker();
		SymbolLookup lib = SymbolLookup.libraryLookup(library, Arena.global());
		MethodHandle create = linker.downcallHandle(lib.find("hematite_new").orElseThrow(), FunctionDescriptor.of(ADDRESS, JAVA_LONG, ADDRESS, ADDRESS));
		MethodHandle palette = linker.downcallHandle(lib.find("hematite_palette").orElseThrow(), FunctionDescriptor.of(JAVA_LONG, ADDRESS, ADDRESS, JAVA_LONG));
		generate = linker.downcallHandle(lib.find("hematite_generate").orElseThrow(), FunctionDescriptor.of(JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, ADDRESS));
		try (Arena a = Arena.ofConfined()) {
			handle = (MemorySegment) create.invokeExact(seed, a.allocateFrom(jar.toString()), a.allocateFrom(JAVA_FLOAT, mthSin()));
			if (handle.equals(MemorySegment.NULL)) throw new IllegalStateException("Hematite could not load worldgen data from " + jar + " (details in the console above)");
			MemorySegment buf = a.allocate(1 << 20);
			long n = (long) palette.invokeExact(handle, buf, buf.byteSize());
			if (n > buf.byteSize()) throw new IllegalStateException("palette too large");
			String[] names = new String(buf.asSlice(0, n).toArray(JAVA_BYTE), StandardCharsets.UTF_8).split("\n");
			states = new BlockState[names.length];
			materials = new Material[names.length];
			for (int i = 0; i < names.length; i++) {
				BlockData b = Bukkit.createBlockData(bukkitName(names[i]));
				states[i] = ((CraftBlockData) b).getState();
				materials[i] = b.getMaterial();
			}
		}
	}

	/** Minecraft's sine table as this JVM built it: Java's Math.sin can differ between platforms in the last bit. */
	static float[] mthSin() throws ReflectiveOperationException {
		var f = net.minecraft.util.Mth.class.getDeclaredField("SIN");
		f.setAccessible(true);
		return (float[]) f.get(null);
	}

	boolean generate(int cx, int cz, short[] out) throws Throwable {
		MemorySegment buf = BUF.get();
		if ((int) generate.invokeExact(handle, cx, cz, buf) != 0) return false;
		MemorySegment.copy(buf, JAVA_SHORT, 0, out, 0, out.length);
		return true;
	}

	/** Hematite writes properties as "name|{k:v}{k2:v2}", Bukkit wants "name[k=v,k2=v2]". */
	static String bukkitName(String n) {
		int i = n.indexOf('|');
		return i < 0 ? n : n.substring(0, i) + "[" + n.substring(i + 2, n.length() - 1).replace("}{", ",").replace(':', '=') + "]";
	}
}
