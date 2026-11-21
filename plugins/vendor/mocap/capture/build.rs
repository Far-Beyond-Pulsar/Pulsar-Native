//! Fetches the bundled body-pose model for the `pose` feature and leaves it in
//! `OUT_DIR/pose_model.onnx` for `include_bytes!`.
//!
//! The model is MoveNet SinglePose Lightning (Apache-2.0), as converted to ONNX
//! by Xenova (https://huggingface.co/Xenova/movenet-singlepose-lightning),
//! pinned to one revision and one SHA-256. It is downloaded with `curl` (on
//! every supported OS), verified, and cached outside `target` so a clean build
//! does not download again. If it cannot be had, the build still succeeds with
//! an empty model and says so; the editor then asks for a model file instead.
//!
//! Offline builds: set `PULSAR_MOCAP_POSE_MODEL_FILE` to a copy of the model.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const URL: &str = "https://huggingface.co/Xenova/movenet-singlepose-lightning/resolve/ed0f314bb7356fd1dbf1e4f52c2d40791bf6534f/onnx/model.onnx";
const SHA256: &str = "1ad4f8d6c2f776a9967db3993c9ca740bc350104f9d37c151dc183fc29a464ad";
const MAX_BYTES: usize = 32 << 20;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PULSAR_MOCAP_POSE_MODEL_FILE");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("pose_model.onnx");
    let bytes = if std::env::var_os("CARGO_FEATURE_POSE").is_some() { obtain() } else { None };
    std::fs::write(&out, bytes.unwrap_or_default()).expect("write the model into OUT_DIR");
}

fn obtain() -> Option<Vec<u8>> {
    if let Some(path) = std::env::var_os("PULSAR_MOCAP_POSE_MODEL_FILE") {
        return match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                println!("cargo:warning=PULSAR_MOCAP_POSE_MODEL_FILE is unreadable ({e}); no pose model is embedded");
                None
            }
        };
    }
    let cache = cache_dir().join(format!("{SHA256}.onnx"));
    if let Some(bytes) = std::fs::read(&cache).ok().filter(|b| verified(b)) {
        return Some(bytes);
    }
    let bytes = download(&cache)?;
    verified(&bytes).then_some(bytes).or_else(|| {
        println!("cargo:warning=the downloaded pose model failed its SHA-256 check; none is embedded");
        None
    })
}

fn verified(bytes: &[u8]) -> bool {
    let hex: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    bytes.len() < MAX_BYTES && hex == SHA256
}

fn cache_dir() -> PathBuf {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| Path::new(&h).join(".cargo")))
        .unwrap_or_else(std::env::temp_dir);
    home.join("pulsar-mocap-models")
}

fn download(cache: &Path) -> Option<Vec<u8>> {
    let _ = std::fs::create_dir_all(cache.parent()?);
    let partial = cache.with_extension("part");
    let status = Command::new("curl")
        .args(["--location", "--fail", "--silent", "--show-error", "--max-time", "300", "--max-filesize"])
        .arg(MAX_BYTES.to_string())
        .arg("--output")
        .arg(&partial)
        .arg(URL)
        .status();
    match status {
        Ok(s) if s.success() => {}
        other => {
            println!("cargo:warning=could not download the pose model ({other:?}); none is embedded. Set PULSAR_MOCAP_POSE_MODEL_FILE or choose a model in the editor.");
            let _ = std::fs::remove_file(&partial);
            return None;
        }
    }
    let bytes = std::fs::read(&partial).ok()?;
    if verified(&bytes) {
        let _ = std::fs::rename(&partial, cache);
    } else {
        let _ = std::fs::remove_file(&partial);
    }
    Some(bytes)
}
