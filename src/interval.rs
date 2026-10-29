#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
	pub min: f32,
	pub max: f32,
	pub nai: bool,
}

pub fn jmin(a: f32, b: f32) -> f32 {
	if a.is_nan() {
		a
	} else if a == 0.0 && b == 0.0 && b.to_bits() == 0x8000_0000 {
		b
	} else if a <= b {
		a
	} else {
		b
	}
}

pub fn jmax(a: f32, b: f32) -> f32 {
	if a.is_nan() {
		a
	} else if a == 0.0 && b == 0.0 && a.to_bits() == 0x8000_0000 {
		b
	} else if a >= b {
		a
	} else {
		b
	}
}

const NAI: Interval = Interval { min: f32::NAN, max: f32::NAN, nai: true };

impl Interval {
	pub const INFINITE: Interval = Interval { min: f32::NEG_INFINITY, max: f32::INFINITY, nai: false };

	pub fn of(min: f32, max: f32) -> Self {
		Self { min, max, nai: false }
	}

	pub fn symmetric(r: f32) -> Self {
		Self::of(-r, r)
	}

	pub fn exact(v: f32) -> Self {
		Self::of(v, v)
	}

	pub fn contains(&self, v: f32) -> bool {
		v >= self.min && v <= self.max
	}

	pub fn encapsulating(list: &[Interval]) -> Self {
		let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
		for i in list.iter().filter(|i| !i.nai) {
			min = jmin(i.min, min);
			max = jmax(i.max, max);
		}
		if max < min { NAI } else { Self::of(min, max) }
	}

	pub fn encapsulating2(a: f32, b: f32) -> Self {
		match (a.is_nan(), b.is_nan()) {
			(true, true) => NAI,
			(true, false) => Self::exact(b),
			(false, true) => Self::exact(a),
			_ => Self::of(jmin(a, b), jmax(a, b)),
		}
	}

	pub fn add(l: Self, r: Self) -> Self {
		let (min, max) = (l.min + r.min, l.max + r.max);
		if min.is_nan() || max.is_nan() { NAI } else { Self::of(min, max) }
	}

	pub fn sub(l: Self, r: Self) -> Self {
		let (min, max) = (l.min - r.max, l.max - r.min);
		if min.is_nan() || max.is_nan() { NAI } else { Self::of(min, max) }
	}

	pub fn mul(l: Self, r: Self) -> Self {
		if l.nai || r.nai {
			return NAI;
		}
		let mb = |a: f32, b: f32| if a != 0.0 && b != 0.0 { a * b } else { 0.0 };
		let (a, b, c, d) = (mb(l.min, r.min), mb(l.min, r.max), mb(l.max, r.min), mb(l.max, r.max));
		Self::of(jmin(jmin(a, b), jmin(c, d)), jmax(jmax(a, b), jmax(c, d)))
	}

	pub fn reciprocal(i: Self) -> Self {
		if i.nai || (i.min == 0.0 && i.max == 0.0) {
			NAI
		} else if !i.contains(0.0) {
			Self::of(1.0 / i.max, 1.0 / i.min)
		} else if i.max == 0.0 {
			Self::of(f32::NEG_INFINITY, 1.0 / i.min)
		} else if i.min == 0.0 {
			Self::of(1.0 / i.max, f32::INFINITY)
		} else {
			Self::INFINITE
		}
	}

	pub fn min_of(l: Self, r: Self) -> Self {
		if l.nai || r.nai { NAI } else { Self::of(jmin(l.min, r.min), jmin(l.max, r.max)) }
	}

	pub fn max_of(l: Self, r: Self) -> Self {
		if l.nai || r.nai { NAI } else { Self::of(jmax(l.min, r.min), jmax(l.max, r.max)) }
	}

	pub fn clamp(i: Self, min: f32, max: f32) -> Self {
		if i.nai {
			NAI
		} else if i.min >= max {
			Self::of(max, max)
		} else if i.max <= min {
			Self::of(min, min)
		} else {
			Self::of(jmax(i.min, min), jmin(i.max, max))
		}
	}

	pub fn abs(i: Self) -> Self {
		if i.nai {
			return NAI;
		}
		let max = jmax(i.min.abs(), i.max.abs());
		if i.contains(0.0) { Self::of(0.0, max) } else { Self::of(jmin(i.min.abs(), i.max.abs()), max) }
	}

	pub fn square(i: Self) -> Self {
		if i.nai {
			return NAI;
		}
		let max = jmax(i.min * i.min, i.max * i.max);
		if i.contains(0.0) { Self::of(0.0, max) } else { Self::of(jmin(i.min * i.min, i.max * i.max), max) }
	}

	pub fn map_monotonic(i: Self, f: impl Fn(f32) -> f32) -> Self {
		if i.nai {
			return NAI;
		}
		let (a, b) = (f(i.min), f(i.max));
		Self::of(jmin(a, b), jmax(a, b))
	}

	pub fn lerp(alpha: Self, first: Self, second: Self) -> Self {
		if alpha.nai || first.nai || second.nai {
			return NAI;
		}
		Self::encapsulating(&[
			Self::lerp_bounds(alpha, first.min, second.min),
			Self::lerp_bounds(alpha, first.max, second.min),
			Self::lerp_bounds(alpha, first.min, second.max),
			Self::lerp_bounds(alpha, first.max, second.max),
		])
	}

	fn lerp_bounds(alpha: Self, first: f32, second: f32) -> Self {
		let mb = |a: f32, b: f32| if a != 0.0 && b != 0.0 { a * b } else { 0.0 };
		if first.is_finite() && second.is_finite() {
			return Self::encapsulating2(first + mb(alpha.min, second - first), first + mb(alpha.max, second - first));
		}
		if first == second {
			return Self::exact(first);
		}
		let bound = |a: f32| {
			let (p1, p2) = (mb(1.0 - a, first), mb(a, second));
			if !p1.is_infinite() || !p2.is_infinite() {
				p1 + p2
			} else if a <= 0.0 {
				if second > first { f32::NEG_INFINITY } else { f32::INFINITY }
			} else if a >= 1.0 {
				if second > first { f32::INFINITY } else { f32::NEG_INFINITY }
			} else {
				f32::NAN
			}
		};
		let (a, b) = (bound(alpha.min), bound(alpha.max));
		if a.is_nan() || b.is_nan() { NAI } else { Self::encapsulating2(a, b) }
	}
}
