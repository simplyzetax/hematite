use crate::interval::{jmax, jmin, Interval};
use crate::noise::{blended_fbm, lerp, lerp3, NoiseStack};
use crate::random::XoroshiroFactory;
use crate::registry::Registry;
use serde_json::Value;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Volume {
	pub sx: i32,
	pub sy: i32,
	pub sz: i32,
	pub mx: i32,
	pub my: i32,
	pub mz: i32,
	pub tx: i32,
	pub ty: i32,
	pub tz: i32,
}

impl Volume {
	pub fn new(sx: i32, sy: i32, sz: i32, mx: i32, my: i32, mz: i32) -> Self {
		Self::stepped(sx, sy, sz, mx, my, mz, 1, 1, 1)
	}

	#[allow(clippy::too_many_arguments)]
	pub fn stepped(sx: i32, sy: i32, sz: i32, mx: i32, my: i32, mz: i32, tx: i32, ty: i32, tz: i32) -> Self {
		assert!(sx > 0 && sy > 0 && sz > 0 && tx > 0 && ty > 0 && tz > 0);
		Self { sx, sy, sz, mx, my, mz, tx, ty, tz }
	}

	#[inline]
	pub fn idx(&self, x: i32, y: i32, z: i32) -> usize {
		(y + (x + z * self.sx) * self.sy) as usize
	}

	#[inline]
	pub fn block_x(&self, i: i32) -> i32 {
		self.mx + i * self.tx
	}

	#[inline]
	pub fn block_y(&self, i: i32) -> i32 {
		self.my + i * self.ty
	}

	#[inline]
	pub fn block_z(&self, i: i32) -> i32 {
		self.mz + i * self.tz
	}

	pub fn max_x(&self) -> i32 {
		self.mx + self.sx * self.tx - 1
	}

	pub fn max_y(&self) -> i32 {
		self.my + self.sy * self.ty - 1
	}

	pub fn max_z(&self) -> i32 {
		self.mz + self.sz * self.tz - 1
	}

	pub fn size(&self) -> usize {
		(self.sx * self.sy * self.sz) as usize
	}

