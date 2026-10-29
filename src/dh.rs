use crate::terrain::{Chunk, Terrain, AIR, HEIGHT, MIN_Y, WATER};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

const LIGHT_STEP: u8 = 9;
const ZSTD_BLOCK: u8 = 4;
const AREA: u8 = 2;

pub struct Section {
	cols: Vec<Vec<u64>>,
	steps: Vec<u8>,
}

fn dp(id: u32, height: i32, bottom: i32, block: u32, sky: u32) -> u64 {
	id as u64 | (height as u64) << 32 | (bottom as u64) << 44 | (sky as u64) << 56 | (block as u64) << 60
}
fn id(d: u64) -> u32 {
	(d & 0x7fff_ffff) as u32
}
fn height(d: u64) -> i32 {
	(d >> 32 & 0xfff) as i32
}
fn bottom(d: u64) -> i32 {
	(d >> 44 & 0xfff) as i32
}
fn sky(d: u64) -> u32 {
	(d >> 56 & 15) as u32
}
fn block_light(d: u64) -> u32 {
	(d >> 60 & 15) as u32
}
fn is_air(id: u32) -> bool {
	id & 0xffff == AIR as u32
}

fn column(chunk: &Chunk, biomes: &[u16], x: i32, z: i32) -> Vec<u64> {
	let biome = |y: i32| biomes[(((x >> 2) + (z >> 2) * 4) * 96 + ((y - MIN_Y) >> 2).clamp(0, 95)) as usize] as u32;
	let mut out = Vec::with_capacity(16);
	let mut cur = biome(MIN_Y + HEIGHT) << 16 | AIR as u32;
	let (mut run_sky, mut sky_above) = (15u32, 15u32);
	let mut last_y = MIN_Y + HEIGHT;
	for y in (MIN_Y..MIN_Y + HEIGHT).rev() {
		let b = chunk.get(x, y, z);
		let new = biome(y) << 16 | b as u32;
		if new != cur {
			out.push(dp(cur, last_y - y, y + 1 - MIN_Y, 0, run_sky));
			cur = new;
			run_sky = sky_above;
			last_y = y;
		}
		sky_above = match b {
			AIR => sky_above,
			WATER => sky_above.saturating_sub(1),
			_ => 0,
		};
	}
	out.push(dp(cur, last_y - MIN_Y + 1, 0, 0, run_sky));
	out
}

fn solid(d: u64) -> bool {
	!is_air(id(d)) && id(d) & 0xffff != WATER as u32
}

fn ground(c: &[u64]) -> i32 {
	c.iter().find(|&&d| solid(d)).map_or(0, |&d| bottom(d) + height(d))
}

// ponytail: section-border columns guess their neighbours as 32 blocks lower instead of looking across sections
fn cull(cols: &mut [Vec<u64>]) {
	let g: Vec<i32> = cols.iter().map(|c| ground(c)).collect();
	for x in 0..64 {
		for z in 0..64 {
			let i = x * 64 + z;
			let n = |dx: i32, dz: i32| {
				let (nx, nz) = (x as i32 + dx, z as i32 + dz);
				if (0..64).contains(&nx) && (0..64).contains(&nz) { g[(nx * 64 + nz) as usize] } else { g[i] - 32 }
			};
			let floor = g[i].min(n(1, 0)).min(n(-1, 0)).min(n(0, 1)).min(n(0, -1));
			let c = &mut cols[i];
			if let Some(k) = c.iter().position(|&d| bottom(d) < floor && solid(d)) {
				let d = c[k];
				c.truncate(k);
				c.push(dp(id(d), bottom(d) + height(d), 0, block_light(d), sky(d)));
			}
		}
	}
}

fn most_common(v: [u32; 4]) -> u32 {
	let mut c = [0; 4];
	for x in v {
		if !is_air(x) {
			c[(0..3).find(|&i| x == v[i]).unwrap_or(3)] += 1;
		}
	}
	let m = *c.iter().max().unwrap();
	v[(0..3).find(|&i| c[i] == m).unwrap_or(3)]
}

