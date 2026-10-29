//! C API used by the Paper plugin. One handle per world seed, callable from many threads at once.
//! Build with `cargo rustc --lib --profile plugin --crate-type cdylib` (that profile unwinds, so a
//! panic comes back as an error instead of aborting the server).

use crate::registry::Registry;
use crate::terrain::{HEIGHT, Terrain};
use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

pub struct Handle {
	t: Terrain,
	biomes: Mutex<HashMap<(i32, i32), Arc<Vec<u16>>>>,
}

const BIOME_CACHE: usize = 16384; // about 50 MB

impl Handle {
	/// A chunk's biomes (see hematite_biomes), shared by every thread.
	fn grid(&self, x: i32, z: i32) -> Arc<Vec<u16>> {
		if let Some(b) = self.biomes.lock().unwrap_or_else(|e| e.into_inner()).get(&(x, z)) {
			return b.clone();
		}
		let b = Arc::new(self.t.chunk_biomes(x, z, &mut None));
		let mut m = self.biomes.lock().unwrap_or_else(|e| e.into_inner());
		if m.len() >= BIOME_CACHE {
			m.clear(); // ponytail: clear-all eviction, switch to LRU if the hit rate suffers
		}
		m.insert((x, z), b.clone());
		b
	}
}

/// Loads worldgen data from `jar` (any jar containing data/minecraft/worldgen). `sin`: the server's 65536-entry
/// Mth.SIN table, `kernel`: its 13824-entry Beardifier.BEARD_KERNEL; null to compute them. Null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hematite_new(seed: i64, jar: *const c_char, sin: *const f32, kernel: *const f32) -> *mut Handle {
	catch_unwind(AssertUnwindSafe(|| {
		// same tables for every world
		if !sin.is_null() {
			let _ = crate::terrain::SIN.set(unsafe { std::slice::from_raw_parts(sin, 65536) }.to_vec());
		}
		if !kernel.is_null() {
			let _ = crate::beard::KERNEL.set(unsafe { std::slice::from_raw_parts(kernel, 24 * 24 * 24) }.to_vec());
		}
		let jar = unsafe { CStr::from_ptr(jar) }.to_str().unwrap();
		let t = Terrain::new(&Registry::load(std::path::Path::new(jar)), seed);
		Box::into_raw(Box::new(Handle { t, biomes: Mutex::default() }))
	}))
	.unwrap_or(std::ptr::null_mut())
}

/// Copies the block names (newline separated) into `buf` and returns their full length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hematite_palette(h: *const Handle, buf: *mut u8, cap: usize) -> usize {
	let s = unsafe { &*h }.t.palette.names.join("\n");
	unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len().min(cap)) };
	s.len()
}

/// Fills `out` (16 * 16 * 384 palette ids, index (z * 16 + x) * 384 + y + 64) with the chunk after
/// noise, surface and carvers. `beard`: `beard_len` ints describing the structure pieces that reshape this chunk
/// (see Beard::from_ints), or none. Returns how many sections from the bottom hold anything but air, or -1 if
/// generation panicked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hematite_generate(h: *const Handle, cx: i32, cz: i32, out: *mut u16, beard: *const i32, beard_len: usize) -> i32 {
	let h = unsafe { &*h };
	let out = unsafe { std::slice::from_raw_parts_mut(out, 256 * HEIGHT as usize) };
	let beard = (beard_len > 0).then(|| unsafe { std::slice::from_raw_parts(beard, beard_len) });
	let biomes = |x: i32, z: i32| h.grid(x, z);
	catch_unwind(AssertUnwindSafe(|| {
		let beard = beard.map(crate::beard::Beard::from_ints);
		out.copy_from_slice(&h.t.generate_cut(cx, cz, &biomes, &mut None, false, beard).0.blocks);
		out.chunks(HEIGHT as usize).filter_map(|c| c.iter().rposition(|&b| b != crate::terrain::AIR)).max().map_or(0, |y| y as i32 / 16 + 1)
	}))
	.unwrap_or(-1)
}

/// Copies the biome names (newline separated) into `buf` and returns their full length. Biome ids index this list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hematite_biome_names(h: *const Handle, buf: *mut u8, cap: usize) -> usize {
	let s = unsafe { &*h }.t.table.names.join("\n");
	unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, s.len().min(cap)) };
	s.len()
}

/// Fills `out` (4 * 96 * 4 biome ids, index (z * 4 + x) * 96 + y + 16 in quarts of the chunk) with the chunk's biomes,
/// the same ones the chunk's terrain uses. Returns 0, or -1 if it panicked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hematite_biomes(h: *const Handle, cx: i32, cz: i32, out: *mut u16) -> i32 {
	let h = unsafe { &*h };
	let out = unsafe { std::slice::from_raw_parts_mut(out, 4 * 96 * 4) };
	catch_unwind(AssertUnwindSafe(|| out.copy_from_slice(&h.grid(cx, cz)))).map_or(-1, |_| 0)
}
