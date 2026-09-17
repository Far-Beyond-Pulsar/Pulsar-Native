//! Durable storage for authored voxel terrain, beside the `.level` file.
//!
//! The design doc's split is "`TerrainStore` for voxel data, `.level` for
//! objects" (§5.5). The live `PlanetTerrainRuntime` is built without a
//! persistence handle, so nothing was writing voxel data anywhere at all —
//! a sculpt survived only as long as the editor process did.
//!
//! This is the smallest thing that honours that split: a sidecar file next to
//! the level holding each planet's canonical **mutation log**, base64-encoded.
//! The log is the authored content — a planet's surface is its deterministic
//! generator with the log replayed over it — so storing it stores the sculpt
//! exactly, and replaying it into a freshly generated planet reproduces it.
//!
//! Keeping this out of `LevelFile` is deliberate:
//!
//! * the level file stays byte-identical for scenes with no terrain edits,
//!   so nothing about existing levels or the loader changes;
//! * voxel data is bulk binary and does not belong in a hand-editable,
//!   diff-reviewed JSON document;
//! * when a real `TerrainStore`-backed persistence handle is wired into
//!   `PlanetTerrainRuntime`, this file is the only thing that has to be
//!   retired, and [`sidecar_path`] is the only place that knows where it is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use engine_backend::services::terrain_edit::{TerrainEditApi, TerrainTarget};
use engine_fs::virtual_fs;

/// Extension appended to the level's own file name.
const SIDECAR_EXTENSION: &str = "terrain";

/// On-disk shape: planet id (hex) → base64 of the encoded edit log.
type SidecarMap = BTreeMap<String, String>;

/// Where a level's terrain sidecar lives.
///
/// `foo.level` → `foo.level.terrain`. The extension is appended rather than
/// replaced so two levels that differ only by extension cannot collide.
pub fn sidecar_path(level_path: &Path) -> PathBuf {
    let mut name = level_path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(SIDECAR_EXTENSION);
    level_path.with_file_name(name)
}

/// Write every registered planet's edit log beside `level_path`.
///
/// A planet whose log is empty contributes nothing, and a level with no
/// authored terrain leaves no sidecar behind — an existing one is removed so
/// a fully-undone sculpt does not resurrect itself on the next load.
pub fn save(level_path: &Path, api: &TerrainEditApi) -> Result<(), String> {
    let mut stored = SidecarMap::new();
    for definition in api.planets() {
        let target = TerrainTarget::Planet(definition.planet_id);
        let Some(encoded) = api.export_edits(target) else {
            continue;
        };
        // An edit log with no operations still encodes to a valid header;
        // skip those so an untouched planet writes nothing.
        if !has_operations(&encoded) {
            continue;
        }
        stored.insert(
            definition.planet_id.to_hex(),
            base64::engine::general_purpose::STANDARD.encode(&encoded),
        );
    }

    let path = sidecar_path(level_path);
    if stored.is_empty() {
        // Best-effort: a missing sidecar is the same as an empty one, so a
        // failed delete is not worth failing the level save over.
        if virtual_fs::exists(&path).unwrap_or(false) {
            let _ = virtual_fs::delete_path(&path);
        }
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("Failed to serialize terrain edits: {error}"))?;
    virtual_fs::write_file(&path, json.as_bytes())
        .map_err(|error| format!("Failed to write terrain sidecar: {error}"))?;
    tracing::info!(
        planets = stored.len(),
        "Terrain edits saved to {}",
        path.display()
    );
    Ok(())
}

/// Replay a level's stored terrain edits into the runtime.
///
/// Planets in the sidecar that are not registered are skipped rather than
/// treated as an error: the scene may have dropped the component, or the
/// planets may not have been synced to the runtime yet. Returns the number of
/// operations replayed.
pub fn load(level_path: &Path, api: &TerrainEditApi) -> usize {
    let path = sidecar_path(level_path);
    let Ok(bytes) = virtual_fs::read_file(&path) else {
        return 0;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        tracing::warn!("terrain sidecar {} is not valid UTF-8", path.display());
        return 0;
    };
    let Ok(stored) = serde_json::from_str::<SidecarMap>(&text) else {
        tracing::warn!("terrain sidecar {} could not be parsed", path.display());
        return 0;
    };

    let registered: Vec<_> = api
        .planets()
        .into_iter()
        .map(|definition| definition.planet_id)
        .collect();
    let mut replayed = 0;
    for (planet_hex, encoded) in stored {
        let Some(planet_id) = registered
            .iter()
            .copied()
            .find(|id| id.to_hex() == planet_hex)
        else {
            tracing::debug!(planet = %planet_hex, "skipping stored terrain for an unregistered planet");
            continue;
        };
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes())
        else {
            tracing::warn!(planet = %planet_hex, "stored terrain edits were not valid base64");
            continue;
        };
        match api.import_edits(TerrainTarget::Planet(planet_id), &decoded) {
            Ok(count) => replayed += count,
            Err(error) => {
                tracing::error!(planet = %planet_hex, %error, "failed to replay stored terrain edits")
            }
        }
    }
    if replayed > 0 {
        tracing::info!(operations = replayed, "Terrain edits restored");
    }
    replayed
}

/// Whether an encoded edit log carries at least one operation.
///
/// The canonical encoding is an 8-byte magic followed by a little-endian u32
/// operation count, so this reads the count without a full decode.
fn has_operations(encoded: &[u8]) -> bool {
    encoded
        .get(8..12)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .is_some_and(|bytes| u32::from_le_bytes(bytes) > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::services::terrain_edit::{
        EditMode, EditOp, EditShape, PlanetId, TerrainEditApi,
    };

    #[test]
    fn the_sidecar_sits_next_to_the_level_without_shadowing_it() {
        let path = sidecar_path(Path::new("/project/levels/main.level"));
        assert_eq!(
            path,
            PathBuf::from("/project/levels/main.level.terrain"),
            "the extension must be appended, not replaced"
        );
    }

    #[test]
    fn levels_that_differ_only_by_extension_get_distinct_sidecars() {
        assert_ne!(
            sidecar_path(Path::new("a.level")),
            sidecar_path(Path::new("a.json"))
        );
    }

    #[test]
    fn an_empty_edit_log_is_recognised_as_having_nothing_to_store() {
        let empty = pulsar_terrain_edit_log(&[]);
        assert!(!has_operations(&empty));
        let populated = pulsar_terrain_edit_log(&[EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [0; 3],
                radius_cells: 4,
            },
            mode: EditMode::Union,
            material: 1,
        }]);
        assert!(has_operations(&populated));
    }

    #[test]
    fn a_truncated_log_is_treated_as_empty_rather_than_panicking() {
        assert!(!has_operations(&[]));
        assert!(!has_operations(b"PTEDIT0"));
    }

    #[test]
    fn loading_without_a_runtime_replays_nothing_and_does_not_panic() {
        let api = TerrainEditApi::default();
        assert_eq!(load(Path::new("/definitely/missing.level"), &api), 0);
    }

    #[test]
    fn exporting_an_unregistered_planet_yields_nothing_to_store() {
        let api = TerrainEditApi::default();
        assert!(api
            .export_edits(TerrainTarget::Planet(PlanetId::from_stable_name("nope")))
            .is_none());
    }

    /// Build an encoded edit log the same way the runtime does, so the header
    /// probe above is tested against the real format.
    fn pulsar_terrain_edit_log(operations: &[EditOp]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PTEDIT01");
        bytes.extend_from_slice(&(operations.len() as u32).to_le_bytes());
        bytes
    }
}