fn merge4(cols: [&[u64]; 4]) -> Vec<u64> {
	let mut ys: Vec<i32> = cols.iter().flat_map(|c| c.iter().flat_map(|&d| [bottom(d), bottom(d) + height(d)])).collect();
	ys.sort_unstable();
	ys.dedup();
	let mut at: [isize; 4] = cols.map(|c| c.len() as isize - 1);
	let mut out = vec![];
	let (mut last, mut acc, mut start) = (0u64, 0, 0);
	for w in ys.windows(2) {
		let sample = w[0] + (w[1] - w[0]) / 2;
		let mut ids = [0u32; 4];
		let mut lights = [(0u32, 0u32); 4];
		for i in 0..4 {
			while at[i] >= 0 {
				let d = cols[i][at[i] as usize];
				if sample < bottom(d) + height(d) {
					if sample >= bottom(d) {
						ids[i] = id(d);
						lights[i] = (block_light(d), sky(d));
					}
					break;
				}
				at[i] -= 1;
			}
		}
		let win = most_common(ids);
		let n = ids.iter().filter(|&&x| x == win).count() as u32;
		let avg = |f: fn((u32, u32)) -> u32| {
			if n == 0 {
				lights.iter().map(|&l| f(l)).sum::<u32>() / 4
			} else {
				(0..4).filter(|&i| ids[i] == win).map(|i| f(lights[i])).sum::<u32>() / n
			}
		};
		let (bl, sl) = (avg(|l| l.0), avg(|l| l.1));
		let key = dp(win, 0, 0, bl, sl);
		if acc == 0 {
			(last, acc, start) = (key, w[1] - w[0], w[0]);
		} else if key == last {
			acc += w[1] - w[0];
		} else {
			out.push(last | dp(0, acc, start, 0, 0));
			(last, acc, start) = (key, w[1] - w[0], w[0]);
		}
	}
	if acc > 0 {
		out.push(last | dp(0, acc, start, 0, 0));
	}
	out.reverse();
	out
}

fn parent(children: &[Option<Section>; 4]) -> Section {
	let mut cols = vec![vec![]; 4096];
	let mut steps = vec![0u8; 4096];
	for x in 0..64 {
		for z in 0..64 {
			let Some(c) = &children[(x / 32) + (z / 32) * 2] else { continue };
			let (cx, cz) = ((x % 32) * 2, (z % 32) * 2);
			let idx = [(0, 0), (0, 1), (1, 0), (1, 1)].map(|(dx, dz)| (cx + dx) * 64 + cz + dz);
			cols[x * 64 + z] = merge4(idx.map(|i| c.cols[i].as_slice()));
			steps[x * 64 + z] = idx.iter().map(|&i| c.steps[i]).min().unwrap();
		}
	}
	Section { cols, steps }
}

fn varint(o: &mut Vec<u8>, mut v: u32) {
	while v >= 128 {
		o.push(v as u8 | 128);
		v >>= 7;
	}
	o.push(v as u8);
}

fn blob(s: &Section, local: &HashMap<u32, u32>, xs: std::ops::Range<usize>, zs: std::ops::Range<usize>) -> Vec<u8> {
	let cols: Vec<&Vec<u64>> = xs.flat_map(|x| zs.clone().map(move |z| x * 64 + z)).map(|i| &s.cols[i]).collect();
	let mut o = vec![];
	for c in &cols {
		varint(&mut o, c.len() as u32);
	}
	let mut prev = 0;
	for &d in cols.iter().flat_map(|c| c.iter()) {
		let disc = bottom(d) != prev - height(d);
		prev = bottom(d);
		let lit = d >> 56 != 0;
		varint(&mut o, local[&id(d)] << 2 | (lit as u32) << 1 | disc as u32);
	}
	for &d in cols.iter().flat_map(|c| c.iter()) {
		varint(&mut o, height(d) as u32);
	}
	prev = 0;
	for &d in cols.iter().flat_map(|c| c.iter()) {
		let e = prev - height(d);
		if bottom(d) != e {
			let diff = bottom(d) - e;
			varint(&mut o, ((diff << 1) ^ (diff >> 31)) as u32);
		}
		prev = bottom(d);
	}
	for &d in cols.iter().flat_map(|c| c.iter()) {
		if d >> 56 != 0 {
			o.push((d >> 56) as u8);
		}
	}
	zstd::bulk::compress(&o, 3).unwrap()
}

pub struct Names {
	biomes: Vec<String>,
	blocks: Vec<String>,
}

