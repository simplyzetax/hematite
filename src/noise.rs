use crate::density::Volume;
use crate::random::Rng;
use crate::interval::Interval;

const GRAD: [[i32; 3]; 16] = [
	[1, 1, 0],
	[-1, 1, 0],
	[1, -1, 0],
	[-1, -1, 0],
	[1, 0, 1],
	[-1, 0, 1],
	[1, 0, -1],
	[-1, 0, -1],
	[0, 1, 1],
	[0, -1, 1],
	[0, 1, -1],
	[0, -1, -1],
	[1, 1, 0],
	[0, -1, 1],
	[-1, 1, 0],
	[0, -1, -1],
];

#[inline]
pub fn floor(v: f64) -> i32 {
	v.floor() as i32
}

#[inline]
pub fn smoothstep(x: f32) -> f32 {
	x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

#[inline]
pub fn lerp(a: f32, p0: f32, p1: f32) -> f32 {
	p0 + a * (p1 - p0)
}

#[inline]
pub fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
	lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub fn lerp3(
	a1: f32,
	a2: f32,
	a3: f32,
	x000: f32,
	x100: f32,
	x010: f32,
	x110: f32,
	x001: f32,
	x101: f32,
	x011: f32,
	x111: f32,
) -> f32 {
	lerp(a3, lerp2(a1, a2, x000, x100, x010, x110), lerp2(a1, a2, x001, x101, x011, x111))
}

#[inline]
fn wrap(x: f64) -> f64 {
	const HALF: f64 = f64::from_bits(16777216f64.to_bits() - 1);
	if (-HALF..HALF).contains(&x) { x } else { x - (x / 33554432.0 + 0.5).floor() * 33554432.0 }
}

#[inline]
fn grad_dot(h: i32, x: f32, y: f32, z: f32) -> f32 {
	let g = GRAD[(h & 15) as usize];
	g[0] as f32 * x + g[1] as f32 * y + g[2] as f32 * z
}

pub struct Perlin {
	perms: [u8; 256],
	ox: f64,
	oy: f64,
	oz: f64,
	smear: Option<f64>,
}

impl Perlin {
	pub fn new(r: &mut impl Rng, smear: Option<f64>) -> Self {
		let ox = r.next_double() * 256.0;
		let oy = r.next_double() * 256.0;
		let oz = r.next_double() * 256.0;
		let mut perms = [0u8; 256];
		for (i, p) in perms.iter_mut().enumerate() {
			*p = i as u8;
		}
		for i in 0..256 {
			let o = r.next_bound(256 - i as i32) as usize;
			perms.swap(i, o + i);
		}
		Self { perms, ox, oy, oz, smear }
	}

	#[inline]
	fn p(&self, x: i32) -> i32 {
		self.perms[(x & 0xFF) as usize] as i32
	}

	fn fudge(&self, scale: f64, orig_y: f64, rel_y: f64) -> f64 {
		let limit = if orig_y >= 0.0 && orig_y < rel_y { orig_y } else { rel_y };
		floor(limit / scale + 1.0E-7f32 as f64) as f64 * scale
	}

	pub fn get(&self, x0: f64, y0: f64, z0: f64) -> f32 {
		let x = wrap(x0) + self.ox;
		let y = wrap(y0) + self.oy;
		let z = wrap(z0) + self.oz;
		let (fx, fy, fz) = (floor(x), floor(y), floor(z));
		let rx = (x - fx as f64) as f32;
		let rz = (z - fz as f64) as f32;
		match self.smear {
			None => {
				let ry = (y - fy as f64) as f32;
				self.sample_lerp(fx, fy, fz, rx, ry, rz, ry)
			}
			Some(scale) => {
				let ry = y - fy as f64;
				let fudged = (ry - self.fudge(scale, y0, ry)) as f32;
				self.sample_lerp(fx, fy, fz, rx, fudged, rz, ry as f32)
			}
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn sample_lerp(&self, x: i32, y: i32, z: i32, rx: f32, ry: f32, rz: f32, ory: f32) -> f32 {
		let x0 = self.p(x);
		let x1 = self.p(x + 1);
		let xy00 = self.p(x0 + y);
		let xy01 = self.p(x0 + y + 1);
		let xy10 = self.p(x1 + y);
		let xy11 = self.p(x1 + y + 1);
		let d000 = grad_dot(self.p(xy00 + z), rx, ry, rz);
		let d100 = grad_dot(self.p(xy10 + z), rx - 1.0, ry, rz);
		let d010 = grad_dot(self.p(xy01 + z), rx, ry - 1.0, rz);
		let d110 = grad_dot(self.p(xy11 + z), rx - 1.0, ry - 1.0, rz);
		let d001 = grad_dot(self.p(xy00 + z + 1), rx, ry, rz - 1.0);
		let d101 = grad_dot(self.p(xy10 + z + 1), rx - 1.0, ry, rz - 1.0);
		let d011 = grad_dot(self.p(xy01 + z + 1), rx, ry - 1.0, rz - 1.0);
		let d111 = grad_dot(self.p(xy11 + z + 1), rx - 1.0, ry - 1.0, rz - 1.0);
		lerp3(smoothstep(rx), smoothstep(ory), smoothstep(rz), d000, d100, d010, d110, d001, d101, d011, d111)
	}

	pub fn add_to_volume(&self, buf: &mut [f32], vol: &Volume, xz: f64, ys: f64, amp: f32, mask: Option<&[bool]>) {
		let mut dxz = [0f32; 8];
		let mut gy = [0f32; 8];
		let mut index = 0;
		for iz in 0..vol.sz {
			let z = wrap(vol.block_z(iz) as f64 * xz) + self.oz;
			let fz = floor(z);
			let rz = (z - fz as f64) as f32;
			let az = smoothstep(rz);
			for ix in 0..vol.sx {
				let x = wrap(vol.block_x(ix) as f64 * xz) + self.ox;
				let fx = floor(x);
				let rx = (x - fx as f64) as f32;
				let x0 = self.p(fx);
				let x1 = self.p(fx + 1);
				let ax = smoothstep(rx);
				let mut last_fy = i32::MIN;
				for iy in 0..vol.sy {
					if let Some(m) = mask
						&& !m[index]
					{
						index += 1;
						continue;
					}
					let oy = vol.block_y(iy) as f64 * ys;
					let y = wrap(oy) + self.oy;
					let fy = floor(y);
					let ry64 = y - fy as f64;
					let ry = ry64 as f32;
					let ay = smoothstep(ry);
					if last_fy != fy {
						let xy00 = self.p(x0 + fy);
						let xy01 = self.p(x0 + fy + 1);
						let xy10 = self.p(x1 + fy);
						let xy11 = self.p(x1 + fy + 1);
						let corners = [
							(xy00 + fz, rx, rz),
							(xy10 + fz, rx - 1.0, rz),
							(xy01 + fz, rx, rz),
							(xy11 + fz, rx - 1.0, rz),
							(xy00 + fz + 1, rx, rz - 1.0),
							(xy10 + fz + 1, rx - 1.0, rz - 1.0),
							(xy01 + fz + 1, rx, rz - 1.0),
							(xy11 + fz + 1, rx - 1.0, rz - 1.0),
						];
						for (i, (h, cx, cz)) in corners.into_iter().enumerate() {
							let g = GRAD[(self.p(h) & 15) as usize];
							dxz[i] = g[0] as f32 * cx + g[2] as f32 * cz;
							gy[i] = g[1] as f32;
						}
						last_fy = fy;
					}
					let ty = match self.smear {
						None => ry,
						Some(scale) => (ry64 - self.fudge(scale, oy, ry64)) as f32,
					};
					let v = lerp3(
						ax,
						ay,
						az,
						dxz[0] + gy[0] * ty,
						dxz[1] + gy[1] * ty,
						dxz[2] + gy[2] * (ty - 1.0),
						dxz[3] + gy[3] * (ty - 1.0),
						dxz[4] + gy[4] * ty,
						dxz[5] + gy[5] * ty,
						dxz[6] + gy[6] * (ty - 1.0),
						dxz[7] + gy[7] * (ty - 1.0),
					);
					buf[index] += amp * v;
					index += 1;
				}
			}
		}
	}
}

pub struct Layer {
	pub noise: Perlin,
	pub freq: f64,
	pub amp: f32,
}

pub struct NoiseStack {
	pub layers: Vec<Layer>,
}

impl NoiseStack {
	pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
		let mut v = 0f32;
		for l in &self.layers {
			v += l.amp * l.noise.get(x * l.freq, y * l.freq, z * l.freq);
		}
		v
	}

	pub fn add_to_volume(&self, buf: &mut [f32], vol: &Volume, xz: f64, y: f64, amp: f32, mask: Option<&[bool]>) {
		for l in &self.layers {
			l.noise.add_to_volume(buf, vol, xz * l.freq, y * l.freq, amp * l.amp, mask);
		}
	}
}

pub struct NoiseParams {
	pub base_amplitude: f64,
	pub base_octave: i32,
	pub octave_count: i32,
	pub amplitude_modifiers: Vec<f64>,
}

struct Octave {
	index: i32,
	freq: f64,
	amp: f64,
}

fn build_octaves(p: &NoiseParams) -> Vec<Octave> {
	let mut freq = 2f64.powi(p.base_octave);
	let n = p.octave_count;
	let mut amp = p.base_amplitude * (0.5f64.powi(-(n - 1)) / (0.5f64.powi(-n) - 1.0));
	let mut out = vec![];
	for i in 0..n {
		let m = if p.amplitude_modifiers.is_empty() { 1.0 } else { p.amplitude_modifiers[i as usize] };
		if m != 0.0 {
			out.push(Octave { index: p.base_octave + i, freq, amp: amp * m });
		}
		freq *= 2.0;
		amp *= 0.5;
	}
	out
}

pub struct NormalNoise {
	octaves: Vec<Octave>,
	norm: f64,
	pub range: Interval,
}

impl NormalNoise {
	pub fn new(p: &NoiseParams) -> Self {
		let octaves = build_octaves(p);
		let target = java_stream_sum(octaves.iter().map(|o| o.amp.abs()));
		let mut variance = 0.0;
		for o in &octaves {
			let d = 0.2702247831245211 * o.amp.abs();
			variance += d * d;
		}
		let dev = variance.sqrt();
		let norm = if dev == 0.0 { 0.0 } else { target * 0.3333333333333333 / (dev * 2f64.sqrt()) };
		let range = Interval::symmetric((target * 0.3333333333333333 * 6.0) as f32);
		Self { octaves, norm, range }
	}

	pub fn create(&self, r: &mut crate::random::Xoroshiro) -> NoiseStack {
		let first = r.fork_positional();
		let second = r.fork_positional();
		let mut layers = vec![];
		for o in &self.octaves {
			let seed = format!("octave_{}", o.index);
			let amp = (self.norm * o.amp) as f32;
			layers.push(Layer { noise: Perlin::new(&mut first.from_hash_of(&seed), None), freq: o.freq, amp });
			layers.push(Layer { noise: Perlin::new(&mut second.from_hash_of(&seed), None), freq: o.freq * 1.0181268882175227, amp });
		}
		NoiseStack { layers }
	}
}

pub fn blended_fbm(r: &mut impl Rng, first_octave: i32, smear_y: f64, mut value_factor: f64) -> NoiseStack {
	let octaves = -first_octave + 1;
	let mut factor = 1.0;
	value_factor /= 2f64.powi(octaves) - 1.0;
	let mut layers = vec![];
	for _ in 0..octaves {
		layers.push(Layer { noise: Perlin::new(r, Some(smear_y * factor)), freq: factor, amp: value_factor as f32 });
		factor /= 2.0;
		value_factor *= 2.0;
	}
	NoiseStack { layers }
}

fn java_stream_sum(values: impl Iterator<Item = f64>) -> f64 {
	let (mut sum, mut comp) = (0.0f64, 0.0f64);
	for v in values {
		let t = v - comp;
		let velvel = sum + t;
		comp = (velvel - sum) - t;
		sum = velvel;
	}
	sum - comp
}

pub struct Simplex {
	perms: [u8; 256],
}

const SQRT_3: f64 = 1.7320508075688772;

impl Simplex {
	pub fn new(r: &mut impl Rng) -> Self {
		for _ in 0..3 {
			r.next_double();
		}
		let mut perms = [0u8; 256];
		for (i, p) in perms.iter_mut().enumerate() {
			*p = i as u8;
		}
		for i in 0..256 {
			let o = r.next_bound(256 - i as i32) as usize;
			perms.swap(i, o + i);
		}
		Self { perms }
	}

	fn p(&self, x: i32) -> i32 {
		self.perms[(x & 0xFF) as usize] as i32
	}

	fn corner(index: i32, x: f64, y: f64) -> f64 {
		let t = 0.5 - x * x - y * y;
		if t < 0.0 {
			return 0.0;
		}
		let t = t * t;
		let g = GRAD[index as usize];
		t * t * (g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * 0.0)
	}

	pub fn get2(&self, xin: f64, yin: f64) -> f32 {
		let f2 = 0.5 * (SQRT_3 - 1.0);
		let g2 = (3.0 - SQRT_3) / 6.0;
		let s = (xin + yin) * f2;
		let i = floor(xin + s);
		let j = floor(yin + s);
		let t = (i + j) as f64 * g2;
		let x0 = xin - (i as f64 - t);
		let y0 = yin - (j as f64 - t);
		let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
		let x1 = x0 - i1 as f64 + g2;
		let y1 = y0 - j1 as f64 + g2;
		let x2 = x0 - 1.0 + 2.0 * g2;
		let y2 = y0 - 1.0 + 2.0 * g2;
		let (ii, jj) = (i & 0xFF, j & 0xFF);
		let gi0 = self.p(ii + self.p(jj)) % 12;
		let gi1 = self.p(ii + i1 + self.p(jj + j1)) % 12;
		let gi2 = self.p(ii + 1 + self.p(jj + 1)) % 12;
		let n = Self::corner(gi0, x0, y0) + Self::corner(gi1, x1, y1) + Self::corner(gi2, x2, y2);
		(70.0 * n) as f32
	}
}
