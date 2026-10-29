use crate::density::{Ctx, Graph, Id, Volume};

type Space = [[i64; 2]; 7];

enum Node {
	Leaf(usize),
	Sub(Space, Vec<Node>),
}

/// R-tree node; children of a node sit next to each other, the root is at 0.
struct Flat {
	space: Space,
	leaf: usize,
	kids: std::ops::Range<usize>,
}

pub struct BiomeTable {
	pub names: Vec<String>,
	leaves: Vec<(Space, u16)>,
	nodes: Vec<Flat>,
}

/// dist(space, t) if it is below `bound`.
fn dist_below(space: &Space, t: &[i64; 7], bound: i64) -> Option<i64> {
	let mut d = 0i64;
	for i in 0..7 {
		let [min, max] = space[i];
		let above = t[i] - max;
		let below = min - t[i];
		let p = if above > 0 { above } else { below.max(0) };
		d += p * p;
		if d >= bound {
			return None;
		}
	}
	Some(d)
}

fn dist(space: &Space, t: &[i64; 7]) -> i64 {
	let mut d = 0i64;
	for i in 0..7 {
		let [min, max] = space[i];
		let above = t[i] - max;
		let below = min - t[i];
		let p = if above > 0 { above } else { below.max(0) };
		d += p * p;
	}
	d
}

impl BiomeTable {
	pub fn overworld() -> Self {
		let mut names: Vec<String> = vec![];
		let mut leaves = vec![];
		for line in include_str!("../data/overworld_biomes.txt").lines() {
			let p: Vec<&str> = line.split(' ').collect();
			let n: Vec<i64> = p[..13].iter().map(|s| s.parse().unwrap()).collect();
			let mut space = [[0; 2]; 7];
			for i in 0..6 {
				space[i] = [n[i * 2], n[i * 2 + 1]];
			}
			space[6] = [n[12], n[12]];
			let name = format!("minecraft:{}", p[13]);
			let idx = names.iter().position(|x| *x == name).unwrap_or_else(|| {
				names.push(name);
				names.len() - 1
			});
			leaves.push((space, idx as u16));
		}
		let mut nodes: Vec<Node> = (0..leaves.len()).map(Node::Leaf).collect();
		assert!(names.len() <= 128, "biome bitsets are u128");
		let root = build(&leaves, std::mem::take(&mut nodes));
		let mut flat = vec![Flat { space: space_of(&leaves, &root), leaf: usize::MAX, kids: 0..0 }];
		let mut pending = vec![(0, root)];
		while let Some((i, node)) = pending.pop() {
			match node {
				Node::Leaf(l) => flat[i].leaf = l,
				Node::Sub(_, children) => {
					let start = flat.len();
					flat.extend(children.iter().map(|c| Flat { space: space_of(&leaves, c), leaf: usize::MAX, kids: 0..0 }));
					flat[i].kids = start..flat.len();
					pending.extend(children.into_iter().enumerate().map(|(k, c)| (start + k, c)));
				}
			}
		}
		Self { names, leaves, nodes: flat }
	}

	pub fn id(&self, name: &str) -> u16 {
		self.names.iter().position(|n| n == name).unwrap_or_else(|| panic!("unknown biome {name}")) as u16
	}

	/// Nearest leaf; on ties `last` wins, then the first leaf in tree order (like vanilla's RTree with its last result).
	pub fn find(&self, t: &[i64; 7], last: &mut Option<usize>) -> u16 {
		let leaf = if self.nodes[0].leaf != usize::MAX {
			self.nodes[0].leaf
		} else {
			let best = last.map_or((usize::MAX, i64::MAX), |c| (c, dist(&self.leaves[c].0, t)));
			self.search(0, t, best).0
		};
		*last = Some(leaf);
		self.leaves[leaf].1
	}

	fn search(&self, n: usize, t: &[i64; 7], mut best: (usize, i64)) -> (usize, i64) {
		for k in self.nodes[n].kids.clone() {
			let child = &self.nodes[k];
			if let Some(d) = dist_below(&child.space, t, best.1) {
				best = if child.leaf != usize::MAX { (child.leaf, d) } else { self.search(k, t, best) };
			}
		}
		best
	}
}

fn space_of(leaves: &[(Space, u16)], n: &Node) -> Space {
	match n {
		Node::Leaf(i) => leaves[*i].0,
		Node::Sub(s, _) => *s,
	}
}

fn bounds(leaves: &[(Space, u16)], children: &[Node]) -> Space {
	let mut b = space_of(leaves, &children[0]);
	for c in &children[1..] {
		let s = space_of(leaves, c);
		for d in 0..7 {
			b[d] = [b[d][0].min(s[d][0]), b[d][1].max(s[d][1])];
		}
	}
	b
}