impl Names {
	pub fn new(t: &Terrain) -> Self {
		let states: HashMap<&str, &str> = include_str!("../data/block_states.txt").lines().filter_map(|l| l.split_once(' ')).collect();
		let blocks = t
			.palette
			.names
			.iter()
			.enumerate()
			.map(|(i, n)| match n.split_once('|') {
				_ if i == AIR as usize => "AIR".into(),
				Some((n, props)) => format!("{n}_STATE_{props}"),
				None => format!("{n}_STATE_{}", states[n.as_str()]),
			})
			.collect();
		Self { biomes: t.table.names.clone(), blocks }
	}
}

struct Row {
	level: u8,
	x: i32,
	z: i32,
	blobs: [Vec<u8>; 8],
	checksum: i32,
}

fn row(s: &Section, names: &Names, level: u8, x: i32, z: i32) -> Row {
	let mut seen = HashMap::new();
	let mut map_out = vec![];
	for &d in s.cols.iter().flatten() {
		let n = seen.len() as u32;
		seen.entry(id(d)).or_insert_with(|| {
			let s = format!("{}_DH-BSW_{}", names.biomes[(id(d) >> 16) as usize], names.blocks[(id(d) & 0xffff) as usize]);
			map_out.extend((s.len() as u16).to_be_bytes());
			map_out.extend(s.as_bytes());
			n
		});
	}
	let mut m = (seen.len() as i32).to_be_bytes().to_vec();
	m.extend(map_out);
	let data = blob(s, &seen, 1..63, 1..63);
	let checksum = data.iter().fold(17i32, |h, &b| h.wrapping_mul(31).wrapping_add(b as i32));
	let z0 = |v: &[u8]| zstd::bulk::compress(v, 3).unwrap();
	Row {
		level,
		x,
		z,
		blobs: [
			data,
			z0(&s.steps),
			z0(&[0u8; 4096]),
			z0(&m),
			blob(s, &seen, 0..64, 0..1),
			blob(s, &seen, 0..64, 63..64),
			blob(s, &seen, 63..64, 0..64),
			blob(s, &seen, 0..1, 0..64),
		],
		checksum,
	}
}

fn open_db(path: &Path) -> Connection {
	let db = Connection::open(path).unwrap();
	db.execute_batch("CREATE TABLE IF NOT EXISTS Schema (SchemaVersionId INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, ScriptName TEXT NOT NULL UNIQUE, AppliedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP)").unwrap();
	let scripts: [(&str, &str); 12] = [
		("0010-sqlite-createInitialDataTables.sql", include_str!("../data/dh_sql/0010-sqlite-createInitialDataTables.sql")),
		("0020-sqlite-createFullDataSourceV2Tables.sql", include_str!("../data/dh_sql/0020-sqlite-createFullDataSourceV2Tables.sql")),
		("0030-sqlite-changeTableJournaling.sql", include_str!("../data/dh_sql/0030-sqlite-changeTableJournaling.sql")),
		("0031-sqlite-useSqliteWalJournaling.sql", include_str!("../data/dh_sql/0031-sqlite-useSqliteWalJournaling.sql")),
		("0040-sqlite-removeRenderCache.sql", include_str!("../data/dh_sql/0040-sqlite-removeRenderCache.sql")),
		("0050-sqlite-addApplyToParentIndex.sql", include_str!("../data/dh_sql/0050-sqlite-addApplyToParentIndex.sql")),
		("0060-sqlite-createChunkHashTable.sql", include_str!("../data/dh_sql/0060-sqlite-createChunkHashTable.sql")),
		("0070-sqlite-createBeaconBeamTable.sql", include_str!("../data/dh_sql/0070-sqlite-createBeaconBeamTable.sql")),
		("0080-sqlite-addApplyToChildrenColumn.sql", include_str!("../data/dh_sql/0080-sqlite-addApplyToChildrenColumn.sql")),
		("0090-sqlite-addAdjacentFullDataColumns.sql", include_str!("../data/dh_sql/0090-sqlite-addAdjacentFullDataColumns.sql")),
		("0100-sqlite-deleteLowDetailDataForRegen.sql", include_str!("../data/dh_sql/0100-sqlite-deleteLowDetailDataForRegen.sql")),
		("0110-sqlite-addApplyToParentIndex.sql", include_str!("../data/dh_sql/0110-sqlite-addApplyToParentIndex.sql")),
	];
	for (name, sql) in scripts {
		let name = format!("sqlScripts/{name}");
		let done: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM Schema WHERE ScriptName=?1)", [&name], |r| r.get(0)).unwrap();
		if !done {
			for part in sql.replace('\r', "").split("--batch--") {
				db.execute_batch(part).unwrap();
			}
			db.execute("INSERT INTO Schema (ScriptName) VALUES(?1)", [&name]).unwrap();
		}
	}
	db.execute_batch("PRAGMA synchronous = OFF").unwrap();
	db
}

