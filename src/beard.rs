//! Vanilla's beardifier: raises or carves terrain around villages, ancient cities, trial chambers and other
//! structures. The server sends each chunk's structure pieces, and this samples them like Beardifier.sampleValue.

use crate::density::Volume;
use std::sync::OnceLock;

/// Vanilla's 24x24x24 falloff kernel. Vanilla builds it with Java's Math.pow, so the plugin sets it from the
/// server's own table; otherwise it's computed here.
pub static KERNEL: OnceLock<Vec<f32>> = OnceLock::new();

fn kernel() -> &'static [f32] {
	KERNEL.get_or_init(|| {
		(0..24 * 24 * 24)
			.map(|i| {
				let (dz, dx, dy) = ((i / 576 - 12) as f64, (i / 24 % 24 - 12) as f64, (i % 24 - 12) as f64 + 0.5);
				std::f64::consts::E.powf(-(dx * dx + dy * dy + dz * dz) / 16.0) as f32
			})
			.collect()
	})
}

#[derive(Clone, Copy, Debug)]
struct Rigid {
	min: [i32; 3],
	max: [i32; 3],
	adjust: i32, // 0 none, 1 bury, 2 beard_thin, 3 beard_box, 4 encapsulate
	ground_delta: i32,
}

#[derive(Clone, Debug)]
pub struct Beard {
	min: [i32; 3],
	max: [i32; 3],
	rigids: Vec<Rigid>,
	junctions: Vec<[i32; 3]>,
	kernel: &'static [f32],
}

impl Beard {
	/// From the plugin's ints: the affected box (min x y z, max x y z), the rigid count, per rigid its box,
	/// adjustment and ground level delta, then x, ground y, z per junction.
	pub fn from_ints(d: &[i32]) -> Self {
		let n = d[6] as usize;
		let rigids = d[7..7 + n * 8].chunks(8).map(|r| Rigid { min: [r[0], r[1], r[2]], max: [r[3], r[4], r[5]], adjust: r[6], ground_delta: r[7] }).collect();
		let junctions = d[7 + n * 8..].as_chunks::<3>().0.to_vec();
		Self { min: [d[0], d[1], d[2]], max: [d[3], d[4], d[5]], rigids, junctions, kernel: kernel() }
	}

	/// The highest y the pieces can change.
	pub fn max_y(&self) -> i32 {
		self.max[1]
	}

	pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
		let inside = (0..3).all(|a| (self.min[a]..=self.max[a]).contains(&[x, y, z][a]));
		if inside { self.unchecked(x, y, z) } else { 0.0 }
	}

	/// Beardifier.sampleVolume: zero outside the affected box.
	pub fn fill(&self, out: &mut [f32], v: &Volume) {
		out.fill(0.0);
		let range = |a: usize, m: i32, t: i32, s: i32| ((self.min[a] - m).max(0).div_euclid(t), (self.max[a] - m).div_euclid(t).min(s - 1));
		let ((x0, x1), (y0, y1), (z0, z1)) = (range(0, v.mx, v.tx, v.sx), range(1, v.my, v.ty, v.sy), range(2, v.mz, v.tz, v.sz));
		for z in z0..=z1 {
			for x in x0..=x1 {
				for y in y0..=y1 {
					out[v.idx(x, y, z)] = self.unchecked(v.block_x(x), v.block_y(y), v.block_z(z));
				}
			}
		}
	}

	fn unchecked(&self, x: i32, y: i32, z: i32) -> f32 {
		let mut v = 0f32;
		for r in &self.rigids {
			let dx = (r.min[0] - x).max(x - r.max[0]).max(0);
			let dz = (r.min[2] - z).max(z - r.max[2]).max(0);
			let ground = r.min[1] + r.ground_delta;
			let to_ground = y - ground;
			v += match r.adjust {
				1 => bury(dx as f32, to_ground as f32 / 2.0, dz as f32),
				2 => self.beard(dx, to_ground, dz, to_ground) * 0.8,
				3 => self.beard(dx, (ground - y).max(y - r.max[1]).max(0), dz, to_ground) * 0.8,
				4 => bury(dx as f32 / 2.0, (r.min[1] - y).max(y - r.max[1]).max(0) as f32 / 2.0, dz as f32 / 2.0) * 0.8,
				_ => continue,
			};
		}
		for j in &self.junctions {
			let dy = y - j[1];
			v += self.beard(x - j[0], dy, z - j[2], dy) * 0.4;
		}
		v
	}

	fn beard(&self, dx: i32, dy: i32, dz: i32, to_ground: i32) -> f32 {
		let (xi, yi, zi) = (dx + 12, dy + 12, dz + 12);
		if !(0..24).contains(&xi) || !(0..24).contains(&yi) || !(0..24).contains(&zi) {
			return 0.0;
		}
		let dyo = to_ground as f32 + 0.5;
		let lsq = dx as f32 * dx as f32 + dyo * dyo + dz as f32 * dz as f32;
		-dyo * fast_inv_sqrt((lsq / 2.0) as f64) as f32 / 2.0 * self.kernel[(zi * 576 + xi * 24 + yi) as usize]
	}
}

fn bury(dx: f32, dy: f32, dz: f32) -> f32 {
	let lsq = dx * dx + dy * dy + dz * dz;
	if lsq >= 36.0 { 0.0 } else { 1.0 - lsq.sqrt() / 6.0 }
}

/// Mth.fastInvSqrt
fn fast_inv_sqrt(x: f64) -> f64 {
	let half = 0.5 * x;
	let y = f64::from_bits((0x5FE6_EB50_C7B5_37AA_i64 - ((x.to_bits() as i64) >> 1)) as u64);
	y * (1.5 - half * y * y)
}
