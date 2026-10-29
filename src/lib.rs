pub mod beard;
pub mod biome;
pub mod density;
pub mod dh;
pub mod ffi;
pub mod interval;
pub mod noise;
pub mod random;
pub mod registry;
pub mod terrain;
pub mod trees;

/// The game jar the CLI and tests read worldgen data from: $HEMATITE_JAR (any 26.3 client or server jar),
/// else the 26.3 install from the Modrinth app on macOS.
pub fn jar() -> std::path::PathBuf {
	if let Some(p) = std::env::var_os("HEMATITE_JAR") {
		return p.into();
	}
	let home = std::env::var("HOME").unwrap();
	std::path::PathBuf::from(home).join("Library/Application Support/ModrinthApp/meta/versions/26.3-0.20.1/26.3-0.20.1.jar")
}
