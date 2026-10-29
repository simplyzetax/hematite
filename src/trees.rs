use crate::noise::Simplex;
use crate::random::{Rng, Xoroshiro};
use crate::registry::Registry;
use crate::terrain::{Palette, AIR, B, HEIGHT, LAVA, MIN_Y, WATER};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const VEGETAL_STEP: usize = 9;

pub trait Level {
	fn get(&self, x: i32, y: i32, z: i32) -> B;
	fn set(&mut self, x: i32, y: i32, z: i32, b: B);
	fn top(&self, x: i32, z: i32) -> i32;
}

#[derive(Clone)]
enum IntP {
	Const(i32),
	Uniform(i32, i32),
	Weighted(Vec<(i32, i32)>, i32),
	Clamped(Box<IntP>, i32, i32),
}

impl IntP {
	fn parse(v: &Value) -> Self {
		if let Some(n) = v.as_i64() {
			return IntP::Const(n as i32);
		}
		let i = |k: &str| v[k].as_i64().unwrap() as i32;
		match v.get("type").and_then(Value::as_str).unwrap_or("") {
			"minecraft:constant" => IntP::Const(i("value")),
			"minecraft:weighted_list" => {
				let d: Vec<(i32, i32)> = v["distribution"].as_array().unwrap().iter().map(|e| (e["weight"].as_i64().unwrap() as i32, e["data"].as_i64().unwrap() as i32)).collect();
				let total = d.iter().map(|e| e.0).sum();
				IntP::Weighted(d, total)
			}
			"minecraft:clamped" => IntP::Clamped(Box::new(IntP::parse(&v["source"])), i("min_inclusive"), i("max_inclusive")),
			_ => IntP::Uniform(i("min_inclusive"), i("max_inclusive")),
		}
	}

	fn sample(&self, r: &mut Xoroshiro) -> i32 {
		match self {
			IntP::Const(n) => *n,
			IntP::Uniform(a, b) => r.next_between(*a, *b),
			IntP::Weighted(d, total) => {
				let mut k = r.next_bound(*total);
				for (w, v) in d {
					k -= w;
					if k < 0 {
						return *v;
					}
				}
				d[0].1
			}
			IntP::Clamped(s, a, b) => s.sample(r).clamp(*a, *b),
		}
	}
}

#[derive(Clone, Copy)]
enum Hm {
	OceanFloor,
	WorldSurface,
	MotionBlocking,
}

enum Pred {
	Survive(Vec<B>),
	Matching(Vec<B>, [i32; 3]),
	Not(Box<Pred>),
	All(Vec<Pred>),
	Any(Vec<Pred>),
	True,
}

impl Pred {
	fn test(&self, l: &dyn Level, x: i32, y: i32, z: i32) -> bool {
		match self {
			Pred::Survive(soil) => soil.contains(&l.get(x, y - 1, z)),
			Pred::Matching(bs, o) => bs.contains(&l.get(x + o[0], y + o[1], z + o[2])),
			Pred::Not(p) => !p.test(l, x, y, z),
			Pred::All(ps) => ps.iter().all(|p| p.test(l, x, y, z)),
			Pred::Any(ps) => ps.iter().any(|p| p.test(l, x, y, z)),
			Pred::True => true,
		}
	}
}

enum Mod {
	Count(IntP),
	InSquare,
	Rarity(i32),
	WaterDepth(i32),
	Height(Hm),
	Filter(Pred),
	Biome,
	NoiseThreshold(f64, i32, i32),
	NoiseCount(i32, f64, f64),
	Scan(i32, i32, Pred),
}

struct Placed {
	feature: usize,
	mods: Vec<Mod>,
}

enum Feat {
	Tree(Box<Tree>),
	Random(Vec<(f32, usize)>, usize),
	Simple(Vec<usize>),
	Bool(usize, usize),
	Mushroom { red: bool, cap: B, stem: B, soil: Vec<B> },
	Nothing,
}

#[derive(Clone, Copy, PartialEq)]
enum TrunkKind {
	Straight,
	Giant,
	MegaJungle,
	DarkOak,
	Fancy,
	Forking,
	Cherry,
}

#[derive(Clone, Copy, PartialEq)]
enum FoliageKind {
	Blob,
	Bush,
	Fancy,
	Jungle,
	Spruce,
	Pine,
	MegaPine,
	Acacia,
	DarkOak,
	Spread,
	Cherry,
	Poplar,
}

