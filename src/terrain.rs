use crate::biome::{BiomeTable, Climate};
use crate::density::{parse, Compiler, Ctx, Graph, Id, Volume};
use crate::noise::{floor, Simplex};
use crate::random::{Legacy, Rng, Xoroshiro, XoroshiroFactory};
use crate::registry::Registry;
use serde_json::Value;
use std::collections::HashMap;

pub type B = u16;
pub const AIR: B = 0;
pub const STONE: B = 1;
pub const WATER: B = 2;
pub const LAVA: B = 3;
const BEDROCK: B = 4;
const FIXED: [&str; 5] = ["minecraft:air", "minecraft:stone", "minecraft:water", "minecraft:lava", "minecraft:bedrock"];

pub const MIN_Y: i32 = -64;
pub const HEIGHT: i32 = 384;
const SEA: i32 = 63;
const WAY_BELOW_MIN_Y: i32 = -32768;

fn is_fluid(b: B) -> bool {
	b == WATER || b == LAVA
}

pub struct Palette {
	pub names: Vec<String>,
}

impl Palette {
	pub fn id(&mut self, name: &str) -> B {
		let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
		if let Some(i) = self.names.iter().position(|n| *n == name) {
			return i as B;
		}
		self.names.push(name);
		(self.names.len() - 1) as B
	}
}

pub struct Chunk {
	pub cx: i32,
	pub cz: i32,
	pub blocks: Vec<B>,
	pub height: [i32; 256],
}

impl Chunk {
	fn new(cx: i32, cz: i32) -> Self {
		Self { cx, cz, blocks: vec![AIR; 256 * HEIGHT as usize], height: [MIN_Y; 256] }
	}

	#[inline]
	fn idx(x: i32, y: i32, z: i32) -> usize {
		((((z << 4) | x) * HEIGHT) + (y - MIN_Y)) as usize
	}

	#[inline]
	pub fn get(&self, x: i32, y: i32, z: i32) -> B {
		if y < MIN_Y || y >= MIN_Y + HEIGHT {
			return AIR;
		}
		self.blocks[Self::idx(x, y, z)]
	}

	pub(crate) fn set(&mut self, x: i32, y: i32, z: i32, b: B) {
		if y < MIN_Y || y >= MIN_Y + HEIGHT {
			return;
		}
		self.blocks[Self::idx(x, y, z)] = b;
		self.update_height(x, y, z, b);
	}

	fn update_height(&mut self, x: i32, y: i32, z: i32, b: B) {
		let col = ((z << 4) | x) as usize;
		let first = self.height[col];
		if y <= first - 2 {
			return;
		}
		if b != AIR {
			if y >= first {
				self.height[col] = y + 1;
			}
		} else if first - 1 == y {
			self.height[col] = (MIN_Y..y).rev().find(|&yy| self.get(x, yy, z) != AIR).map_or(MIN_Y, |yy| yy + 1);
		}
	}

	fn surface(&self, x: i32, z: i32) -> i32 {
		self.height[((z << 4) | x) as usize] - 1
	}

	fn grad_x(&self, x: i32, z: i32) -> i32 {
		self.surface((x + 1).min(15), z) - self.surface((x - 1).max(0), z)
	}

	fn grad_z(&self, x: i32, z: i32) -> i32 {
		self.surface(x, (z + 1).min(15)) - self.surface(x, (z - 1).max(0))
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Fluid {
	level: i32,
	ty: B,
}

impl Fluid {
	fn at(self, y: i32) -> B {
		if y < self.level { self.ty } else { AIR }
	}
}

fn global_fluid(y: i32) -> Fluid {
	if y < -54 { Fluid { level: -54, ty: LAVA } } else { Fluid { level: SEA, ty: WATER } }
}

const SURFACE_OFFSETS: [[i32; 2]; 13] =
	[[0, 0], [-2, -1], [-1, -1], [0, -1], [1, -1], [-3, 0], [-2, 0], [-1, 0], [1, 0], [-2, 1], [-1, 1], [0, 1], [1, 1]];

fn similarity(d1: i32, d2: i32) -> f64 {
	1.0 - (d2 - d1) as f64 / 25.0
}

fn clampd(v: f64, min: f64, max: f64) -> f64 {
	if v < min { min } else { v.min(max) }
}

fn map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
	c + (v - a) / (b - a) * (d - c)
}

struct Aquifer {
	min_gx: i32,
	min_gy: i32,
	min_gz: i32,
	sx: i32,
	sz: i32,
	status: Vec<Option<Fluid>>,
	loc: Vec<Option<(i32, i32, i32)>>,
	skip_above: i32,
	surface_cache: HashMap<(i32, i32), i32>,
	/// (x, z, grid y) the candidates below are for: (grid index, horizontal distance squared, y)
	cand_at: (i32, i32, i32),
	cand: [(usize, i32, i32); 12],
}

impl Aquifer {
	fn new(t: &Terrain, c: &mut Ctx, v: &Volume) -> Self {
		let min_gx = (v.mx - 5) >> 4;
		let max_gx = ((v.max_x() - 5) >> 4) + 1;
		let min_gy = (v.my + 1).div_euclid(12) - 1;
		let max_gy = (v.max_y() + 1).div_euclid(12) + 1;
		let min_gz = (v.mz - 5) >> 4;
		let max_gz = ((v.max_z() - 5) >> 4) + 1;
		let (sx, sy, sz) = (max_gx - min_gx + 1, max_gy - min_gy + 1, max_gz - min_gz + 1);
		let n = (sx * sy * sz) as usize;
		let mut a = Self {
			min_gx,
			min_gy,
			min_gz,
			sx,
			sz,
			status: vec![None; n],
			loc: vec![None; n],
			skip_above: 0,
			surface_cache: HashMap::new(),
			cand_at: (i32::MIN, 0, 0),
			cand: [(0, 0, 0); 12],
		};
		let max_surface = a.max_surface(t, c, min_gx << 4, min_gz << 4, (max_gx << 4) + 9, (max_gz << 4) + 9) + 8;
		let skip_gy = (max_surface + 12).div_euclid(12) + 1;
		a.skip_above = skip_gy * 12 + 11 - 1;
		a
	}

	fn max_surface(&mut self, t: &Terrain, c: &mut Ctx, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> i32 {
		let (qx0, qx1, qz0, qz1) = (min_x >> 2, max_x >> 2, min_z >> 2, max_z >> 2);
		let v = Volume::stepped(qx1 - qx0 + 1, 1, qz1 - qz0 + 1, qx0 << 2, 0, qz0 << 2, 4, 1, 4);
		let mut buf = vec![0f32; v.size()];
		t.g.vol(c, t.aq_surface, &mut buf, &v);
		let mut max = i32::MIN;
		for z in 0..v.sz {
			for x in 0..v.sx {
				let s = floor(buf[v.idx(x, 0, z)] as f64);
				self.surface_cache.insert((v.block_x(x), v.block_z(z)), s);
				max = max.max(s);
			}
		}
		max
	}

	fn surface_level(&mut self, t: &Terrain, c: &mut Ctx, bx: i32, bz: i32) -> i32 {
		let (qx, qz) = ((bx >> 2) << 2, (bz >> 2) << 2);
		*self.surface_cache.entry((qx, qz)).or_insert_with(|| floor(t.g.val(c, t.aq_surface, qx, 0, qz) as f64))
	}

	fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
		(((gy - self.min_gy) * self.sz + (gz - self.min_gz)) * self.sx + (gx - self.min_gx)) as usize
	}

	fn compute(&mut self, t: &Terrain, c: &mut Ctx, bx: i32, by: i32, bz: i32, density: f64) -> Option<B> {
		if density > 0.0 {
			return None;
		}
		let global = global_fluid(by);
		if by > self.skip_above {
			return Some(global.at(by));
		}
		if global.at(by) == LAVA {
			return Some(LAVA);
		}
		let ya = (by + 1).div_euclid(12);
		if self.cand_at != (bx, bz, ya) {
			self.cand_at = (bx, bz, ya);
			let (xa, za) = ((bx - 5) >> 4, (bz - 5) >> 4);
			let mut k = 0;
			for x1 in 0..=1 {
				for y1 in -1..=1 {
					for z1 in 0..=1 {
						let (gx, gy, gz) = (xa + x1, ya + y1, za + z1);
						let index = self.index(gx, gy, gz);
						let (lx, ly, lz) = *self.loc[index].get_or_insert_with(|| {
							let mut r = t.aquifer_random.at(gx, gy, gz);
							let ox = r.next_bound(10);
							let oy = r.next_bound(9);
							let oz = r.next_bound(10);
							((gx << 4) + ox, gy * 12 + oy, (gz << 4) + oz)
						});
						let (dx, dz) = (lx - bx, lz - bz);
						self.cand[k] = (index, dx * dx + dz * dz, ly);
						k += 1;
					}
				}
			}
		}
		let (mut d0, mut d1, mut d2, mut d3) = (i32::MAX, i32::MAX, i32::MAX, i32::MAX);
		let (mut c0, mut c1, mut c2, mut c3) = (0usize, 0usize, 0usize, 0usize);
		for &(index, dxz, ly) in &self.cand {
			let dy = ly - by;
			let nd = dxz + dy * dy;
			// insert into the sorted d0..d3 (later equal ones go first), written as selects
			let (b0, b1, b2, b3) = (d0 >= nd, d1 >= nd, d2 >= nd, d3 >= nd);
			(d3, c3) = if b2 { (d2, c2) } else if b3 { (nd, index) } else { (d3, c3) };
			(d2, c2) = if b1 { (d1, c1) } else if b2 { (nd, index) } else { (d2, c2) };
			(d1, c1) = if b0 { (d0, c0) } else if b1 { (nd, index) } else { (d1, c1) };
			(d0, c0) = if b0 { (nd, index) } else { (d0, c0) };
		}
		let (d, ci) = ([d0, d1, d2, d3], [c0, c1, c2, c3]);
		let flow = similarity(100, 144);
		let s1 = self.get_status(t, c, ci[0]);
		let sim12 = similarity(d[0], d[1]);
		let fluid = s1.at(by);
		if sim12 <= 0.0 {
			if sim12 >= flow {
				self.get_status(t, c, ci[1]);
			}
			return Some(fluid);
		}
		if fluid == WATER && global_fluid(by - 1).at(by - 1) == LAVA {
			return Some(fluid);
		}
		let mut barrier = f64::NAN;
		let s2 = self.get_status(t, c, ci[1]);
		if density + sim12 * self.pressure(t, c, bx, by, bz, &mut barrier, s1, s2) > 0.0 {
			return None;
		}
		let s3 = self.get_status(t, c, ci[2]);
		let sim13 = similarity(d[0], d[2]);
		if sim13 > 0.0 && density + sim12 * sim13 * self.pressure(t, c, bx, by, bz, &mut barrier, s1, s3) > 0.0 {
			return None;
		}
		let sim23 = similarity(d[1], d[2]);
		if sim23 > 0.0 && density + sim12 * sim23 * self.pressure(t, c, bx, by, bz, &mut barrier, s2, s3) > 0.0 {
			return None;
		}
		let may_flow = s1 != s2 || (sim23 >= flow && s2 != s3) || (sim13 >= flow && s1 != s3);
		if !may_flow && sim13 >= flow && similarity(d[0], d[3]) >= flow {
			self.get_status(t, c, ci[3]);
		}
		Some(fluid)
	}

