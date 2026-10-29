import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import java.io.*;

public class DumpDensity {
	public static void main(String[] args) throws Exception {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
		long seed = Long.parseLong(args[0]);
		var lookup = VanillaRegistries.createWorldLookup();
		var settings = lookup.lookupOrThrow(Registries.NOISE_SETTINGS).getOrThrow(NoiseGeneratorSettings.OVERWORLD).value();
		RandomState rs = RandomState.create(lookup.lookupOrThrow(Registries.NOISE), seed, settings);
		var router = settings.noiseRouter();
		DensityFunction[] fns = {router.finalDensity(), router.continents(), router.erosion()};
		var out = new DataOutputStream(new BufferedOutputStream(new FileOutputStream(args[1])));
		int[][] chunks = {{0, 0}, {-3, 7}, {100, -250}};
		for (int[] c : chunks) {
			SamplerContext ctx = SamplerContext.builder().enableCaches().build();
			var vol = new DensityVolume(16, 384, 16, c[0] * 16, -64, c[1] * 16);
			var buf = rs.samplersWithContext(ctx).get(fns[0]).sampleVolume(vol);
			for (int i = 0; i < vol.size(); i++) out.writeInt(Float.floatToRawIntBits(buf.get(i)));
		}
		var samplers = rs.samplersWithContext(SamplerContext.EMPTY_UNCACHED);
		for (int i = 0; i < 2000; i++) {
			int x = i * 7919 % 5000 - 2500, y = i % 384 - 64, z = i * 104729 % 5000 - 2500;
			for (var f : fns) out.writeInt(Float.floatToRawIntBits(samplers.sampleValue(f, x, y, z)));
		}
		out.close();
	}
}
