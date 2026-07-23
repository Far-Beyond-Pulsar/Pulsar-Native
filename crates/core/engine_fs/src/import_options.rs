//! Per-project import-options persistence (issue #409).
//!
//! Stores the options a user chose in an asset import configurator so that
//! re-loading / reimporting the same source model reuses them. Layout:
//! `<project>/.pulsar/import_options.json` — a JSON object keyed by the asset's
//! path **relative to the project root** (forward-slashed).
//!
//! Values are opaque `serde_json::Value`, keeping this module decoupled from any
//! particular options type (the configurator's `OptionValues` serialises to/from
//! JSON). All I/O goes through [`crate::virtual_fs`] so it works for local and
//! remote/cloud projects alike.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::virtual_fs;

const DIR: &str = ".pulsar";
const FILE: &str = "import_options.json";

fn store_path(project_root: &Path) -> PathBuf {
    project_root.join(DIR).join(FILE)
}

/// Normalise an asset key: the asset path relative to the project root, using
/// forward slashes. Falls back to the full path if it isn't under the root.
pub fn asset_key(project_root: &Path, asset_path: &Path) -> String {
    let rel = asset_path.strip_prefix(project_root).unwrap_or(asset_path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Read the whole import-options map. Returns an empty map if the file is
/// missing or unparseable (persistence is best-effort — never fail a load).
pub fn read_all(project_root: &Path) -> Map<String, Value> {
    let path = store_path(project_root);
    match virtual_fs::exists(&path) {
        Ok(true) => virtual_fs::read_file(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok())
            .unwrap_or_default(),
        _ => Map::new(),
    }
}

/// Look up the stored options for one asset by its relative [`asset_key`].
pub fn get(project_root: &Path, key: &str) -> Option<Value> {
    read_all(project_root).get(key).cloned()
}

/// Store options for one asset (read-modify-write). Creates `.pulsar/` if needed.
pub fn set(project_root: &Path, key: &str, value: Value) -> Result<()> {
    let mut map = read_all(project_root);
    map.insert(key.to_string(), value);
    virtual_fs::create_dir_all(&project_root.join(DIR)).context("create .pulsar dir")?;
    let bytes = serde_json::to_vec_pretty(&map).context("serialize import options")?;
    virtual_fs::write_file(&store_path(project_root), &bytes).context("write import options")
}

/// Remove stored options for one asset. No-op if absent.
pub fn remove(project_root: &Path, key: &str) -> Result<()> {
    let mut map = read_all(project_root);
    if map.remove(key).is_some() {
        let bytes = serde_json::to_vec_pretty(&map).context("serialize import options")?;
        virtual_fs::write_file(&store_path(project_root), &bytes).context("write import options")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_key_is_relative_and_forward_slashed() {
        let root = Path::new("/proj");
        assert_eq!(
            asset_key(root, Path::new("/proj/models/a.fbx")),
            "models/a.fbx"
        );
        // Paths outside the project root are kept whole.
        assert_eq!(asset_key(root, Path::new("/other/a.fbx")), "/other/a.fbx");
    }

    #[test]
    fn set_get_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pulsar-import-opts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let key = "models/a.fbx";
        let val = serde_json::json!({ "flip_uv_v": true, "import_scale": 0.01 });

        assert_eq!(get(&dir, key), None);
        set(&dir, key, val.clone()).unwrap();
        assert_eq!(get(&dir, key), Some(val));

        // A second asset coexists.
        set(&dir, "models/b.obj", serde_json::json!({ "triangulate": true })).unwrap();
        assert!(get(&dir, "models/b.obj").is_some());

        remove(&dir, key).unwrap();
        assert_eq!(get(&dir, key), None);
        assert!(get(&dir, "models/b.obj").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