	#[allow(clippy::too_many_arguments)]
	fn pressure(&self, t: &Terrain, c: &mut Ctx, bx: i32, by: i32, bz: i32, barrier: &mut f64, a: Fluid, b: Fluid) -> f64 {
		let (ta, tb) = (a.at(by), b.at(by));
		if (ta == LAVA && tb == WATER) || (ta == WATER && tb == LAVA) {
			return 2.0;
		}
		let diff = (a.level - b.level).abs();
		if diff == 0 {
			return 0.0;
		}
		let avg = 0.5 * (a.level + b.level) as f64;
		let above = by as f64 + 0.5 - avg;
		let base = diff as f64 / 2.0;
		let towards_middle = base - above.abs();
		let gradient = if above > 0.0 {
			let p = 0.0 + towards_middle;
			if p > 0.0 { p / 1.5 } else { p / 2.5 }
		} else {
			let p = 3.0 + towards_middle;
			if p > 0.0 { p / 3.0 } else { p / 10.0 }
		};
		let noise = if !(gradient < -2.0) && !(gradient > 2.0) {
			if barrier.is_nan() {
				*barrier = t.g.val(c, t.aq[0], bx, by, bz) as f64;
			}
			*barrier
		} else {
			0.0
		};
		2.0 * (noise + gradient)
	}

	fn get_status(&mut self, t: &Terrain, c: &mut Ctx, index: usize) -> Fluid {
		if let Some(s) = self.status[index] {
			return s;
		}
		let (x, y, z) = self.loc[index].unwrap();
		let s = self.compute_fluid(t, c, x, y, z);
		self.status[index] = Some(s);
		s
	}

	fn compute_fluid(&mut self, t: &Terrain, c: &mut Ctx, x: i32, y: i32, z: i32) -> Fluid {
		let global = global_fluid(y);
		let mut lowest = i32::MAX;
		let (top, bottom) = (y + 12, y - 12);
		let mut center_under = false;
		for o in SURFACE_OFFSETS {
			let (sx, sz) = (x + (o[0] << 4), z + (o[1] << 4));
			let surface = self.surface_level(t, c, sx, sz);
			let adjusted = surface + 8;
			let start = o == [0, 0];
			if start && bottom > adjusted {
				return global;
			}
			let pokes = top > adjusted;
			if pokes || start {
				let at_surface = global_fluid(adjusted);
				if at_surface.at(adjusted) != AIR {
					if start {
						center_under = true;
					}
					if pokes {
						return at_surface;
					}
				}
			}
			lowest = lowest.min(surface);
		}
		let level = self.compute_surface_level(t, c, x, y, z, global, lowest, center_under);
		let mut ty = global.ty;
		if level <= -10 && level != WAY_BELOW_MIN_Y && global.ty != LAVA {
			let lava = t.g.val(c, t.aq[3], x.div_euclid(64), y.div_euclid(40), z.div_euclid(64)) as f64;
			if lava.abs() > 0.3 {
				ty = LAVA;
			}
		}
		Fluid { level, ty }
	}

	#[allow(clippy::too_many_arguments)]
	fn compute_surface_level(&self, t: &Terrain, c: &mut Ctx, x: i32, y: i32, z: i32, global: Fluid, lowest: i32, center_under: bool) -> i32 {
		let (partial, full);
		if t.g.val(c, t.aq[4], x, y, z) > 0.0 {
			partial = -1.0;
			full = -1.0;
		} else {
			let below = lowest + 8 - y;
			let factor = if center_under {
				let f = (below as f64 - 0.0) / (64.0 - 0.0);
				if f < 0.0 { 1.0 } else if f > 1.0 { 0.0 } else { 1.0 + f * (0.0 - 1.0) }
			} else {
				0.0
			};
			let noise = clampd(t.g.val(c, t.aq[1], x, y, z) as f64, -1.0, 1.0);
			full = noise - map(factor, 1.0, 0.0, -0.3, 0.8);
			partial = noise - map(factor, 1.0, 0.0, -0.8, 0.4);
		}
		if full > 0.0 {
			global.level
		} else if partial > 0.0 {
			let (cx, cy, cz) = (x.div_euclid(16), y.div_euclid(40), z.div_euclid(16));
			let spread = (t.g.val(c, t.aq[2], cx, cy, cz) * 10.0f32) as f64;
			let q = floor(spread / 3.0) * 3;
			lowest.min(cy * 40 + 20 + q)
		} else {
			WAY_BELOW_MIN_Y
		}
	}
}

enum Cond {
	Biome(u128),
	Noise { n: usize, min: f64, max: f64, is3d: bool },
	Gradient { rf: XoroshiroFactory, true_at: i32, false_at: i32 },
	YAbove { anchor: i32, mult: i32, add: bool },
	Water { offset: i32, mult: i32, add: bool },
	Steep,
	Hole,
	AbovePreliminary,
	StoneDepth { offset: i32, add_surface: bool, secondary: i32, ceiling: bool },
	Not(Box<Cond>),
}

enum Rule {
	Block(B),
	Seq(Vec<Rule>),
	Cond(Cond, Box<Rule>),
	Bandlands,
	Ore { k: usize, ore: B, raw: B, filler: B, chance: f32, gap: Id },
}

fn anchor(v: &Value) -> i32 {
	if let Some(a) = v.get("absolute") {
		a.as_i64().unwrap() as i32
	} else if let Some(a) = v.get("above_bottom") {
		MIN_Y + a.as_i64().unwrap() as i32
	} else if let Some(a) = v.get("below_top") {
		HEIGHT - 1 + MIN_Y - a.as_i64().unwrap() as i32
	} else {
		panic!("anchor {v}")
	}
}

fn ty(v: &Value) -> &str {
	v["type"].as_str().unwrap().trim_start_matches("minecraft:")
}

struct Builder<'a, 'r> {
	reg: &'r Registry,
	comp: &'a mut Compiler<'r>,
	palette: &'a mut Palette,
	table: &'a BiomeTable,
	random: XoroshiroFactory,
	ores: Vec<(Id, Id)>,
}

impl Builder<'_, '_> {
	fn rule(&mut self, v: &Value) -> Rule {
		if let Some(name) = v.as_str() {
			return self.rule(&self.reg.get("worldgen/material_rule", name).clone());
		}
		match ty(v) {
			"block" => Rule::Block(self.palette.id(v["result_state"].as_str().unwrap())),
			"sequence" => {
				let mut items: Vec<Rule> = v["sequence"].as_array().unwrap().iter().map(|r| self.rule(r)).collect();
				if items.len() == 1 { items.pop().unwrap() } else { Rule::Seq(items) }
			}
			"condition" => {
				let c = self.cond(&v["if_true"]);
				Rule::Cond(c, Box::new(self.rule(&v["then_run"])))
			}
			"bandlands" => Rule::Bandlands,
			"ore_vein" => {
				let d = self.comp.sampler(&parse(&v["density"]));
				let r = self.comp.sampler(&parse(&v["richness"]));
				let gap = self.comp.sampler(&parse(&v["filler_gap"]));
				self.ores.push((d, r));
				let mut b = |k: &str| self.palette.id(v[k].as_str().unwrap());
				Rule::Ore {
					k: self.ores.len() - 1,
					ore: b("ore_block"),
					raw: b("raw_ore_block"),
					filler: b("filler_block"),
					chance: v["raw_ore_chance"].as_f64().unwrap() as f32,
					gap,
				}
			}
			t => panic!("material rule {t}"),
		}
	}

	fn cond(&mut self, v: &Value) -> Cond {
		if let Some(name) = v.as_str() {
			return self.cond(&self.reg.get("worldgen/material_condition", name).clone());
		}
		let i = |k: &str| v[k].as_i64().unwrap() as i32;
		let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
		match ty(v) {
			"biome" => {
				let names: Vec<&str> = match &v["biome_is"] {
					Value::String(s) => vec![s.as_str()],
					Value::Array(a) => a.iter().map(|s| s.as_str().unwrap()).collect(),
					o => panic!("biome_is {o}"),
				};
				assert!(names.iter().all(|n| !n.starts_with('#')), "biome tags unsupported");
				Cond::Biome(names.iter().filter_map(|n| self.table.names.iter().position(|x| x == n)).fold(0, |m, i| m | 1 << i))
			}
			"noise_threshold" => Cond::Noise {
				n: self.comp.noise(v["noise"].as_str().unwrap()),
				min: v["min_threshold"].as_f64().unwrap(),
				max: v["max_threshold"].as_f64().unwrap(),
				is3d: b("is_3d"),
			},
			// Paper's bedrock condition is vertical_gradient unless the server enables flat bedrock
			"vertical_gradient" | "paper:optionally_flat_bedrock_material_condition" => Cond::Gradient {
				rf: self.random.from_hash_of(v["random_name"].as_str().unwrap()).fork_positional(),
				true_at: anchor(&v["true_at_and_below"]),
				false_at: anchor(&v["false_at_and_above"]),
			},
			"y_above" => Cond::YAbove { anchor: anchor(&v["anchor"]), mult: i("surface_depth_multiplier"), add: b("add_stone_depth") },
			"water" => Cond::Water { offset: i("offset"), mult: i("surface_depth_multiplier"), add: b("add_stone_depth") },
			"steep" => Cond::Steep,
			"hole" => Cond::Hole,
			"above_preliminary_surface" => Cond::AbovePreliminary,
			"stone_depth" => Cond::StoneDepth {
				offset: i("offset"),
				add_surface: b("add_surface_depth"),
				secondary: i("secondary_depth_range"),
				ceiling: v["surface_type"].as_str() == Some("ceiling"),
			},
			"not" => Cond::Not(Box::new(self.cond(&v["invert"]))),
			t => panic!("material condition {t}"),
		}
	}
}

#[derive(Clone, Copy)]
enum FloatP {
	Const(f32),
	Uniform(f32, f32),
	Trapezoid(f32, f32, f32),
}

impl FloatP {
	fn parse(v: &Value) -> Self {
		if let Some(n) = v.as_f64() {
			return FloatP::Const(n as f32);
		}
		let g = |k: &str| v[k].as_f64().unwrap() as f32;
		match ty(v) {
			"constant" => FloatP::Const(g("value")),
			"uniform" => FloatP::Uniform(g("min_inclusive"), g("max_exclusive")),
			"trapezoid" => FloatP::Trapezoid(g("min"), g("max"), g("plateau")),
			t => panic!("float provider {t}"),
		}
	}

