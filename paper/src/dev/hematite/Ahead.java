package dev.hematite;

import ca.spottedleaf.concurrentutil.util.Priority;
import ca.spottedleaf.moonrise.patches.chunk_system.scheduling.ChunkTaskScheduler;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.LongAdder;
import net.minecraft.server.level.ChunkTrackingView;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import org.bukkit.Bukkit;
import org.bukkit.World;
import org.bukkit.command.CommandSender;
import org.bukkit.craftbukkit.CraftWorld;
import org.bukkit.plugin.Plugin;
import org.bukkit.scheduler.BukkitTask;

/**
 * Generates chunks ahead of fast players (elytra, horses, ice boats) so they are done before they come into view.
 * Guesses a straight line from the last quarter second of movement and asks for the chunks that line will bring
 * into view, at low priority so chunks players already need go first.
 */
final class Ahead {
	static final double MIN_SPEED = 8; // blocks/s; sprinting is 5.6
	static final double SECONDS = 5, MAX_BLOCKS = 320;
	static final int MAX_QUEUED = 2048;
	final AtomicInteger queued = new AtomicInteger();
	final LongAdder requested = new LongAdder();
	final Set<Long> asked = new HashSet<>(); // main thread only

	static ChunkTaskScheduler scheduler(World w) {
		return ((CraftWorld) w).getHandle().moonrise$getChunkTaskScheduler();
	}

	/** The chunks Paper sends a player standing in chunk cx, cz with view distance r. */
	static boolean inView(int cx, int cz, int r, int x, int z) {
		return ChunkTrackingView.isWithinDistance(cx, cz, r, x, z, true);
	}

	static int chunk(double block) {
		return Math.floorDiv((int) Math.floor(block), 16);
	}

	/** x, z: block position; vx, vz: blocks per second; r: view distance in chunks. Main thread only. */
	void predict(World w, double x, double z, double vx, double vz, int r) {
		double speed = Math.hypot(vx, vz);
		if (speed < MIN_SPEED) return;
		if (asked.size() > 200_000) asked.clear(); // ponytail: forgets everything at once; asking again for a done chunk is cheap
		double t = Math.min(SECONDS, MAX_BLOCKS / speed);
		int steps = (int) Math.ceil(speed * t / 16), px = chunk(x), pz = chunk(z);
		ChunkTaskScheduler s = scheduler(w);
		for (int i = 1; i <= steps; i++) {
			double f = t * i / steps;
			int cx = chunk(x + vx * f), cz = chunk(z + vz * f);
			if (cx == px && cz == pz) continue;
			for (int ax = cx - r - 2; ax <= cx + r + 2; ax++) {
				for (int az = cz - r - 2; az <= cz + r + 2; az++) {
					// on a straight line, a chunk in view now and at step i is also in view at step i - 1
					if (!inView(cx, cz, r, ax, az) || inView(px, pz, r, ax, az)) continue;
					if (queued.get() >= MAX_QUEUED) return;
					if (!asked.add(Generator.key(ax, az))) continue;
					queued.incrementAndGet();
					requested.increment();
					// LIGHT is everything but the final step to a full chunk, which the player's own load does
					s.scheduleChunkLoad(ax, az, true, ChunkStatus.LIGHT, true, Priority.LOW, c -> queued.decrementAndGet());
				}
			}
			px = cx;
			pz = cz;
		}
	}
}

/**
 * /hematite fly: simulated players fly straight out from one point in evenly spread directions. For each chunk that
 * comes into view it measures how long until the chunk is fully generated, which is how long a real player sees a
 * hole there. Doesn't model Paper's per-player send limits or the network.
 */
final class Fly implements Runnable {
	static int runs;
	final World w;
	final CommandSender to;
	final Ahead ahead;
	final double speed, x0;
	final int ticks, r, players;
	final String name;
	final BukkitTask task;
	final Map<Long, Long> waiting = new ConcurrentHashMap<>();
	final Set<Long> ready = ConcurrentHashMap.newKeySet();
	final List<Double> delays = Collections.synchronizedList(new ArrayList<>());
	final long requestedBefore;
	boolean flying;
	int tick, maxHoles, nearTicks, slowTicks, msptSamples;
	long holes, last;
	double msptSum, msptMax;