fn write_rows(db: &mut Connection, rx: mpsc::Receiver<Row>) -> u64 {
	let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
	let mut bytes = 0u64;
	let mut tx = db.transaction().unwrap();
	let mut n = 0;
	for r in rx {
		bytes += r.blobs.iter().map(|b| b.len() as u64).sum::<u64>();
		tx.prepare_cached(
			"INSERT OR REPLACE INTO FullData (DetailLevel, PosX, PosZ, MinY, DataChecksum, Data, ColumnGenerationStep, ColumnWorldCompressionMode, Mapping, NorthAdjData, SouthAdjData, EastAdjData, WestAdjData, DataFormatVersion, CompressionMode, ApplyToParent, ApplyToChildren, Regenerate, LastModifiedUnixDateTime, CreatedUnixDateTime) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 2, ?13, 0, 0, 0, ?14, ?14)",
		)
		.unwrap()
		.execute(params![r.level, r.x, r.z, r.checksum, r.blobs[0], r.blobs[1], r.blobs[2], r.blobs[3], r.blobs[4], r.blobs[5], r.blobs[6], r.blobs[7], ZSTD_BLOCK, now])
		.unwrap();
		n += 1;
		if n % 2000 == 0 {
			tx.commit().unwrap();
			tx = db.transaction().unwrap();
		}
	}
	tx.commit().unwrap();
	bytes
}

type Biomes = HashMap<(i32, i32), std::sync::Arc<Vec<u16>>>;

/// Chunks of a cut area (see Terrain::generate_cut) whose culled columns would need blocks below their exact range.
fn failing(chunks: &[Chunk], trusted: &[[i32; 256]], n: i32) -> Vec<usize> {
	let w = n * 16;
	let solid = |c: &Chunk, x: i32, y: i32, z: i32| !matches!(c.get(x & 15, y, z & 15), AIR | WATER);
	let at = |x: i32, z: i32| ((z >> 4) * n + (x >> 4)) as usize;
	let col = |x: i32, z: i32| ((z & 15) << 4 | (x & 15)) as usize;
	let ground: Vec<i32> = (0..w * w)
		.map(|i| {
			let (x, z) = (i % w, i / w);
			let c = &chunks[at(x, z)];
			(MIN_Y..c.height[col(x, z)]).rev().find(|&y| solid(c, x, y, z)).map_or(MIN_Y, |y| y + 1)
		})
		.collect();
	let mut bad: Vec<usize> = (0..w * w)
		.filter_map(|i| {
			let (x, z) = (i % w, i / w);
			let (k, g) = (at(x, z), ground[i as usize]);
			let t = trusted[k][col(x, z)];
			if t == MIN_Y {
				return None;
			}
			let nb = |dx: i32, dz: i32| {
				let (nx, nz) = (x + dx, z + dz);
				if (nx >> 6, nz >> 6) == (x >> 6, z >> 6) { ground[(nz * w + nx) as usize] } else { g - 32 }
			};
			let floor = g.min(nb(1, 0)).min(nb(-1, 0)).min(nb(0, 1)).min(nb(0, -1));
			let needed = (MIN_Y..floor).rev().find(|&y| solid(&chunks[k], x, y, z));
			needed.is_none_or(|y| y < t).then_some(k)
		})
		.collect();
	bad.sort_unstable();
	bad.dedup();
	bad
}

