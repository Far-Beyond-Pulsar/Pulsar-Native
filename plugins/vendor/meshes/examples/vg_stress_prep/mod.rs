//! Self-provisioning of the `vg_stress` scan meshes in pure Rust.
//!
//! Shared verbatim by `build.rs` (feature `vg-stress`) and the example itself
//! (runtime fallback). Downloads the Stanford scans over HTTP(S) into
//! `assets/vg_stress/source/`, decompresses and parses them in streaming
//! fashion, normalises each one and writes `assets/vg_stress/meshes/<name>.glb`.
//! No Python, Blender, curl or tar executables are involved.
#![allow(dead_code)]

pub mod archive;
pub mod catalog;
pub mod download;
pub mod glb;
pub mod md5;
pub mod ply;
pub mod polyhaven;
pub mod scans;

use catalog::Config;
use polyhaven::Unit;
use scans::{Model, MODELS, SOURCES};
use std::{
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime},
};

pub const LICENCE_NOTICE: &str = "The VG museum scans are from the Stanford 3D Scanning Repository \
(Stanford Computer Graphics Laboratory). They are free for research with credit and may NOT be used \
commercially or included in a product for sale. See assets/vg_stress/CREDITS.md.";

pub type Progress<'a> = &'a (dyn Fn(&str) + Sync);

fn glb_path(assets: &Path, m: &Model) -> PathBuf {
    assets.join("meshes").join(format!("{}.glb", m.name))
}

fn manifest_path(assets: &Path) -> PathBuf {
    assets.join("meshes").join("prepared.json")
}

fn read_manifest(assets: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(manifest_path(assets))
        .ok()
        .and_then(|s| serde_json_lite::parse_map(&s))
        .unwrap_or_default()
}

/// Tiny `{"k":"v",...}` reader/writer so this module needs no JSON crate
/// (it is compiled into build.rs too).
mod serde_json_lite {
    use std::collections::BTreeMap;

    pub fn write_map(m: &BTreeMap<String, String>) -> String {
        let body: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("  \"{}\": \"{}\"", k, v))
            .collect();
        format!("{{\n{}\n}}\n", body.join(",\n"))
    }

    pub fn parse_map(s: &str) -> Option<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for line in s.lines() {
            let parts: Vec<&str> = line.trim().trim_end_matches(',').split('"').collect();
            if parts.len() == 5 {
                out.insert(parts[1].to_string(), parts[3].to_string());
            }
        }
        Some(out)
    }
}

/// Models whose GLB is missing or was made by an older recipe.
pub fn missing_models(assets: &Path) -> Vec<&'static Model> {
    let manifest = read_manifest(assets);
    MODELS
        .iter()
        .filter(|m| {
            !glb_path(assets, m).is_file()
                || manifest.get(m.name).map(String::as_str) != Some(m.recipe().as_str())
        })
        .collect()
}

/// Poly Haven units (marble sets, selected trees) not yet on disk by the
/// current recipe.
pub fn missing_units(assets: &Path, config: &Config) -> Vec<Unit> {
    let prepared = read_manifest(assets);
    polyhaven::units(config)
        .into_iter()
        .filter(|u| {
            !(polyhaven::unit_complete(assets, u, config.fetch_displacement)
                && prepared.get(&u.key()).map(String::as_str) == Some(u.recipe().as_str()))
        })
        .collect()
}

pub fn is_provisioned(assets: &Path) -> bool {
    is_provisioned_with(assets, &Config::from_env_and_args())
}

pub fn is_provisioned_with(assets: &Path, config: &Config) -> bool {
    missing_models(assets).is_empty() && missing_units(assets, config).is_empty()
}

fn record_units(assets: &Path, units: &[(String, String)]) {
    let mut map = read_manifest(assets);
    for (k, v) in units {
        map.insert(k.clone(), v.clone());
    }
    let _ = std::fs::create_dir_all(assets.join("meshes"));
    let _ = std::fs::write(manifest_path(assets), serde_json_lite::write_map(&map));
}

struct Lock(PathBuf);

