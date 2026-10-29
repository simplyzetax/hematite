import com.seibel.distanthorizons.core.sql.dto.FullDataSourceV2DTO;
import com.seibel.distanthorizons.core.util.FullDataPointUtil;
import io.netty.buffer.Unpooled;
import java.io.*;
import java.nio.file.*;

public class CheckDh {
	public static void main(String[] args) throws Exception {
		for (String f : args) {
			byte[] b = Files.readAllBytes(Path.of(f));
			var dto = FullDataSourceV2DTO.CreateEmptyDataSourceForDecoding();
			dto.decode(Unpooled.wrappedBuffer(b));
			var ds = dto.createUnitTestDataSource();
			int cols = 0, pts = 0, bad = 0;
			for (int i = 0; i < 4096; i++) {
				var c = ds.dataPoints[i];
				if (c == null || c.size() == 0 || (c.size() == 1 && c.getLong(0) == 0)) continue;
				cols++;
				pts += c.size();
				int top = 385;
				for (int k = 0; k < c.size(); k++) {
					long d = c.getLong(k);
					FullDataPointUtil.validateDatapoint(d);
					if (FullDataPointUtil.getBottomY(d) + FullDataPointUtil.getHeight(d) != top) bad++;
					top = FullDataPointUtil.getBottomY(d);
				}
				if (top != 0) bad++;
			}
			byte[] mz = dto.compressedMappingByteArray.toByteArray();
			byte[] raw = dhcomgithubluben.zstd.Zstd.decompress(mz, (int) dhcomgithubluben.zstd.Zstd.getFrameContentSize(mz));
			var in = new DataInputStream(new ByteArrayInputStream(raw));
			int n = in.readInt();
			String first = n > 0 ? in.readUTF() : "";
			for (int i = 1; i < n; i++) in.readUTF();
			StringBuilder sample = new StringBuilder();
			var c = ds.dataPoints[33 * 64 + 33];
			for (int k = 0; c != null && k < Math.min(6, c.size()); k++) sample.append(FullDataPointUtil.toString(c.getLong(k))).append(' ');
			System.out.println(f + ": cols=" + cols + " pts=" + pts + " bad=" + bad + " mapping=" + n + " first=" + first + " leftover=" + in.available() + " genstep[0]=" + ds.columnGenerationSteps.getByte(0) + "\n   " + sample);
		}
	}
}