/// `cut` skips blocks hidden by culling; the area must then be whole DH sections (n and origin multiples of 4).
pub fn area_chunks(t: &Terrain, ax: i32, az: i32, n: i32, cut: bool) -> (Vec<Chunk>, Biomes) {
	let mut cache = Biomes::new();
	let mut last = None;
	for cz in az - 1..az + n + 1 {
		for cx in ax - 1..ax + n + 1 {
			cache.insert((cx, cz), std::sync::Arc::new(t.chunk_biomes(cx, cz, &mut last)));
		}
	}
	let biomes = |x: i32, z: i32| cache[&(x, z)].clone();
	let (mut chunks, mut trusted): (Vec<Chunk>, Vec<[i32; 256]>) = (0..n * n).map(|i| t.generate_cut(ax + i % n, az + i / n, &biomes, &mut last, cut, None)).unzip();
	loop {
		let mut bad = if cut { failing(&chunks, &trusted, n) } else { vec![] };
		if bad.is_empty() {
			bad = t.decorate(&mut chunks, ax, az, n, &biomes, &trusted);
			if bad.is_empty() {
				break;
			}
		}


		for i in bad {
			chunks[i] = t.generate(ax + i as i32 % n, az + i as i32 / n, &biomes, &mut last);
			trusted[i] = [MIN_Y; 256];
		}
	}
	for chunk in chunks.iter_mut() {
		t.freeze(chunk, &biomes);
	}
	(chunks, cache)
}

fn color(name: &str) -> [f32; 3] {
	let n = name.trim_start_matches("minecraft:").split('|').next().unwrap();
	match n {
		_ if n.ends_with("leaves") => match n {
			"birch_leaves" => [0.50, 0.65, 0.33],
			"spruce_leaves" => [0.24, 0.38, 0.24],
			"cherry_leaves" => [0.93, 0.66, 0.80],
			"azalea_leaves" | "flowering_azalea_leaves" => [0.40, 0.55, 0.20],
			_ => [0.22, 0.48, 0.12],
		},
		_ if n.ends_with("_log") => [0.40, 0.30, 0.18],
		_ if n.contains("mushroom") => [0.75, 0.25, 0.20],
		"grass_block" | "moss_block" => [0.42, 0.66, 0.30],
		"water" => [0.20, 0.35, 0.75],
		"sand" | "sandstone" => [0.86, 0.81, 0.62],
		"red_sand" | "red_sandstone" => [0.75, 0.42, 0.20],
		"snow" | "snow_block" | "powder_snow" => [0.97, 0.98, 1.0],
		"ice" | "packed_ice" | "blue_ice" => [0.62, 0.75, 0.98],
		"dirt" | "coarse_dirt" | "podzol" | "rooted_dirt" | "mud" => [0.48, 0.34, 0.22],
		"mycelium" => [0.45, 0.38, 0.45],
		"gravel" => [0.55, 0.52, 0.50],
		"lava" => [0.95, 0.40, 0.05],
		_ if n.contains("terracotta") => [0.66, 0.40, 0.30],
		_ => [0.50, 0.50, 0.50],
	}
}

fn write_ppm(out: &Path, w: usize, top: &[(String, i32)]) {
	let mut img = format!("P6 {w} {w} 255\n").into_bytes();
	for z in 0..w {
		for x in 0..w {
			let (b, h) = &top[z * w + x];
			let hn = if z > 0 { top[(z - 1) * w + x].1 } else { *h };
			let shade = (1.0 + (h - hn) as f32 * 0.08).clamp(0.6, 1.3);
			img.extend(color(b).map(|c| (c * shade * 255.0).clamp(0.0, 255.0) as u8));
		}
	}
	std::fs::write(out, img).unwrap();
}

pub fn render(t: &Terrain, out: &Path, x0: i32, z0: i32, sections: i32) {
	let w = (sections * 64) as usize;
	let tiles: Vec<(i32, i32, Vec<Chunk>)> = (0..sections * sections)
		.into_par_iter()
		.map(|i| {
			let (sx, sz) = (x0 + i % sections, z0 + i / sections);
			(sx - x0, sz - z0, area_chunks(t, sx * 4, sz * 4, 4, false).0)
		})
		.collect();
	let mut top = vec![(0u16, 0i32); w * w];
	for (tx, tz, chunks) in &tiles {
		for (i, c) in chunks.iter().enumerate() {
			for z in 0..16 {
				for x in 0..16 {
					let h = c.height[((z << 4) | x) as usize];
					let px = (tx * 64 + (i as i32 % 4) * 16 + x) as usize;
					let pz = (tz * 64 + (i as i32 / 4) * 16 + z) as usize;
					top[pz * w + px] = (c.get(x, h - 1, z), h);
				}
			}
		}
	}
	let top: Vec<(String, i32)> = top.into_iter().map(|(b, h)| (t.palette.names[b as usize].clone(), h)).collect();
	write_ppm(out, w, &top);
}

