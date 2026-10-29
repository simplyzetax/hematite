import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import java.io.*;

public class DumpBiomeGrid {
	public static void main(String[] args) throws Exception {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
		long seed = Long.parseLong(args[0]);
		int radius = Integer.parseInt(args[2]);
		var lookup = VanillaRegistries.createWorldLookup();
		var settings = lookup.lookupOrThrow(Registries.NOISE_SETTINGS).getOrThrow(NoiseGeneratorSettings.OVERWORLD).value();
		RandomState rs = RandomState.create(lookup.lookupOrThrow(Registries.NOISE), seed, settings);
		var source = MultiNoiseBiomeSource.createFromPreset(lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
		var out = new PrintWriter(new BufferedWriter(new FileWriter(args[1])));
		for (int cx = -radius; cx <= radius; cx++) {
			for (int cz = -radius; cz <= radius; cz++) {
				var sampler = rs.createClimateSampler(SamplerContext.builder().enableCaches().build());
				var resolver = source.createResolverForChunk(sampler, cx * 4, -16, cz * 4, 4, 96, 4);
				StringBuilder sb = new StringBuilder();
				for (int sy = -4; sy < 20; sy++)
					for (int x = 0; x < 4; x++)
						for (int y = 0; y < 4; y++)
							for (int z = 0; z < 4; z++)
								sb.append(resolver.getNoiseBiome(cx * 4 + x, sy * 4 + y, cz * 4 + z).unwrapKey().get().identifier().getPath()).append(' ');
				out.println(sb);
			}
		}
		var uncached = source.createUncachedResolver(rs);
		StringBuilder sb = new StringBuilder();
		for (int i = 0; i < 4000; i++) {
			int qx = i * 7919 % 20000 - 10000, qy = i % 96 - 16, qz = i * 104729 % 20000 - 10000;
			sb.append(uncached.getNoiseBiome(qx, qy, qz).unwrapKey().get().identifier().getPath()).append(' ');
		}
		out.println(sb);
		out.close();
	}
}