struct Tree {
	log: B,
	leaves: B,
	trunk: TrunkKind,
	base: i32,
	rand_a: i32,
	rand_b: i32,
	branches: IntP,
	foliage: FoliageKind,
	radius: IntP,
	offset: IntP,
	height: IntP,
	attempts: i32,
	size: (i32, i32, i32),
}

struct Attach {
	x: i32,
	y: i32,
	z: i32,
	r: i32,
	double: bool,
}

pub struct Trees {
	placed: Vec<Placed>,
	feats: Vec<Feat>,
	order: Vec<usize>,
	biome_has: Vec<HashSet<usize>>,
	replaceable: Vec<B>,
	logs: Vec<B>,
}

fn state(p: &mut Palette, v: &Value) -> B {
	let name = v["id"].as_str().or_else(|| v["Name"].as_str()).unwrap();
	let mut props: Vec<(String, String)> = v.get("properties").or_else(|| v.get("Properties")).and_then(Value::as_object).map_or(vec![], |o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect());
	if props.is_empty() {
		return p.id(name);
	}
	props.sort();
	let s: String = props.iter().map(|(k, v)| format!("{{{k}:{v}}}")).collect();
	let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
	p.id(&format!("{name}|{s}"))
}

struct Builder<'a> {
	reg: &'a Registry,
	pal: &'a mut Palette,
	placed: Vec<Placed>,
	feats: Vec<Feat>,
	placed_names: HashMap<String, usize>,
	feat_names: HashMap<String, usize>,
	dirt: Vec<B>,
}