	fn sample(self, r: &mut impl Rng) -> f32 {
		match self {
			FloatP::Const(v) => v,
			FloatP::Uniform(a, b) => r.next_float() * (b - a) + a,
			FloatP::Trapezoid(min, max, plateau) => {
				let range = max - min;
				let start = (range - plateau) / 2.0;
				let end = range - start;
				min + r.next_float() * end + r.next_float() * start
			}
		}
	}
}

fn uniform_height(v: &Value) -> (i32, i32) {
	assert_eq!(ty(v), "uniform");
	(anchor(&v["min_inclusive"]), anchor(&v["max_inclusive"]))
}

fn sample_height(r: &mut impl Rng, (min, max): (i32, i32)) -> i32 {
	if min > max { min } else { r.next_bound(max - min + 1) + min }
}

#[derive(Clone, Copy)]
enum Carver {
	Cave {
		prob: f32,
		y: (i32, i32),
		count: (i32, i32),
		thickness: FloatP,
		weird: bool,
		room_vr: FloatP,
		hr: FloatP,
		vr: FloatP,
		start_vr: FloatP,
		floor: FloatP,
	},
	Canyon {
		prob: f32,
		y: (i32, i32),
		vrot: FloatP,
		distance: FloatP,
		thickness: FloatP,
		smooth: i32,
		hr_factor: FloatP,
		vr_default: f32,
		vr_center: f32,
		y_scale: FloatP,
	},
}

impl Carver {
	fn parse(v: &Value) -> Self {
		let f = |k: &str| FloatP::parse(&v[k]);
		let prob = v["probability"].as_f64().unwrap() as f32;
		match ty(v) {
			"cave" => {
				let c = &v["count"];
				let count = match c.as_i64() {
					Some(n) => (n as i32, -1),
					None => {
						assert_eq!(ty(c), "very_biased_to_bottom");
						(c["min_inclusive"].as_i64().unwrap() as i32, c["max_inclusive"].as_i64().unwrap() as i32)
					}
				};
				Carver::Cave {
					prob,
					y: uniform_height(&v["y"]),
					count,
					thickness: f("thickness"),
					weird: v.get("weird_thickness_bias").and_then(Value::as_bool).unwrap_or(false),
					room_vr: f("room_vertical_radius_multiplier"),
					hr: f("horizontal_radius_multiplier"),
					vr: f("vertical_radius_multiplier"),
					start_vr: v.get("start_vertical_radius_multiplier").map_or(FloatP::Const(1.0), FloatP::parse),
					floor: f("floor_level"),
				}
			}
			"canyon" => {
				let s = &v["shape"];
				let g = |k: &str| FloatP::parse(&s[k]);
				Carver::Canyon {
					prob,
					y: uniform_height(&v["y"]),
					vrot: f("vertical_rotation"),
					distance: g("distance_factor"),
					thickness: g("thickness"),
					smooth: s["width_smoothness"].as_i64().unwrap() as i32,
					hr_factor: g("horizontal_radius_factor"),
					vr_default: s["vertical_radius_default_factor"].as_f64().unwrap() as f32,
					vr_center: s["vertical_radius_center_factor"].as_f64().unwrap() as f32,
					y_scale: g("y_scale"),
				}
			}
			t => panic!("carver {t}"),
		}
	}

	fn prob(&self) -> f32 {
		match self {
			Carver::Cave { prob, .. } | Carver::Canyon { prob, .. } => *prob,
		}
	}
}

/// Minecraft's sine table. Vanilla builds it with Java's Math.sin, whose last bit can differ between platforms,
/// so the plugin sets it from the server's own table; otherwise it's computed here.
pub static SIN: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();

fn sin_table() -> &'static [f32] {
	SIN.get_or_init(|| (0..65536).map(|i| (i as f64 / 10430.378350470453).sin() as f32).collect())
}

fn msin(v: f64) -> f32 {
	sin_table()[((v * 10430.378350470453) as i64 & 65535) as usize]
}

fn mcos(v: f64) -> f32 {
	sin_table()[((v * 10430.378350470453 + 16384.0) as i64 & 65535) as usize]
}

const PI_F: f32 = std::f32::consts::PI;

struct Mask {
	bits: Vec<bool>,
}

const MASK_MIN: i32 = MIN_Y + 1;
const MASK_MAX: i32 = MIN_Y + HEIGHT - 1 - 7;
const MASK_H: i32 = MASK_MAX - MASK_MIN + 1;

impl Mask {
	fn carve(&mut self, x: i32, y: i32, z: i32) {
		self.bits[(y - MASK_MIN + (z + (x << 4)) * MASK_H) as usize] = true;
	}

	fn has(&self, x: i32, y: i32, z: i32) -> bool {
		(MASK_MIN..=MASK_MAX).contains(&y) && self.bits[(y - MASK_MIN + (z + (x << 4)) * MASK_H) as usize]
	}
}

fn ceiling_offset(r: &Rule) -> i32 {
	fn cond(c: &Cond) -> i32 {
		match c {
			Cond::StoneDepth { offset, secondary, ceiling: true, .. } => {
				assert_eq!(*secondary, 0, "ceiling stone_depth with secondary depth breaks the cut margin");
				*offset
			}
			Cond::Not(c) => cond(c),
			_ => 0,
		}
	}
	match r {
		Rule::Seq(rs) => rs.iter().map(ceiling_offset).max().unwrap_or(0),
		Rule::Cond(c, then) => cond(c).max(ceiling_offset(then)),
		_ => 0,
	}
}

const CUT_SLACK: i32 = 4;
const CUT_SPLIT: i32 = 48;

struct CarveCtx<'a> {
	cx: i32,
	cz: i32,
	mask: &'a mut Mask,
}

