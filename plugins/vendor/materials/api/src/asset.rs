//! Material assets as other kinds see them: the asset kind, the scalar
//! surface values and their row, the scalar surface document, and the
//! interface a loaded material asset offers whoever draws with it.
//!
//! The materials plugin (`materials_component`) loads material assets (a
//! scalar surface document, or a shader-graph folder it compiles) and
//! registers the kind's SceneDB loader; its payload is a
//! [`MaterialPayload`]. A kind that draws with materials (a mesh's slots)
//! names material assets by id ([`MATERIAL_ASSET_KIND`]), binds the loaded
//! [`MaterialPayload`] and asks it for the row ([`MaterialAsset::resolve`]);
//! it never links the plugin. Without the plugin no loader serves the kind,
//! the asset fails to load, and the drawer keeps its own surface (a mesh's
//! imported one), as the stand-in row is the table's absence.
//!
//! As Unreal's mesh components hold `UMaterialInterface`s (the material
//! module's interface, its render proxy) and never `UMaterial`'s compiler.

use std::sync::Arc;

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb::AssetId;
use serde::{Deserialize, Serialize};

use crate::{GpuMaterial, FLAG_ALPHA_BLEND, FLAG_TRANSPARENT_ONLY};

/// The SceneDB asset kind of material assets.
pub const MATERIAL_ASSET_KIND: &str = "material";

/// The file a shader-graph material folder saves its graph in: a folder
/// holding it is a material asset.
pub const SHADER_GRAPH_FILE: &str = "shader_graph_save.json";

/// Scalar PBR surface values: a surface document's, or the surface a model
/// file's import carries (a mesh's imported surface).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceValues {
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: [f32; 3],
    pub emissive_intensity: f32,
    pub alpha: f32,
}

impl Default for SurfaceValues {
    fn default() -> Self {
        Self {
            base_color: [0.22, 0.15, 0.08, 1.0],
            roughness: 0.7,
            metallic: 0.0,
            emissive: [0.0; 3],
            emissive_intensity: 0.0,
            alpha: 1.0,
        }
    }
}

impl SurfaceValues {
    /// The values as a material row, sampling `texture_slots` (base colour,
    /// normal, roughness, emissive, occlusion; [`GpuMaterial::NO_TEXTURE`]
    /// for none). `alpha < 1` selects the blended, transparent-only path
    /// (glass).
    pub fn row(&self, texture_slots: [u32; 5]) -> GpuMaterial {
        let alpha = self.alpha.clamp(0.0, 1.0);
        let mut flags = 0;
        if alpha < 1.0 {
            flags |= FLAG_ALPHA_BLEND | FLAG_TRANSPARENT_ONLY;
        }
        GpuMaterial {
            base_color: [
                self.base_color[0],
                self.base_color[1],
                self.base_color[2],
                alpha,
            ],
            emissive: [
                self.emissive[0],
                self.emissive[1],
                self.emissive[2],
                self.emissive_intensity,
            ],
            roughness_metallic: [self.roughness, self.metallic, 1.5, 0.5],
            tex_base_color: texture_slots[0],
            tex_normal: texture_slots[1],
            tex_roughness: texture_slots[2],
            tex_emissive: texture_slots[3],
            tex_occlusion: texture_slots[4],
            workflow: 0,
            flags,
            material_class: 0,
            class_params: [0.0; 4],
        }
    }
}

/// A scalar surface material document (a `.mat` file): the material asset
/// that is not a graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceMaterialDocument {
    pub version: u32,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_color: [f32; 3],
    pub emissive_intensity: f32,
    pub alpha: f32,
}

impl SurfaceMaterialDocument {
    /// The document version this crate reads and writes.
    pub const VERSION: u32 = 1;

    /// The document's surface values.
    pub fn values(&self) -> SurfaceValues {
        SurfaceValues {
            base_color: self.base_color,
            roughness: self.roughness,
            metallic: self.metallic,
            emissive: self.emissive_color,
            emissive_intensity: self.emissive_intensity,
            alpha: self.alpha,
        }
    }
}

impl Default for SurfaceMaterialDocument {
    fn default() -> Self {
        SurfaceValues::default().into()
    }
}

impl From<SurfaceValues> for SurfaceMaterialDocument {
    fn from(value: SurfaceValues) -> Self {
        Self {
            version: Self::VERSION,
            base_color: value.base_color,
            metallic: value.metallic,
            roughness: value.roughness,
            emissive_color: value.emissive,
            emissive_intensity: value.emissive_intensity,
            alpha: value.alpha,
        }
    }
}

/// A material as one draw uses it: its row, its shading class and, for a
/// compiled graph, the graph's registered source
/// ([`crate::register_graph_source`]; 0 for none).
#[derive(Clone, Copy, Debug)]
pub struct ResolvedMaterial {
    pub row: GpuMaterial,
    pub material_class: u32,
    pub graph_hash: u64,
}

impl ResolvedMaterial {
    /// `surface` drawn as the default class.
    pub fn surface(surface: &SurfaceValues) -> Self {
        Self {
            row: surface.row([GpuMaterial::NO_TEXTURE; 5]),
            material_class: crate::MATERIAL_CLASS_DEFAULT,
            graph_hash: 0,
        }
    }
}

/// What a loaded material asset offers whoever draws with it.
pub trait MaterialAsset: Send + Sync {
    /// The material for a draw in the scene `mirror` holds (its textures
    /// take slots in that scene's texture store), as asset `id` at content
    /// `version`. `fallback` is the drawer's own surface, for what the
    /// material leaves to it (a graph's scalar inputs). Without a mirror a
    /// material that needs one returns an error, and the drawer keeps its
    /// surface.
    fn resolve(
        &self,
        id: AssetId,
        version: u64,
        mirror: Option<&GpuMirrorHandle>,
        fallback: &SurfaceValues,
    ) -> Result<ResolvedMaterial, String>;

    /// Whether the material reads the frame clock (its surface animates).
    fn reads_time(&self) -> bool {
        false
    }

    /// Start bringing back what the material reads that a cache dropped
    /// (its textures), ahead of the scene that will upload it.
    fn warm(&self) {}

    /// Bytes the loaded material holds outside SceneDB's residency cache,
    /// for the cache budget.
    fn held_bytes(&self) -> usize {
        0
    }
}

/// The payload of a [`MATERIAL_ASSET_KIND`] asset, as the plugin's loader
/// produces it ([`pulsar_scenedb::AssetView::payload`]).
#[derive(Clone)]
pub struct MaterialPayload(pub Arc<dyn MaterialAsset>);

impl std::fmt::Debug for MaterialPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MaterialPayload")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_translucent_surface_is_blended_and_transparent_only() {
        let glass = SurfaceValues {
            alpha: 0.4,
            ..Default::default()
        };
        let row = glass.row([GpuMaterial::NO_TEXTURE; 5]);
        assert_eq!(row.base_color[3], 0.4);
        assert_eq!(row.flags, FLAG_ALPHA_BLEND | FLAG_TRANSPARENT_ONLY);
        assert_eq!(SurfaceValues::default().row([0; 5]).flags, 0);
    }

    #[test]
    fn a_document_round_trips_its_values() {
        let values = SurfaceValues {
            base_color: [0.1, 0.2, 0.3, 1.0],
            roughness: 0.25,
            metallic: 1.0,
            emissive: [1.0, 0.5, 0.0],
            emissive_intensity: 3.0,
            alpha: 1.0,
        };
        let document = SurfaceMaterialDocument::from(values);
        assert_eq!(document.version, 1);
        assert_eq!(document.values(), values);
    }
}