impl Builder<'_> {
	fn blocks(&mut self, v: &Value) -> Vec<B> {
		match v {
			Value::String(s) => vec![self.pal.id(s)],
			Value::Array(a) => a.iter().map(|s| self.pal.id(s.as_str().unwrap())).collect(),
			_ => vec![],
		}
	}

	fn pred(&mut self, v: &Value) -> Pred {
		let offset = v.get("offset").and_then(Value::as_array).map_or([0; 3], |a| [0, 1, 2].map(|i| a[i].as_i64().unwrap() as i32));
		match v["type"].as_str().unwrap() {
			"minecraft:would_survive" => {
				let s = v["state"].as_str().or_else(|| v["state"]["Name"].as_str()).unwrap_or("");
				let mut soil = self.dirt.clone();
				if s.contains("propagule") {
					soil.push(self.pal.id("clay"));
				}
				Pred::Survive(soil)
			}
			"minecraft:matching_blocks" => Pred::Matching(self.blocks(&v["blocks"]), offset),
			"minecraft:not" => Pred::Not(Box::new(self.pred(&v["predicate"]))),
			"minecraft:all_of" => Pred::All(v["predicates"].as_array().unwrap().iter().map(|p| self.pred(p)).collect()),
			"minecraft:any_of" => Pred::Any(v["predicates"].as_array().unwrap().iter().map(|p| self.pred(p)).collect()),
			_ => Pred::True,
		}
	}

	fn placed(&mut self, v: &Value) -> usize {
		if let Some(name) = v.as_str() {
			if let Some(&i) = self.placed_names.get(name) {
				return i;
			}
			let i = self.placed(self.reg.get("worldgen/placed_feature", name));
			self.placed_names.insert(name.to_string(), i);
			return i;
		}
		let feature = self.feature(&v["feature"]);
		let mut mods = vec![];
		for m in v["placement"].as_array().unwrap() {
			let i = |k: &str| m[k].as_i64().unwrap() as i32;
			mods.push(match m["type"].as_str().unwrap() {
				"minecraft:count" => Mod::Count(IntP::parse(&m["count"])),
				"minecraft:in_square" => Mod::InSquare,
				"minecraft:rarity_filter" => Mod::Rarity(i("chance")),
				"minecraft:surface_water_depth_filter" => Mod::WaterDepth(i("max_water_depth")),
				"minecraft:heightmap" => Mod::Height(match m["heightmap"].as_str().unwrap() {
					"OCEAN_FLOOR" | "OCEAN_FLOOR_WG" => Hm::OceanFloor,
					"WORLD_SURFACE" | "WORLD_SURFACE_WG" => Hm::WorldSurface,
					_ => Hm::MotionBlocking,
				}),
				"minecraft:block_predicate_filter" => Mod::Filter(self.pred(&m["predicate"])),
				"minecraft:biome" => Mod::Biome,
				"minecraft:noise_threshold_count" => Mod::NoiseThreshold(m["noise_level"].as_f64().unwrap(), i("below_noise"), i("above_noise")),
				"minecraft:noise_based_count" => Mod::NoiseCount(i("noise_to_count_ratio"), m["noise_factor"].as_f64().unwrap(), m.get("noise_offset").and_then(Value::as_f64).unwrap_or(0.0)),
				"minecraft:environment_scan" => Mod::Scan(if m["direction_of_search"] == "up" { 1 } else { -1 }, i("max_steps"), self.pred(&m["target_condition"])),
				_ => continue,
			});
		}
		self.placed.push(Placed { feature, mods });
		self.placed.len() - 1
	}

	fn feature(&mut self, v: &Value) -> usize {
		if let Some(name) = v.as_str() {
			if let Some(&i) = self.feat_names.get(name) {
				return i;
			}
			let i = self.feature(self.reg.get("worldgen/feature", name));
			self.feat_names.insert(name.to_string(), i);
			return i;
		}
		let f = match v["type"].as_str().unwrap() {
			"minecraft:tree" => self.tree(v).map_or(Feat::Nothing, |t| Feat::Tree(Box::new(t))),
			"minecraft:random_selector" => {
				let opts = v["features"].as_array().unwrap().iter().map(|o| (o["chance"].as_f64().unwrap() as f32, self.placed(&o["feature"]))).collect();
				Feat::Random(opts, self.placed(&v["default"]))
			}
			"minecraft:simple_random_selector" => Feat::Simple(v["features"].as_array().unwrap().iter().map(|o| self.placed(o)).collect()),
			"minecraft:random_boolean_selector" => Feat::Bool(self.placed(&v["feature_true"]), self.placed(&v["feature_false"])),
			t @ ("minecraft:huge_red_mushroom" | "minecraft:huge_brown_mushroom") => {
				let mut soil = self.dirt.clone();
				soil.push(self.pal.id("mycelium"));
				Feat::Mushroom { red: t.contains("red"), cap: state(self.pal, &v["cap_provider"]), stem: state(self.pal, &v["stem_provider"]), soil }
			}
			_ => Feat::Nothing,
		};
		self.feats.push(f);
		self.feats.len() - 1
	}

	fn tree(&mut self, v: &Value) -> Option<Tree> {
		let tp = &v["trunk_placer"];
		let fp = &v["foliage_placer"];
		if v["trunk_provider"].get("id").is_none() || v["foliage_provider"].get("id").is_none() {
			return None;
		}
		let trunk = match tp["type"].as_str().unwrap() {
			"minecraft:giant_trunk_placer" => TrunkKind::Giant,
			"minecraft:mega_jungle_trunk_placer" => TrunkKind::MegaJungle,
			"minecraft:dark_oak_trunk_placer" => TrunkKind::DarkOak,
			"minecraft:fancy_trunk_placer" => TrunkKind::Fancy,
			"minecraft:forking_trunk_placer" => TrunkKind::Forking,
			"minecraft:cherry_trunk_placer" => TrunkKind::Cherry,
			_ => TrunkKind::Straight,
		};
		let foliage = match fp["type"].as_str().unwrap() {
			"minecraft:bush_foliage_placer" => FoliageKind::Bush,
			"minecraft:fancy_foliage_placer" => FoliageKind::Fancy,
			"minecraft:jungle_foliage_placer" => FoliageKind::Jungle,
			"minecraft:spruce_foliage_placer" => FoliageKind::Spruce,
			"minecraft:pine_foliage_placer" => FoliageKind::Pine,
			"minecraft:mega_pine_foliage_placer" => FoliageKind::MegaPine,
			"minecraft:acacia_foliage_placer" => FoliageKind::Acacia,
			"minecraft:dark_oak_foliage_placer" => FoliageKind::DarkOak,
			"minecraft:random_spread_foliage_placer" => FoliageKind::Spread,
			"minecraft:cherry_foliage_placer" => FoliageKind::Cherry,
			"minecraft:poplar_foliage_placer" => FoliageKind::Poplar,
			_ => FoliageKind::Blob,
		};
		let ip = |k: &str, d: i32| fp.get(k).map_or(IntP::Const(d), IntP::parse);
		let height = match foliage {
			FoliageKind::Spruce => ip("trunk_height", 1),
			FoliageKind::MegaPine => ip("crown_height", 13),
			FoliageKind::Spread => ip("foliage_height", 2),
			FoliageKind::DarkOak => IntP::Const(4),
			FoliageKind::Acacia => IntP::Const(0),
			_ => ip("height", 3),
		};
		let ms = &v["minimum_size"];
		let mi = |k: &str, d: i64| ms.get(k).and_then(Value::as_i64).unwrap_or(d) as i32;
		Some(Tree {
			log: state(self.pal, &v["trunk_provider"]),
			leaves: state(self.pal, &v["foliage_provider"]),
			trunk,
			base: tp["base_height"].as_i64().unwrap() as i32,
			rand_a: tp["height_rand_a"].as_i64().unwrap() as i32,
			rand_b: tp["height_rand_b"].as_i64().unwrap() as i32,
			branches: tp.get("branch_count").map_or(IntP::Const(1), IntP::parse),
			foliage,
			radius: ip("radius", 2),
			offset: ip("offset", 0),
			height,
			attempts: fp.get("leaf_placement_attempts").and_then(Value::as_i64).unwrap_or(0) as i32,
			size: (mi("limit", 1), mi("lower_size", 0), mi("upper_size", 1)),
		})
	}
}