	pub fn index_of(&self, bx: i32, by: i32, bz: i32) -> Option<usize> {
		let (rx, ry, rz) = (bx - self.mx, by - self.my, bz - self.mz);
		if self.tx == 1 && self.ty == 1 && self.tz == 1 {
			if rx >= 0 && ry >= 0 && rz >= 0 && rx < self.sx && ry < self.sy && rz < self.sz {
				return Some(self.idx(rx, ry, rz));
			}
		} else if rx >= 0
			&& ry >= 0 && rz >= 0
			&& rx < self.sx * self.tx
			&& ry < self.sy * self.ty
			&& rz < self.sz * self.tz
			&& rx.rem_euclid(self.tx) == 0
			&& ry.rem_euclid(self.ty) == 0
			&& rz.rem_euclid(self.tz) == 0
		{
			return Some(self.idx(rx / self.tx, ry / self.ty, rz / self.tz));
		}
		None
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Un {
	Abs,
	Square,
	Cube,
	Sqrt,
	HalfNeg,
	QuarterNeg,
	Recip,
	Negate,
	Squeeze,
	Log,
	Sign,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bin {
	Add,
	Sub,
	Mul,
	Div,
	Min,
	Max,
}

#[derive(Clone, Debug)]
pub enum Spline<C> {
	Const(f32),
	Multi { coord: C, locs: Vec<f32>, vals: Vec<Spline<C>>, ders: Vec<f32> },
}

type R = Rc<Df>;

#[derive(Clone, Debug)]
pub enum Df {
	Const(f32),
	Ref(Rc<str>),
	BlendAlpha,
	BlendOffset,
	Beardifier,
	Noise { noise: Rc<str>, xz: f64, y: f64, sx: R, sy: R, sz: R },
	ShiftA(Rc<str>),
	ShiftB(Rc<str>),
	Shift(Rc<str>),
	Gradient { axis: u8, tiling: u8, from: i32, to: i32, fv: f32, tv: f32 },
	Unary(Un, R),
	Binary(Bin, R, R),
	Clamp(R, f32, f32),
	Spline(Rc<Spline<R>>),
	Lerp(R, R, R),
	RangeChoice(R, f32, f32, R, R),
	Select(R, Vec<f32>, Vec<R>),
	Cache(R),
	Prepared { id: usize, range: Interval, axes: u8 },
	BlendDensity(R),
	Interpolated(R, i32, i32),
	Slice(u8, i32, R),
	FindTop(R, R, i32, i32),
	OldBlended { xz_scale: f64, y_scale: f64, xz_factor: f64, y_factor: f64, smear: f64 },
}

fn axis_bit(a: u8) -> u8 {
	[1, 2, 4][a as usize]
}

fn axis_of(v: &Value) -> u8 {
	match v.as_str().unwrap() {
		"x" => 0,
		"y" => 1,
		"z" => 2,
		a => panic!("axis {a}"),
	}
}

fn f(v: &Value, k: &str) -> f64 {
	v[k].as_f64().unwrap_or_else(|| panic!("missing {k} in {v}"))
}

pub fn parse(v: &Value) -> R {
	if let Some(n) = v.as_f64() {
		return Rc::new(Df::Const(n as f32));
	}
	if let Some(s) = v.as_str() {
		return Rc::new(Df::Ref(s.into()));
	}
	let ty = v["type"].as_str().unwrap().trim_start_matches("minecraft:");
	let p = |k: &str| parse(&v[k]);
	let opt = |k: &str| if v.get(k).is_some() { parse(&v[k]) } else { Rc::new(Df::Const(0.0)) };
	let name = |k: &str| -> Rc<str> { v[k].as_str().unwrap().into() };
	let un = |u| Df::Unary(u, p("input"));
	let bin = |b| Df::Binary(b, p("left"), p("right"));
	Rc::new(match ty {
		"constant" => Df::Const(f(v, "argument") as f32),
		"blend_alpha" => Df::BlendAlpha,
		"blend_offset" => Df::BlendOffset,
		"beardifier" => Df::Beardifier,
		"noise" => Df::Noise {
			noise: name("noise"),
			xz: f(v, "xz_scale"),
			y: f(v, "y_scale"),
			sx: opt("shift_x"),
			sy: opt("shift_y"),
			sz: opt("shift_z"),
		},
		"shift_a" => Df::ShiftA(name("noise")),
		"shift_b" => Df::ShiftB(name("noise")),
		"shift" => Df::Shift(name("noise")),
		"gradient" => Df::Gradient {
			axis: axis_of(&v["axis"]),
			tiling: match v.get("tiling").and_then(|t| t.as_str()).unwrap_or("clamp_to_edge") {
				"clamp_to_edge" => 0,
				"repeat" => 1,
				_ => 2,
			},
			from: f(v, "from_coordinate") as i32,
			to: f(v, "to_coordinate") as i32,
			fv: f(v, "from_value") as f32,
			tv: f(v, "to_value") as f32,
		},
		"abs" => un(Un::Abs),
		"square" => un(Un::Square),
		"cube" => un(Un::Cube),
		"sqrt" => un(Un::Sqrt),
		"half_negative" => un(Un::HalfNeg),
		"quarter_negative" => un(Un::QuarterNeg),
		"reciprocal" => un(Un::Recip),
		"negate" => un(Un::Negate),
		"squeeze" => un(Un::Squeeze),
		"log" => un(Un::Log),
		"sign" => un(Un::Sign),
		"add" => bin(Bin::Add),
		"sub" => bin(Bin::Sub),
		"mul" => bin(Bin::Mul),
		"div" => bin(Bin::Div),
		"min" => bin(Bin::Min),
		"max" => bin(Bin::Max),
		"clamp" => Df::Clamp(p("input"), f(v, "min") as f32, f(v, "max") as f32),
		"spline" => Df::Spline(Rc::new(parse_spline(&v["spline"]))),
		"lerp" => Df::Lerp(p("alpha"), p("first"), p("second")),
		"range_choice" => Df::RangeChoice(
			p("input"),
			f(v, "min_inclusive") as f32,
			f(v, "max_exclusive") as f32,
			p("when_in_range"),
			p("when_out_of_range"),
		),
		"interval_select" => Df::Select(
			p("input"),
			v["thresholds"].as_array().unwrap().iter().map(|t| t.as_f64().unwrap() as f32).collect(),
			v["functions"].as_array().unwrap().iter().map(parse).collect(),
		),
		"cache" => Df::Cache(p("input")),
		"blend_density" => Df::BlendDensity(p("input")),
		"interpolated" => Df::Interpolated(p("input"), f(v, "cell_size_xz") as i32, f(v, "cell_size_y") as i32),
		"slice" => Df::Slice(axis_of(&v["axis"]), f(v, "coordinate") as i32, p("input")),
		"find_top_surface" => {
			Df::FindTop(p("density"), p("upper_bound"), f(v, "lower_bound") as i32, f(v, "cell_height") as i32)
		}
		"old_blended_noise" => Df::OldBlended {
			xz_scale: f(v, "xz_scale"),
			y_scale: f(v, "y_scale"),
			xz_factor: f(v, "xz_factor"),
			y_factor: f(v, "y_factor"),
			smear: f(v, "smear_scale_multiplier"),
		},
		t => panic!("unsupported density function type {t}"),
	})
}

fn parse_spline(v: &Value) -> Spline<R> {
	if let Some(n) = v.as_f64() {
		return Spline::Const(n as f32);
	}
	let pts = v["points"].as_array().unwrap();
	Spline::Multi {
		coord: parse(&v["coordinate"]),
		locs: pts.iter().map(|p| f(p, "location") as f32).collect(),
		vals: pts.iter().map(|p| parse_spline(&p["value"])).collect(),
		ders: pts.iter().map(|p| f(p, "derivative") as f32).collect(),
	}
}

fn key(d: &Df) -> String {
	format!("{d:?}")
}

pub type Id = u32;
const MAX_SPLINE_COORDS: usize = 16;

#[derive(Debug)]
pub enum S {
	Const(f32),
	Noise { n: usize, xz: f64, y: f64 },
	ShiftedXz { sx: Id, sz: Id, n: usize, xz: f64, y: f64 },
	ShiftedXyz { sx: Id, sy: Id, sz: Id, n: usize, xz: f64, y: f64 },
	ShiftB { n: usize },
	Gradient { axis: u8, tiling: u8, from: i32, min: i32, max: i32, range: i32, fv: f32, factor: f32 },
	Cache { id: usize, input: Id },
	Clamp(Id, f32, f32),
	Add(Id, Id),
	ConstAdd(Id, f32),
	Sub(Id, Id),
	ConstSub(f32, Id),
	Mul(Id, Id),
	ConstMul(Id, f32),
	Div(Id, Id),
	ConstDiv(f32, Id),
	Min(Id, Id, f32),
	ConstMin(Id, f32),
	Max(Id, Id, f32),
	ConstMax(Id, f32),
	Unary(Un, Id),
	Interp { input: Id, cxz: i32, cy: i32, ixz: f32, iy: f32 },
	SliceX(Id, i32),
	SliceY(Id, i32),
	SliceZ(Id, i32),
	SliceXz(Id, i32, i32),
	Spline { spline: Spline<usize>, coords: Vec<Id> },
	Lerp(Id, Id, Id),
	LerpConstFirst(Id, f32, Id),
	LerpConstSecond(Id, Id, f32),
	RangeChoice { input: Id, min: f32, max: f32, inr: Id, outr: Id },
	RangeConst { input: Id, min: f32, max: f32, inr: f32, outr: f32 },
	Select { input: Id, thresholds: Vec<f32>, samplers: Vec<Id> },
	Single { input: Id, t: f32, below: Id, above: Id },
	FindTop { density: Id, upper: Id, lower: i32, cell: i32 },
	Beardifier,
}

pub struct Compiler<'a> {
	reg: &'a Registry,
	random: XoroshiroFactory,
	pub nodes: Vec<S>,
	pub noises: Vec<NoiseStack>,
	noise_ids: HashMap<String, usize>,
	blended: Option<[usize; 3]>,
	prepared: HashMap<String, R>,
	pub cache_count: usize,
	cache_nodes: Vec<Id>,
	top: HashMap<String, Id>,
}

impl<'a> Compiler<'a> {
	pub fn new(reg: &'a Registry, random: XoroshiroFactory) -> Self {
		Self {
			reg,
			random,
			nodes: vec![],
			noises: vec![],
			noise_ids: HashMap::new(),
			blended: None,
			prepared: HashMap::new(),
			cache_count: 0,
			cache_nodes: vec![],
			top: HashMap::new(),
		}
	}

	pub fn noise(&mut self, name: &str) -> usize {
		if let Some(&i) = self.noise_ids.get(name) {
			return i;
		}
		let stack = self.reg.normal_noise(name).create(&mut self.random.from_hash_of(name));
		self.noises.push(stack);
		self.noise_ids.insert(name.to_string(), self.noises.len() - 1);
		self.noises.len() - 1
	}

	pub fn sampler(&mut self, f: &R) -> Id {
		let k = key(f);
		if let Some(&id) = self.top.get(&k) {
			return id;
		}
		let opt = self.optimize(f);
		let id = self.compile(&opt);
		self.top.insert(k, id);
		id
	}

	pub fn sampler_named(&mut self, name: &str) -> Id {
		self.sampler(&Rc::new(Df::Ref(name.into())))
	}

	fn optimize(&mut self, f: &R) -> R {
		let r = self.inline_and_cache(f);
		self.slice(7, &r)
	}

	fn resolve(&self, f: &R) -> R {
		let mut f = f.clone();
		while let Df::Ref(n) = &*f {
			f = self.reg.function(n);
		}
		f
	}

	fn inline_and_cache(&mut self, f: &R) -> R {
		let f = self.resolve(f);
		if let Df::Cache(input) = &*f {
			return self.prepare(input);
		}
		self.map_children(&f, &mut |c, s| s.inline_and_cache(c))
	}

	fn prepare(&mut self, input: &R) -> R {
		let k = key(input);
		if let Some(p) = self.prepared.get(&k) {
			return p.clone();
		}
		let id = self.cache_count;
		self.cache_count += 1;
		let inp = self.optimize(input);
		let compiled = self.compile(&inp);
		let node = self.push(S::Cache { id, input: compiled });
		self.cache_nodes.resize(self.cache_count, 0);
		self.cache_nodes[id] = node;
		let p = Rc::new(Df::Prepared { id, range: self.range(&inp), axes: self.axes(&inp) });
		self.prepared.insert(k, p.clone());
		p
	}

	fn slice(&mut self, parent: u8, f: &R) -> R {
		if matches!(**f, Df::Const(_) | Df::Gradient { .. }) {
			return f.clone();
		}
		let axes = self.axes(f);
		if parent == axes {
			return self.map_children(f, &mut |c, s| s.slice(parent, c));
		}
		let mut nf = self.map_children(f, &mut |c, s| s.slice(axes, c));
		let mut existing = 0;
		let mut g = &nf;
		while let Df::Slice(a, _, input) = &**g {
			existing |= axis_bit(*a);
			g = input;
		}
		let filtered = (parent & !axes) & !existing;
		if filtered & 1 != 0 {
			nf = Rc::new(Df::Slice(0, 0, nf));
		}
		if filtered & 4 != 0 {
			nf = Rc::new(Df::Slice(2, 0, nf));
		}
		if filtered & 2 != 0 {
			nf = Rc::new(Df::Slice(1, 0, nf));
		}
		nf
	}

	fn map_children(&mut self, f: &R, m: &mut dyn FnMut(&R, &mut Self) -> R) -> R {
		use Df::*;
		Rc::new(match &**f {
			Noise { noise, xz, y, sx, sy, sz } => {
				let (sx, sy, sz) = (m(sx, self), m(sy, self), m(sz, self));
				Noise { noise: noise.clone(), xz: *xz, y: *y, sx, sy, sz }
			}
			Unary(u, i) => Unary(*u, m(i, self)),
			Binary(b, l, r) => {
				let l = m(l, self);
				Binary(*b, l, m(r, self))
			}
			Clamp(i, a, b) => Clamp(m(i, self), *a, *b),
			Spline(s) => Spline(Rc::new(self.map_spline(s, m))),
			Lerp(a, b, c) => {
				let a = m(a, self);
				let b = m(b, self);
				Lerp(a, b, m(c, self))
			}
			RangeChoice(i, a, b, x, y) => {
				let i = m(i, self);
				let x = m(x, self);
				RangeChoice(i, *a, *b, x, m(y, self))
			}
			Select(i, t, fs) => {
				let i = m(i, self);
				Select(i, t.clone(), fs.iter().map(|c| m(c, self)).collect())
			}
			Cache(i) => Cache(m(i, self)),
			BlendDensity(i) => BlendDensity(m(i, self)),
			Interpolated(i, a, b) => Interpolated(m(i, self), *a, *b),
			Slice(a, c, i) => Slice(*a, *c, m(i, self)),
			FindTop(d, u, l, c) => {
				let d = m(d, self);
				FindTop(d, m(u, self), *l, *c)
			}
			Ref(_) => panic!("unresolved reference in rewrite"),
			_ => return f.clone(),
		})
	}

	fn map_spline(&mut self, s: &Spline<R>, m: &mut dyn FnMut(&R, &mut Self) -> R) -> Spline<R> {
		match s {
			Spline::Const(v) => Spline::Const(*v),
			Spline::Multi { coord, locs, vals, ders } => {
				let coord = m(coord, self);
				Spline::Multi {
					coord,
					locs: locs.clone(),
					vals: vals.iter().map(|v| self.map_spline(v, m)).collect(),
					ders: ders.clone(),
				}
			}
		}
	}

	pub fn axes(&self, f: &Df) -> u8 {
		use Df::*;
		match f {
			Const(_) => 0,
			Ref(n) => self.axes(&self.reg.function(n)),
			BlendAlpha | BlendOffset | ShiftA(_) | ShiftB(_) => 5,
			Beardifier | Shift(_) | OldBlended { .. } => 7,
			Noise { xz, y, sx, sy, sz, .. } => {
				let mut a = 7;
				if *y == 0.0 {
					a &= !2;
				}
				if *xz == 0.0 {
					a &= !5;
				}
				a | self.axes(sx) | self.axes(sy) | self.axes(sz)
			}
			Gradient { axis, .. } => axis_bit(*axis),
			Unary(_, i) | Clamp(i, _, _) | Cache(i) | BlendDensity(i) | Interpolated(i, _, _) => self.axes(i),
			Binary(_, l, r) => self.axes(l) | self.axes(r),
			Spline(s) => self.spline_axes(s),
			Lerp(a, b, c) => self.axes(a) | self.axes(b) | self.axes(c),
			RangeChoice(i, _, _, a, b) => self.axes(i) | self.axes(a) | self.axes(b),
			Select(i, _, fs) => fs.iter().fold(self.axes(i), |a, c| a | self.axes(c)),
			Prepared { axes, .. } => *axes,
			Slice(a, _, i) => self.axes(i) & !axis_bit(*a),
			FindTop(d, u, _, _) => (self.axes(d) | self.axes(u)) & !2,
		}
	}

	fn spline_axes(&self, s: &Spline<R>) -> u8 {
		match s {
			Spline::Const(_) => 0,
			Spline::Multi { coord, vals, .. } => vals.iter().fold(self.axes(coord), |a, v| a | self.spline_axes(v)),
		}
	}

	/// The lowest multiple of 8 above which `f` is surely below zero, structure pieces aside (from its range there).
	pub fn negative_above(&self, f: &R) -> Option<i32> {
		// ponytail: margin for float rounding in interpolation, far above what it can add up to
		let neg = |y: i32| self.range_from_y(f, y).max < -1e-3;
		neg(320).then(|| (-64..=320).step_by(8).find(|&y| neg(y)).unwrap())
	}

	/// `range`, but only over points at or above y0; Beardifier counts as 0 there.
	fn range_from_y(&self, f: &Df, y0: i32) -> Interval {
		use Df::*;
		let r = |g: &Df| self.range_from_y(g, y0);
		match f {
			Ref(n) => r(&self.reg.function(n)),
			Beardifier => Interval::exact(0.0),
			Gradient { axis: 1, tiling: 0, from, to, fv, tv } => {
				let (lo, hi) = (*from.min(to), *from.max(to));
				let at = |c: i32| (*fv as f64 + (c.clamp(lo, hi) - from) as f64 * (*tv as f64 - *fv as f64) / (to - from) as f64) as f32;
				Interval::encapsulating2(at(y0), at(hi))
			}
			Interpolated(i, _, cy) => self.range_from_y(i, y0.div_euclid(*cy) * cy),
			Lerp(a, b, c) => {
				let ra = r(a);
				if (ra.min, ra.max) == (0.0, 0.0) {
					r(b)
				} else if (ra.min, ra.max) == (1.0, 1.0) {
					r(c)
				} else {
					Interval::lerp(ra, r(b), r(c))
				}
			}
			RangeChoice(i, min, max, a, b) => {
				let ri = r(i);
				if ri.nai {
					Interval::encapsulating(&[r(a), r(b)])
				} else if ri.min >= *min && ri.max < *max {
					r(a)
				} else if ri.max < *min || ri.min >= *max {
					r(b)
				} else {
					Interval::encapsulating(&[r(a), r(b)])
				}
			}
			Unary(u, i) => self.unary_range(*u, r(i)),
			Binary(b, x, y) => self.binary_range(*b, r(x), r(y)),
			Clamp(i, a, b) => Interval::clamp(r(i), *a, *b),
			Cache(i) | BlendDensity(i) | Slice(_, _, i) => r(i),
			_ => self.range(f),
		}
	}

	fn unary_range(&self, u: Un, r: Interval) -> Interval {
		match u {
			Un::Abs => Interval::abs(r),
			Un::Square => Interval::square(r),
			Un::Cube => Interval::map_monotonic(r, |x| x * x * x),
			Un::HalfNeg => Interval::map_monotonic(r, |x| if x > 0.0 { x } else { x * 0.5 }),
			Un::QuarterNeg => Interval::map_monotonic(r, |x| if x > 0.0 { x } else { x * 0.25 }),
			Un::Recip => Interval::reciprocal(r),
			Un::Negate => Interval::sub(Interval::exact(0.0), r),
			Un::Squeeze => Interval::map_monotonic(r, squeeze),
			u => panic!("range of {u:?} not supported"),
		}
	}

	fn binary_range(&self, b: Bin, l: Interval, r: Interval) -> Interval {
		match b {
			Bin::Add => Interval::add(l, r),
			Bin::Sub => Interval::sub(l, r),
			Bin::Mul => Interval::mul(l, r),
			Bin::Div => Interval::mul(l, Interval::reciprocal(r)),
			Bin::Min => Interval::min_of(l, r),
			Bin::Max => Interval::max_of(l, r),
		}
	}

	pub fn range(&self, f: &Df) -> Interval {
		use Df::*;
		match f {
			Const(v) => Interval::exact(*v),
			Ref(n) => self.range(&self.reg.function(n)),
			BlendAlpha => Interval::of(0.0, 1.0),
			BlendOffset | Beardifier => Interval::INFINITE,
			Noise { noise, .. } => self.reg.normal_noise(noise).range,
			ShiftA(n) | ShiftB(n) | Shift(n) => Interval::mul(self.reg.normal_noise(n).range, Interval::exact(4.0)),
			Gradient { fv, tv, .. } => Interval::encapsulating2(*fv, *tv),
			Unary(u, i) => self.unary_range(*u, self.range(i)),
			Binary(b, l, r) => self.binary_range(*b, self.range(l), self.range(r)),
			Clamp(i, a, b) => Interval::clamp(self.range(i), *a, *b),
			Spline(s) => self.spline_range(s),
			Lerp(a, b, c) => Interval::lerp(self.range(a), self.range(b), self.range(c)),
			RangeChoice(_, _, _, a, b) => Interval::encapsulating(&[self.range(a), self.range(b)]),
			Select(_, _, fs) => Interval::encapsulating(&fs.iter().map(|c| self.range(c)).collect::<Vec<_>>()),
			Cache(i) | BlendDensity(i) | Interpolated(i, _, _) | Slice(_, _, i) => self.range(i),
			Prepared { range, .. } => *range,
			FindTop(_, u, lower, _) => Interval::of(*lower as f32, jmax(*lower as f32, self.range(u).max)),
			OldBlended { y_scale, smear, .. } => {
				let smear_y = 684.412 * y_scale * smear;
				let mut factor = 1.0;
				let mut vf = 0.99998474f32 as f64 / (2f64.powi(16) - 1.0);
				let mut r = Interval::exact(0.0);
				for _ in 0..16 {
					let layer = Interval::symmetric(((smear_y * factor).abs() + 2.0) as f32);
					r = Interval::add(r, Interval::mul(layer, Interval::exact(vf as f32)));
					factor /= 2.0;
					vf *= 2.0;
				}
				r
			}
		}
	}

	fn spline_range(&self, s: &Spline<R>) -> Interval {
		let Spline::Multi { coord, locs, vals, ders } = s else {
			let Spline::Const(v) = s else { unreachable!() };
			return Interval::exact(*v);
		};
		let last = locs.len() - 1;
		let (mut minv, mut maxv) = (f32::INFINITY, f32::NEG_INFINITY);
		let input = self.range(coord);
		if input.nai {
			return input;
		}
		let ext = |inp: f32, val: f32, i: usize| if ders[i] == 0.0 { val } else { val + ders[i] * (inp - locs[i]) };
		if input.min < locs[0] {
			let r = self.spline_range(&vals[0]);
			let (a, b) = (ext(input.min, r.min, 0), ext(input.min, r.max, 0));
			minv = jmin(minv, jmin(a, b));
			maxv = jmax(maxv, jmax(a, b));
		}
		if input.max > locs[last] {
			let r = self.spline_range(&vals[last]);
			let (a, b) = (ext(input.max, r.min, last), ext(input.max, r.max, last));
			minv = jmin(minv, jmin(a, b));
			maxv = jmax(maxv, jmax(a, b));
		}
		let vr: Vec<Interval> = vals.iter().map(|v| self.spline_range(v)).collect();
		for r in &vr {
			minv = jmin(minv, r.min);
			maxv = jmax(maxv, r.max);
		}
		for i in 0..last {
			let xd = locs[i + 1] - locs[i];
			let (r1, r2) = (vr[i], vr[i + 1]);
			let (d1, d2) = (ders[i], ders[i + 1]);
			if d1 != 0.0 || d2 != 0.0 {
				let p1 = d1 * xd;
				let p2 = d2 * xd;
				let min_l1 = jmin(r1.min, r2.min);
				let max_l1 = jmax(r1.max, r2.max);
				let min_a = p1 - r2.max + r1.min;
				let max_a = p1 - r2.min + r1.max;
				let min_b = -p2 + r2.min - r1.max;
				let max_b = -p2 + r2.max - r1.min;
				minv = jmin(minv, min_l1 + 0.25 * jmin(min_a, min_b));
				maxv = jmax(maxv, max_l1 + 0.25 * jmax(max_a, max_b));
			}
		}
		Interval::of(minv, maxv)
	}

	fn push(&mut self, s: S) -> Id {
		self.nodes.push(s);
		(self.nodes.len() - 1) as Id
	}

	fn const_of(f: &Df) -> Option<f32> {
		if let Df::Const(v) = f { Some(*v) } else { None }
	}

	fn compile(&mut self, f: &R) -> Id {
		use Df::*;
		match &**f {
			Const(v) => self.push(S::Const(*v)),
			BlendAlpha => self.push(S::Const(1.0)),
			BlendOffset => self.push(S::Const(0.0)),
			Beardifier => self.push(S::Beardifier),
			Ref(_) => panic!("unresolved reference in compile"),
			Noise { noise, xz, y, sx, sy, sz } => {
				let n = self.noise(noise);
				let zero = |d: &Df| matches!(d, Const(v) if v.to_bits() == 0);
				if zero(sx) && zero(sy) && zero(sz) {
					return self.push(S::Noise { n, xz: *xz, y: *y });
				}
				let csx = self.compile(sx);
				let csz = self.compile(sz);
				if zero(sy) {
					self.push(S::ShiftedXz { sx: csx, sz: csz, n, xz: *xz, y: *y })
				} else {
					let csy = self.compile(sy);
					self.push(S::ShiftedXyz { sx: csx, sy: csy, sz: csz, n, xz: *xz, y: *y })
				}
			}
			ShiftA(name) => {
				let n = self.noise(name);
				let inner = self.push(S::Noise { n, xz: 0.25, y: 0.0 });
				self.push(S::ConstMul(inner, 4.0))
			}
			Shift(name) => {
				let n = self.noise(name);
				let inner = self.push(S::Noise { n, xz: 0.25, y: 0.25 });
				self.push(S::ConstMul(inner, 4.0))
			}
			ShiftB(name) => {
				let n = self.noise(name);
				self.push(S::ShiftB { n })
			}
			Gradient { axis, tiling, from, to, fv, tv } => {
				let range = to - from;
				let factor = (tv - fv) / range as f32;
				self.push(S::Gradient {
					axis: *axis,
					tiling: *tiling,
					from: *from,
					min: (*from).min(*to),
					max: (*from).max(*to),
					range,
					fv: *fv,
					factor,
				})
			}
			Unary(u, i) => {
				let i = self.compile(i);
				self.push(S::Unary(*u, i))
			}
			Binary(b, l, r) => {
				let cl = self.compile(l);
				let cr = self.compile(r);
				let (lc, rc) = (Self::const_of(l), Self::const_of(r));
				let s = match b {
					Bin::Add => match (lc, rc) {
						(Some(c), _) => S::ConstAdd(cr, c),
						(_, Some(c)) => S::ConstAdd(cl, c),
						_ => S::Add(cl, cr),
					},
					Bin::Sub => match (lc, rc) {
						(Some(c), _) => S::ConstSub(c, cr),
						(_, Some(c)) => S::ConstAdd(cl, -c),
						_ => S::Sub(cl, cr),
					},
					Bin::Mul => match (lc, rc) {
						(Some(c), _) => S::ConstMul(cr, c),
						(_, Some(c)) => S::ConstMul(cl, c),
						_ => S::Mul(cl, cr),
					},
					Bin::Div => match (lc, rc) {
						(Some(c), _) => S::ConstDiv(c, cr),
						(_, Some(c)) => S::ConstMul(cl, 1.0 / c),
						_ => S::Div(cl, cr),
					},
					Bin::Min => {
						let (lr, rr) = (self.range(l), self.range(r));
						if lr.max < rr.min {
							return cl;
						} else if rr.max < lr.min {
							return cr;
						}
						match (lc, rc) {
							(Some(c), _) => S::ConstMin(cr, c),
							(_, Some(c)) => S::ConstMin(cl, c),
							_ => S::Min(cl, cr, rr.min),
						}
					}
					Bin::Max => {
						let (lr, rr) = (self.range(l), self.range(r));
						if lr.min > rr.max {
							return cl;
						} else if rr.min > lr.max {
							return cr;
						}
						match (lc, rc) {
							(Some(c), _) => S::ConstMax(cr, c),
							(_, Some(c)) => S::ConstMax(cl, c),
							_ => S::Max(cl, cr, rr.max),
						}
					}
				};
				self.push(s)
			}
			Clamp(i, a, b) => {
				let i = self.compile(i);
				self.push(S::Clamp(i, *a, *b))
			}
			Spline(s) => {
				let mut keys: Vec<String> = vec![];
				let mut coords: Vec<Id> = vec![];
				let spline = self.compile_spline(s, &mut keys, &mut coords);
				assert!(coords.len() <= MAX_SPLINE_COORDS);
				self.push(S::Spline { spline, coords })
			}
			Lerp(a, b, c) => {
				let (ca, cb, cc) = (self.compile(a), self.compile(b), self.compile(c));
				if let Some(v) = Self::const_of(b) {
					self.push(S::LerpConstFirst(ca, v, cc))
				} else if let Some(v) = Self::const_of(c) {
					self.push(S::LerpConstSecond(ca, cb, v))
				} else {
					self.push(S::Lerp(ca, cb, cc))
				}
			}
			RangeChoice(i, min, max, a, b) => {
				let input = self.compile(i);
				if let (Some(inr), Some(outr)) = (Self::const_of(a), Self::const_of(b)) {
					return self.push(S::RangeConst { input, min: *min, max: *max, inr, outr });
				}
				let inr = self.compile(a);
				let outr = self.compile(b);
				self.push(S::RangeChoice { input, min: *min, max: *max, inr, outr })
			}
			Select(i, ts, fs) => {
				let input = self.compile(i);
				if ts.len() == 1 {
					let below = self.compile(&fs[0]);
					let above = self.compile(fs.last().unwrap());
					return self.push(S::Single { input, t: ts[0], below, above });
				}
				let samplers = fs.iter().map(|c| self.compile(c)).collect();
				self.push(S::Select { input, thresholds: ts.clone(), samplers })
			}
			Cache(_) => panic!("cache must be prepared before compile"),
			Prepared { id, .. } => self.cache_sampler(*id),
			BlendDensity(i) => self.compile(i),
			Interpolated(i, cxz, cy) => {
				let input = self.compile(i);
				self.push(S::Interp { input, cxz: *cxz, cy: *cy, ixz: 1.0 / *cxz as f32, iy: 1.0 / *cy as f32 })
			}
			Slice(a, c, i) => {
				if let Slice(ia, ic, ii) = &**i
					&& ((*a == 0 && *ia == 2) || (*a == 2 && *ia == 0))
				{
					let (x, z) = if *a == 0 { (*c, *ic) } else { (*ic, *c) };
					let inner = self.compile(ii);
					return self.push(S::SliceXz(inner, x, z));
				}
				let input = self.compile(i);
				self.push(match a {
					0 => S::SliceX(input, *c),
					1 => S::SliceY(input, *c),
					_ => S::SliceZ(input, *c),
				})
			}
			FindTop(d, u, lower, cell) => {
				let density = self.compile(d);
				let upper = self.compile(u);
				let inner = self.push(S::FindTop { density, upper, lower: *lower, cell: *cell });
				self.push(S::SliceY(inner, 0))
			}
			OldBlended { xz_scale, y_scale, xz_factor, y_factor, smear } => {
				let [min_n, max_n, main_n] = self.blended_noises(*y_scale, *y_factor, *smear);
				let xzm = 684.412 * xz_scale;
				let ym = 684.412 * y_scale;
				let min_s = self.push(S::Noise { n: min_n, xz: xzm, y: ym });
				let max_s = self.push(S::Noise { n: max_n, xz: xzm, y: ym });
				let main_s = self.push(S::Noise { n: main_n, xz: xzm / xz_factor, y: ym / y_factor });
				let add = self.push(S::ConstAdd(main_s, 0.5));
				let choice = self.push(S::Clamp(add, 0.0, 1.0));
				self.push(S::Lerp(choice, min_s, max_s))
			}
		}
	}

	fn cache_sampler(&self, id: usize) -> Id {
		self.cache_nodes[id]
	}

	fn blended_noises(&mut self, y_scale: f64, y_factor: f64, smear: f64) -> [usize; 3] {
		if let Some(b) = self.blended {
			return b;
		}
		let mut r = self.random.from_hash_of("minecraft:terrain");
		let limit_smear = 684.412 * y_scale * smear;
		let main_smear = limit_smear / y_factor;
		let a = blended_fbm(&mut r, -15, limit_smear, 0.99998474f32 as f64);
		let b = blended_fbm(&mut r, -15, limit_smear, 0.99998474f32 as f64);
		let c = blended_fbm(&mut r, -7, main_smear, 12.75);
		let base = self.noises.len();
		self.noises.extend([a, b, c]);
		self.blended = Some([base, base + 1, base + 2]);
		self.blended.unwrap()
	}

	fn compile_spline(&mut self, s: &Spline<R>, keys: &mut Vec<String>, coords: &mut Vec<Id>) -> Spline<usize> {
		match s {
			Spline::Const(v) => Spline::Const(*v),
			Spline::Multi { coord, locs, vals, ders } => {
				let k = key(coord);
				let slot = match keys.iter().position(|e| *e == k) {
					Some(i) => i,
					None => {
						keys.push(k);
						let c = self.compile(coord);
						coords.push(c);
						coords.len() - 1
					}
				};
				Spline::Multi {
					coord: slot,
					locs: locs.clone(),
					vals: vals.iter().map(|v| self.compile_spline(v, keys, coords)).collect(),
					ders: ders.clone(),
				}
			}
		}
	}
}

#[inline(always)]
fn squeeze(x: f32) -> f32 {
	let c = clampf(x, -1.0, 1.0);
	c / 2.0 - c * c * c / 24.0
}

#[inline]
pub fn clampf(v: f32, min: f32, max: f32) -> f32 {
	if v < min { min } else { jmin(v, max) }
}

fn jsignum(v: f32) -> f32 {
	if v == 0.0 || v.is_nan() { v } else { 1f32.copysign(v) }
}

/// `unary` over a slice, with the op resolved once so each loop is tight.
fn unary_all(u: Un, out: &mut [f32]) {
	#[inline(always)]
	fn each(out: &mut [f32], u: Un) {
		out.iter_mut().for_each(|o| *o = unary(u, *o));
	}
	match u {
		Un::Abs => each(out, Un::Abs),
		Un::Square => each(out, Un::Square),
		Un::Cube => each(out, Un::Cube),
		Un::Sqrt => each(out, Un::Sqrt),
		Un::HalfNeg => each(out, Un::HalfNeg),
		Un::QuarterNeg => each(out, Un::QuarterNeg),
		Un::Recip => each(out, Un::Recip),
		Un::Negate => each(out, Un::Negate),
		Un::Squeeze => each(out, Un::Squeeze),
		Un::Log => each(out, Un::Log),
		Un::Sign => each(out, Un::Sign),
	}
}

#[inline(always)]
fn unary(u: Un, x: f32) -> f32 {
	match u {
		Un::Abs => x.abs(),
		Un::Square => x * x,
		Un::Cube => x * x * x,
		Un::Sqrt => x.sqrt(),
		Un::HalfNeg => {
			if x > 0.0 {
				x
			} else {
				x * 0.5
			}
		}
		Un::QuarterNeg => {
			if x > 0.0 {
				x
			} else {
				x * 0.25
			}
		}
		Un::Recip => 1.0 / x,
		Un::Negate => -x,
		Un::Squeeze => squeeze(x),
		Un::Log => (x as f64).ln() as f32,
		Un::Sign => jsignum(x),
	}
}

pub fn block_key(x: i32, y: i32, z: i32) -> i64 {
	((x as i64 & 0x3FF_FFFF) << 38) | ((z as i64 & 0x3FF_FFFF) << 12) | (y as i64 & 0xFFF)
}

#[derive(Default)]
struct Cell {
	vol: Option<Volume>,
	buf: Vec<f32>,
	done: Vec<bool>,
	full: bool,
	key: i64,
	value: f32,
}

pub struct Ctx {
	cells: Option<Vec<Cell>>,
	pool: Vec<Vec<f32>>,
	masks: Vec<Vec<bool>>,
	/// Structure pieces that reshape this chunk's terrain, if any.
	pub beard: Option<crate::beard::Beard>,
}

impl Ctx {
	pub fn new(caches: usize, enabled: bool) -> Self {
		let cells = enabled.then(|| (0..caches).map(|_| Cell { value: f32::NAN, ..Default::default() }).collect());
		Self { cells, pool: vec![], masks: vec![], beard: None }
	}

	/// A buffer of `n` values, with whatever an earlier user left in it. Best fit, so it rarely needs filling.
	pub fn take(&mut self, n: usize) -> Vec<f32> {
		let fit = (0..self.pool.len()).filter(|&i| self.pool[i].len() >= n).min_by_key(|&i| self.pool[i].len());
		let mut v = match fit {
			Some(i) => self.pool.swap_remove(i),
			None => self.pool.pop().unwrap_or_default(),
		};
		v.resize(n, 0.0);
		v
	}

	pub fn give(&mut self, v: Vec<f32>) {
		self.pool.push(v);
	}

	fn take_mask(&mut self, n: usize) -> Vec<bool> {
		let mut v = self.masks.pop().unwrap_or_default();
		v.clear();
		v.resize(n, false);
		v
	}

	fn give_mask(&mut self, v: Vec<bool>) {
		self.masks.push(v);
	}
}

pub struct Graph {
	pub nodes: Vec<S>,
	pub noises: Vec<NoiseStack>,
	pub cache_count: usize,
}

impl Graph {
	pub fn val(&self, c: &mut Ctx, id: Id, x: i32, y: i32, z: i32) -> f32 {
		match &self.nodes[id as usize] {
			S::Const(v) => *v,
			S::Noise { n, xz, y: ys } => self.noises[*n].get(x as f64 * xz, y as f64 * ys, z as f64 * xz),
			S::ShiftedXz { sx, sz, n, xz, y: ys } => {
				let nx = x as f64 * xz + self.val(c, *sx, x, y, z) as f64;
				let ny = y as f64 * ys;
				let nz = z as f64 * xz + self.val(c, *sz, x, y, z) as f64;
				self.noises[*n].get(nx, ny, nz)
			}
			S::ShiftedXyz { sx, sy, sz, n, xz, y: ys } => {
				let nx = x as f64 * xz + self.val(c, *sx, x, y, z) as f64;
				let ny = y as f64 * ys + self.val(c, *sy, x, y, z) as f64;
				let nz = z as f64 * xz + self.val(c, *sz, x, y, z) as f64;
				self.noises[*n].get(nx, ny, nz)
			}
			S::ShiftB { n } => self.noises[*n].get(z as f64 * 0.25, x as f64 * 0.25, 0.0) * 4.0,
			S::Gradient { axis, .. } => self.gradient(id, [x, y, z][*axis as usize]),
			S::Cache { id: cid, input } => self.val_cached(c, *cid, *input, x, y, z),
			S::Clamp(i, a, b) => clampf(self.val(c, *i, x, y, z), *a, *b),
			S::Add(l, r) => self.val(c, *l, x, y, z) + self.val(c, *r, x, y, z),
			S::ConstAdd(l, k) => self.val(c, *l, x, y, z) + k,
			S::Sub(l, r) => self.val(c, *l, x, y, z) - self.val(c, *r, x, y, z),
			S::ConstSub(k, r) => k - self.val(c, *r, x, y, z),
			S::Mul(l, r) => {
				let a = self.val(c, *l, x, y, z);
				if a == 0.0 { 0.0 } else { a * self.val(c, *r, x, y, z) }
			}
			S::ConstMul(l, k) => self.val(c, *l, x, y, z) * k,
			S::Div(l, r) => {
				let a = self.val(c, *l, x, y, z);
				if a == 0.0 { 0.0 } else { a / self.val(c, *r, x, y, z) }
			}
			S::ConstDiv(k, r) => k / self.val(c, *r, x, y, z),
			S::Min(l, r, rmin) => {
				let a = self.val(c, *l, x, y, z);
				if a <= *rmin { a } else { jmin(a, self.val(c, *r, x, y, z)) }
			}
			S::ConstMin(l, k) => jmin(self.val(c, *l, x, y, z), *k),
			S::Max(l, r, rmax) => {
				let a = self.val(c, *l, x, y, z);
				if a >= *rmax { a } else { jmax(a, self.val(c, *r, x, y, z)) }
			}
			S::ConstMax(l, k) => jmax(self.val(c, *l, x, y, z), *k),
			S::Unary(u, i) => unary(*u, self.val(c, *i, x, y, z)),
			S::Interp { input, cxz, cy, .. } => {
				let (ix, iy, iz) = (x.rem_euclid(*cxz), y.rem_euclid(*cy), z.rem_euclid(*cxz));
				if ix == 0 && iy == 0 && iz == 0 {
					return self.val(c, *input, x, y, z);
				}
				let v = Volume::stepped(2, 2, 2, x - ix, y - iy, z - iz, *cxz, *cy, *cxz);
				let mut b = c.take(8);
				self.vol(c, *input, &mut b, &v);
				let r = lerp3(
					ix as f32 / *cxz as f32,
					iy as f32 / *cy as f32,
					iz as f32 / *cxz as f32,
					b[v.idx(0, 0, 0)],
					b[v.idx(1, 0, 0)],
					b[v.idx(0, 1, 0)],
					b[v.idx(1, 1, 0)],
					b[v.idx(0, 0, 1)],
					b[v.idx(1, 0, 1)],
					b[v.idx(0, 1, 1)],
					b[v.idx(1, 1, 1)],
				);
				c.give(b);
				r
			}
			S::SliceX(i, sx) => self.val(c, *i, *sx, y, z),
			S::SliceY(i, sy) => self.val(c, *i, x, *sy, z),
			S::SliceZ(i, sz) => self.val(c, *i, x, y, *sz),
			S::SliceXz(i, sx, sz) => self.val(c, *i, *sx, y, *sz),
			S::Spline { spline, coords } => {
				let mut cached = [f32::NAN; MAX_SPLINE_COORDS];
				let mut get = |slot: usize, c: &mut Ctx| {
					if !cached[slot].is_nan() {
						return cached[slot];
					}
					let v = self.val(c, coords[slot], x, y, z);
					cached[slot] = v;
					v
				};
				sample_spline(spline, c, &mut get)
			}
			S::Lerp(a, f, s) => {
				let al = self.val(c, *a, x, y, z);
				if al == 0.0 {
					self.val(c, *f, x, y, z)
				} else if al == 1.0 {
					self.val(c, *s, x, y, z)
				} else {
					let fv = self.val(c, *f, x, y, z);
					lerp(al, fv, self.val(c, *s, x, y, z))
				}
			}
			S::LerpConstFirst(a, fv, s) => {
				let al = self.val(c, *a, x, y, z);
				if al == 0.0 {
					*fv
				} else if al == 1.0 {
					self.val(c, *s, x, y, z)
				} else {
					lerp(al, *fv, self.val(c, *s, x, y, z))
				}
			}
			S::LerpConstSecond(a, f, sv) => {
				let al = self.val(c, *a, x, y, z);
				if al == 0.0 {
					self.val(c, *f, x, y, z)
				} else if al == 1.0 {
					*sv
				} else {
					lerp(al, self.val(c, *f, x, y, z), *sv)
				}
			}
			S::RangeChoice { input, min, max, inr, outr } => {
				let v = self.val(c, *input, x, y, z);
				if v >= *min && v < *max { self.val(c, *inr, x, y, z) } else { self.val(c, *outr, x, y, z) }
			}
			S::RangeConst { input, min, max, inr, outr } => {
				let v = self.val(c, *input, x, y, z);
				if v >= *min && v < *max { *inr } else { *outr }
			}
			S::Select { input, thresholds, samplers } => {
				let v = self.val(c, *input, x, y, z);
				self.val(c, samplers[select_index(thresholds, samplers.len(), v)], x, y, z)
			}
			S::Single { input, t, below, above } => {
				let v = self.val(c, *input, x, y, z);
				if v < *t { self.val(c, *below, x, y, z) } else { self.val(c, *above, x, y, z) }
			}
			S::FindTop { density, upper, lower, cell } => {
				let u = self.val(c, *upper, x, y, z);
				self.find_top(c, *density, *lower, *cell, x, z, u) as f32
			}
			S::Beardifier => c.beard.as_ref().map_or(0.0, |b| b.sample(x, y, z)),
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn find_top(&self, c: &mut Ctx, density: Id, lower: i32, cell: i32, x: i32, z: i32, upper: f32) -> i32 {
		let top = ((upper / cell as f32) as f64).floor() as i32 * cell;
		if top <= lower {
			return lower;
		}
		let mut y = top;
		while y >= lower {
			if self.val(c, density, x, y, z) > 0.0 {
				return y;
			}
			y -= cell;
		}
		lower
	}

	pub fn y_band(&self, id: Id) -> Option<(i32, i32)> {
		let S::RangeChoice { input, min, max, outr, .. } = &self.nodes[id as usize] else { return None };
		let S::Gradient { axis: 1, tiling: 0, from, min: gmin, max: gmax, fv, factor, .. } = &self.nodes[*input as usize] else { return None };
		let identity = *factor == 1.0 && *fv == *from as f32 && *gmin <= -64 && *gmax >= 320;
		(identity && matches!(self.nodes[*outr as usize], S::Const(k) if k <= 0.0)).then(|| (min.ceil() as i32, max.ceil() as i32))
	}

	fn gradient(&self, id: Id, coord: i32) -> f32 {
		let S::Gradient { tiling, from, min, max, range, fv, factor, .. } = &self.nodes[id as usize] else {
			unreachable!()
		};
		match tiling {
			0 => fv + (coord.clamp(*min, *max) - from) as f32 * factor,
			1 => fv + (coord - from).rem_euclid(*range) as f32 * factor,
			_ => {
				let rel = coord - from;
				let tile = rel.div_euclid(*range);
				let local = rel - tile * range;
				if tile & 1 == 0 { fv + local as f32 * factor } else { fv + (range - local) as f32 * factor }
			}
		}
	}

	fn val_cached(&self, c: &mut Ctx, cid: usize, input: Id, x: i32, y: i32, z: i32) -> f32 {
		if c.cells.is_none() {
			return self.val(c, input, x, y, z);
		}
		let cell = &mut c.cells.as_mut().unwrap()[cid];
		let k = block_key(x, y, z);
		if cell.key == k && !cell.value.is_nan() {
			return cell.value;
		}
		if let Some(v) = &cell.vol
			&& let Some(i) = v.index_of(x, y, z)
		{
			return cell.buf[i];
		}
		let v = self.val(c, input, x, y, z);
		let cell = &mut c.cells.as_mut().unwrap()[cid];
		cell.key = k;
		cell.value = v;
		v
	}

	pub fn vol(&self, c: &mut Ctx, id: Id, out: &mut [f32], v: &Volume) {
		self.vm(c, id, out, v, None);
	}

	fn sub_mask(c: &mut Ctx, m: Option<&[bool]>, n: usize, keep: impl Fn(usize) -> bool) -> (Vec<bool>, usize) {
		let mut s = c.take_mask(n);
		let mut count = 0;
		for (i, e) in s.iter_mut().enumerate() {
			if m.is_none_or(|m| m[i]) && keep(i) {
				*e = true;
				count += 1;
			}
		}
		(s, count)
	}

	fn vm_sub(&self, c: &mut Ctx, id: Id, out: &mut [f32], v: &Volume, sub: &[bool], count: usize) {
		if count == sub.len() {
			self.vm(c, id, out, v, None);
		} else if count > 0 {
			self.vm(c, id, out, v, Some(sub));
		}
	}

	fn vm(&self, c: &mut Ctx, id: Id, out: &mut [f32], v: &Volume, m: Option<&[bool]>) {
		debug_assert_eq!(out.len(), v.size());
		let on = |i: usize| m.is_none_or(|m| m[i]);
		let lazy = v.tx != 1 || v.ty != 1 || v.tz != 1;
		match &self.nodes[id as usize] {
			S::Const(k) => out.fill(*k),
			S::Noise { n, xz, y } => {
				out.fill(0.0);
				self.noises[*n].add_to_volume(out, v, *xz, *y, 1.0, m);
			}
			S::ShiftedXz { sx, sz, n, xz, y: ys } => {
				self.vm(c, *sx, out, v, m);
				let mut bz = c.take(v.size());
				self.vm(c, *sz, &mut bz, v, m);
				let mut i = 0;
				for z in 0..v.sz {
					let base_z = v.block_z(z) as f64 * xz;
					for x in 0..v.sx {
						let base_x = v.block_x(x) as f64 * xz;
						for y in 0..v.sy {
							if on(i) {
								let nx = base_x + out[i] as f64;
								let ny = v.block_y(y) as f64 * ys;
								let nz = base_z + bz[i] as f64;
								out[i] = self.noises[*n].get(nx, ny, nz);
							}
							i += 1;
						}
					}
				}
				c.give(bz);
			}
			S::ShiftedXyz { sx, sy, sz, n, xz, y: ys } => {
				self.vm(c, *sx, out, v, m);
				let mut by = c.take(v.size());
				self.vm(c, *sy, &mut by, v, m);
				let mut bz = c.take(v.size());
				self.vm(c, *sz, &mut bz, v, m);
				let mut i = 0;
				for z in 0..v.sz {
					let base_z = v.block_z(z) as f64 * xz;
					for x in 0..v.sx {
						let base_x = v.block_x(x) as f64 * xz;
						for y in 0..v.sy {
							if on(i) {
								let nx = base_x + out[i] as f64;
								let ny = v.block_y(y) as f64 * ys + by[i] as f64;
								let nz = base_z + bz[i] as f64;
								out[i] = self.noises[*n].get(nx, ny, nz);
							}
							i += 1;
						}
					}
				}
				c.give(bz);
				c.give(by);
			}
			S::ShiftB { n } => {
				let t = Volume::stepped(v.sz, v.sx, 1, v.mz, v.mx, 0, v.tz, v.tx, 1);
				let mut tb = c.take(t.size());
				tb.fill(0.0);
				self.noises[*n].add_to_volume(&mut tb, &t, 0.25, 0.25, 4.0, None);
				for z in 0..v.sz {
					for x in 0..v.sx {
						let val = tb[t.idx(z, x, 0)];
						let s = v.idx(x, 0, z);
						out[s..s + v.sy as usize].fill(val);
					}
				}
				c.give(tb);
			}
			S::Gradient { axis, .. } => match axis {
				0 => {
					for x in 0..v.sx {
						let val = self.gradient(id, v.block_x(x));
						for z in 0..v.sz {
							let s = v.idx(x, 0, z);
							out[s..s + v.sy as usize].fill(val);
						}
					}
				}
				1 => {
					for y in 0..v.sy {
						let val = self.gradient(id, v.block_y(y));
						for z in 0..v.sz {
							for x in 0..v.sx {
								out[v.idx(x, y, z)] = val;
							}
						}
					}
				}
				_ => {
					for z in 0..v.sz {
						let val = self.gradient(id, v.block_z(z));
						let s = v.idx(0, 0, z);
						out[s..s + (v.sx * v.sy) as usize].fill(val);
					}
				}
			},
			S::Cache { id: cid, input } => self.vol_cached(c, *cid, *input, out, v, m),
			S::Clamp(i, a, b) => {
				self.vm(c, *i, out, v, m);
				out.iter_mut().for_each(|o| *o = clampf(*o, *a, *b));
			}
			S::Add(l, r) => self.vol_bin(c, *l, *r, out, v, m, |a, b| a + b),
			S::Sub(l, r) => self.vol_bin(c, *l, *r, out, v, m, |a, b| a + -b),
			S::Mul(l, r) => self.vol_bin(c, *l, *r, out, v, m, |a, b| a * b),
			S::Div(l, r) => self.vol_bin(c, *l, *r, out, v, m, |a, b| a / b),
			S::Min(l, r, _) if !lazy => self.vol_bin(c, *l, *r, out, v, None, |a, b| if b < a { b } else { a }),
			S::Max(l, r, _) if !lazy => self.vol_bin(c, *l, *r, out, v, None, |a, b| if b > a { b } else { a }),
			S::Min(l, r, rmin) => {
				self.vm(c, *l, out, v, m);
				let (sub, count) = Self::sub_mask(c, m, out.len(), |i| !(out[i] <= *rmin));
				if count > 0 {
					let mut rb = c.take(v.size());
					self.vm_sub(c, *r, &mut rb, v, &sub, count);
					for i in 0..out.len() {
						if sub[i] && rb[i] < out[i] {
							out[i] = rb[i];
						}
					}
					c.give(rb);
				}
				c.give_mask(sub);
			}
			S::Max(l, r, rmax) => {
				self.vm(c, *l, out, v, m);
				let (sub, count) = Self::sub_mask(c, m, out.len(), |i| !(out[i] >= *rmax));
				if count > 0 {
					let mut rb = c.take(v.size());
					self.vm_sub(c, *r, &mut rb, v, &sub, count);
					for i in 0..out.len() {
						if sub[i] && rb[i] > out[i] {
							out[i] = rb[i];
						}
					}
					c.give(rb);
				}
				c.give_mask(sub);
			}
			S::ConstAdd(l, k) => {
				self.vm(c, *l, out, v, m);
				out.iter_mut().for_each(|o| *o += k);
			}
			S::ConstSub(k, r) => {
				self.vm(c, *r, out, v, m);
				out.iter_mut().for_each(|o| *o = k - *o);
			}
			S::ConstMul(l, k) => {
				self.vm(c, *l, out, v, m);
				out.iter_mut().for_each(|o| *o *= k);
			}
			S::ConstDiv(k, r) => {
				self.vm(c, *r, out, v, m);
				out.iter_mut().for_each(|o| *o = k / *o);
			}
			S::ConstMin(l, k) => {
				self.vm(c, *l, out, v, m);
				out.iter_mut().for_each(|o| {
					if *k < *o {
						*o = *k
					}
				});
			}
			S::ConstMax(l, k) => {
				self.vm(c, *l, out, v, m);
				out.iter_mut().for_each(|o| {
					if *k > *o {
						*o = *k
					}
				});
			}
			S::Unary(u, i) => {
				self.vm(c, *i, out, v, m);
				unary_all(*u, out);
			}
			S::Interp { input, cxz, cy, ixz, iy } => {
				let aligned = (v.tx == *cxz || v.sx == 1)
					&& (v.ty == *cy || v.sy == 1)
					&& (v.tz == *cxz || v.sz == 1)
					&& v.mx.rem_euclid(*cxz) == 0
					&& v.my.rem_euclid(*cy) == 0
					&& v.mz.rem_euclid(*cxz) == 0;
				if aligned {
					self.vm(c, *input, out, v, m);
				} else if v.tx == 1 && v.ty == 1 && v.tz == 1 {
					self.interp_blocks(c, *input, *cxz, *cy, *ixz, *iy, out, v, m);
				} else {
					let bv = Volume::new(v.sx * v.tx, v.sy * v.ty, v.sz * v.tz, v.mx, v.my, v.mz);
					let mut bb = c.take(bv.size());
					self.interp_blocks(c, *input, *cxz, *cy, *ixz, *iy, &mut bb, &bv, None);
					for z in 0..v.sz {
						for x in 0..v.sx {
							for y in 0..v.sy {
								out[v.idx(x, y, z)] = bb[bv.idx(x * v.tx, y * v.ty, z * v.tz)];
							}
						}
					}
					c.give(bb);
				}
			}
			S::SliceX(i, sx) => {
				if v.sx == 1 && v.mx == *sx {
					return self.vm(c, *i, out, v, m);
				}
				let iv = Volume::stepped(1, v.sy, v.sz, *sx, v.my, v.mz, v.tx, v.ty, v.tz);
				let mut b = c.take(iv.size());
				self.vol(c, *i, &mut b, &iv);
				let mut idx = 0;
				for z in 0..v.sz {
					for _ in 0..v.sx {
						for y in 0..v.sy {
							out[idx] = b[iv.idx(0, y, z)];
							idx += 1;
						}
					}
				}
				c.give(b);
			}
			S::SliceY(i, sy) => {
				if v.sy == 1 && v.my == *sy {
					return self.vm(c, *i, out, v, m);
				}
				let iv = Volume::stepped(v.sx, 1, v.sz, v.mx, *sy, v.mz, v.tx, v.ty, v.tz);
				let mut b = c.take(iv.size());
				self.vol(c, *i, &mut b, &iv);
				for z in 0..v.sz {
					for x in 0..v.sx {
						let s = v.idx(x, 0, z);
						out[s..s + v.sy as usize].fill(b[iv.idx(x, 0, z)]);
					}
				}
				c.give(b);
			}
			S::SliceZ(i, sz) => {
				if v.sz == 1 && v.mz == *sz {
					return self.vm(c, *i, out, v, m);
				}
				let iv = Volume::stepped(v.sx, v.sy, 1, v.mx, v.my, *sz, v.tx, v.ty, v.tz);
				let mut b = c.take(iv.size());
				self.vol(c, *i, &mut b, &iv);
				let mut idx = 0;
				for _ in 0..v.sz {
					for x in 0..v.sx {
						for y in 0..v.sy {
							out[idx] = b[iv.idx(x, y, 0)];
							idx += 1;
						}
					}
				}
				c.give(b);
			}
			S::SliceXz(i, sx, sz) => {
				if v.sx == 1 && v.sz == 1 && v.mx == *sx && v.mz == *sz {
					return self.vm(c, *i, out, v, m);
				}
				let iv = Volume::stepped(1, v.sy, 1, *sx, v.my, *sz, v.tx, v.ty, v.tz);
				let mut b = c.take(iv.size());
				self.vol(c, *i, &mut b, &iv);
				for y in 0..v.sy {
					let val = b[iv.idx(0, y, 0)];
					let mut idx = v.idx(0, y, 0);
					for _ in 0..v.sz * v.sx {
						out[idx] = val;
						idx += v.sy as usize;
					}
				}
				c.give(b);
			}
			S::Spline { spline, coords } => {
				let n = v.size();
				let mut bufs: Vec<Option<Vec<f32>>> = (0..coords.len()).map(|_| None).collect();
				for i in 0..n {
					if !on(i) {
						continue;
					}
					let mut get = |slot: usize, c: &mut Ctx| {
						if bufs[slot].is_none() {
							let mut b = c.take(n);
							self.vm(c, coords[slot], &mut b, v, m);
							bufs[slot] = Some(b);
						}
						bufs[slot].as_ref().unwrap()[i]
					};
					out[i] = sample_spline(spline, c, &mut get);
				}
				bufs.into_iter().flatten().for_each(|b| c.give(b));
			}
			S::Lerp(a, f, s) if !lazy => {
				self.vm(c, *a, out, v, None);
				let mut fb = c.take(v.size());
				self.vm(c, *f, &mut fb, v, None);
				let mut sb = c.take(v.size());
				self.vm(c, *s, &mut sb, v, None);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						fb[i]
					} else if al == 1.0 {
						sb[i]
					} else {
						lerp(al, fb[i], sb[i])
					};
				}
				c.give(sb);
				c.give(fb);
			}
			S::LerpConstFirst(a, fv, s) if !lazy => {
				self.vm(c, *a, out, v, None);
				let mut sb = c.take(v.size());
				self.vm(c, *s, &mut sb, v, None);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						*fv
					} else if al == 1.0 {
						sb[i]
					} else {
						lerp(al, *fv, sb[i])
					};
				}
				c.give(sb);
			}
			S::LerpConstSecond(a, f, sv) if !lazy => {
				self.vm(c, *a, out, v, None);
				let mut fb = c.take(v.size());
				self.vm(c, *f, &mut fb, v, None);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						fb[i]
					} else if al == 1.0 {
						*sv
					} else {
						lerp(al, fb[i], *sv)
					};
				}
				c.give(fb);
			}
			S::RangeChoice { input, min, max, inr, outr } if !lazy => {
				self.vm(c, *inr, out, v, None);
				let mut ib = c.take(v.size());
				self.vm(c, *input, &mut ib, v, None);
				let mut ob = c.take(v.size());
				self.vm(c, *outr, &mut ob, v, None);
				for ((o, &b), &x) in out.iter_mut().zip(&ob).zip(&ib) {
					*o = if (x >= *min) & (x < *max) { *o } else { b };
				}
				c.give(ob);
				c.give(ib);
			}
			S::Single { input, t, below, above } if !lazy => {
				self.vm(c, *input, out, v, None);
				let mut bb = c.take(v.size());
				self.vm(c, *below, &mut bb, v, None);
				let mut ab = c.take(v.size());
				self.vm(c, *above, &mut ab, v, None);
				for i in 0..out.len() {
					out[i] = if out[i] < *t { bb[i] } else { ab[i] };
				}
				c.give(ab);
				c.give(bb);
			}
			S::Lerp(a, f, s) => {
				self.vm(c, *a, out, v, m);
				let (fm, fc) = Self::sub_mask(c, m, out.len(), |i| out[i] != 1.0);
				let mut fb = c.take(v.size());
				self.vm_sub(c, *f, &mut fb, v, &fm, fc);
				let (sm, sc) = Self::sub_mask(c, m, out.len(), |i| out[i] != 0.0);
				let mut sb = c.take(v.size());
				self.vm_sub(c, *s, &mut sb, v, &sm, sc);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						fb[i]
					} else if al == 1.0 {
						sb[i]
					} else {
						lerp(al, fb[i], sb[i])
					};
				}
				c.give(sb);
				c.give(fb);
				c.give_mask(sm);
				c.give_mask(fm);
			}
			S::LerpConstFirst(a, fv, s) => {
				self.vm(c, *a, out, v, m);
				let (sm, sc) = Self::sub_mask(c, m, out.len(), |i| out[i] != 0.0);
				let mut sb = c.take(v.size());
				self.vm_sub(c, *s, &mut sb, v, &sm, sc);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						*fv
					} else if al == 1.0 {
						sb[i]
					} else {
						lerp(al, *fv, sb[i])
					};
				}
				c.give(sb);
				c.give_mask(sm);
			}
			S::LerpConstSecond(a, f, sv) => {
				self.vm(c, *a, out, v, m);
				let (fm, fc) = Self::sub_mask(c, m, out.len(), |i| out[i] != 1.0);
				let mut fb = c.take(v.size());
				self.vm_sub(c, *f, &mut fb, v, &fm, fc);
				for i in 0..out.len() {
					let al = out[i];
					out[i] = if al == 0.0 {
						fb[i]
					} else if al == 1.0 {
						*sv
					} else {
						lerp(al, fb[i], *sv)
					};
				}
				c.give(fb);
				c.give_mask(fm);
			}
			S::RangeChoice { input, min, max, inr, outr } => {
				let mut ib = c.take(v.size());
				self.vm(c, *input, &mut ib, v, m);
				let inside = |i: usize| ib[i] >= *min && ib[i] < *max;
				let (im, ic) = Self::sub_mask(c, m, out.len(), inside);
				self.vm_sub(c, *inr, out, v, &im, ic);
				let (om, oc) = Self::sub_mask(c, m, out.len(), |i| !inside(i));
				if oc > 0 {
					let mut ob = c.take(v.size());
					self.vm_sub(c, *outr, &mut ob, v, &om, oc);
					for i in 0..out.len() {
						if om[i] {
							out[i] = ob[i];
						}
					}
					c.give(ob);
				}
				c.give_mask(om);
				c.give_mask(im);
				c.give(ib);
			}
			S::RangeConst { input, min, max, inr, outr } => {
				self.vm(c, *input, out, v, m);
				out.iter_mut().for_each(|o| *o = if *o >= *min && *o < *max { *inr } else { *outr });
			}
			S::Select { input, thresholds, samplers } => {
				self.vm(c, *input, out, v, m);
				let pick: Vec<usize> = out.iter().map(|&x| select_index(thresholds, samplers.len(), x)).collect();
				let mut res = c.take(v.size());
				for (k, s) in samplers.iter().enumerate() {
					let (sm, sc) = Self::sub_mask(c, m, out.len(), |i| pick[i] == k);
					if sc > 0 {
						let mut b = c.take(v.size());
						self.vm_sub(c, *s, &mut b, v, &sm, sc);
						for i in 0..out.len() {
							if sm[i] {
								res[i] = b[i];
							}
						}
						c.give(b);
					}
					c.give_mask(sm);
				}
				out.copy_from_slice(&res);
				c.give(res);
			}
			S::Single { input, t, below, above } => {
				self.vm(c, *input, out, v, m);
				let (bm, bc) = Self::sub_mask(c, m, out.len(), |i| out[i] < *t);
				let (am, ac) = Self::sub_mask(c, m, out.len(), |i| !(out[i] < *t));
				let mut bb = c.take(v.size());
				self.vm_sub(c, *below, &mut bb, v, &bm, bc);
				let mut ab = c.take(v.size());
				self.vm_sub(c, *above, &mut ab, v, &am, ac);
				for i in 0..out.len() {
					out[i] = if out[i] < *t { bb[i] } else { ab[i] };
				}
				c.give(ab);
				c.give(bb);
				c.give_mask(am);
				c.give_mask(bm);
			}
			S::FindTop { density, upper, lower, cell } => {
				assert_eq!(v.sy, 1, "find_top_surface needs sizeY=1");
				self.vol(c, *upper, out, v);
				let mut i = 0;
				for z in 0..v.sz {
					let bz = v.block_z(z);
					for x in 0..v.sx {
						let bx = v.block_x(x);
						out[i] = self.find_top(c, *density, *lower, *cell, bx, bz, out[i]) as f32;
						i += 1;
					}
				}
			}
			S::Beardifier => match &c.beard {
				Some(b) => b.fill(out, v),
				None => out.fill(0.0),
			},
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn vol_bin(&self, c: &mut Ctx, l: Id, r: Id, out: &mut [f32], v: &Volume, m: Option<&[bool]>, op: impl Fn(f32, f32) -> f32) {
		self.vm(c, l, out, v, m);
		let mut rb = c.take(v.size());
		self.vm(c, r, &mut rb, v, m);
		for (o, b) in out.iter_mut().zip(&rb) {
			*o = op(*o, *b);
		}
		c.give(rb);
	}

	fn vol_cached(&self, c: &mut Ctx, cid: usize, input: Id, out: &mut [f32], v: &Volume, m: Option<&[bool]>) {
		if c.cells.is_none() {
			return self.vm(c, input, out, v, m);
		}
		let n = v.size();
		let cell = &mut c.cells.as_mut().unwrap()[cid];
		if cell.vol != Some(*v) {
			cell.vol = Some(*v);
			cell.buf.resize(n, 0.0);
			cell.buf.truncate(n);
			cell.done.clear();
			cell.full = false;
		}
		if !cell.full && m.is_none() {
			// all of it at once, no bookkeeping of which values are done
			let mut buf = std::mem::take(&mut cell.buf);
			self.vm(c, input, &mut buf, v, None);
			let cell = &mut c.cells.as_mut().unwrap()[cid];
			cell.buf = buf;
			cell.full = true;
		} else if !cell.full {
			let mut done = std::mem::take(&mut cell.done);
			done.resize(n, false);
			let (need, count) = Self::sub_mask(c, m, n, |i| !done[i]);
			if count > 0 {
				let mut buf = c.take(n);
				self.vm_sub(c, input, &mut buf, v, &need, count);
				let cell = &mut c.cells.as_mut().unwrap()[cid];
				cell.done = done;
				let mut all = true;
				for i in 0..n {
					if need[i] {
						cell.buf[i] = buf[i];
						cell.done[i] = true;
					}
					all &= cell.done[i];
				}
				cell.full = all;
				c.give(buf);
			} else {
				c.cells.as_mut().unwrap()[cid].done = done;
			}
			c.give_mask(need);
		}
		out.copy_from_slice(&c.cells.as_ref().unwrap()[cid].buf);
	}

	#[allow(clippy::too_many_arguments)]
	fn interp_blocks(&self, c: &mut Ctx, input: Id, cxz: i32, cy: i32, ixz: f32, iy: f32, out: &mut [f32], v: &Volume, m: Option<&[bool]>) {
		let (minx, miny, minz) = (v.mx.div_euclid(cxz), v.my.div_euclid(cy), v.mz.div_euclid(cxz));
		let (maxx, maxy, maxz) = (v.max_x().div_euclid(cxz), v.max_y().div_euclid(cy), v.max_z().div_euclid(cxz));
		let (cnx, cny, cnz) = (maxx - minx + 1, maxy - miny + 1, maxz - minz + 1);
		let cv = Volume::stepped(
			if v.max_x().rem_euclid(cxz) == 0 { cnx } else { cnx + 1 },
			if v.max_y().rem_euclid(cy) == 0 { cny } else { cny + 1 },
			if v.max_z().rem_euclid(cxz) == 0 { cnz } else { cnz + 1 },
			minx * cxz,
			miny * cy,
			minz * cxz,
			cxz,
			cy,
			cxz,
		);
		let cell_range = |cx: i32, cyi: i32, cz: i32| {
			let ox = cv.block_x(cx) - v.mx;
			let oy = cv.block_y(cyi) - v.my;
			let oz = cv.block_z(cz) - v.mz;
			let (x0, y0, z0) = (0.max(-ox), 0.max(-oy), 0.max(-oz));
			let x1 = cxz.min(v.sx - ox) - 1;
			let y1 = cy.min(v.sy - oy) - 1;
			let z1 = cxz.min(v.sz - oz) - 1;
			(ox, oy, oz, x0, y0, z0, x1, y1, z1)
		};
		let active: Option<Vec<bool>> = m.map(|m| {
			let mut act = vec![false; (cnx * cny * cnz) as usize];
			for cz in 0..cnz {
				for cx in 0..cnx {
					for cyi in 0..cny {
						let (ox, oy, oz, x0, y0, z0, x1, y1, z1) = cell_range(cx, cyi, cz);
						'cell: for z in z0..=z1 {
							for x in x0..=x1 {
								let s = v.idx(ox + x, oy + y0, oz + z);
								if m[s..=s + (y1 - y0) as usize].iter().any(|&b| b) {
									act[((cz * cnx + cx) * cny + cyi) as usize] = true;
									break 'cell;
								}
							}
						}
					}
				}
			}
			act
		});
		let mut cb = c.take(cv.size());
		match &active {
			None => self.vm(c, input, &mut cb, &cv, None),
			Some(act) => {
				let mut cm = c.take_mask(cv.size());
				let mut count = 0;
				for cz in 0..cnz {
					for cx in 0..cnx {
						for cyi in 0..cny {
							if act[((cz * cnx + cx) * cny + cyi) as usize] {
								for (dx, dy, dz) in [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)] {
									let k = cv.idx((cx + dx).min(cv.sx - 1), (cyi + dy).min(cv.sy - 1), (cz + dz).min(cv.sz - 1));
									if !cm[k] {
										cm[k] = true;
										count += 1;
									}
								}
							}
						}
					}
				}
				self.vm_sub(c, input, &mut cb, &cv, &cm, count);
				c.give_mask(cm);
			}
		}
		for cz in 0..cnz {
			let nz = (cz + 1).min(cv.sz - 1);
			for cx in 0..cnx {
				let nx = (cx + 1).min(cv.sx - 1);
				let mut v000 = cb[cv.idx(cx, 0, cz)];
				let mut v100 = cb[cv.idx(nx, 0, cz)];
				let mut v001 = cb[cv.idx(cx, 0, nz)];
				let mut v101 = cb[cv.idx(nx, 0, nz)];
				for cyi in 0..cny {
					let ny = (cyi + 1).min(cv.sy - 1);
					let v010 = cb[cv.idx(cx, ny, cz)];
					let v110 = cb[cv.idx(nx, ny, cz)];
					let v011 = cb[cv.idx(cx, ny, nz)];
					let v111 = cb[cv.idx(nx, ny, nz)];
					if active.as_ref().is_none_or(|a| a[((cz * cnx + cx) * cny + cyi) as usize]) {
						let (ox, oy, oz, x0, y0, z0, x1, y1, z1) = cell_range(cx, cyi, cz);
						if (cxz, cy, x0, x1, y0, y1) == (4, 8, 0, 3, 0, 7) {
							// a whole cell row: its four columns side by side, so the adds vectorize
							for z in z0..=z1 {
								let az = z as f32 * ixz;
								let (v00, v01, v10, v11) = (lerp(az, v000, v001), lerp(az, v010, v011), lerp(az, v100, v101), lerp(az, v110, v111));
								let (mut val, mut st) = ([0f32; 4], [0f32; 4]);
								for x in 0..4 {
									let ax = x as f32 * ixz;
									let (a, b) = (lerp(ax, v00, v10), lerp(ax, v01, v11));
									st[x] = (b - a) * iy;
									val[x] = a + st[x] * 0.0;
								}
								let mut rows = [[0f32; 4]; 8];
								for row in &mut rows {
									*row = val;
									for x in 0..4 {
										val[x] += st[x];
									}
								}
								for x in 0..4 {
									let oi = v.idx(ox + x as i32, oy, oz + z);
									for (o, row) in out[oi..oi + 8].iter_mut().zip(&rows) {
										*o = row[x];
									}
								}
							}
						} else {
							for z in z0..=z1 {
								let az = z as f32 * ixz;
								let v00 = lerp(az, v000, v001);
								let v01 = lerp(az, v010, v011);
								let v10 = lerp(az, v100, v101);
								let v11 = lerp(az, v110, v111);
								for x in x0..=x1 {
									let ax = x as f32 * ixz;
									let a = lerp(ax, v00, v10);
									let b = lerp(ax, v01, v11);
									let step = (b - a) * iy;
									let mut value = a + step * y0 as f32;
									let mut oi = v.idx(ox + x, oy + y0, oz + z);
									for _ in y0..=y1 {
										out[oi] = value;
										oi += 1;
										value += step;
									}
								}
							}
						}
					}
					v000 = v010;
					v100 = v110;
					v001 = v011;
					v101 = v111;
				}
			}
		}
		c.give(cb);
	}
}