fn read_varint(d: &[u8], i: &mut usize) -> u32 {
	let mut v = 0;
	for shift in (0..32).step_by(7) {
		let b = d[*i];
		*i += 1;
		v |= ((b & 127) as u32) << shift;
		if b & 128 == 0 {
			break;
		}
	}
	v
}

fn decode(blob: &[u8], xs: std::ops::Range<usize>, zs: std::ops::Range<usize>, cols: &mut [Vec<(u32, i32)>]) {
	let d = zstd::stream::decode_all(blob).unwrap();
	let idx: Vec<usize> = xs.flat_map(|x| zs.clone().map(move |z| x * 64 + z)).collect();
	let mut i = 0;
	let counts: Vec<u32> = idx.iter().map(|_| read_varint(&d, &mut i)).collect();
	let ids: Vec<Vec<u32>> = counts.iter().map(|&n| (0..n).map(|_| read_varint(&d, &mut i) >> 2).collect()).collect();
	for (k, col) in idx.iter().enumerate() {
		cols[*col] = ids[k].iter().map(|&id| (id, read_varint(&d, &mut i) as i32)).collect();
	}
}

pub fn preview(db: &Path, level: u8, out: &Path) {
	let db = Connection::open(db).unwrap();
	let rows: Vec<(i32, i32, Vec<[Vec<u8>; 6]>)> = {
		let mut q = db.prepare("SELECT PosX, PosZ, Data, NorthAdjData, SouthAdjData, EastAdjData, WestAdjData, Mapping FROM FullData WHERE DetailLevel = ?1").unwrap();
		q.query_map([level], |r| Ok((r.get(0)?, r.get(1)?, vec![[r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?]])))
			.unwrap()
			.map(Result::unwrap)
			.collect()
	};
	let (minx, minz) = (rows.iter().map(|r| r.0).min().unwrap(), rows.iter().map(|r| r.1).min().unwrap());
	let n = (rows.iter().map(|r| r.0).max().unwrap() - minx).max(rows.iter().map(|r| r.1).max().unwrap() - minz) as usize + 1;
	let w = n * 64;
	let mut top = vec![("minecraft:void".to_string(), 0); w * w];
	for (sx, sz, b) in &rows {
		let b = &b[0];
		let mut cols = vec![vec![]; 4096];
		decode(&b[0], 1..63, 1..63, &mut cols);
		decode(&b[1], 0..64, 0..1, &mut cols);
		decode(&b[2], 0..64, 63..64, &mut cols);
		decode(&b[3], 63..64, 0..64, &mut cols);
		decode(&b[4], 0..1, 0..64, &mut cols);
		let m = zstd::stream::decode_all(&b[5][..]).unwrap();
		let mut names = vec![];
		let mut i = 4;
		while i < m.len() {
			let len = u16::from_be_bytes([m[i], m[i + 1]]) as usize;
			let s = std::str::from_utf8(&m[i + 2..i + 2 + len]).unwrap();
			names.push(s.split_once("_DH-BSW_").unwrap().1.split("_STATE_").next().unwrap().to_string());
			i += 2 + len;
		}
		for x in 0..64 {
			for z in 0..64 {
				let mut y = 385;
				for &(id, h) in &cols[x * 64 + z] {
					if names[id as usize] != "AIR" {
						let px = (sx - minx) as usize * 64 + x;
						let pz = (sz - minz) as usize * 64 + z;
						top[pz * w + px] = (names[id as usize].clone(), y);
						break;
					}
					y -= h;
				}
			}
		}
	}
	write_ppm(out, w, &top);
}

struct Job<'a> {
	t: &'a Terrain,
	names: &'a Names,
	radius: i32,
	tx: mpsc::SyncSender<Row>,
	done: AtomicUsize,
	total: usize,
	start: std::time::Instant,
}