impl Lock {
    /// Another process (build script or the running demo) may be provisioning
    /// already: wait for it, stealing the lock if it stops making progress.
    fn acquire(path: PathBuf, progress: Progress) -> Result<Lock, String> {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut announced = false;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Lock(path)),
                Err(_) => {
                    let age = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .unwrap_or(Duration::ZERO);
                    if age > Duration::from_secs(120) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if !announced {
                        progress("another process is preparing the scans; waiting");
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    fn touch(&self) {
        let _ = std::fs::write(&self.0, b"busy");
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn convert(assets: &Path, model: &Model, progress: Progress) -> Result<(), String> {
    let archive = assets.join("source").join(model.archive);
    progress(&format!("{}: reading {}", model.name, model.archive));
    let stream = archive::open_gz(&archive, model.member)?;
    let mut mesh = ply::read_ply(stream).map_err(|e| format!("{}: {e}", model.name))?;
    progress(&format!(
        "{}: {} vertices, {} triangles; normalising",
        model.name,
        mesh.positions.len(),
        mesh.indices.len() / 3
    ));
    scans::normalise(&mut mesh.positions, model);
    let normals = scans::smooth_normals(&mesh.positions, &mesh.indices);
    let out = glb_path(assets, model);
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("glb.tmp");
    {
        let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        glb::write_glb(
            &mut w,
            model.name,
            &format!("{}_stone", model.name),
            &mesh.positions,
            &normals,
            &mesh.indices,
        )?;
        std::io::Write::flush(&mut w).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    progress(&format!("{}: wrote {}", model.name, out.display()));
    Ok(())
}

/// Download whatever is missing and write every missing GLB. Safe to call
/// repeatedly and concurrently; returns quickly when everything is present.
pub fn provision(assets: &Path, progress: Progress) -> Result<(), String> {
    provision_with(assets, &Config::from_env_and_args(), progress)
}

pub fn provision_with(assets: &Path, config: &Config, progress: Progress) -> Result<(), String> {
    if is_provisioned_with(assets, config) {
        return Ok(());
    }
    let lock = Lock::acquire(assets.join("source").join(".provision.lock"), progress)?;
    // Another process may have finished while we waited.
    let todo = missing_models(assets);
    let todo_units = missing_units(assets, config);
    if todo.is_empty() && todo_units.is_empty() {
        return Ok(());
    }
    let mut errors = Vec::new();
    if !todo.is_empty() {
        errors.extend(provision_scans(assets, todo, progress, &lock));
    }
    if !todo_units.is_empty() {
        errors.extend(provision_polyhaven(
            assets,
            config,
            &polyhaven::Api::production(),
            &todo_units,
            progress,
            &lock,
        ));
    }
    if errors.is_empty() {
        progress("assets ready");
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub const POLYHAVEN_NOTICE: &str = "The museum trees and marble are CC0 assets from Poly Haven (https://polyhaven.com, CC0 1.0). Credits are written to assets/vg_stress/CREDITS_POLYHAVEN.md.";

fn provision_polyhaven(
    assets: &Path,
    config: &Config,
    api: &polyhaven::Api,
    todo: &[Unit],
    progress: Progress,
    lock: &Lock,
) -> Vec<String> {
    progress(POLYHAVEN_NOTICE);
    let throttled = Throttle::new(progress, lock);
    let mut errors = Vec::new();
    let mut done = Vec::new();
    for unit in todo {
        match polyhaven::provision_unit(
            api,
            assets,
            unit,
            config.fetch_displacement,
            Duration::from_secs(2),
            &|key, m| throttled.say(key, m),
        ) {
            Ok(_) => done.push((unit.key(), unit.recipe())),
            // One failed asset must not stop the rest; the scene falls back to
            // plain colours for whatever is missing.
            Err(e) => errors.push(format!("{}: {e}", unit.id)),
        }
    }
    record_units(assets, &done);
    if let Err(e) = polyhaven::write_credits(assets, config) {
        errors.push(format!("credits: {e}"));
    }
    errors
}

/// Stanford scans: download, then convert to GLB. Returns the errors.
fn provision_scans(
    assets: &Path,
    mut todo: Vec<&'static Model>,
    progress: Progress,
    lock: &Lock,
) -> Vec<String> {
    progress(LICENCE_NOTICE);
    let source_dir = assets.join("source");
    let throttled = Throttle::new(progress, lock);

    // Downloads (parallel; the bandwidth-bound stage).
    let needed: Vec<_> = SOURCES
        .iter()
        .filter(|s| todo.iter().any(|m| m.archive == s.file))
        .collect();
    let errors = Mutex::new(Vec::<String>::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..needed.len().min(3) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(src) = needed.get(i) else { break };
                if let Err(e) = download::fetch(
                    src.urls,
                    &source_dir.join(src.file),
                    src.size,
                    src.sha256,
                    Duration::from_secs(2),
                    &|m| throttled.say(src.file, m),
                ) {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });

    // Conversions: two workers, largest archive first, bounds peak memory to
    // roughly Lucy plus one mid-sized scan.
    todo.retain(|m| {
        SOURCES
            .iter()
            .any(|s| s.file == m.archive && source_dir.join(s.file).is_file())
            && source_dir.join(m.archive).metadata().is_ok()
            && errors
                .lock()
                .unwrap()
                .iter()
                .all(|e| !e.contains(m.archive))
    });
    todo.sort_by_key(|m| {
        std::cmp::Reverse(
            source_dir
                .join(m.archive)
                .metadata()
                .map(|x| x.len())
                .unwrap_or(0),
        )
    });
    let next = AtomicUsize::new(0);
    let done = Mutex::new(read_manifest(assets));
    std::thread::scope(|scope| {
        for _ in 0..todo.len().min(2) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(model) = todo.get(i) else { break };
                match convert(assets, model, &|m| throttled.say(model.name, m)) {
                    Ok(()) => {
                        let mut d = done.lock().unwrap();
                        d.insert(model.name.to_string(), model.recipe());
                        let _ =
                            std::fs::write(manifest_path(assets), serde_json_lite::write_map(&d));
                    }
                    Err(e) => errors.lock().unwrap().push(e),
                }
            });
        }
    });
    // The decompressed PLYs of the old pipeline are never needed again.
    let _ = std::fs::remove_dir_all(source_dir.join("_extracted"));

    let errors = errors.into_inner().unwrap();
    if errors.is_empty() {
        progress("scan meshes ready");
    }
    errors
}

/// Rate-limits progress messages per key and keeps the lock fresh.
struct Throttle<'a> {
    progress: Progress<'a>,
    lock: &'a Lock,
    last: Mutex<std::collections::HashMap<String, Instant>>,
}

impl<'a> Throttle<'a> {
    fn new(progress: Progress<'a>, lock: &'a Lock) -> Self {
        Self {
            progress,
            lock,
            last: Mutex::new(Default::default()),
        }
    }

    fn say(&self, key: &str, message: &str) {
        self.lock.touch();
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        let fresh = last
            .get(key)
            .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs(3));
        if fresh || !message.starts_with("downloading") {
            last.insert(key.to_string(), now);
            drop(last);
            (self.progress)(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_and_detects_stale_recipes() {
        let dir = std::env::temp_dir().join(format!("vgprep-mod-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        assert_eq!(missing_models(&dir).len(), MODELS.len());
        let mut m = std::collections::BTreeMap::new();
        for model in MODELS {
            std::fs::write(glb_path(&dir, model), b"x").unwrap();
            m.insert(model.name.to_string(), model.recipe());
        }
        std::fs::write(manifest_path(&dir), serde_json_lite::write_map(&m)).unwrap();
        assert!(missing_models(&dir).is_empty());
        m.insert("lucy".into(), "old".into());
        std::fs::write(manifest_path(&dir), serde_json_lite::write_map(&m)).unwrap();
        assert_eq!(missing_models(&dir).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn end_to_end_from_tiny_fixtures_without_network() {
        // Fixture archives with correct size/hash cannot match the real table,
        // so exercise convert() directly on a generated tar.gz.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("vgprep-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("source")).unwrap();
        let mut ply = b"ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nelement face 2\nproperty list uchar int vertex_indices\nend_header\n".to_vec();
        ply.extend_from_slice(b"0 0 0\n1 0 0\n1 4 0\n0 4 0\n3 0 1 2\n3 0 2 3\n");
        let tar = {
            let mut t = archive::tests::tar_entry("dragon_recon/dragon_vrip.ply", b'0', &ply);
            t.extend([0u8; 1024]);
            t
        };
        std::fs::File::create(dir.join("source/dragon_recon.tar.gz"))
            .unwrap()
            .write_all(&archive::tests::gzip(&tar))
            .unwrap();
        let model = MODELS.iter().find(|m| m.name == "dragon").unwrap();
        convert(&dir, model, &|_| {}).unwrap();
        let glb = std::fs::read(glb_path(&dir, model)).unwrap();
        assert_eq!(&glb[..4], b"glTF");
        assert!(glb.len() > 100);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
