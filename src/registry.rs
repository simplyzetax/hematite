use crate::density::{parse, Df};
use crate::noise::{NoiseParams, NormalNoise};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

pub struct Registry {
	json: HashMap<String, Value>,
	functions: RefCell<HashMap<String, Rc<Df>>>,
	noises: HashMap<String, NormalNoise>,
}

impl Registry {
	pub fn load(jar: &Path) -> Self {
		let mut zip = zip::ZipArchive::new(std::fs::File::open(jar).expect("open jar")).expect("read jar");
		let mut json: HashMap<String, Value> = HashMap::new();
		for i in 0..zip.len() {
			let mut e = zip.by_index(i).unwrap();
			let name = e.name().unwrap().to_string();
			let Some(rest) = name.strip_prefix("data/minecraft/") else { continue };
			let Some(rest) = rest.strip_suffix(".json") else { continue };
			if !(rest.starts_with("worldgen/") || rest.starts_with("tags/")) {
				continue;
			}
			let (kind, path) = split_kind(rest);
			let mut s = String::new();
			e.read_to_string(&mut s).unwrap();
			json.insert(format!("{kind}/minecraft:{path}"), serde_json::from_str(&s).expect("json"));
		}
		let mut noises = HashMap::new();
		for (k, v) in &json {
			if let Some(n) = k.strip_prefix("worldgen/noise/") {
				let p = NoiseParams {
					base_amplitude: v.get("base_amplitude").and_then(Value::as_f64).unwrap_or(1.0),
					base_octave: v["base_octave"].as_i64().unwrap() as i32,
					octave_count: v.get("octave_count").and_then(Value::as_i64).unwrap_or(1) as i32,
					amplitude_modifiers: v
						.get("amplitude_modifiers")
						.and_then(Value::as_array)
						.map(|a| a.iter().map(|x| x.as_f64().unwrap()).collect())
						.unwrap_or_default(),
				};
				assert!(v.get("normalize").is_none(), "noise {n} uses non-default normalization");
				noises.insert(n.to_string(), NormalNoise::new(&p));
			}
		}
		Self { json, functions: RefCell::default(), noises }
	}

	pub fn get(&self, kind: &str, name: &str) -> &Value {
		let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
		self.json.get(&format!("{kind}/{name}")).unwrap_or_else(|| panic!("missing {kind}/{name}"))
	}

	pub fn function(&self, name: &str) -> Rc<Df> {
		self.functions
			.borrow_mut()
			.entry(name.to_string())
			.or_insert_with(|| parse(self.get("worldgen/density_function", name)))
			.clone()
	}

	pub fn normal_noise(&self, name: &str) -> &NormalNoise {
		self.noises.get(name).unwrap_or_else(|| panic!("missing noise {name}"))
	}
}

fn split_kind(rest: &str) -> (&str, &str) {
	const KINDS: [&str; 6] = ["worldgen/multi_noise_biome_source_parameter_list", "worldgen/configured_carver", "worldgen/placed_feature", "worldgen/configured_feature", "tags/block", "tags/worldgen/biome"];
	for k in KINDS {
		if let Some(p) = rest.strip_prefix(k).and_then(|p| p.strip_prefix('/')) {
			return (k, p);
		}
	}
	let mut it = rest.splitn(3, '/');
	let (a, b) = (it.next().unwrap(), it.next().unwrap());
	let kind_len = a.len() + 1 + b.len();
	(&rest[..kind_len], &rest[kind_len + 1..])
}