impl Job<'_> {
	fn area(&self, x: i32, z: i32) -> Section {
		let (chunks, cache) = area_chunks(self.t, x * 16, z * 16, 16, std::env::var_os("NO_CULL").is_none());
		let mut cur: Vec<Option<Section>> = (0..16)
			.map(|i| {
				let (sx, sz) = (i % 4, i / 4);
				if !self.inside(0, x * 4 + sx, z * 4 + sz) {
					return None;
				}
				let mut cols = vec![vec![]; 4096];
				for c in 0..16 {
					let chunk = &chunks[((sz * 4 + c / 4) * 16 + sx * 4 + c % 4) as usize];
					let b = &cache[&(chunk.cx, chunk.cz)];
					for lz in 0..16 {
						for lx in 0..16 {
							cols[(((c % 4) * 16 + lx) * 64 + (c / 4) * 16 + lz) as usize] = column(chunk, b, lx, lz);
						}
					}
				}
				if std::env::var_os("NO_CULL").is_none() {
					cull(&mut cols);
				}
				let s = Section { cols, steps: vec![LIGHT_STEP; 4096] };
				let r = row(&s, self.names, 0, x * 4 + sx, z * 4 + sz);
				self.tx.send(r).unwrap();
				Some(s)
			})
			.collect();
		let n = self.done.fetch_add(16, Ordering::Relaxed) + 16;
		if n % 256 == 0 || n >= self.total {
			let el = self.start.elapsed().as_secs_f64();
			eprintln!("{n}/{} sections, {:.0} chunks/s, eta {:.0}s", self.total, n as f64 * 16.0 / el, el / n as f64 * self.total.saturating_sub(n) as f64);
		}
		for level in 1..=AREA {
			let m = 4 >> level;
			let mut next = vec![];
			for i in 0..m * m {
				let (px, pz) = (i % m, i / m);
				let kids: [Option<Section>; 4] = [0, 1, 2, 3].map(|k| cur[((pz * 2 + k / 2) * m * 2 + px * 2 + k % 2) as usize].take());
				if kids.iter().all(Option::is_none) {
					next.push(None);
					continue;
				}
				let s = parent(&kids);
				if level < AREA {
					self.tx.send(row(&s, self.names, level, (x << (AREA - level)) + px, (z << (AREA - level)) + pz)).unwrap();
				}
				next.push(Some(s));
			}
			cur = next;
		}
		cur.pop().flatten().unwrap()
	}

	fn inside(&self, level: u8, x: i32, z: i32) -> bool {
		let w = 64i64 << level;
		let (x0, z0) = (x as i64 * w, z as i64 * w);
		let r = self.radius as i64 * 16;
		x0 < r && x0 + w > -r && z0 < r && z0 + w > -r
	}

	fn build(&self, level: u8, x: i32, z: i32) -> Option<Section> {
		if !self.inside(level, x, z) {
			return None;
		}
		let s = if level == AREA {
			self.area(x, z)
		} else {
			let kids: Vec<Option<Section>> = (0..4).into_par_iter().map(|i| self.build(level - 1, x * 2 + (i & 1), z * 2 + (i >> 1))).collect();
			parent(&kids.try_into().ok().unwrap())
		};
		self.tx.send(row(&s, self.names, level, x, z)).unwrap();
		Some(s)
	}
}

fn run(t: &Terrain, radius: i32, top: u8, write: impl FnOnce(mpsc::Receiver<Row>) -> u64 + Send) -> u64 {
	let names = Names::new(t);
	let (tx, rx) = mpsc::sync_channel(256);
	let side = (radius as usize * 2).div_ceil(4);
	let job = Job { t, names: &names, radius, tx, done: AtomicUsize::new(0), total: side * side, start: std::time::Instant::now() };
	std::thread::scope(|s| {
		let w = s.spawn(move || write(rx));
		[(-1, -1), (0, -1), (-1, 0), (0, 0)].par_iter().for_each(|&(x, z)| {
			job.build(top, x, z);
		});
		drop(job);
		w.join().unwrap()
	})
}

pub fn export(t: &Terrain, db_path: &Path, radius: i32, top: u8) {
	let mut db = open_db(db_path);
	let bytes = run(t, radius, top, move |rx| write_rows(&mut db, rx));
	let db = Connection::open(db_path).unwrap();
	db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
	eprintln!("wrote {:.1} MB", bytes as f64 / 1e6);
}