impl Trees {
	pub fn new(reg: &Registry, pal: &mut Palette, biomes: &[String]) -> Self {
		let dirt = ["grass_block", "dirt", "coarse_dirt", "podzol", "mycelium", "rooted_dirt", "moss_block", "mud", "muddy_mangrove_roots", "pale_moss_block", "farmland"].map(|n| pal.id(n)).to_vec();
		let mut b = Builder { reg, pal, placed: vec![], feats: vec![], placed_names: HashMap::new(), feat_names: HashMap::new(), dirt };
		let mut order = vec![];
		let mut biome_has = vec![];
		for name in biomes {
			let bj = reg.get("worldgen/biome", name);
			let mut has = HashSet::new();
			for (step, list) in bj["features"].as_array().unwrap().iter().enumerate() {
				for f in list.as_array().unwrap() {
					let name = f.as_str().unwrap();
					if step != VEGETAL_STEP || !leads_to_tree(reg, &reg.get("worldgen/placed_feature", name)["feature"], 0) {
						continue;
					}
					let i = b.placed(f);
					has.insert(i);
					if !order.contains(&i) {
						order.push(i);
					}
				}
			}
			biome_has.push(has);
		}
		let replaceable = ["short_grass", "fern", "tall_grass", "large_fern", "dead_bush", "vine", "glow_lichen", "seagrass", "tall_seagrass", "snow", "water"].map(|n| b.pal.id(n)).to_vec();
		let logs = b.feats.iter().filter_map(|f| if let Feat::Tree(t) = f { Some(t.log) } else { None }).collect();
		let leaves: Vec<B> = b.feats.iter().filter_map(|f| if let Feat::Tree(t) = f { Some(t.leaves) } else { None }).collect();
		let mut replaceable = replaceable;
		replaceable.extend(leaves);
		Self { placed: b.placed, feats: b.feats, order, biome_has, replaceable, logs }
	}

	fn free(&self, b: B) -> bool {
		b == AIR || self.replaceable.contains(&b)
	}

	fn height(&self, l: &dyn Level, hm: Hm, x: i32, z: i32) -> i32 {
		let mut y = l.top(x, z);
		while y > MIN_Y {
			let b = l.get(x, y - 1, z);
			let pass = match hm {
				Hm::WorldSurface => false,
				Hm::MotionBlocking => b == self.replaceable[9],
				Hm::OceanFloor => b == WATER || b == LAVA || b == self.replaceable[9],
			};
			if !pass {
				break;
			}
			y -= 1;
		}
		y
	}