fn center(s: &Space, d: usize) -> i64 {
	(s[d][0] + s[d][1]) / 2
}

fn cmp_dim(a: &Space, b: &Space, dim: usize, absolute: bool) -> std::cmp::Ordering {
	for k in 0..7 {
		let d = (dim + k) % 7;
		let (ca, cb) = (center(a, d), center(b, d));
		let (ca, cb) = if absolute { (ca.abs(), cb.abs()) } else { (ca, cb) };
		match ca.cmp(&cb) {
			std::cmp::Ordering::Equal => continue,
			o => return o,
		}
	}
	std::cmp::Ordering::Equal
}

fn build(leaves: &[(Space, u16)], mut children: Vec<Node>) -> Node {
	const PER_NODE: usize = 19;
	if children.len() == 1 {
		return children.pop().unwrap();
	}
	if children.len() <= PER_NODE {
		children.sort_by_key(|c| {
			let s = space_of(leaves, c);
			(0..7).map(|d| center(&s, d).abs()).sum::<i64>()
		});
		let b = bounds(leaves, &children);
		return Node::Sub(b, children);
	}
	let spaces: Vec<Space> = children.iter().map(|c| space_of(leaves, c)).collect();
	let mut order: Vec<usize> = (0..children.len()).collect();
	let mut best: Option<(i64, usize, Vec<usize>)> = None;
	let sizes = bucket_sizes(children.len());
	for d in 0..7 {
		order.sort_by(|&a, &b| cmp_dim(&spaces[a], &spaces[b], d, false));
		let mut cost = 0i64;
		let mut start = 0;
		for n in &sizes {
			let mut b = spaces[order[start]];
			for &i in &order[start + 1..start + n] {
				for k in 0..7 {
					b[k] = [b[k][0].min(spaces[i][k][0]), b[k][1].max(spaces[i][k][1])];
				}
			}
			cost += b.iter().map(|p| (p[1] - p[0]).abs()).sum::<i64>();
			start += n;
		}
		if best.as_ref().is_none_or(|(c, _, _)| *c > cost) {
			best = Some((cost, d, order.clone()));
		}
	}
	let (_, dim, best_order) = best.unwrap();
	let mut slots: Vec<Option<Node>> = children.into_iter().map(Some).collect();
	let mut buckets: Vec<(Space, Vec<Node>)> = vec![];
	let mut start = 0;
	for n in sizes {
		let group: Vec<Node> = best_order[start..start + n].iter().map(|&i| slots[i].take().unwrap()).collect();
		buckets.push((bounds(leaves, &group), group));
		start += n;
	}
	buckets.sort_by(|a, b| cmp_dim(&a.0, &b.0, dim, true));
	let built: Vec<Node> = buckets.into_iter().map(|(_, g)| build(leaves, g)).collect();
	let b = bounds(leaves, &built);
	Node::Sub(b, built)
}

fn bucket_sizes(len: usize) -> Vec<usize> {
	let k = ((len as f64 - 0.01).ln() / 19f64.ln()).floor() as u32;
	let expected = 19usize.pow(k);
	let mut sizes = vec![expected; len / expected];
	if len % expected != 0 {
		sizes.push(len % expected);
	}
	sizes
}

pub struct Climate {
	pub fns: [Id; 6],
}

pub fn quantize(v: f32) -> i64 {
	(v * 10000.0) as i64
}

impl Climate {
	pub fn target_at(&self, g: &Graph, c: &mut Ctx, qx: i32, qy: i32, qz: i32) -> [i64; 7] {
		let (x, y, z) = (qx << 2, qy << 2, qz << 2);
		let mut t = [0i64; 7];
		for (i, f) in self.fns.iter().enumerate() {
			t[i] = quantize(g.val(c, *f, x, y, z));
		}
		t
	}

	pub fn chunk_biomes(&self, g: &Graph, table: &BiomeTable, cx: i32, cz: i32, last: &mut Option<usize>) -> Vec<u16> {
		let v = Volume::stepped(4, 96, 4, cx * 16, -64, cz * 16, 4, 4, 4);
		let mut c = Ctx::new(g.cache_count, true);
		let bufs: Vec<Vec<f32>> = self
			.fns
			.iter()
			.map(|f| {
				let mut b = vec![0f32; v.size()];
				g.vol(&mut c, *f, &mut b, &v);
				b
			})
			.collect();
		let mut out = vec![0u16; v.size()];
		for sy in 0..24 {
			for x in 0..4 {
				for y in sy * 4..sy * 4 + 4 {
					for z in 0..4 {
						let i = v.idx(x, y, z);
						let mut t = [0i64; 7];
						for k in 0..6 {
							t[k] = quantize(bufs[k][i]);
						}
						out[i] = table.find(&t, last);
					}
				}
			}
		}
		out
	}
}