fn select_index(thresholds: &[f32], n: usize, v: f32) -> usize {
	thresholds.iter().position(|t| v < *t).unwrap_or(n - 1)
}

fn sample_spline(s: &Spline<usize>, c: &mut Ctx, get: &mut dyn FnMut(usize, &mut Ctx) -> f32) -> f32 {
	let Spline::Multi { coord, locs, vals, ders } = s else {
		let Spline::Const(v) = s else { unreachable!() };
		return *v;
	};
	let input = get(*coord, c);
	let start = locs.partition_point(|l| !(input < *l)) as i32 - 1;
	let last = locs.len() - 1;
	let ext = |val: f32, i: usize| if ders[i] == 0.0 { val } else { val + ders[i] * (input - locs[i]) };
	if start < 0 {
		let v = sample_spline(&vals[0], c, get);
		return ext(v, 0);
	}
	let start = start as usize;
	if start == last {
		let v = sample_spline(&vals[last], c, get);
		return ext(v, last);
	}
	let (x1, x2) = (locs[start], locs[start + 1]);
	let t = (input - x1) / (x2 - x1);
	let y1 = sample_spline(&vals[start], c, get);
	let y2 = sample_spline(&vals[start + 1], c, get);
	let (d1, d2) = (ders[start], ders[start + 1]);
	let a = d1 * (x2 - x1) - (y2 - y1);
	let b = -d2 * (x2 - x1) + (y2 - y1);
	lerp(t, y1, y2) + t * (1.0 - t) * lerp(t, a, b)
}

impl Compiler<'_> {
	pub fn finish(self) -> Graph {
		Graph { nodes: self.nodes, noises: self.noises, cache_count: self.cache_count }
	}
}
