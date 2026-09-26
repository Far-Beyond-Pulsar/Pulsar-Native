//! The asset registry: `Pulsar/asset_registry.json`.
//!
//! One record per asset a packaged game ships (meshes, textures,
//! materials, ...), keyed by its content-relative path, which is the id
//! cooked levels and prefabs refer to. A record says where the asset came
//! from in the project, its size and content hash, and, when it is in
//! `game.pak`, its offset and length there.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The registry file, content-relative.
pub const ASSET_REGISTRY_FILE: &str = "Pulsar/asset_registry.json";

/// What an asset is, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Mesh,
    Texture,
    Material,
    Audio,
    Other,
}

impl AssetKind {
    /// The kind of a file by its extension.
    pub fn from_path(path: &str) -> Self {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "mesh" | "fbx" | "gltf" | "glb" | "obj" | "ply" | "stl" => Self::Mesh,
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "gif" | "webp" | "hdr" | "exr" | "ktx2" | "dds" | "tex" => {
                Self::Texture
            }
            "mat" | "material" | "pmat" => Self::Material,
            "wav" | "ogg" | "mp3" | "flac" => Self::Audio,
            _ => Self::Other,
        }
    }

    /// Whether a string value that names a file of this kind looks like an
    /// asset reference.
    pub fn is_asset_path(value: &str) -> bool {
        Self::from_path(value) != Self::Other
    }
}

/// Where an asset's bytes are in `game.pak`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PakLocation {
    pub offset: u64,
    pub len: u64,
}

/// One shipped asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Content-relative path: the id cooked data refers to.
    pub id: String,
    pub kind: AssetKind,
    /// Where it came from, relative to the project (or the engine's
    /// built-in assets), for diagnostics.
    pub source: String,
    pub size: u64,
    /// XXH3-128 of the contents, hex.
    pub hash: String,
    /// Its place in `game.pak`; `None` when shipped as a loose file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pak: Option<PakLocation>,
}

/// Every shipped asset, by id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRegistry {
    pub format: u32,
    pub assets: BTreeMap<String, AssetRecord>,
}

impl AssetRegistry {
    pub const FORMAT: u32 = 1;

    pub fn new() -> Self {
        Self { format: Self::FORMAT, assets: BTreeMap::new() }
    }

    pub fn insert(&mut self, record: AssetRecord) {
        self.assets.insert(record.id.clone(), record);
    }

    pub fn get(&self, id: &str) -> Option<&AssetRecord> {
        self.assets.get(&crate::normalize_rel(id)?)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|error| format!("{ASSET_REGISTRY_FILE}: {error}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

/// Hex form of a content hash.
pub fn hash_hex(hash: u128) -> String {
    format!("{hash:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_round_trip() {
        assert_eq!(AssetKind::from_path("meshes/a.MESH"), AssetKind::Mesh);
        assert_eq!(AssetKind::from_path("t/b.png"), AssetKind::Texture);
        assert_eq!(AssetKind::from_path("scenes/x.level"), AssetKind::Other);
        let mut registry = AssetRegistry::new();
        registry.insert(AssetRecord {
            id: "assets/meshes/a.mesh".into(),
            kind: AssetKind::Mesh,
            source: "meshes/a.mesh".into(),
            size: 3,
            hash: hash_hex(7),
            pak: Some(PakLocation { offset: 40, len: 3 }),
        });
        let back = AssetRegistry::from_json(registry.to_json().as_bytes()).unwrap();
        assert_eq!(back, registry);
        assert!(back.get("./assets/meshes/a.mesh").is_some());
    }
}
