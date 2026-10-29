import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import java.io.*;
import java.lang.reflect.*;
import java.util.*;
import java.util.function.Supplier;

public class DumpChunks {
	static Method m(Class<?> c, String name) {
		for (Method x : c.getDeclaredMethods()) if (x.getName().equals(name)) { x.setAccessible(true); return x; }
		throw new RuntimeException(name);
	}

	/** Vanilla's Beardifier from the plugin's ints: affected box, rigid count, rigids (box, adjustment, ground delta), junctions (x, ground y, z). */
	static Beardifier beardifier(String line) {
		if (line.isBlank()) return null;
		int[] d = Arrays.stream(line.trim().split("\\s+")).mapToInt(Integer::parseInt).toArray();
		var rigids = new ArrayList<Beardifier.Rigid>();
		var junctions = new ArrayList<net.minecraft.world.level.levelgen.structure.pools.JigsawJunction>();
		int n = d[6], p = 7;
		for (int k = 0; k < n; k++, p += 8)
			rigids.add(new Beardifier.Rigid(new net.minecraft.world.level.levelgen.structure.BoundingBox(d[p], d[p + 1], d[p + 2], d[p + 3], d[p + 4], d[p + 5]),
				net.minecraft.world.level.levelgen.structure.TerrainAdjustment.values()[d[p + 6]], d[p + 7]));
		for (; p + 2 < d.length; p += 3)
			junctions.add(new net.minecraft.world.level.levelgen.structure.pools.JigsawJunction(d[p], d[p + 1], d[p + 2], 0, net.minecraft.world.level.levelgen.structure.pools.StructureTemplatePool.Projection.RIGID));
		return new Beardifier(rigids, junctions, new net.minecraft.world.level.levelgen.structure.BoundingBox(d[0], d[1], d[2], d[3], d[4], d[5]));
	}

	public static void main(String[] args) throws Exception {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
		long seed = Long.parseLong(args[0]);
		Method bt = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
		bt.setAccessible(true);
		bt.invoke(Blocks.BEDROCK.builtInRegistryHolder(), List.of(net.minecraft.tags.BlockTags.UNCARVABLE));
		var lookup = VanillaRegistries.createWorldLookup();
		var settingsHolder = lookup.lookupOrThrow(Registries.NOISE_SETTINGS).getOrThrow(NoiseGeneratorSettings.OVERWORLD);
		var settings = settingsHolder.value();
		RandomState rs = RandomState.create(lookup.lookupOrThrow(Registries.NOISE), seed, settings);
		var source = MultiNoiseBiomeSource.createFromPreset(lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
		var gen = new NoiseBasedChunkGenerator(source, settingsHolder);
		IdMapper<Holder<Biome>> biomeIds = new IdMapper<>();
		lookup.lookupOrThrow(Registries.BIOME).listElements().forEach(biomeIds::add);
		var plains = lookup.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
		var factory = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(biomeIds), plains, null);
		LevelHeightAccessor height = LevelHeightAccessor.create(-64, 384);
		Method doCreateBiomes = m(ChunkGenerator.class, "doCreateBiomes");
		Method doFill = m(NoiseBasedChunkGenerator.class, "doFill");
		Method buildSurface = m(NoiseBasedChunkGenerator.class, "buildSurface");
		Method generateCarvers = m(NoiseBasedChunkGenerator.class, "generateCarvers");
		Field fp = NoiseBasedChunkGenerator.class.getDeclaredField("globalFluidPicker");
		fp.setAccessible(true);
		Aquifer.FluidPicker picker = ((Supplier<Aquifer.FluidPicker>)fp.get(gen)).get();
		Map<Long, ProtoChunk> biomeChunks = new HashMap<>();
		java.util.function.BiFunction<Integer, Integer, ProtoChunk> biomeChunk = (x, z) -> biomeChunks.computeIfAbsent(ChunkPos.pack(x, z), k -> {
			ProtoChunk c = new ProtoChunk(new ChunkPos(x, z), UpgradeData.EMPTY, height, factory, null);
			try { doCreateBiomes.invoke(gen, Blender.empty(), rs, c); } catch (Exception e) { throw new RuntimeException(e); }
			c.setPersistedStatus(net.minecraft.world.level.chunk.status.ChunkStatus.BIOMES);
			return c;
		});
		BiomeResolver resolver = (qx, qy, qz) -> biomeChunk.apply(qx >> 2, qz >> 2).getNoiseBiome(qx, qy, qz);
		BiomeManager biomeManager = new BiomeManager(resolver, BiomeManager.obfuscateSeed(seed));
		var rule = settings.materialRule().value();
		Map<String, Integer> palette = new LinkedHashMap<>();
		var out = new DataOutputStream(new BufferedOutputStream(new FileOutputStream(args[1])));
		// optional: one line of structure pieces per chunk, in the ints the plugin passes to Rust (empty for none)
		String beardFile = System.getenv("HEMATITE_BEARDS");
		Iterator<String> beards = beardFile == null ? Collections.emptyIterator() : java.nio.file.Files.readAllLines(java.nio.file.Path.of(beardFile)).iterator();
		for (int i = 2; i + 1 < args.length; i += 2) {
			int cx = Integer.parseInt(args[i]), cz = Integer.parseInt(args[i + 1]);
			ProtoChunk chunk = new ProtoChunk(new ChunkPos(cx, cz), UpgradeData.EMPTY, height, factory, null);
			doCreateBiomes.invoke(gen, Blender.empty(), rs, chunk);
			chunk.setPersistedStatus(net.minecraft.world.level.chunk.status.ChunkStatus.BIOMES);
			var volume = new DensityVolume(16, 384, 16, cx * 16, -64, cz * 16);
			try (NoiseChunk nc = new NoiseChunk(rs, beards.hasNext() ? beardifier(beards.next()) : null, settings, picker, Blender.empty(), volume)) {
				doFill.invoke(gen, nc, chunk);
				buildSurface.invoke(gen, chunk, nc, rs, biomeManager, null, rule);
				generateCarvers.invoke(gen, chunk, Blender.empty(), nc, rs, biomeManager, null, rule);
			}
			for (int z = 0; z < 16; z++)
				for (int x = 0; x < 16; x++)
					for (int y = -64; y < 320; y++) {
						BlockState s = chunk.getBlockState(new BlockPos(cx * 16 + x, y, cz * 16 + z));
						String name = BuiltInRegistries.BLOCK.getKey(s.getBlock()).toString();
						out.writeShort(palette.computeIfAbsent(name, k -> palette.size()));
					}
		}
		out.close();
		try (var pw = new PrintWriter(args[1] + ".palette")) { palette.keySet().forEach(pw::println); }
	}
}