impl CarveCtx<'_> {
	#[allow(clippy::too_many_arguments)]
	fn ellipsoid(&mut self, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: &dyn Fn(f64, f64, f64, i32) -> bool) {
		let (center_x, center_z) = ((self.cx * 16 + 8) as f64, (self.cz * 16 + 8) as f64);
		let max_delta = 16.0 + hr * 2.0;
		if (x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta {
			return;
		}
		let (mx, mz) = (self.cx * 16, self.cz * 16);
		let min_xi = (floor(x - hr) - mx - 1).max(0);
		let max_xi = (floor(x + hr) - mx).min(15);
		let min_y = (floor(y - vr) - 1).max(MASK_MIN);
		let max_y = (floor(y + vr) + 1).min(MASK_MAX);
		let min_zi = (floor(z - hr) - mz - 1).max(0);
		let max_zi = (floor(z + hr) - mz).min(15);
		for xi in min_xi..=max_xi {
			let xd = ((mx + xi) as f64 + 0.5 - x) / hr;
			for zi in min_zi..=max_zi {
				let zd = ((mz + zi) as f64 + 0.5 - z) / hr;
				if xd * xd + zd * zd >= 1.0 {
					continue;
				}
				let mut wy = max_y;
				while wy > min_y {
					let yd = (wy as f64 - 0.5 - y) / vr;
					if !skip(xd, yd, zd, wy) {
						self.mask.carve(xi, wy, zi);
					}
					wy -= 1;
				}
			}
		}
	}

	fn can_reach(&self, x: f64, z: f64, step: i32, total: i32, thickness: f32) -> bool {
		let xd = x - (self.cx * 16 + 8) as f64;
		let zd = z - (self.cz * 16 + 8) as f64;
		let remaining = (total - step) as f64;
		let rr = (thickness + 2.0 + 16.0) as f64;
		xd * xd + zd * zd - remaining * remaining <= rr * rr
	}

	fn carve(&mut self, carver: &Carver, r: &mut Legacy, sx: i32, sz: i32) {
		match *carver {
			Carver::Cave { y, count, thickness, weird, room_vr, hr, vr, start_vr, floor: fl, .. } => {
				let max_distance = (4 * 2 - 1) << 4;
				let n = if count.1 < 0 {
					count.0
				} else {
					let a = r.next_bound(count.1 - count.0 + 1) + 1;
					let b = r.next_bound(a) + 1;
					count.0 + r.next_bound(b)
				};
				for _ in 0..n {
					let x = (sx * 16 + r.next_bound(16)) as f64;
					let yy = sample_height(r, y) as f64;
					let z = (sz * 16 + r.next_bound(16)) as f64;
					let hrm = hr.sample(r) as f64;
					let vrm = vr.sample(r) as f64;
					let svr = start_vr.sample(r) as f64;
					let floor_level = fl.sample(r) as f64;
					let skip = move |xd: f64, yd: f64, zd: f64, _: i32| yd <= floor_level || xd * xd + yd * yd + zd * zd >= 1.0;
					let mut tunnels = 1;
					if r.next_bound(4) == 0 {
						let y_scale = room_vr.sample(r) as f64;
						let th = 1.0 + r.next_float() * 6.0;
						let hrad = 1.5 + (msin((PI_F / 2.0) as f64) * th) as f64;
						self.ellipsoid(x + 1.0, yy, z, hrad, hrad * y_scale, &skip);
						tunnels += r.next_bound(4);
					}
					for _ in 0..tunnels {
						let hrot = r.next_float() * (PI_F * 2.0);
						let vrot = (r.next_float() - 0.5) / 4.0;
						let mut th = thickness.sample(r);
						if weird && r.next_bound(10) == 0 {
							th *= r.next_float() * r.next_float() * 3.0 + 1.0;
						}
						let dist = max_distance - r.next_bound(max_distance / 4);
						let seed = r.next_long();
						self.tunnel(seed, x, yy, z, hrm, vrm, th, hrot, vrot, 0, dist, svr, &skip);
					}
				}
			}
			Carver::Canyon { y, vrot, distance, thickness, smooth, hr_factor, vr_default, vr_center, y_scale, .. } => {
				let max_distance = (4 * 2 - 1) * 16;
				let x = (sx * 16 + r.next_bound(16)) as f64;
				let yy = sample_height(r, y) as f64;
				let z = (sz * 16 + r.next_bound(16)) as f64;
				let mut hrot = r.next_float() * (PI_F * 2.0);
				let mut vr = vrot.sample(r);
				let ys = y_scale.sample(r) as f64;
				let th = thickness.sample(r);
				let dist = (max_distance as f32 * distance.sample(r)) as i32;
				let mut r = Legacy::new(r.next_long());
				let mut widths = vec![0f32; HEIGHT as usize];
				let mut w = 1.0f32;
				for (i, slot) in widths.iter_mut().enumerate() {
					if i == 0 || r.next_bound(smooth) == 0 {
						w = 1.0 + r.next_float() * r.next_float();
					}
					*slot = w * w;
				}
				let skip = |xd: f64, yd: f64, zd: f64, wy: i32| {
					(xd * xd + zd * zd) * widths[(wy - MIN_Y - 1) as usize] as f64 + yd * yd / 6.0 >= 1.0
				};
				let (mut x, mut yy, mut z) = (x, yy, z);
				let (mut yrota, mut xrota) = (0f32, 0f32);
				for step in 0..dist {
					let mut hrad = 1.5 + (msin((step as f32 * PI_F / dist as f32) as f64) * th) as f64;
					let mut vrad = hrad * ys;
					hrad *= hr_factor.sample(&mut r) as f64;
					let vm = 1.0 - (0.5 - step as f32 / dist as f32).abs() * 2.0;
					let factor = vr_default + vr_center * vm;
					vrad = factor as f64 * vrad * (r.next_float() * (1.0 - 0.75) + 0.75) as f64;
					let xc = mcos(vr as f64);
					let xs = msin(vr as f64);
					x += (mcos(hrot as f64) * xc) as f64;
					yy += xs as f64;
					z += (msin(hrot as f64) * xc) as f64;
					vr *= 0.7;
					vr += xrota * 0.05;
					hrot += yrota * 0.05;
					xrota *= 0.8;
					yrota *= 0.5;
					let (a, b) = (r.next_float(), r.next_float());
					xrota += (a - b) * r.next_float() * 2.0;
					let (a, b) = (r.next_float(), r.next_float());
					yrota += (a - b) * r.next_float() * 4.0;
					if r.next_bound(4) != 0 {
						if !self.can_reach(x, z, step, dist, th) {
							return;
						}
						self.ellipsoid(x, yy, z, hrad, vrad, &skip);
					}
				}
			}
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn tunnel(
		&mut self,
		seed: i64,
		mut x: f64,
		mut y: f64,
		mut z: f64,
		hrm: f64,
		vrm: f64,
		thickness: f32,
		mut hrot: f32,
		mut vrot: f32,
		step: i32,
		dist: i32,
		y_scale: f64,
		skip: &dyn Fn(f64, f64, f64, i32) -> bool,
	) {
		let mut r = Legacy::new(seed);
		let split = r.next_bound(dist / 2) + dist / 4;
		let steep = r.next_bound(6) == 0;
		let (mut yrota, mut xrota) = (0f32, 0f32);
		for cur in step..dist {
			let hrad = 1.5 + (msin((PI_F * cur as f32 / dist as f32) as f64) * thickness) as f64;
			let vrad = hrad * y_scale;
			let cos_x = mcos(vrot as f64);
			x += (mcos(hrot as f64) * cos_x) as f64;
			y += msin(vrot as f64) as f64;
			z += (msin(hrot as f64) * cos_x) as f64;
			vrot *= if steep { 0.92 } else { 0.7 };
			vrot += xrota * 0.1;
			hrot += yrota * 0.1;
			xrota *= 0.9;
			yrota *= 0.75;
			let (a, b) = (r.next_float(), r.next_float());
			xrota += (a - b) * r.next_float() * 2.0;
			let (a, b) = (r.next_float(), r.next_float());
			yrota += (a - b) * r.next_float() * 4.0;
			if cur == split && thickness > 1.0 {
				let s = r.next_long();
				let t = r.next_float() * 0.5 + 0.5;
				self.tunnel(s, x, y, z, hrm, vrm, t, hrot - PI_F / 2.0, vrot / 3.0, cur, dist, 1.0, skip);
				let s = r.next_long();
				let t = r.next_float() * 0.5 + 0.5;
				self.tunnel(s, x, y, z, hrm, vrm, t, hrot + PI_F / 2.0, vrot / 3.0, cur, dist, 1.0, skip);
				return;
			}
			if r.next_bound(4) != 0 {
				if !self.can_reach(x, z, cur, dist, thickness) {
					return;
				}
				self.ellipsoid(x, y, z, hrad * hrm, vrad * vrm, skip);
			}
		}
	}
}

fn lcg(r: i64, c: i64) -> i64 {
	r.wrapping_mul(r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)).wrapping_add(c)
}

fn fiddle(r: i64) -> f64 {
	((r >> 24).rem_euclid(1024) as f64 / 1024.0 - 0.5) * 0.9
}

pub fn zoom(seed: i64, x: i32, y: i32, z: i32, mut biome: impl FnMut(i32, i32, i32) -> u16) -> u16 {
	let (ax, ay, az) = (x - 2, y - 2, z - 2);
	let (px, py, pz) = (ax >> 2, ay >> 2, az >> 2);
	let (fx, fy, fz) = ((ax & 3) as f64 / 4.0, (ay & 3) as f64 / 4.0, (az & 3) as f64 / 4.0);
	let mut min_i = 0;
	let mut min_d = f64::INFINITY;
	for i in 0..8 {
		let (xe, ye, ze) = (i & 4 == 0, i & 2 == 0, i & 1 == 0);
		let (cx, cy, cz) = (if xe { px } else { px + 1 }, if ye { py } else { py + 1 }, if ze { pz } else { pz + 1 });
		let (dx, dy, dz) = (if xe { fx } else { fx - 1.0 }, if ye { fy } else { fy - 1.0 }, if ze { fz } else { fz - 1.0 });
		let mut r = lcg(seed, cx as i64);
		r = lcg(r, cy as i64);
		r = lcg(r, cz as i64);
		r = lcg(r, cx as i64);
		r = lcg(r, cy as i64);
		r = lcg(r, cz as i64);
		let ffx = fiddle(r);
		r = lcg(r, seed);
		let ffy = fiddle(r);
		r = lcg(r, seed);
		let ffz = fiddle(r);
		let d = (dz + ffz) * (dz + ffz) + (dy + ffy) * (dy + ffy) + (dx + ffx) * (dx + ffx);
		if min_d > d {
			min_i = i;
			min_d = d;
		}
	}
	biome(if min_i & 4 == 0 { px } else { px + 1 }, if min_i & 2 == 0 { py } else { py + 1 }, if min_i & 1 == 0 { pz } else { pz + 1 })
}

pub fn zoom_seed(seed: i64) -> i64 {
	use sha2::Digest;
	let h = sha2::Sha256::digest(seed.to_le_bytes());
	i64::from_le_bytes(h[..8].try_into().unwrap())
}

pub struct Terrain {
	pub g: Graph,
	pub palette: Palette,
	pub table: BiomeTable,
	seed: i64,
	zoom_seed: i64,
	climate: Climate,
	final_density: Id,
	aq: [Id; 5],
	aq_surface: Id,
	chunk_surface: Id,
	rule: Rule,
	ceiling_offset: i32,
	ores: Vec<(Id, Id)>,
	noise_random: XoroshiroFactory,
	aquifer_random: XoroshiroFactory,
	ore_random: XoroshiroFactory,
	bands: [B; 192],
	n: [usize; 9],
	frozen_noise: Vec<(Simplex, f64, f32)>,
	biome_info: Simplex,
	carvers: Vec<Vec<Carver>>,
	frozen_base: Vec<Option<f32>>,
	eroded_badlands: u16,
	grass: B,
	mycelium: B,
	dirt: B,
	snow_block: B,
	packed_ice: B,
	temp_noise: Simplex,
	climate_info: Vec<(f32, bool)>,
	freeze_blocks: [B; 3],
	snowy: Vec<(B, B)>,
	trees: crate::trees::Trees,
}

const N_SURFACE: usize = 0;
const N_SECONDARY: usize = 1;
const N_CLAY: usize = 2;
const N_PILLAR: usize = 3;
const N_PILLAR_ROOF: usize = 4;
const N_BADLANDS: usize = 5;
const N_ICE_PILLAR: usize = 6;
const N_ICE_ROOF: usize = 7;
const N_ICE_SURFACE: usize = 8;

fn bands(r: &mut Xoroshiro, p: &mut Palette) -> [B; 192] {
	let terracotta = p.id("terracotta");
	let [orange, yellow, brown, red, white, light_gray] =
		["orange", "yellow", "brown", "red", "white", "light_gray"].map(|c| p.id(&format!("{c}_terracotta")));
	let mut b = [terracotta; 192];
	let mut i = 0;
	while i < 192 {
		i += r.next_bound(5) as usize + 1;
		if i < 192 {
			b[i] = orange;
		}
		i += 1;
	}
	let mut make = |r: &mut Xoroshiro, base: i32, s: B| {
		let n = r.next_between(6, 15);
		for _ in 0..n {
			let width = base + r.next_bound(3);
			let start = r.next_bound(192);
			let mut p = 0;
			while start + p < 192 && p < width {
				b[(start + p) as usize] = s;
				p += 1;
			}
		}
	};
	make(r, 1, yellow);
	make(r, 2, brown);
	make(r, 1, red);
	let whites = r.next_between(9, 15);
	let (mut ix, mut start) = (0, 0usize);
	while ix < whites && start < 192 {
		b[start] = white;
		if start as i32 - 1 > 0 && r.next_bool() {
			b[start - 1] = light_gray;
		}
		if start + 1 < 192 && r.next_bool() {
			b[start + 1] = light_gray;
		}
		ix += 1;
		start += r.next_bound(16) as usize + 4;
	}
	b
}

/// Stone blocks y0..=y1 of a run whose top block is `top` and bottom `bot` (the next air or fluid is right below),
/// with `above0` stone blocks counted above the run and the run's water level.
#[derive(Clone, Copy)]
struct Span {
	y0: i32,
	y1: i32,
	top: i32,
	bot: i32,
	above0: i32,
	water: i32,
}

struct RuleCtx {
	ore_bufs: Vec<Option<(Volume, Vec<f32>, Vec<f32>)>>,
	prelim_vol: Volume,
	prelim: Option<Vec<f32>>,
	bx: i32,
	bz: i32,
	grad_x: i32,
	grad_z: i32,
	surface_depth: i32,
	secondary: Option<f64>,
	min_surface: Option<i32>,
	by: i32,
	water: i32,
	stone_below: i32,
	stone_above: i32,
	biome: Option<u16>,
	present: u128,
	noise2d: Vec<f64>,
	noise3d: Vec<f64>,
	noise3d_dirty: bool,
}

impl Terrain {
	pub fn new(reg: &Registry, seed: i64) -> Self {
		let mut base = Xoroshiro::new(seed);
		let random = base.fork_positional();
		let mut comp = Compiler::new(reg, random);
		let settings = reg.get("worldgen/noise_settings", "overworld");
		let router = &settings["noise_router"];
		let fun = |c: &mut Compiler, v: &Value| c.sampler(&parse(v));
		let climate = Climate {
			fns: ["temperature", "vegetation", "continents", "erosion", "depth", "ridges"].map(|k| fun(&mut comp, &router[k])),
		};
		let aquifers = &settings["aquifers"];
		let aq_surface = fun(&mut comp, &aquifers["surface_level"]);
		let aq = ["barrier", "fluid_level_floodedness", "fluid_level_spread", "lava", "exclusion"].map(|k| fun(&mut comp, &aquifers[k]));
		let final_density = fun(&mut comp, &router["final_density"]);
		let chunk_surface = fun(&mut comp, &router["chunk_surface_level"]);
		let table = BiomeTable::overworld();
		let mut palette = Palette { names: FIXED.iter().map(|s| s.to_string()).collect() };
		assert_eq!(settings["default_block"].as_str(), Some("minecraft:stone"));
		let n = [
			"surface",
			"surface_secondary",
			"clay_bands_offset",
			"badlands_pillar",
			"badlands_pillar_roof",
			"badlands_surface",
			"iceberg_pillar",
			"iceberg_pillar_roof",
			"iceberg_surface",
		]
		.map(|k| comp.noise(&format!("minecraft:{k}")));
		let bands = bands(&mut random.from_hash_of("minecraft:clay_bands"), &mut palette);
		let mut b = Builder { reg, comp: &mut comp, palette: &mut palette, table: &table, random, ores: vec![] };
		let rule = b.rule(&settings["material_rule"]);
		let ores = std::mem::take(&mut b.ores);
		let mut frozen_rng = Legacy::new(3456);
		let frozen_noise = [(1.0, 0.14285715f32), (0.5, 0.2857143), (0.25, 0.5714286)]
			.map(|(f, a)| (Simplex::new(&mut frozen_rng), f, a))
			.into_iter()
			.collect();
		let biome_info = Simplex::new(&mut Legacy::new(2345));
		let mut carver_cache: HashMap<String, usize> = HashMap::new();
		let mut carver_list: Vec<Carver> = vec![];
		let mut carvers = vec![];
		let mut frozen_base = vec![];
		let mut climate_info = vec![];
		for name in &table.names {
			let bj = reg.get("worldgen/biome", name);
			climate_info.push((bj["temperature"].as_f64().unwrap() as f32, bj["has_precipitation"].as_bool().unwrap()));
			let ids: Vec<&str> = match &bj["carvers"] {
				Value::String(s) => vec![s.as_str()],
				Value::Array(a) => a.iter().map(|s| s.as_str().unwrap()).collect(),
				_ => vec![],
			};
			let list = ids
				.iter()
				.map(|id| {
					*carver_cache.entry(id.to_string()).or_insert_with(|| {
						carver_list.push(Carver::parse(reg.get("worldgen/carver", id)));
						carver_list.len() - 1
					})
				})
				.collect::<Vec<_>>();
			carvers.push(list);
			frozen_base.push((bj.get("temperature_modifier").and_then(Value::as_str) == Some("frozen")).then(|| bj["temperature"].as_f64().unwrap() as f32));
		}
		if carvers.windows(2).all(|w| w[0] == w[1]) {
			carvers.truncate(1);
		}
		let carvers = carvers.into_iter().map(|l: Vec<usize>| l.into_iter().map(|i| carver_list[i]).collect()).collect();
		Self {
			seed,
			zoom_seed: zoom_seed(seed),
			climate,
			final_density,
			aq,
			aq_surface,
			chunk_surface,
			ceiling_offset: ceiling_offset(&rule).max(0),
			rule,
			ores,
			noise_random: random,
			aquifer_random: random.from_hash_of("minecraft:aquifer").fork_positional(),
			ore_random: random.from_hash_of("minecraft:ore").fork_positional(),
			bands,
			n,
			frozen_noise,
			biome_info,
			carvers,
			frozen_base,
			eroded_badlands: table.id("minecraft:eroded_badlands"),
			grass: palette.id("grass_block"),
			mycelium: palette.id("mycelium"),
			dirt: palette.id("dirt"),
			snow_block: palette.id("snow_block"),
			packed_ice: palette.id("packed_ice"),
			temp_noise: Simplex::new(&mut Legacy::new(1234)),
			climate_info,
			freeze_blocks: ["ice", "snow", "powder_snow"].map(|n| palette.id(n)),
			trees: crate::trees::Trees::new(reg, &mut palette, &table.names),
			snowy: ["grass_block", "mycelium", "podzol"].map(|n| (palette.id(n), palette.id(&format!("minecraft:{n}|{{snowy:true}}")))).to_vec(),
			palette,
			table,
			g: comp.finish(),
		}
	}

	fn noise(&self, i: usize, x: f64, y: f64, z: f64) -> f32 {
		self.g.noises[self.n[i]].get(x, y, z)
	}

	pub fn chunk_biomes(&self, cx: i32, cz: i32, last: &mut Option<usize>) -> Vec<u16> {
		self.climate.chunk_biomes(&self.g, &self.table, cx, cz, last)
	}

	pub fn uncached_biome(&self, c: &mut Ctx, qx: i32, qy: i32, qz: i32, last: &mut Option<usize>) -> u16 {
		let t = self.climate.target_at(&self.g, c, qx, qy, qz);
		self.table.find(&t, last)
	}

	pub fn generate(&self, cx: i32, cz: i32, biomes: &dyn Fn(i32, i32) -> std::sync::Arc<Vec<u16>>, last: &mut Option<usize>) -> Chunk {
		self.generate_cut(cx, cz, biomes, last, false).0
	}

	/// With `cut`, columns are only generated down to roughly what Distant Horizons culling keeps.
	/// Also returns, per column (index z * 16 + x), the lowest y whose blocks are exact (MIN_Y when not cut).
	pub fn generate_cut(&self, cx: i32, cz: i32, biomes: &dyn Fn(i32, i32) -> std::sync::Arc<Vec<u16>>, last: &mut Option<usize>, cut: bool) -> (Chunk, [i32; 256]) {
		let mut chunk = Chunk::new(cx, cz);
		let v = Volume::new(16, HEIGHT, 16, cx * 16, MIN_Y, cz * 16);
		let mut c = Ctx::new(self.g.cache_count, true);
		let mut aq = Aquifer::new(self, &mut c, &v);
		let mut dens = vec![0f32; v.size()];
		let split = if cut {
			let s = (0..16).map(|q| aq.surface_level(self, &mut c, cx * 16 + (q & 3) * 4, cz * 16 + (q >> 2) * 4)).min().unwrap();
			((s - CUT_SPLIT).div_euclid(8) * 8).clamp(MIN_Y, MIN_Y + HEIGHT)
		} else {
			MIN_Y
		};
		self.density_range(&mut c, &mut dens, &v, split, MIN_Y + HEIGHT);
		let mask = self.carve_mask(cx, cz, last);
		let mut lo = if cut { self.cut_depths(&dens, &v, mask.as_ref()) } else { [MIN_Y; 256] };
		if *lo.iter().min().unwrap() < split {
			self.density_range(&mut c, &mut dens, &v, MIN_Y, split);
			lo = self.cut_depths(&dens, &v, mask.as_ref());
		}
		for z in 0..16 {
			for x in 0..16 {
				let d0 = v.idx(x, 0, z);
				let dense_top = dens[d0..d0 + HEIGHT as usize].iter().rposition(|&d| d > 0.0).map_or(0, |y| y as i32);
				// above this, compute() can only answer air
				let top = dense_top.max(aq.skip_above - MIN_Y).max(SEA - 1 - MIN_Y).min(HEIGHT - 1);
				for y in (lo[(z << 4 | x) as usize] - MIN_Y..=top).rev() {
					let by = MIN_Y + y;
					let s = aq.compute(self, &mut c, cx * 16 + x, by, cz * 16 + z, dens[v.idx(x, y, z)] as f64).unwrap_or(STONE);
					if s != AIR {
						chunk.set(x, by, z, s);
					}
				}
			}
		}
		drop(dens);
		let grids: Vec<std::sync::Arc<Vec<u16>>> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (dx, dz))).map(|(dx, dz)| biomes(cx + dx, cz + dz)).collect();
		let qv = Volume::stepped(4, 96, 4, 0, 0, 0, 4, 4, 4);
		let chunk_biome = |qx: i32, qy: i32, qz: i32| {
			let (dx, dz) = ((qx >> 2) - cx, (qz >> 2) - cz);
			let grid = &grids[((dz + 1) * 3 + dx + 1) as usize];
			grid[qv.idx(qx & 3, qy.clamp(-16, 79) + 16, qz & 3)]
		};
		let mut present = 0u128;
		for q in 0..36 {
			let (qx, qz) = (q % 6 - 1, q / 6 - 1);
			let i = qv.idx(qx & 3, 0, qz & 3);
			for &b in &grids[(((qz >> 2) + 1) * 3 + (qx >> 2) + 1) as usize][i..i + 96] {
				present |= 1 << b;
			}
		}
		let trusted = self.build_surface(&mut chunk, &mut c, &chunk_biome, present, &lo);
		if let Some(mask) = mask {
			self.carve(&mut chunk, &mut c, &mut aq, &mask, &lo, last);
		}
		(chunk, trusted)
	}

	/// Final density for y0..y1 (cell aligned) into the full-chunk buffer `dens` laid out by `v`.
	fn density_range(&self, c: &mut Ctx, dens: &mut [f32], v: &Volume, y0: i32, y1: i32) {
		if y0 >= y1 {
			return;
		}
		let part = Volume::new(16, y1 - y0, 16, v.mx, y0, v.mz);
		let mut b = c.take(part.size());
		self.g.vol(c, self.final_density, &mut b, &part);
		let n = part.sy as usize;
		for z in 0..16 {
			for x in 0..16 {
				let d = v.idx(x, y0 - v.my, z);
				dens[d..d + n].copy_from_slice(&b[part.idx(x, 0, z)..][..n]);
			}
		}
		c.give(b);
	}

	fn surface_depth(&self, bx: i32, bz: i32) -> i32 {
		let n = self.noise(N_SURFACE, bx as f64, 0.0, bz as f64) as f64;
		(n * 2.75 + 3.0 + self.noise_random.at(bx, 0, bz).next_double() * 0.25) as i32
	}

	/// Blocks this far above a column's cut are exact: ceiling stone depth looks this far down, carving one more.
	fn cut_margin(&self, surface_depth: i32) -> i32 {
		2 + self.ceiling_offset + surface_depth.max(0)
	}

	/// Per column, where to stop generating. Mirrors the culling in dh.rs on a solid-only-if-certain guess
	/// (dense and not carved), with neighbours in other chunks guessed 32 lower. The caller checks the result.
	fn cut_depths(&self, dens: &[f32], v: &Volume, mask: Option<&Mask>) -> [i32; 256] {
		let solid = |x: i32, y: i32, z: i32| dens[v.idx(x, y - MIN_Y, z)] > 0.0 && !mask.is_some_and(|m| m.has(x, y, z));
		let ground: Vec<i32> = (0..256).map(|i| (MIN_Y..MIN_Y + HEIGHT).rev().find(|&y| solid(i & 15, y, i >> 4)).map_or(MIN_Y, |y| y + 1)).collect();
		std::array::from_fn(|i| {
			let (x, z) = (i as i32 & 15, i as i32 >> 4);
			let g = ground[i];
			let n = |dx: i32, dz: i32| {
				let (nx, nz) = (x + dx, z + dz);
				if (0..16).contains(&nx) && (0..16).contains(&nz) { ground[(nz << 4 | nx) as usize] } else { g - 32 }
			};
			let floor = g.min(n(1, 0)).min(n(-1, 0)).min(n(0, 1)).min(n(0, -1));
			let margin = self.cut_margin(self.surface_depth(v.mx + x, v.mz + z));
			(MIN_Y..floor).rev().find(|&y| solid(x, y, z)).map_or(MIN_Y, |y| (y - CUT_SLACK - margin).max(MIN_Y))
		})
	}

	fn rule_ctx(&self, c: &mut Ctx, vol: Volume) -> RuleCtx {
		let ore_bufs = self
			.ores
			.iter()
			.map(|&(d, r)| {
				let (lo, hi) = self.g.y_band(d).map_or((vol.my, vol.max_y() + 1), |(lo, hi)| (lo.div_euclid(16) * 16, (hi + 15).div_euclid(16) * 16));
				let (lo, hi) = (lo.max(vol.my), hi.min(vol.max_y() + 1));
				(lo < hi).then(|| {
					let ov = Volume::new(vol.sx, hi - lo, vol.sz, vol.mx, lo, vol.mz);
					let mut db = vec![0f32; ov.size()];
					self.g.vol(c, d, &mut db, &ov);
					let mut rb = vec![0f32; ov.size()];
					self.g.vol(c, r, &mut rb, &ov);
					(ov, db, rb)
				})
			})
			.collect();
		RuleCtx {
			ore_bufs,
			prelim_vol: Volume::new(vol.sx, 1, vol.sz, vol.mx, 0, vol.mz),
			prelim: None,
			bx: 0,
			bz: 0,
			grad_x: 0,
			grad_z: 0,
			surface_depth: 0,
			secondary: None,
			min_surface: None,
			by: 0,
			water: 0,
			stone_below: 0,
			stone_above: 0,
			biome: None,
			present: u128::MAX,
			noise2d: vec![f64::NAN; self.g.noises.len()],
			noise3d: vec![f64::NAN; self.g.noises.len()],
			noise3d_dirty: false,
		}
	}

	fn update_xz(&self, rc: &mut RuleCtx, bx: i32, bz: i32, gx: i32, gz: i32) {
		rc.bx = bx;
		rc.bz = bz;
		rc.grad_x = gx;
		rc.grad_z = gz;
		rc.surface_depth = self.surface_depth(bx, bz);
		rc.secondary = None;
		rc.min_surface = None;
		rc.noise2d.fill(f64::NAN);
		rc.noise3d.fill(f64::NAN);
		rc.noise3d_dirty = false;
		rc.biome = None;
	}

	fn update_y(rc: &mut RuleCtx, above: i32, below: i32, water: i32, by: i32) {
		rc.biome = None;
		rc.by = by;
		rc.water = water;
		rc.stone_below = below;
		rc.stone_above = above;
		if rc.noise3d_dirty {
			rc.noise3d.fill(f64::NAN);
			rc.noise3d_dirty = false;
		}
	}

	fn min_surface(&self, rc: &mut RuleCtx, c: &mut Ctx) -> i32 {
		if let Some(m) = rc.min_surface {
			return m;
		}
		let p = match rc.prelim_vol.index_of(rc.bx, 0, rc.bz) {
			Some(i) => {
				if rc.prelim.is_none() {
					let mut b = vec![0f32; rc.prelim_vol.size()];
					self.g.vol(c, self.chunk_surface, &mut b, &rc.prelim_vol);
					rc.prelim = Some(b);
				}
				rc.prelim.as_ref().unwrap()[i]
			}
			None => self.g.val(c, self.chunk_surface, rc.bx, 0, rc.bz),
		};
		let m = floor(p as f64) + rc.surface_depth - 8;
		rc.min_surface = Some(m);
		m
	}

	fn test(&self, cond: &Cond, rc: &mut RuleCtx, c: &mut Ctx, biome: &dyn Fn(i32, i32, i32) -> u16) -> bool {
		match cond {
			Cond::Biome(set) => {
				if rc.present & set == 0 {
					return false;
				}
				let b = *rc.biome.get_or_insert_with(|| biome(rc.bx, rc.by, rc.bz));
				set >> b & 1 == 1
			}
			Cond::Noise { n, min, max, is3d } => {
				let cache = if *is3d { &mut rc.noise3d } else { &mut rc.noise2d };
				if cache[*n].is_nan() {
					let y = if *is3d { rc.by as f64 } else { 0.0 };
					rc.noise3d_dirty |= *is3d;
					cache[*n] = self.g.noises[*n].get(rc.bx as f64, y, rc.bz as f64) as f64;
				}
				let v = cache[*n];
				v >= *min && v <= *max
			}
			Cond::Gradient { rf, true_at, false_at } => {
				if rc.by <= *true_at {
					true
				} else if rc.by >= *false_at {
					false
				} else {
					let p = map(rc.by as f64, *true_at as f64, *false_at as f64, 1.0, 0.0);
					(rf.at(rc.bx, rc.by, rc.bz).next_float() as f64) < p
				}
			}
			Cond::YAbove { anchor, mult, add } => rc.by + if *add { rc.stone_above } else { 0 } >= anchor + rc.surface_depth * mult,
			Cond::Water { offset, mult, add } => {
				rc.water == i32::MIN || rc.by + if *add { rc.stone_above } else { 0 } >= rc.water + offset + rc.surface_depth * mult
			}
			Cond::Steep => rc.grad_x <= -4 || rc.grad_z >= 4,
			Cond::Hole => rc.surface_depth <= 0,
			Cond::AbovePreliminary => rc.by >= self.min_surface(rc, c),
			Cond::StoneDepth { offset, add_surface, secondary, ceiling } => {
				let depth = if *ceiling { rc.stone_below } else { rc.stone_above };
				let sd = if *add_surface { rc.surface_depth } else { 0 };
				let sec = if *secondary == 0 {
					0
				} else {
					let s = *rc.secondary.get_or_insert_with(|| self.noise(N_SECONDARY, rc.bx as f64, 0.0, rc.bz as f64) as f64);
					map(s, -1.0, 1.0, 0.0, *secondary as f64) as i32
				};
				depth <= 1 + offset + sd + sec
			}
			Cond::Not(inner) => !self.test(inner, rc, c, biome),
		}
	}

	fn apply(&self, rule: &Rule, rc: &mut RuleCtx, c: &mut Ctx, biome: &dyn Fn(i32, i32, i32) -> u16) -> Option<B> {
		match rule {
			Rule::Block(b) => Some(*b),
			Rule::Seq(items) => {
				for r in items {
					if let Some(b) = self.apply(r, rc, c, biome) {
						return Some(b);
					}
				}
				None
			}
			Rule::Cond(cond, then) => {
				if self.test(cond, rc, c, biome) {
					self.apply(then, rc, c, biome)
				} else {
					None
				}
			}
			Rule::Bandlands => Some(self.band(rc.bx, rc.by, rc.bz)),
			Rule::Ore { k, ore, raw, filler, chance, gap } => {
				let (d, r) = self.ores[*k];
				let get = |rc: &RuleCtx, c: &mut Ctx, id: Id, which: usize| match &rc.ore_bufs[*k] {
					Some((ov, db, rb)) if let Some(i) = ov.index_of(rc.bx, rc.by, rc.bz) => {
						if which == 0 { db[i] } else { rb[i] }
					}
					_ => self.g.val(c, id, rc.bx, rc.by, rc.bz),
				};
				let density = get(rc, c, d, 0);
				if density <= 0.0 {
					return None;
				}
				let mut rnd = self.ore_random.at(rc.bx, rc.by, rc.bz);
				if rnd.next_float() > density {
					return None;
				}
				let richness = get(rc, c, r, 1);
				if rnd.next_float() < richness && self.g.val(c, *gap, rc.bx, rc.by, rc.bz) < 0.0 {
					Some(if rnd.next_float() < *chance { *raw } else { *ore })
				} else {
					Some(*filler)
				}
			}
		}
	}

	/// Applies the surface rule to the stone blocks y0..=y1 of one run. Where the rule gives the same answer for a
	/// whole range (deep stone mostly does), that range takes one evaluation instead of one per block.
	#[allow(clippy::too_many_arguments)]
	fn fill(&self, chunk: &mut Chunk, rc: &mut RuleCtx, c: &mut Ctx, biome: &dyn Fn(i32, i32, i32) -> u16, x: i32, z: i32, s: Span) {
		if s.y1 - s.y0 >= 4 {
			if let Some(r) = self.apply_span(&self.rule, rc, c, &s) {
				if let Some(b) = r {
					for y in s.y0..=s.y1 {
						chunk.set(x, y, z, b);
					}
				}
				return;
			}
			let mid = (s.y0 + s.y1).div_euclid(2);
			self.fill(chunk, rc, c, biome, x, z, Span { y0: mid + 1, ..s });
			self.fill(chunk, rc, c, biome, x, z, Span { y1: mid, ..s });
			return;
		}
		for y in (s.y0..=s.y1).rev() {
			Self::update_y(rc, s.above0 + s.top - y + 1, y - s.bot + 1, s.water, y);
			if let Some(b) = self.apply(&self.rule, rc, c, biome) {
				chunk.set(x, y, z, b);
			}
		}
	}

	/// `apply` for every block of a span at once: Some(answer) if it's the same for all of them, None if it varies.
	fn apply_span(&self, rule: &Rule, rc: &mut RuleCtx, c: &mut Ctx, s: &Span) -> Option<Option<B>> {
		match rule {
			Rule::Block(b) => Some(Some(*b)),
			Rule::Seq(items) => {
				for r in items {
					if let Some(b) = self.apply_span(r, rc, c, s)? {
						return Some(Some(b));
					}
				}
				Some(None)
			}
			Rule::Cond(cond, then) => match self.test_span(cond, rc, c, s) {
				Some(true) => self.apply_span(then, rc, c, s),
				Some(false) => Some(None),
				// unknown condition: still fine if the branch can't place anything here
				None => self.apply_span(then, rc, c, s)?.is_none().then_some(None),
			},
			Rule::Bandlands => None,
			Rule::Ore { k, .. } => {
				// places nothing unless the vein density is above 0; outside the buffer's y band it's a constant <= 0
				let Some((ov, db, _)) = &rc.ore_bufs[*k] else { return Some(None) };
				let (a, b) = (s.y0.max(ov.my), s.y1.min(ov.max_y()));
				if a > b {
					return Some(None);
				}
				let (i, j) = (ov.index_of(rc.bx, a, rc.bz)?, ov.index_of(rc.bx, b, rc.bz)?);
				db[i..=j].iter().all(|&d| d <= 0.0).then_some(None)
			}
		}
	}

	/// `test` for every block of a span at once: Some(result) if it's the same for all of them.
	fn test_span(&self, cond: &Cond, rc: &mut RuleCtx, c: &mut Ctx, s: &Span) -> Option<bool> {
		// value >= thr for every value in lo..=hi?
		let at_least = |lo: i32, hi: i32, thr: i32| if lo >= thr { Some(true) } else if hi < thr { Some(false) } else { None };
		match cond {
			Cond::Biome(set) => (rc.present & set == 0).then_some(false),
			Cond::Noise { n, min, max, is3d: false } => {
				if rc.noise2d[*n].is_nan() {
					rc.noise2d[*n] = self.g.noises[*n].get(rc.bx as f64, 0.0, rc.bz as f64) as f64;
				}
				let v = rc.noise2d[*n];
				Some(v >= *min && v <= *max)
			}
			Cond::Noise { is3d: true, .. } => None,
			Cond::Gradient { true_at, false_at, .. } => {
				if s.y1 <= *true_at {
					Some(true)
				} else if s.y0 >= *false_at {
					Some(false)
				} else {
					None
				}
			}
			// y + stone above is the same for every block of a run
			Cond::YAbove { anchor, mult, add } => {
				let thr = anchor + rc.surface_depth * mult;
				if *add { Some(s.above0 + s.top + 1 >= thr) } else { at_least(s.y0, s.y1, thr) }
			}
			Cond::Water { offset, mult, add } => {
				if s.water == i32::MIN {
					return Some(true);
				}
				let thr = s.water + offset + rc.surface_depth * mult;
				if *add { Some(s.above0 + s.top + 1 >= thr) } else { at_least(s.y0, s.y1, thr) }
			}
			Cond::Steep => Some(rc.grad_x <= -4 || rc.grad_z >= 4),
			Cond::Hole => Some(rc.surface_depth <= 0),
			Cond::AbovePreliminary => {
				let m = self.min_surface(rc, c);
				at_least(s.y0, s.y1, m)
			}
			Cond::StoneDepth { offset, add_surface, secondary, ceiling } => {
				// depth <= thr; stone above shrinks going up the span, stone below grows
				let (lo, hi) = if *ceiling { (s.y0 - s.bot + 1, s.y1 - s.bot + 1) } else { (s.above0 + s.top - s.y1 + 1, s.above0 + s.top - s.y0 + 1) };
				let sd = if *add_surface { rc.surface_depth } else { 0 };
				let sec = if *secondary == 0 {
					0
				} else {
					let v = *rc.secondary.get_or_insert_with(|| self.noise(N_SECONDARY, rc.bx as f64, 0.0, rc.bz as f64) as f64);
					map(v, -1.0, 1.0, 0.0, *secondary as f64) as i32
				};
				let thr = 1 + offset + sd + sec;
				if hi <= thr {
					Some(true)
				} else if lo > thr {
					Some(false)
				} else {
					None
				}
			}
			Cond::Not(inner) => self.test_span(inner, rc, c, s).map(|b| !b),
		}
	}

	fn band(&self, x: i32, y: i32, z: i32) -> B {
		let v = self.noise(N_CLAY, x as f64, 0.0, z as f64) * 4.0;
		let r = if v - v.floor() == 0.5 { v.ceil() } else { v.round() };
		self.bands[((y + r as i32 + 192) % 192) as usize]
	}

	/// `present` has a bit for every biome within one quart of the chunk.
	fn build_surface(&self, chunk: &mut Chunk, c: &mut Ctx, biome: &dyn Fn(i32, i32, i32) -> u16, present: u128, lo: &[i32; 256]) -> [i32; 256] {
		let (mx, mz) = (chunk.cx * 16, chunk.cz * 16);
		let top = (0..HEIGHT).rev().find(|&y| (0..256).any(|col| chunk.blocks[col * HEIGHT as usize + y as usize] != AIR));
		let max_block_y = match top {
			Some(y) => ((y >> 4) << 4) + MIN_Y + 15,
			None => MIN_Y - 1,
		};
		let y0 = *lo.iter().min().unwrap();
		let vol = Volume::new(16, (max_block_y - y0 + 1).max(1), 16, mx, y0, mz);
		let mut rc = self.rule_ctx(c, vol);
		rc.present = present;
		let zb = |x: i32, y: i32, z: i32| zoom(self.zoom_seed, x, y, z, biome);
		let mut trusted = *lo;
		for x in 0..16 {
			for z in 0..16 {
				let (bx, bz) = (mx + x, mz + z);
				let col = (z << 4 | x) as usize;
				let start = chunk.surface(x, z) + 1;
				let surface_biome = zb(bx, start, bz);
				let ok = surface_biome != self.eroded_badlands || self.eroded_badlands_ext(chunk, x, z, start, lo[col]);
				let height = chunk.surface(x, z) + 1;
				self.update_xz(&mut rc, bx, bz, chunk.grad_x(x, z), chunk.grad_z(x, z));
				let mut above = 0;
				let mut water = i32::MIN;
				let mut next_ceiling = i32::MAX;
				let mut y = height;
				while y >= lo[col] {
					let old = chunk.get(x, y, z);
					if old == AIR {
						above = 0;
						water = i32::MIN;
					} else if is_fluid(old) {
						if water == i32::MIN {
							water = y + 1;
						}
					} else {
						if next_ceiling >= y {
							next_ceiling = WAY_BELOW_MIN_Y;
							let mut ly = y - 1;
							while ly >= MIN_Y - 1 {
								let s = chunk.get(x, ly, z);
								if s == AIR || is_fluid(s) {
									next_ceiling = ly + 1;
									break;
								}
								ly -= 1;
							}
						}
						// the whole stone run down to the next air or fluid at once
						let y0 = next_ceiling.max(lo[col]);
						let run = Span { y0, y1: y.min(MIN_Y + HEIGHT - 1), top: y, bot: next_ceiling, above0: above, water };
						if run.y0 <= run.y1 {
							self.fill(chunk, &mut rc, c, &zb, x, z, run);
						}
						above += y - y0 + 1;
						y = y0;
					}
					y -= 1;
				}
				if let Some(base) = self.frozen_base[surface_biome as usize] {
					let m = self.min_surface(&mut rc, c);
					self.frozen_ocean_ext(chunk, m, base, x, z, start);
				}
				if lo[col] > MIN_Y {
					trusted[col] = if ok { lo[col] + self.cut_margin(rc.surface_depth) } else { i32::MAX };
				}
			}
		}
		trusted
	}

	/// False when it would need blocks below the cut `lo`.
	fn eroded_badlands_ext(&self, chunk: &mut Chunk, x: i32, z: i32, height: i32, lo: i32) -> bool {
		let (bx, bz) = ((chunk.cx * 16 + x) as f64, (chunk.cz * 16 + z) as f64);
		let a = (self.noise(N_BADLANDS, bx, 0.0, bz) as f64 * 8.25).abs();
		let b = (self.noise(N_PILLAR, bx * 0.2, 0.0, bz * 0.2) * 15.0) as f64;
		let buffer = a.min(b);
		if buffer <= 0.0 {
			return true;
		}
		let floor_n = (self.noise(N_PILLAR_ROOF, bx * 0.75, 0.0, bz * 0.75) as f64 * 1.5).abs();
		let top = 64.0 + (buffer * buffer * 2.5).min((floor_n * 50.0).ceil() + 24.0);
		let start = floor(top);
		if height > start {
			return true;
		}
		let mut y = start;
		while y >= lo {
			let s = chunk.get(x, y, z);
			if s == STONE {
				break;
			}
			if s == WATER {
				return true;
			}
			y -= 1;
		}
		if y < lo && lo > MIN_Y {
			return false;
		}
		let mut y = start;
		while y >= MIN_Y && chunk.get(x, y, z) == AIR {
			chunk.set(x, y, z, STONE);
			y -= 1;
		}
		true
	}

	fn frozen_ocean_ext(&self, chunk: &mut Chunk, min_surface: i32, base_temp: f32, x: i32, z: i32, height: i32) {
		let (ix, iz) = (chunk.cx * 16 + x, chunk.cz * 16 + z);
		let (bx, bz) = (ix as f64, iz as f64);
		let a = (self.noise(N_ICE_SURFACE, bx, 0.0, bz) as f64 * 8.25).abs();
		let b = (self.noise(N_ICE_PILLAR, bx * 1.28, 0.0, bz * 1.28) * 15.0) as f64;
		let iceberg = a.min(b);
		if iceberg <= 1.8 {
			return;
		}
		let roof = (self.noise(N_ICE_ROOF, bx * 1.17, 0.0, bz * 1.17) as f64 * 1.5).abs();
		let mut top = (iceberg * iceberg * 1.2).min((roof * 40.0).ceil() + 14.0);
		if self.frozen_temperature(ix, iz, base_temp) > 0.1 {
			top -= 2.0;
		}
		if top <= 2.0 {
			return;
		}
		let bottom = SEA as f64 - top - 7.0;
		top += SEA as f64;
		let mut r = self.noise_random.at(ix, 0, iz);
		let max_snow = 2 + r.next_bound(4);
		let min_snow_height = SEA + 18 + r.next_bound(10);
		let mut snow = 0;
		let mut y = height.max(top as i32 + 1);
		while y >= min_surface {
			let s = chunk.get(x, y, z);
			if (s == AIR && y < top as i32 && r.next_double() > 0.01) || (s == WATER && y > bottom as i32 && y < SEA && r.next_double() > 0.15) {
				if snow <= max_snow && y > min_snow_height {
					chunk.set(x, y, z, self.snow_block);
					snow += 1;
				} else {
					chunk.set(x, y, z, self.packed_ice);
				}
			}
			y -= 1;
		}
	}

	pub fn temperature(&self, b: u16, x: i32, y: i32, z: i32) -> f32 {
		let base = self.climate_info[b as usize].0;
		let t = self.frozen_base[b as usize].map_or(base, |f| self.frozen_temperature(x, z, f));
		if y <= SEA + 17 {
			return t;
		}
		let v = self.temp_noise.get2((x as f32 / 8.0) as f64, (z as f32 / 8.0) as f64) * 8.0;
		t - (v + y as f32 - (SEA + 17) as f32) * 0.05 / 40.0
	}

	/// Places trees. If one reads below a column's exact range (`trusted`, see generate_cut), all placements
	/// are undone and the chunks read are returned, to be regenerated in full.
	#[allow(clippy::too_many_arguments)]
	pub fn decorate(&self, chunks: &mut [Chunk], ox: i32, oz: i32, n: i32, biomes: &dyn Fn(i32, i32) -> std::sync::Arc<Vec<u16>>, trusted: &[[i32; 256]]) -> Vec<usize> {
		struct Area<'a> {
			chunks: &'a mut [Chunk],
			ox: i32,
			oz: i32,
			n: i32,
			trusted: &'a [[i32; 256]],
			low: std::cell::RefCell<Vec<usize>>,
			log: Vec<(usize, usize, B)>,
		}
		impl Area<'_> {
			fn at(&self, x: i32, z: i32) -> Option<usize> {
				let (cx, cz) = ((x >> 4) - self.ox, (z >> 4) - self.oz);
				((0..self.n).contains(&cx) && (0..self.n).contains(&cz)).then_some((cz * self.n + cx) as usize)
			}
		}
		impl crate::trees::Level for Area<'_> {
			fn get(&self, x: i32, y: i32, z: i32) -> B {
				self.at(x, z).map_or(AIR, |i| {
					if y < self.trusted[i][((z & 15) << 4 | (x & 15)) as usize] {
						self.low.borrow_mut().push(i);
					}
					self.chunks[i].get(x & 15, y, z & 15)
				})
			}
			fn set(&mut self, x: i32, y: i32, z: i32, b: B) {
				if let Some(i) = self.at(x, z) {
					if (MIN_Y..MIN_Y + HEIGHT).contains(&y) {
						let k = Chunk::idx(x & 15, y, z & 15);
						self.log.push((i, k, self.chunks[i].blocks[k]));
					}
					self.chunks[i].set(x & 15, y, z & 15, b);
				}
			}
			fn top(&self, x: i32, z: i32) -> i32 {
				self.at(x, z).map_or(MIN_Y, |i| self.chunks[i].height[(((z & 15) << 4) | (x & 15)) as usize])
			}
		}
		let qv = Volume::stepped(4, 96, 4, 0, 0, 0, 4, 4, 4);
		let quart = |qx: i32, qy: i32, qz: i32| biomes(qx >> 2, qz >> 2)[qv.idx(qx & 3, qy.clamp(-16, 79) + 16, qz & 3)];
		let biome_at = |x: i32, y: i32, z: i32| zoom(self.zoom_seed, x, y, z, quart);
		let heights: Vec<[i32; 256]> = chunks.iter().map(|c| c.height).collect();
		// ponytail: trees only see their own 4x4-chunk area, so canopies are clipped at area borders
		let mut area = Area { chunks, ox, oz, n, trusted, low: Default::default(), log: vec![] };
		for cz in oz..oz + n {
			for cx in ox..ox + n {
				self.trees.decorate(&mut area, cx, cz, self.seed, &biomes(cx, cz), &biome_at, &self.biome_info);
			}
		}
		let Area { chunks, low, log, .. } = area;
		let mut low = low.into_inner();
		if !low.is_empty() {
			for (i, k, b) in log.into_iter().rev() {
				chunks[i].blocks[k] = b;
			}
			for (c, h) in chunks.iter_mut().zip(heights) {
				c.height = h;
			}
		}
		low.sort_unstable();
		low.dedup();
		low
	}

	pub fn freeze(&self, chunk: &mut Chunk, biomes: &dyn Fn(i32, i32) -> std::sync::Arc<Vec<u16>>) {
		let (cx, cz) = (chunk.cx, chunk.cz);
		let grids: Vec<std::sync::Arc<Vec<u16>>> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (dx, dz))).map(|(dx, dz)| biomes(cx + dx, cz + dz)).collect();
		let qv = Volume::stepped(4, 96, 4, 0, 0, 0, 4, 4, 4);
		let quart = |qx: i32, qy: i32, qz: i32| grids[((((qz >> 2) - cz) + 1) * 3 + ((qx >> 2) - cx) + 1) as usize][qv.idx(qx & 3, qy.clamp(-16, 79) + 16, qz & 3)];
		let [ice, snow, powder] = self.freeze_blocks;
		for x in 0..16 {
			for z in 0..16 {
				let (wx, wz) = (cx * 16 + x, cz * 16 + z);
				let y = (MIN_Y..MIN_Y + HEIGHT).rev().find(|&y| chunk.get(x, y, z) != AIR).map_or(MIN_Y, |y| y + 1);
				let b = zoom(self.zoom_seed, wx, y, wz, quart);
				if y > MIN_Y && chunk.get(x, y - 1, z) == WATER && self.temperature(b, wx, y - 1, wz) < 0.15 {
					chunk.set(x, y - 1, z, ice);
				}
				let below = chunk.get(x, y - 1, z);
				let survives = ![AIR, WATER, LAVA, ice, self.packed_ice, powder].contains(&below);
				if y < MIN_Y + HEIGHT && y > MIN_Y && self.climate_info[b as usize].1 && survives && self.temperature(b, wx, y, wz) < 0.15 {
					chunk.set(x, y, z, snow);
					if let Some(&(_, s)) = self.snowy.iter().find(|(g, _)| *g == below) {
						chunk.set(x, y - 1, z, s);
					}
				}
			}
		}
	}

	fn frozen_temperature(&self, x: i32, z: i32, base: f32) -> f32 {
		let mut large = 0f32;
		for (n, f, a) in &self.frozen_noise {
			large += a * n.get2(x as f64 * 0.05 * f, z as f64 * 0.05 * f);
		}
		let large = (large * 7.0) as f64;
		let edge = self.biome_info.get2(x as f64 * 0.2, z as f64 * 0.2) as f64;
		if large + edge < 0.3 && (self.biome_info.get2(x as f64 * 0.09, z as f64 * 0.09) as f64) < 0.8 {
			return 0.2;
		}
		base
	}

	fn carve_mask(&self, cx: i32, cz: i32, last: &mut Option<usize>) -> Option<Mask> {
		let mut uc = Ctx::new(self.g.cache_count, false);
		let mut mask = Mask { bits: vec![false; (256 * MASK_H) as usize] };
		let mut r = Legacy::new(0);
		let mut any = false;
		for dx in -8..=8 {
			for dz in -8..=8 {
				let (sx, sz) = (cx + dx, cz + dz);
				let b = if self.carvers.len() == 1 { 0 } else { self.uncached_biome(&mut uc, sx * 4, 0, sz * 4, last) };
				for (index, carver) in self.carvers[b as usize].iter().enumerate() {
					r.set_large_feature_seed(self.seed.wrapping_add(index as i64), sx, sz);
					if r.next_float() <= carver.prob() {
						any = true;
						CarveCtx { cx, cz, mask: &mut mask }.carve(carver, &mut r, sx, sz);
					}
				}
			}
		}
		(any && mask.bits.contains(&true)).then_some(mask)
	}

	#[allow(clippy::too_many_arguments)]
	fn carve(&self, chunk: &mut Chunk, c: &mut Ctx, aq: &mut Aquifer, mask: &Mask, lo: &[i32; 256], last: &mut Option<usize>) {
		let (mx, mz) = (chunk.cx * 16, chunk.cz * 16);
		let mut i = 0usize;
		let n = mask.bits.len();
		let last_cell = std::cell::RefCell::new(*last);
		while i < n {
			if !mask.bits[i] {
				i += 1;
				continue;
			}
			let mut j = i;
			while j + 1 < n && mask.bits[j + 1] {
				j += 1;
			}
			let (sc, ec) = (i / MASK_H as usize, j / MASK_H as usize);
			for col in sc..=ec {
				let (x, z) = ((col >> 4 & 15) as i32, (col & 15) as i32);
				let base = col * MASK_H as usize;
				let bottom = (i.saturating_sub(base) as i32 + MASK_MIN).max(lo[(z << 4 | x) as usize]);
				let top = (j - base).min(MASK_H as usize - 1) as i32 + MASK_MIN;
				let mut has_grass = false;
				let mut y = top;
				while y >= bottom {
					let s = chunk.get(x, y, z);
					if s != BEDROCK {
						if s == self.grass || s == self.mycelium {
							has_grass = true;
						}
						if let Some(state) = aq.compute(self, c, mx + x, y, mz + z, 0.0) {
							chunk.set(x, y, z, state);
							if has_grass && chunk.get(x, y - 1, z) == self.dirt {
								let biome = |qx: i32, qy: i32, qz: i32| {
									let mut l = last_cell.borrow_mut();
									self.uncached_biome(&mut Ctx::new(0, false), qx, qy, qz, &mut l)
								};
								if let Some(top_mat) = self.top_material(chunk, c, mx + x, y - 1, mz + z, is_fluid(state), &biome) {
									chunk.set(x, y - 1, z, top_mat);
								}
							}
						}
					}
					y -= 1;
				}
			}
			i = j + 1;
		}
		*last = last_cell.into_inner();
	}

	#[allow(clippy::too_many_arguments)]
	fn top_material(&self, chunk: &Chunk, c: &mut Ctx, bx: i32, by: i32, bz: i32, under_fluid: bool, biome: &dyn Fn(i32, i32, i32) -> u16) -> Option<B> {
		let mut rc = self.rule_ctx(c, Volume::new(1, 1, 1, bx, by, bz));
		let (lx, lz) = (bx & 15, bz & 15);
		self.update_xz(&mut rc, bx, bz, chunk.grad_x(lx, lz), chunk.grad_z(lx, lz));
		Self::update_y(&mut rc, 1, 1, if under_fluid { by + 1 } else { i32::MIN }, by);
		let zb = |x: i32, y: i32, z: i32| zoom(self.zoom_seed, x, y, z, biome);
		self.apply(&self.rule, &mut rc, c, &zb)
	}
}