/// The uuid Paper gives a world, from <world>/dimensions/minecraft/overworld/data/paper/metadata.dat.
pub fn world_uuid(world: &Path) -> String {
	let f = std::fs::File::open(world.join("dimensions/minecraft/overworld/data/paper/metadata.dat")).expect("no Paper metadata.dat in that world (start the server once)");
	let mut nbt = vec![];
	std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(f), &mut nbt).unwrap();
	// ponytail: finds the int array tag by its bytes instead of parsing NBT; the file holds little else
	let tag = b"\x0b\x00\x04uuid\x00\x00\x00\x04";
	let at = nbt.windows(tag.len()).position(|w| w == tag).expect("no uuid in metadata.dat") + tag.len();
	let h: String = nbt[at..at + 16].iter().map(|b| format!("{b:02x}")).collect();
	format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A full-detail row as the DH Support plugin stores and sends it: the client's FullDataSourceV2DTO network encoding.
fn wire(r: &Row, now: i64) -> Vec<u8> {
	let pos = 6 | (r.x as u64 & 0xfff_ffff) << 8 | (r.z as u64 & 0xfff_ffff) << 36; // detail level 6 = 64 blocks wide
	let mut o = pos.to_be_bytes().to_vec();
	o.extend(r.checksum.to_be_bytes());
	// data, north, south, east, west, generation steps, world compression, mapping
	for i in [0, 4, 5, 6, 7, 1, 2, 3] {
		o.extend((r.blobs[i].len() as i32).to_be_bytes());
		o.extend(&r.blobs[i]);
	}
	// format version, compression, apply to parent (the client builds the lower detail levels), to children, regenerate
	o.extend([2, ZSTD_BLOCK, 1, 0, 0]);
	o.extend(now.to_be_bytes()); // last modified
	o.extend(now.to_be_bytes()); // created
	o
}

/// Fills the DH Support plugin's database, so it serves LODs without generating any chunks.
pub fn export_dhs(t: &Terrain, world: &Path, db_path: &Path, radius: i32) {
	let uuid = world_uuid(world);
	let mut db = Connection::open(db_path).unwrap();
	let ok: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'lods')", [], |r| r.get(0)).unwrap();
	assert!(ok, "no lods table in {}: start the server with DH Support once first", db_path.display());
	eprintln!("world {uuid}");
	let bytes = run(t, radius, 12, move |rx| {
		let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
		let (mut bytes, mut n) = (0, 0);
		let mut tx = db.transaction().unwrap();
		// ponytail: the plugin only serves full detail, so the coarser rows are built and dropped (about a quarter of the row work)
		for r in rx.iter().filter(|r| r.level == 0) {
			let data = wire(&r, now);
			bytes += data.len() as u64;
			tx.prepare_cached("REPLACE INTO lods (worldId, x, z, data, beacons, timestamp, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 3)")
				.unwrap()
				.execute(params![uuid, r.x, r.z, data, [0u8; 4], now / 1000])
				.unwrap();
			n += 1;
			if n % 2000 == 0 {
				tx.commit().unwrap();
				tx = db.transaction().unwrap();
			}
		}
		tx.commit().unwrap();
		bytes
	});
	eprintln!("wrote {:.1} MB", bytes as f64 / 1e6);
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn forest_area_gets_trees() {
		let reg = crate::registry::Registry::load(&crate::jar());
		let t = Terrain::new(&reg, 5344236771624635736);
		let (chunks, _) = area_chunks(&t, -16, -24, 4, false);
		let count = |suffix: &str| chunks.iter().flat_map(|c| c.blocks.iter()).filter(|&&b| t.palette.names[b as usize].split('|').next().unwrap().ends_with(suffix)).count();
		let (logs, leaves) = (count("_log"), count("_leaves"));
		assert!(logs > 200 && leaves > 2000, "logs {logs}, leaves {leaves}");
	}

	#[test]
	fn merge_picks_non_air_and_keeps_order() {
		let stone = 1u32;
		let air = 0u32;
		let col = |top: i32| vec![dp(air, 384 - top, top, 0, 15), dp(stone, top, 0, 0, 0)];
		let (a, b, c, d) = (col(100), col(100), col(120), col(80));
		let m = merge4([&a, &b, &c, &d]);
		assert_eq!(m.iter().map(|&d| (id(d), bottom(d), height(d))).collect::<Vec<_>>(), vec![(air, 120, 264), (stone, 0, 120)]);
		assert_eq!(most_common([air, 5, air, 7]), 5);
		assert_eq!(most_common([air, air, air, air]), air);
	}
}