	Fly(Plugin plugin, World w, CommandSender to, Ahead ahead, double speed, int seconds, int r, int players) {
		this.w = w;
		this.to = to;
		this.ahead = ahead;
		this.speed = speed;
		this.ticks = seconds * 20;
		this.r = r;
		this.players = players;
		this.x0 = 40_000.0 * ++runs; // fresh terrain for each run
		this.requestedBefore = ahead == null ? 0 : ahead.requested.sum();
		name = String.format("%d player%s at %.0f b/s, view %d, %d s, look-ahead %s", players, players == 1 ? "" : "s", speed, r, seconds, ahead == null ? "off" : "on");
		to.sendMessage("fly: loading the start area for " + name);
		task = Bukkit.getScheduler().runTaskTimer(plugin, this, 1, 1);
	}

	@Override
	public void run() {
		if (!flying) {
			// like joining: load the start area first, then time the flight
			step();
			if (!waiting.isEmpty() || (ahead != null && ahead.queued.get() > 0)) return;
			flying = true;
			delays.clear();
			last = System.nanoTime();
			to.sendMessage("fly: start area ready, flying");
			return;
		}
		long now = System.nanoTime();
		if (now - last > 60_000_000L) slowTicks++;
		last = now;
		tick++;
		int h = step();
		holes += h;
		maxHoles = Math.max(maxHoles, h);
		if (tick % 20 == 0) {
			double mspt = Bukkit.getAverageTickTime();
			msptSum += mspt;
			msptMax = Math.max(msptMax, mspt);
			msptSamples++;
		}
		if (tick >= ticks) finish();
	}

	/** Asks for the chunks in view of each player and returns how many are not done yet. */
	int step() {
		double t = tick / 20.0;
		long now = System.nanoTime();
		ChunkTaskScheduler s = Ahead.scheduler(w);
		int h = 0;
		boolean near = false;
		for (int i = 0; i < players; i++) {
			double a = 2 * Math.PI * i / players, vx = speed * Math.cos(a), vz = speed * Math.sin(a), x = x0 + vx * t, z = vz * t;
			int cx = Ahead.chunk(x), cz = Ahead.chunk(z);
			for (int ax = cx - r - 2; ax <= cx + r + 2; ax++) {
				for (int az = cz - r - 2; az <= cz + r + 2; az++) {
					long k = Generator.key(ax, az);
					if (!Ahead.inView(cx, cz, r, ax, az) || ready.contains(k)) continue;
					h++;
					near |= Math.abs(ax - cx) <= 3 && Math.abs(az - cz) <= 3;
					if (waiting.putIfAbsent(k, now) == null) {
						s.scheduleChunkLoad(ax, az, true, ChunkStatus.FULL, true, Priority.NORMAL, c -> {
							ready.add(k);
							Long since = waiting.remove(k);
							if (since != null) delays.add((System.nanoTime() - since) / 1e6);
						});
					}
				}
			}
			if (ahead != null && flying && tick % 5 == 0) ahead.predict(w, x, z, vx, vz, r);
		}
		if (near && flying) nearTicks++;
		return h;
	}

	void finish() {
		task.cancel();
		List<Double> d;
		synchronized (delays) {
			d = new ArrayList<>(delays);
		}
		Collections.sort(d);
		to.sendMessage(String.format("fly done: %s: %d chunks came into view; ready after median %.0f ms, 95%% %.0f ms, max %.0f ms; "
				+ "%.1f missing on average (max %d), a hole within 3 chunks of a player in %.0f%% of ticks; MSPT avg %.1f, max %.1f; %d slow ticks; %d fetched ahead",
			name, d.size() + waiting.size(), pct(d, 0.5), pct(d, 0.95), d.isEmpty() ? 0 : d.getLast(), (double) holes / ticks, maxHoles,
			100.0 * nearTicks / ticks, msptSum / Math.max(1, msptSamples), msptMax, slowTicks, ahead == null ? 0 : ahead.requested.sum() - requestedBefore));
	}

	static double pct(List<Double> d, double p) {
		return d.isEmpty() ? 0 : d.get((int) Math.min(d.size() - 1, Math.floor(p * d.size())));
	}
}
