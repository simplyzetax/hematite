use md5::{Digest, Md5};

const FLOAT_UNIT: f32 = 5.9604645E-8;
const DOUBLE_UNIT: f64 = 1.110223E-16f32 as f64;

pub fn mix_stafford13(mut z: i64) -> i64 {
	z = (z ^ ((z as u64) >> 30) as i64).wrapping_mul(-4658895280553007687);
	z = (z ^ ((z as u64) >> 27) as i64).wrapping_mul(-7723592293110705685);
	z ^ ((z as u64) >> 31) as i64
}

pub fn upgrade_seed(seed: i64) -> (i64, i64) {
	let lo = seed ^ 7640891576956012809;
	let hi = lo.wrapping_add(-7046029254386353131);
	(mix_stafford13(lo), mix_stafford13(hi))
}

pub fn seed_from_hash_of(name: &str) -> (i64, i64) {
	let h = Md5::digest(name.as_bytes());
	(
		i64::from_be_bytes(h[0..8].try_into().unwrap()),
		i64::from_be_bytes(h[8..16].try_into().unwrap()),
	)
}

pub fn pos_seed(x: i32, y: i32, z: i32) -> i64 {
	let mut s = (x.wrapping_mul(3129871) as i64) ^ (z as i64).wrapping_mul(116129781) ^ y as i64;
	s = s.wrapping_mul(s).wrapping_mul(42317861).wrapping_add(s.wrapping_mul(11));
	s >> 16
}

pub trait Rng {
	fn next_long(&mut self) -> i64;
	fn next_int(&mut self) -> i32;
	fn next_bound(&mut self, bound: i32) -> i32;
	fn next_float(&mut self) -> f32;
	fn next_double(&mut self) -> f64;
	fn next_bool(&mut self) -> bool;
	fn set_seed(&mut self, seed: i64);

	fn next_between(&mut self, min: i32, max_inclusive: i32) -> i32 {
		self.next_bound(max_inclusive - min + 1) + min
	}
}

#[derive(Clone)]
pub struct Xoroshiro {
	lo: i64,
	hi: i64,
}

impl Xoroshiro {
	pub fn new(seed: i64) -> Self {
		let (lo, hi) = upgrade_seed(seed);
		Self::from_parts(lo, hi)
	}

	pub fn from_parts(lo: i64, hi: i64) -> Self {
		if lo | hi == 0 {
			return Self { lo: -7046029254386353131, hi: 7640891576956012809 };
		}
		Self { lo, hi }
	}

	pub fn fork_positional(&mut self) -> XoroshiroFactory {
		let lo = self.next_long();
		XoroshiroFactory { lo, hi: self.next_long() }
	}

	fn bits(&mut self, n: u32) -> i64 {
		((self.next_long() as u64) >> (64 - n)) as i64
	}
}

impl Rng for Xoroshiro {
	fn next_long(&mut self) -> i64 {
		let (s0, mut s1) = (self.lo, self.hi);
		let r = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
		s1 ^= s0;
		self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
		self.hi = s1.rotate_left(28);
		r
	}

	fn next_int(&mut self) -> i32 {
		self.next_long() as i32
	}

	fn next_bound(&mut self, bound: i32) -> i32 {
		let bound = bound as i64;
		let mut m = (self.next_int() as u32 as i64).wrapping_mul(bound);
		let mut frac = m & 0xFFFF_FFFF;
		if frac < bound {
			let start = ((bound as u32).wrapping_neg() % bound as u32) as i64;
			while frac < start {
				m = (self.next_int() as u32 as i64).wrapping_mul(bound);
				frac = m & 0xFFFF_FFFF;
			}
		}
		(m >> 32) as i32
	}

	fn next_float(&mut self) -> f32 {
		self.bits(24) as f32 * FLOAT_UNIT
	}

	fn next_double(&mut self) -> f64 {
		self.bits(53) as f64 * DOUBLE_UNIT
	}

	fn next_bool(&mut self) -> bool {
		self.next_long() & 1 != 0
	}

	fn set_seed(&mut self, seed: i64) {
		*self = Self::new(seed);
	}
}

#[derive(Clone, Copy)]
pub struct XoroshiroFactory {
	pub lo: i64,
	pub hi: i64,
}

impl XoroshiroFactory {
	pub fn at(&self, x: i32, y: i32, z: i32) -> Xoroshiro {
		Xoroshiro::from_parts(pos_seed(x, y, z) ^ self.lo, self.hi)
	}

	pub fn from_hash_of(&self, name: &str) -> Xoroshiro {
		let (lo, hi) = seed_from_hash_of(name);
		Xoroshiro::from_parts(lo ^ self.lo, hi ^ self.hi)
	}
}

#[derive(Clone)]
pub struct Legacy {
	seed: i64,
}

impl Legacy {
	pub fn new(seed: i64) -> Self {
		Self { seed: (seed ^ 25214903917) & 0xFFFF_FFFF_FFFF }
	}

	pub fn next(&mut self, bits: u32) -> i32 {
		self.seed = self.seed.wrapping_mul(25214903917).wrapping_add(11) & 0xFFFF_FFFF_FFFF;
		(self.seed >> (48 - bits)) as i32
	}

	pub fn set_large_feature_seed(&mut self, seed: i64, cx: i32, cz: i32) {
		self.set_seed(seed);
		let xs = self.next_long();
		let zs = self.next_long();
		self.set_seed((cx as i64).wrapping_mul(xs) ^ (cz as i64).wrapping_mul(zs) ^ seed);
	}
}

impl Rng for Legacy {
	fn next_long(&mut self) -> i64 {
		let upper = self.next(32);
		let lower = self.next(32);
		((upper as i64) << 32).wrapping_add(lower as i64)
	}

	fn next_int(&mut self) -> i32 {
		self.next(32)
	}

	fn next_bound(&mut self, bound: i32) -> i32 {
		if bound & (bound - 1) == 0 {
			return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
		}
		loop {
			let s = self.next(31);
			let m = s % bound;
			if s.wrapping_sub(m).wrapping_add(bound - 1) >= 0 {
				return m;
			}
		}
	}

	fn next_float(&mut self) -> f32 {
		self.next(24) as f32 * FLOAT_UNIT
	}

	fn next_double(&mut self) -> f64 {
		let upper = self.next(26) as i64;
		let lower = self.next(27) as i64;
		((upper << 27) + lower) as f64 * DOUBLE_UNIT
	}

	fn next_bool(&mut self) -> bool {
		self.next(1) != 0
	}

	fn set_seed(&mut self, seed: i64) {
		*self = Self::new(seed);
	}
}
