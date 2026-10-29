use hematite::density::{Compiler, Ctx, Volume};
use hematite::random::Xoroshiro;
use hematite::{biome, dh, registry, terrain};
use hematite::jar;

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.get(1).map(String::as_str) == Some("preview") {
		return dh::preview(std::path::Path::new(&args[2]), args[3].parse().unwrap(), std::path::Path::new(&args[4]));
	}
	let reg = registry::Registry::load(&jar());
	let seed: i64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(12345);
	let random = Xoroshiro::new(seed).fork_positional();
	let mut comp = Compiler::new(&reg, random);
	match args.get(1).map(String::as_str) {
		Some("dump-density") => {
			let names = ["minecraft:overworld/final_density", "minecraft:overworld/continents", "minecraft:overworld/erosion"];
			let ids: Vec<_> = names.iter().map(|n| comp.sampler_named(n)).collect();
			let g = comp.finish();
			let mut out = Vec::new();
			for (cx, cz) in [(0, 0), (-3, 7), (100, -250)] {
				let v = Volume::new(16, 384, 16, cx * 16, -64, cz * 16);
				let mut ctx = Ctx::new(g.cache_count, true);
				let mut buf = vec![0f32; v.size()];
				g.vol(&mut ctx, ids[0], &mut buf, &v);
				out.extend(buf.iter().map(|f| f.to_bits()));
			}
			let mut ctx = Ctx::new(g.cache_count, false);
			for i in 0..2000i32 {
				let (x, y, z) = (i.wrapping_mul(7919) % 5000 - 2500, i % 384 - 64, i.wrapping_mul(104729) % 5000 - 2500);
				for id in &ids {
					out.push(g.val(&mut ctx, *id, x, y, z).to_bits());
				}
			}
			let bytes: Vec<u8> = out.iter().flat_map(|u| u.to_be_bytes()).collect();
			std::fs::write(&args[3], bytes).unwrap();
		}
		Some("dump-biomes") => {
			let radius: i32 = args[4].parse().unwrap();
			let names = ["temperature", "vegetation", "continents", "erosion", "depth", "ridges"];
			let fns = names.map(|n| comp.sampler_named(&format!("minecraft:overworld/{n}")));
			let g = comp.finish();
			let table = biome::BiomeTable::overworld();
			let climate = biome::Climate { fns };
			let mut last = None;
			let mut out = String::new();
			for cx in -radius..=radius {
				for cz in -radius..=radius {
					let b = climate.chunk_biomes(&g, &table, cx, cz, &mut last);
					let v = Volume::stepped(4, 96, 4, 0, 0, 0, 4, 4, 4);
					for sy in 0..24 {
						for x in 0..4 {
							for y in sy * 4..sy * 4 + 4 {
								for z in 0..4 {
									out += table.names[b[v.idx(x, y, z)] as usize].trim_start_matches("minecraft:");
									out.push(' ');
								}
							}
						}
					}
					out.push('\n');
				}
			}
			let mut ctx = Ctx::new(g.cache_count, false);
			for i in 0..4000i32 {
				let (qx, qy, qz) = (i.wrapping_mul(7919) % 20000 - 10000, i % 96 - 16, i.wrapping_mul(104729) % 20000 - 10000);
				let t = climate.target_at(&g, &mut ctx, qx, qy, qz);
				out += table.names[table.find(&t, &mut last) as usize].trim_start_matches("minecraft:");
				out.push(' ');
			}
			out.push('\n');
			std::fs::write(&args[3], out).unwrap();
		}
		Some("dump-chunks") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let cache = std::cell::RefCell::new(std::collections::HashMap::new());
			let last = std::cell::RefCell::new(None);
			let biomes = |cx: i32, cz: i32| {
				cache
					.borrow_mut()
					.entry((cx, cz))
					.or_insert_with(|| std::sync::Arc::new(t.chunk_biomes(cx, cz, &mut last.borrow_mut())))
					.clone()
			};
			let mut out = Vec::new();
			let mut palette: Vec<String> = vec![];
			for p in args[4..].chunks(2) {
				let (cx, cz) = (p[0].parse().unwrap(), p[1].parse().unwrap());
				let start = std::time::Instant::now();
				for dz in -1..=1 {
					for dx in -1..=1 {
						biomes(cx + dx, cz + dz);
					}
				}
				let mut l = *last.borrow();
				let chunk = t.generate(cx, cz, &biomes, &mut l);
				*last.borrow_mut() = l;
				eprintln!("chunk {cx} {cz} in {:?}", start.elapsed());
				for b in &chunk.blocks {
					let name = &t.palette.names[*b as usize];
					let i = palette.iter().position(|n| n == name).unwrap_or_else(|| {
						palette.push(name.clone());
						palette.len() - 1
					});
					out.extend((i as u16).to_be_bytes());
				}
			}
			std::fs::write(&args[3], out).unwrap();
			std::fs::write(format!("{}.palette", args[3]), palette.join("\n") + "\n").unwrap();
		}
		Some("find-biome") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let want = t.table.id(&args[3]);
			let n: usize = args[4].parse().unwrap();
			let mut c = Ctx::new(t.g.cache_count, false);
			let mut last = None;
			let mut found = vec![];
			'outer: for r in 0..4000i32 {
				for (cx, cz) in [(r * 7, r * 3), (-r * 5, r * 11), (r * 13, -r * 2), (-r * 3, -r * 9)] {
					if t.uncached_biome(&mut c, cx * 4 + 2, 16, cz * 4 + 2, &mut last) == want {
						found.push(format!("{cx} {cz}"));
						if found.len() >= n {
							break 'outer;
						}
					}
				}
			}
			println!("{}", found.join(" "));
		}
		Some("dump-temps") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let mut out = String::new();
			for i in 0..20000i32 {
				let b = (i % t.table.names.len() as i32) as u16;
				let (x, y, z) = (i.wrapping_mul(7919) % 30000 - 15000, i % 300 - 20, i.wrapping_mul(104729) % 30000 - 15000);
				out += &format!("{} {x} {y} {z} {}\n", t.table.names[b as usize], t.temperature(b, x, y, z).to_bits());
			}
			std::fs::write(&args[3], out).unwrap();
		}
		Some("spawn") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let mut last = None;
			let rings = (0..64i32).flat_map(|r| (-r..=r).flat_map(move |z| (-r..=r).map(move |x| (x, z))).filter(move |&(x, z)| x.abs().max(z.abs()) == r));
			// ponytail: nearest dry land to 0,0 instead of vanilla's climate-based spawn search
			for (cx, cz) in rings {
				let b: std::collections::HashMap<(i32, i32), std::sync::Arc<Vec<u16>>> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (cx + dx, cz + dz))).map(|p| (p, std::sync::Arc::new(t.chunk_biomes(p.0, p.1, &mut last)))).collect();
				let c = t.generate(cx, cz, &|x, z| b[&(x, z)].clone(), &mut last);
				let h = c.height[8 * 16 + 8];
				let top = c.get(8, h - 1, 8);
				if h > 63 && top != terrain::WATER && top != terrain::LAVA {
					println!("{} {h} {}", cx * 16 + 8, cz * 16 + 8);
					return;
				}
			}
		}
		Some("render") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let n = |i: usize| args[i].parse::<i32>().unwrap();
			dh::render(&t, std::path::Path::new(&args[3]), n(4), n(5), n(6));
		}
		Some("export") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let radius: i32 = args[4].parse().unwrap();
			dh::export(&t, std::path::Path::new(&args[3]), radius, 12);
		}
		Some("export-dhs") => {
			drop(comp);
			let t = terrain::Terrain::new(&reg, seed);
			let p = |i: usize| std::path::Path::new(&args[i]);
			dh::export_dhs(&t, p(3), p(4), args[5].parse().unwrap());
		}
		_ => eprintln!("usage: hematite export <seed> <DistantHorizons.sqlite> <radius in chunks>\n       hematite export-dhs <seed> <server world folder> <plugins/DHSupport/data.sqlite> <radius in chunks>"),
	}
}
