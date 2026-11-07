//! Fetches the CC0 Deep-Space Skybox galaxy and nebula layers (see
//! `assets/deep-space/CREDITS.md`) into `OUT_DIR` so they are embedded into
//! the sky pass without living in the repository.
//!
//! The archive is verified against a pinned SHA-256 before use. Set
//! `PULSAR_SKY_ARCHIVE` to a local copy of the archive to build offline, or
//! `PULSAR_SKY_SKIP_DOWNLOAD=1` to skip the presets. When the files cannot be
//! obtained the build still succeeds with `cfg(sky_presets_missing)` set, and
//! the presets report themselves unavailable at run time.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const URL: &str = "https://toolbenchdev.com/downloads/deep-space-skybox-layers-1.0.zip";
const SHA256: &str = "873456f4ef8689d1ef8274d25c0524517b734be304cbbc28176b1778c5432acb";
const LAYERS: [(&str, &str); 2] = [
    ("hdr/deep_space_galaxy_4k.hdr", "deep_space_galaxy_4k.hdr"),
    ("hdr/deep_space_nebula_4k.hdr", "deep_space_nebula_4k.hdr"),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PULSAR_SKY_ARCHIVE");
    println!("cargo:rerun-if-env-changed=PULSAR_SKY_SKIP_DOWNLOAD");
    println!("cargo::rustc-check-cfg=cfg(sky_presets_missing)");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("deep-space");
    std::fs::create_dir_all(&out).unwrap();
    if LAYERS.iter().all(|(_, name)| {
        out.join(name)
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    }) {
        return;
    }
    match fetch(&out) {
        Ok(()) => {}
        Err(error) => {
            println!("cargo:warning=sky presets unavailable: {error}");
            for (_, name) in LAYERS {
                let _ = std::fs::write(out.join(name), b"");
            }
            println!("cargo:rustc-cfg=sky_presets_missing");
        }
    }
}

fn fetch(out: &Path) -> Result<(), String> {
    if std::env::var_os("PULSAR_SKY_SKIP_DOWNLOAD").is_some() {
        return Err("PULSAR_SKY_SKIP_DOWNLOAD is set".into());
    }
    let bytes = match std::env::var_os("PULSAR_SKY_ARCHIVE") {
        Some(path) => std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?,
        None => {
            let response = ureq::get(URL)
                .timeout(std::time::Duration::from_secs(300))
                .call()
                .map_err(|e| format!("download {URL}: {e}"))?;
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(256 << 20)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("download {URL}: {e}"))?;
            bytes
        }
    };
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != SHA256 {
        return Err(format!("archive checksum mismatch (got {digest})"));
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for (entry, name) in LAYERS {
        let mut file = archive
            .by_name(entry)
            .map_err(|e| format!("{entry}: {e}"))?;
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        std::fs::write(out.join(name), data).map_err(|e| e.to_string())?;
    }
    Ok(())
}
