import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.world.level.biome.*;
import java.nio.file.*;
import java.lang.reflect.*;

public class CheckTemps {
	public static void main(String[] args) throws Exception {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
		var biomes = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.BIOME);
		Method gt = Biome.class.getDeclaredMethod("getHeightAdjustedTemperature", BlockPos.class, int.class);
		gt.setAccessible(true);
		int bad = 0, n = 0, cold = 0;
		for (String line : Files.readAllLines(Path.of(args[0]))) {
			String[] p = line.split(" ");
			var b = biomes.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(p[0]))).value();
			var pos = new BlockPos(Integer.parseInt(p[1]), Integer.parseInt(p[2]), Integer.parseInt(p[3]));
			float t = (float) gt.invoke(b, pos, 63);
			if (Float.floatToIntBits(t) != (int) Long.parseLong(p[4])) { if (bad++ < 5) System.out.println("diff " + line + " java " + t + " rust " + Float.intBitsToFloat((int) Long.parseLong(p[4]))); }
			if (t < 0.15f) cold++;
			n++;
		}
		System.out.println(n + " checked, " + bad + " diffs, " + cold + " cold");
	}
}
