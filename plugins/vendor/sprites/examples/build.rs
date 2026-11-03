//! Generates `$OUT_DIR/embedded_sprites.rs`, a `static EMBEDDED_SPRITES: &[(&str, &[u8])]`
//! of `include_bytes!` calls over every PNG in `assets/sprites/` — used by
//! `sprite_dig_demo.rs` so the sprite set ships inside the binary instead of
//! being read from disk at runtime (no dependency on the working directory
//! the exe happens to be launched from).
//!
//! Names are the PNG's path *relative to `assets/sprites/`* with the
//! extension stripped (e.g. `Character/Idle/Idle-Sheet`, `Assets/Tiles`) —
//! the pack stores sheets in nested folders where plain file stems collide
//! (`Idle-Sheet`, `Run-Sheet`, `Background`, … each appear twice), so a
//! bare-stem key would silently drop sprites in `pack_atlas`'s HashMap.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let sprites_dir = Path::new(&manifest_dir).join("assets/sprites");
    println!("cargo:rerun-if-changed={}", sprites_dir.display());

    let mut entries: Vec<(String, String)> = Vec::new();
    fn walk(dir: &Path, sprites_dir: &Path, entries: &mut Vec<(String, String)>) {
        if let Ok(rd) = fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, sprites_dir, entries);
                } else if path.extension().and_then(|e| e.to_str()) == Some("png") {
                    let rel = path.strip_prefix(sprites_dir).unwrap();
                    let name = rel.with_extension("").to_string_lossy().replace('\\', "/");
                    let abs = path
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    entries.push((name, abs));
                }
            }
        }
    }
    walk(&sprites_dir, &sprites_dir, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::new();
    code.push_str("pub static EMBEDDED_SPRITES: &[(&str, &[u8])] = &[\n");
    for (name, abs) in &entries {
        code.push_str(&format!("    (\"{name}\", include_bytes!(\"{abs}\")),\n"));
    }
    code.push_str("];\n");

    let out_dir = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out_dir).join("embedded_sprites.rs"), code).unwrap();
}