	#[allow(clippy::too_many_arguments)]
	pub fn decorate(&self, l: &mut dyn Level, cx: i32, cz: i32, seed: i64, chunk_biomes: &[u16], biome_at: &dyn Fn(i32, i32, i32) -> u16, info: &Simplex) {
		let present: HashSet<u16> = chunk_biomes.iter().copied().collect();
		for (k, &p) in self.order.iter().enumerate() {
			if !present.iter().any(|&b| self.biome_has[b as usize].contains(&p)) {
				continue;
			}
			let mut r = Xoroshiro::new(seed ^ (cx as i64).wrapping_mul(341873128712) ^ (cz as i64).wrapping_mul(132897987541) ^ (k as i64) << 48);
			let ctx = Ctx { top: p, biome_at, info };
			self.place(l, &mut r, &ctx, p, cx * 16, MIN_Y, cz * 16);
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn place(&self, l: &mut dyn Level, r: &mut Xoroshiro, ctx: &Ctx, p: usize, x: i32, y: i32, z: i32) {
		let mut pos = vec![(x, y, z)];
		for m in &self.placed[p].mods {
			let mut next = vec![];
			for (x, y, z) in pos {
				match m {
					Mod::Count(c) => (0..c.sample(r)).for_each(|_| next.push((x, y, z))),
					Mod::NoiseThreshold(level, below, above) => {
						let n = ctx.info.get2(x as f64 / 200.0, z as f64 / 200.0) as f64;
						(0..if n < *level { *below } else { *above }).for_each(|_| next.push((x, y, z)));
					}
					Mod::NoiseCount(ratio, factor, offset) => {
						let n = ctx.info.get2(x as f64 / factor, z as f64 / factor) as f64;
						(0..((n + offset) * *ratio as f64).ceil() as i32).for_each(|_| next.push((x, y, z)));
					}
					Mod::InSquare => next.push((x + r.next_bound(16), y, z + r.next_bound(16))),
					Mod::Rarity(c) => {
						if r.next_float() < 1.0 / *c as f32 {
							next.push((x, y, z));
						}
					}
					Mod::WaterDepth(d) => {
						if self.height(l, Hm::WorldSurface, x, z) - self.height(l, Hm::OceanFloor, x, z) <= *d {
							next.push((x, y, z));
						}
					}
					Mod::Height(hm) => {
						let h = self.height(l, *hm, x, z);
						if h > MIN_Y {
							next.push((x, h, z));
						}
					}
					Mod::Filter(pr) => {
						if pr.test(l, x, y, z) {
							next.push((x, y, z));
						}
					}
					Mod::Biome => {
						if self.biome_has[(ctx.biome_at)(x, y, z) as usize].contains(&ctx.top) {
							next.push((x, y, z));
						}
					}
					Mod::Scan(dir, steps, pr) => {
						if let Some(s) = (0..=*steps).find(|s| pr.test(l, x, y + s * dir, z)) {
							next.push((x, y + s * dir, z));
						}
					}
				}
			}
			pos = next;
		}
		for (x, y, z) in pos {
			match &self.feats[self.placed[p].feature] {
				Feat::Tree(t) => self.tree(l, r, t, x, y, z),
				Feat::Random(opts, def) => {
					let pick = opts.iter().find(|(c, _)| r.next_float() < *c).map_or(*def, |o| o.1);
					self.place(l, r, ctx, pick, x, y, z);
				}
				Feat::Simple(opts) => {
					let pick = opts[r.next_bound(opts.len() as i32) as usize];
					self.place(l, r, ctx, pick, x, y, z);
				}
				Feat::Bool(a, b) => {
					let pick = if r.next_bool() { *a } else { *b };
					self.place(l, r, ctx, pick, x, y, z);
				}
				Feat::Mushroom { red, cap, stem, soil } => mushroom(l, r, *red, *cap, *stem, soil, x, y, z),
				Feat::Nothing => {}
			}
		}
	}

	fn tree(&self, l: &mut dyn Level, r: &mut Xoroshiro, t: &Tree, x: i32, y: i32, z: i32) {
		let h = t.base + r.next_bound(t.rand_a + 1) + r.next_bound(t.rand_b + 1);
		let fh = match t.foliage {
			FoliageKind::Spruce => (h - t.height.sample(r)).max(4),
			_ => t.height.sample(r),
		};
		let trunk_h = h - fh;
		let mut radius = t.radius.sample(r);
		if t.foliage == FoliageKind::Pine {
			radius += r.next_bound(trunk_h.max(1));
		}
		if y < MIN_Y + 1 || y + h + 1 > MIN_Y + HEIGHT {
			return;
		}
		let double = matches!(t.trunk, TrunkKind::Giant | TrunkKind::MegaJungle | TrunkKind::DarkOak);
		let width = if double { 1 } else { 0 };
		for dy in 0..=h + 1 {
			let s = if dy < t.size.0 { t.size.1 } else { t.size.2 };
			for dx in -s..=s + width {
				for dz in -s..=s + width {
					let b = l.get(x + dx, y + dy, z + dz);
					if !self.free(b) && !self.logs.contains(&b) {
						return;
					}
				}
			}
		}
		let log = |l: &mut dyn Level, x: i32, y: i32, z: i32| {
			if self.free(l.get(x, y, z)) {
				l.set(x, y, z, t.log);
				true
			} else {
				false
			}
		};
		let mut att = vec![];
		match t.trunk {
			TrunkKind::Straight => {
				(0..h).for_each(|dy| _ = log(l, x, y + dy, z));
				att.push(Attach { x, y: y + h, z, r: 0, double: false });
			}
			TrunkKind::Giant | TrunkKind::DarkOak | TrunkKind::MegaJungle => {
				for dy in 0..h {
					for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
						log(l, x + dx, y + dy, z + dz);
					}
				}
				att.push(Attach { x, y: y + h, z, r: 0, double: true });
				if t.trunk == TrunkKind::MegaJungle {
					let mut bh = h - 2 - r.next_bound(4);
					while bh > h / 2 {
						let a = r.next_float() * std::f32::consts::TAU;
						let (mut bx, mut bz) = (0, 0);
						for b in 0..5 {
							bx = (1.5 + a.cos() * b as f32) as i32;
							bz = (1.5 + a.sin() * b as f32) as i32;
							log(l, x + bx, y + bh - 3 + b / 2, z + bz);
						}
						att.push(Attach { x: x + bx, y: y + bh, z: z + bz, r: -2, double: false });
						bh -= 2 + r.next_bound(4);
					}
				}
			}
			TrunkKind::Forking => {
				let dirs = [(0, -1), (0, 1), (-1, 0), (1, 0)];
				let d = dirs[r.next_bound(4) as usize];
				let lean_h = h - r.next_bound(4) - 1;
				let mut steps = 3 - r.next_bound(3);
				let (mut tx, mut tz, mut top) = (x, z, None);
				for dy in 0..h {
					if dy >= lean_h && steps > 0 {
						(tx, tz, steps) = (tx + d.0, tz + d.1, steps - 1);
					}
					if log(l, tx, y + dy, tz) {
						top = Some(y + dy + 1);
					}
				}
				if let Some(ty) = top {
					att.push(Attach { x: tx, y: ty, z: tz, r: 1, double: false });
				}
				let bd = dirs[r.next_bound(4) as usize];
				if bd != d {
					let (mut tx, mut tz, mut top) = (x, z, None);
					let mut dy = lean_h - r.next_bound(2) - 1;
					let mut bs = 1 + r.next_bound(3);
					while dy < h && bs > 0 {
						if dy >= 1 {
							(tx, tz) = (tx + bd.0, tz + bd.1);
							if log(l, tx, y + dy, tz) {
								top = Some(y + dy + 1);
							}
						}
						dy += 1;
						bs -= 1;
					}
					if let Some(ty) = top {
						att.push(Attach { x: tx, y: ty, z: tz, r: 0, double: false });
					}
				}
			}
			// ponytail: fancy oak and cherry branches are approximated (random clusters/branches), not vanilla's placer
			TrunkKind::Fancy => {
				let total = h + 2;
				let trunk_top = (total as f32 * 0.618) as i32;
				(0..trunk_top).for_each(|dy| _ = log(l, x, y + dy, z));
				let clusters = 1 + total / 5;
				for i in 0..clusters {
					let cy = if i == 0 { total - 5 } else { trunk_top - 3 + r.next_bound((total - trunk_top).max(1)) };
					let (a, dist) = (r.next_float() * std::f32::consts::TAU, if i == 0 { 0.0 } else { 1.0 + r.next_float() * 2.5 });
					let (ox, oz) = ((a.cos() * dist).round() as i32, (a.sin() * dist).round() as i32);
					for s in 0..=4 {
						let f = s as f32 / 4.0;
						log(l, x + (ox as f32 * f).round() as i32, y + trunk_top - 1 + ((cy - trunk_top + 1) as f32 * f).round() as i32, z + (oz as f32 * f).round() as i32);
					}
					att.push(Attach { x: x + ox, y: y + cy, z: z + oz, r: 0, double: false });
				}
			}
			TrunkKind::Cherry => {
				(0..h).for_each(|dy| _ = log(l, x, y + dy, z));
				let n = t.branches.sample(r).max(1);
				if n == 1 {
					att.push(Attach { x, y: y + h, z, r: 0, double: false });
				}
				for _ in 0..n {
					let (a, len) = (r.next_float() * std::f32::consts::TAU, 2 + r.next_bound(3));
					let (ox, oz) = ((a.cos() * len as f32).round() as i32, (a.sin() * len as f32).round() as i32);
					let start = h - 4 + r.next_bound(2);
					let end = h - 1 + r.next_bound(2);
					for s in 1..=len {
						let f = s as f32 / len as f32;
						log(l, x + (ox as f32 * f).round() as i32, y + start + ((end - start) as f32 * f).round() as i32, z + (oz as f32 * f).round() as i32);
					}
					att.push(Attach { x: x + ox, y: y + end + 1, z: z + oz, r: 0, double: false });
				}
			}
		}
		for a in att {
			let off = t.offset.sample(r);
			self.foliage(l, r, t, &a, fh, radius, off);
		}
	}

	#[allow(clippy::too_many_arguments)]
	fn foliage(&self, l: &mut dyn Level, r: &mut Xoroshiro, t: &Tree, a: &Attach, fh: i32, radius: i32, offset: i32) {
		let kind = t.foliage;
		let rad = radius + a.r;
		let row = |l: &mut dyn Level, r: &mut Xoroshiro, cy: i32, rr: i32, yo: i32| {
			let w = a.double as i32;
			for dx in -rr..=rr + w {
				for dz in -rr..=rr + w {
					let (ax, az) = if a.double { (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs())) } else { (dx.abs(), dz.abs()) };
					let skip = match kind {
						FoliageKind::Blob | FoliageKind::Cherry => ax == rr && az == rr && (r.next_bound(2) == 0 || yo == 0),
						FoliageKind::Bush => ax == rr && az == rr && r.next_bound(2) == 0,
						FoliageKind::Fancy => (ax as f32 + 0.5).powi(2) + (az as f32 + 0.5).powi(2) > (rr * rr) as f32,
						FoliageKind::Jungle | FoliageKind::MegaPine => ax + az >= 7 || ax * ax + az * az > rr * rr,
						FoliageKind::Spruce | FoliageKind::Pine | FoliageKind::Poplar => ax == rr && az == rr && rr > 0,
						FoliageKind::Acacia => {
							if yo == 0 {
								(ax > 1 || az > 1) && ax != 0 && az != 0
							} else {
								ax == rr && az == rr && rr > 0
							}
						}
						FoliageKind::DarkOak => {
							if yo == 0 && a.double && (dx == -rr || dx >= rr) && (dz == -rr || dz >= rr) {
								true
							} else if yo == -1 && !a.double {
								ax == rr && az == rr
							} else {
								yo == 1 && ax + az > rr * 2 - 2
							}
						}
						FoliageKind::Spread => false,
					};
					if !skip {
						let (x, y, z) = (a.x + dx, cy + yo, a.z + dz);
						if self.free(l.get(x, y, z)) {
							l.set(x, y, z, t.leaves);
						}
					}
				}
			}
		};
		match kind {
			FoliageKind::Blob | FoliageKind::Cherry => {
				for yo in (offset - fh..=offset).rev() {
					row(l, r, a.y, (rad - 1 - yo / 2).max(0) + (kind == FoliageKind::Cherry) as i32, yo);
				}
			}
			FoliageKind::Bush => {
				for yo in (offset - fh..=offset).rev() {
					row(l, r, a.y, rad - 1 - yo, yo);
				}
			}
			FoliageKind::Fancy => {
				for yo in (offset - fh..=offset).rev() {
					row(l, r, a.y, radius + (yo != offset && yo != offset - fh) as i32, yo);
				}
			}
			FoliageKind::Jungle => {
				let lh = if a.double { fh } else { 1 + r.next_bound(2) };
				for yo in (offset - lh..=offset).rev() {
					row(l, r, a.y, rad + 1 - yo, yo);
				}
			}
			FoliageKind::Spruce => {
				let (mut cur, mut max, mut min) = (r.next_bound(2), 1, 0);
				for yo in (-fh..=offset).rev() {
					row(l, r, a.y, cur, yo);
					if cur >= max {
						cur = min;
						min = 1;
						max = (max + 1).min(rad);
					} else {
						cur += 1;
					}
				}
			}
			FoliageKind::Pine => {
				let mut cur = 0;
				for yo in (offset - fh..=offset).rev() {
					row(l, r, a.y, cur, yo);
					if cur >= 1 && yo == offset - fh + 1 {
						cur -= 1;
					} else if cur < rad {
						cur += 1;
					}
				}
			}
			FoliageKind::MegaPine => {
				let mut prev = 0;
				for yy in a.y - fh + offset..=a.y + offset {
					let yo = a.y - yy;
					let smooth = rad + (yo as f32 / fh as f32 * 3.5).floor() as i32;
					let jag = if yo > 0 && smooth == prev && yy & 1 == 0 { smooth + 1 } else { smooth };
					row(l, r, yy, jag, 0);
					prev = smooth;
				}
			}
			FoliageKind::Acacia => {
				let cy = a.y + offset;
				row(l, r, cy, rad, -1 - fh);
				row(l, r, cy, radius - 1, -fh);
				row(l, r, cy, rad - 1, 0);
			}
			FoliageKind::DarkOak => {
				let cy = a.y + offset;
				if a.double {
					row(l, r, cy, radius + 2, -1);
					row(l, r, cy, radius + 3, 0);
					row(l, r, cy, radius + 2, 1);
					if r.next_bool() {
						row(l, r, cy, radius, 2);
					}
				} else {
					row(l, r, cy, radius + 2, -1);
					row(l, r, cy, radius + 1, 0);
				}
			}
			FoliageKind::Spread => {
				let (fr, fhh) = (radius.max(1), fh.max(1));
				for _ in 0..t.attempts {
					let x = a.x + r.next_bound(fr) - r.next_bound(fr);
					let y = a.y + r.next_bound(fhh) - r.next_bound(fhh);
					let z = a.z + r.next_bound(fr) - r.next_bound(fr);
					if self.free(l.get(x, y, z)) {
						l.set(x, y, z, t.leaves);
					}
				}
			}
			// ponytail: poplar foliage is a guessed narrow spindle; check vanilla's poplar placer if they look off
			FoliageKind::Poplar => {
				let tall = radius.max(fh);
				for yo in (-tall + 1..=1).rev() {
					let rr = if yo == 1 || yo <= -tall + 2 { 0 } else { 1 + (yo > -tall / 2 && yo < -1) as i32 };
					row(l, r, a.y, rr, yo);
				}
			}
		}
	}
}

