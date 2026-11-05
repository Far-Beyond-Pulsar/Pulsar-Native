//! A static mesh's slot materials: the material assets its slots name, as
//! the world binds them, and the material each section draws with.
//!
//! A slot names a material asset by path (its own assignment, or the mesh
//! asset's default for the slot). [`StaticMeshComponent`]'s handle
//! collector declares one [`AssetId`] of the materials' kind
//! ([`helio_mats::asset::MATERIAL_ASSET_KIND`]) per slot material, so the
//! world counts the references however the value was written, loads each
//! with the materials plugin's loader and binds the loaded
//! [`MaterialPayload`] into the component's [`BoundMaterials`]. The draw
//! derived for the mesh asks the bound material for its row
//! ([`helio_mats::asset::MaterialAsset::resolve`]); it never reads a file
//! or links the materials plugin. Without the plugin (or while a material
//! is loading or failed) a slot draws the mesh's imported surface.

use std::collections::HashSet;
use std::sync::Arc;

use helio_mats::asset::{MATERIAL_ASSET_KIND, MaterialAsset, MaterialPayload, ResolvedMaterial};
use pulsar_scenedb::{AssetId, AssetView};

use crate::components::{StaticMeshComponent, StaticMeshMaterialSlot};

/// The materials a mesh's slots are bound to, as the world loaded them, each
/// with its content version. Never authored or saved.
#[derive(Clone, Default)]
pub struct BoundMaterials(Vec<(AssetId, u64, Arc<dyn MaterialAsset>)>);

impl BoundMaterials {
    /// The bound material `id`, with its content version.
    pub fn get(&self, id: AssetId) -> Option<(u64, &Arc<dyn MaterialAsset>)> {
        if id.is_zero() {
            return None;
        }
        self.0
            .iter()
            .find(|(bound, _, _)| *bound == id)
            .map(|(_, version, material)| (*version, material))
    }
}

impl std::fmt::Debug for BoundMaterials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(id, version, _)| (id.0, version)))
            .finish()
    }
}

/// Registers how a [`StaticMeshComponent`] takes in a loaded material.
/// Idempotent; done on the first [`slot_material_asset_id`].
fn register_mesh_material_binding() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_binding::<StaticMeshComponent>(
            MATERIAL_ASSET_KIND,
            bind_mesh_material,
        );
    });
}

/// The asset id of the material `path` names, without declaring it.
pub fn material_asset_id_of(path: &str) -> AssetId {
    crate::asset_component::asset_source(path).map_or(AssetId::ZERO, |source| {
        AssetId::of(MATERIAL_ASSET_KIND, &source)
    })
}

/// The asset id of the material `path` names, declared so the world can
/// load it. [`AssetId::ZERO`] for an empty path.
pub fn slot_material_asset_id(path: &str) -> AssetId {
    let Some(source) = crate::asset_component::asset_source(path) else {
        return AssetId::ZERO;
    };
    register_mesh_material_binding();
    crate::asset_component::declared_asset_id(MATERIAL_ASSET_KIND, &source)
}

/// The material a slot draws with, as an asset: none for an explicit
/// surface override (which ignores any material) or an empty assignment.
pub fn slot_material_id(slot: &StaticMeshMaterialSlot) -> AssetId {
    if slot.surface_override.is_some() {
        return AssetId::ZERO;
    }
    material_asset_id_of(slot.effective_material_asset())
}

/// `StaticMeshComponent`'s handle collector's material half: one declared
/// id per slot that names a material.
pub(crate) fn collect_mesh_materials(
    mesh: &StaticMeshComponent,
    out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>,
) {
    for slot in &mesh.material_slots.slots {
        if slot.surface_override.is_none() {
            let id = slot_material_asset_id(slot.effective_material_asset());
            if !id.is_zero() {
                out.push(id.handle());
            }
        }
    }
}

/// How a mesh takes in a loaded (or failed) material: kept for the slots
/// naming it, and only the materials its slots still name are kept.
fn bind_mesh_material(mesh: &mut StaticMeshComponent, view: &AssetView) {
    let named: HashSet<AssetId> = mesh
        .material_slots
        .slots
        .iter()
        .map(slot_material_id)
        .filter(|id| !id.is_zero())
        .collect();
    let bound = &mut mesh.materials.0;
    bound.retain(|(id, _, _)| named.contains(id) && *id != view.id);
    if !named.contains(&view.id) {
        return;
    }
    match view.payload::<MaterialPayload>() {
        Some(material) => {
            // What the material reads that a cache dropped (a graph's
            // textures) starts coming back now, ahead of the scene that
            // will upload it.
            material.0.warm();
            bound.push((view.id, view.version, material.0.clone()))
        }
        None => tracing::warn!(
            source = view.source,
            state = ?view.state,
            "static mesh material could not be loaded; its slots use the imported material"
        ),
    }
}

/// The material a slot draws with: an explicit surface override, the bound
/// material (`bound`, the `version` of asset `id`) resolved in `mirror`'s
/// scene, or the mesh's imported surface while it has none (unloaded,
/// failed, nothing assigned, or a material that needs a scene drawn
/// without one).
pub fn resolve_material(
    slot: Option<&StaticMeshMaterialSlot>,
    bound: Option<(AssetId, u64, &dyn MaterialAsset)>,
    mirror: Option<&pulsar_scenedb::gpu::GpuMirrorHandle>,
) -> ResolvedMaterial {
    let Some(slot) = slot else {
        return ResolvedMaterial::surface(&Default::default());
    };
    if let Some(surface) = slot.surface_override {
        return ResolvedMaterial::surface(&surface);
    }
    let imported = slot.imported_surface;
    match bound {
        Some((id, version, material)) => material
            .resolve(id, version, mirror, &imported)
            .unwrap_or_else(|error| {
                if mirror.is_some() {
                    tracing::warn!(%error, "could not resolve a slot material; using the imported material");
                }
                ResolvedMaterial::surface(&imported)
            }),
        None => ResolvedMaterial::surface(&imported),
    }
}
