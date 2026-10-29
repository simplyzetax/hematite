import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.state.properties.Property;
import java.util.*;

public class DumpStates {
	public static void main(String[] args) throws Exception {
		SharedConstants.tryDetectVersion();
		Bootstrap.bootStrap();
		StringBuilder out = new StringBuilder();
		for (var b : BuiltInRegistries.BLOCK) {
			var st = b.defaultBlockState();
			List<Property<?>> ps = new ArrayList<>(st.getProperties());
			ps.sort((a, c) -> a.getName().compareTo(c.getName()));
			StringBuilder s = new StringBuilder();
			for (var p : ps) s.append("{").append(p.getName()).append(":").append(st.getValue(p).toString()).append("}");
			out.append(BuiltInRegistries.BLOCK.getKey(b)).append(" ").append(s).append("\n");
		}
		java.nio.file.Files.writeString(java.nio.file.Path.of(args[0]), out);
	}
}