struct Ctx<'a> {
	top: usize,
	biome_at: &'a dyn Fn(i32, i32, i32) -> u16,
	info: &'a Simplex,
}

fn leads_to_tree(reg: &Registry, v: &Value, depth: u32) -> bool {
	if depth > 6 {
		return false;
	}
	let v = match v.as_str() {
		Some(n) => reg.get("worldgen/feature", n),
		None => v,
	};
	let sub = |p: &Value| {
		let f = match p.as_str() {
			Some(n) => &reg.get("worldgen/placed_feature", n)["feature"],
			None => &p["feature"],
		};
		leads_to_tree(reg, f, depth + 1)
	};
	match v["type"].as_str().unwrap_or("") {
		"minecraft:tree" | "minecraft:huge_red_mushroom" | "minecraft:huge_brown_mushroom" => true,
		"minecraft:random_selector" => sub(&v["default"]) || v["features"].as_array().unwrap().iter().any(|o| sub(&o["feature"])),
		"minecraft:simple_random_selector" => v["features"].as_array().unwrap().iter().any(sub),
		"minecraft:random_boolean_selector" => sub(&v["feature_true"]) || sub(&v["feature_false"]),
		_ => false,
	}
}

#[allow(clippy::too_many_arguments)]
fn mushroom(l: &mut dyn Level, r: &mut Xoroshiro, red: bool, cap: B, stem: B, soil: &[B], x: i32, y: i32, z: i32) {
	let mut h = r.next_bound(3) + 4;
	if r.next_bound(12) == 0 {
		h *= 2;
	}
	if !soil.contains(&l.get(x, y - 1, z)) || y + h + 1 > MIN_Y + HEIGHT {
		return;
	}
	let radius = if red { 2 } else { 3 };
	for dy in 0..=h {
		let rr = if dy < h - 3 { 0 } else { radius };
		for dx in -rr..=rr {
			for dz in -rr..=rr {
				if l.get(x + dx, y + dy, z + dz) != AIR {
					return;
				}
			}
		}
	}
	for dy in 0..h {
		l.set(x, y + dy, z, stem);
	}
	if red {
		for dy in h - 3..=h {
			let rr = if dy < h { radius } else { radius - 1 };
			for dx in -rr..=rr {
				for dz in -rr..=rr {
					let (xe, ze) = (dx.abs() == rr, dz.abs() == rr);
					if dy >= h || xe != ze {
						l.set(x + dx, y + dy, z + dz, cap);
					}
				}
			}
		}
	} else {
		for dx in -radius..=radius {
			for dz in -radius..=radius {
				if !(dx.abs() == radius && dz.abs() == radius) {
					l.set(x + dx, y + h, z + dz, cap);
				}
			}
		}
	}
}
