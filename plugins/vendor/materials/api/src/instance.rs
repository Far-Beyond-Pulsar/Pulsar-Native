//! Material instances as entities: the scene's material table.
//!
//! A material instance is an entity carrying a [`MaterialInstanceComponent`]:
//! what it draws (a material asset, or an explicit surface) and the surface it
//! falls back to (while the asset loads, when it fails, or with the materials
//! plugin disabled). SceneDB mirrors each instance into two packed rows at
//! the instance entity's row, the [`crate::MATERIALS`] row every drawer reads
//! (`material_at(id)`) and a [`MaterialKeyComponent`] (its shading class and
//! graph hash, which sort and group draws). An instance's id in the table *is*
//! its entity row, so a kind that draws with materials writes that row into
//! its draw rows and reads no material data itself.
//!
//! Kinds acquire instances by what they draw ([`acquire`]): equal sources
//! share one instance (one row however many meshes use a material), and an
//! instance lives while some user holds it ([`release`]). The asset an
//! instance names is declared to the world's handle ledger, loaded by the
//! materials plugin's loader and bound here; the instance's rows are
//! resolved again on every bind, so a reloaded material changes one row.
//!
//! How others do it: Unreal's mesh components reference material instances
//! (assets) and the renderer keeps one uniform buffer per material proxy;
//! Bevy keeps materials as assets and its render world prepares one bind
//! group per asset handle. Ours is Bevy's sharing with SceneDB entities as
//! the handles: the table is the materials' own column, written once per
//! material, and drawers index it by entity row.

use std::collections::HashMap;
use std::sync::Arc;

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb::handle_ledger::HandleId;
use pulsar_scenedb::{AssetId, AssetView, Entity, World};
use pulsar_scenedb_derive::SceneStore;

use crate::asset::{
    MATERIAL_ASSET_KIND, MaterialAsset, MaterialPayload, ResolvedMaterial, SurfaceValues,
};
use crate::host_rows::MaterialComponent;

/// Where the instances' keys live: one [`MaterialKeyComponent`] per
/// instance entity row.
pub const MATERIAL_KEYS: &str = "material_keys";

/// What a material instance draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialSource {
    /// The material asset's SceneDB source (normalised, project-relative),
    /// or empty for none.
    pub asset: String,
    /// An explicit surface, drawn instead of any asset.
    pub surface: Option<SurfaceValues>,
    /// The surface drawn while the asset is not bound (loading, failed, no
    /// materials plugin), and the scalar inputs a graph material leaves to
    /// its user.
    pub fallback: SurfaceValues,
}

impl MaterialSource {
    /// An explicit surface.
    pub fn surface(surface: SurfaceValues) -> Self {
        Self {
            asset: String::new(),
            surface: Some(surface),
            fallback: surface,
        }
    }

    /// The asset this source draws, if it draws one.
    pub fn asset_id(&self) -> AssetId {
        if self.surface.is_some() || self.asset.is_empty() {
            AssetId::ZERO
        } else {
            AssetId::of(MATERIAL_ASSET_KIND, &self.asset)
        }
    }

    /// The identity two equal sources share: asset, surface and fallback,
    /// bit for bit.
    fn key(&self) -> SourceKey {
        let bits = |surface: &SurfaceValues| {
            let row = surface.row([crate::GpuMaterial::NO_TEXTURE; 5]);
            bytemuck::bytes_of(&row).to_vec()
        };
        SourceKey {
            asset: if self.surface.is_some() {
                String::new()
            } else {
                self.asset.clone()
            },
            surface: self.surface.as_ref().map(bits),
            fallback: bits(&self.fallback),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourceKey {
    asset: String,
    surface: Option<Vec<u8>>,
    fallback: Vec<u8>,
}

/// A material instance: its source and, once the world loaded it, the
/// material asset the source names (with its content version). The bound
/// asset is never authored or saved.
#[derive(Clone)]
pub struct MaterialInstanceComponent {
    source: MaterialSource,
    bound: Option<(AssetId, u64, Arc<dyn MaterialAsset>)>,
}

impl std::fmt::Debug for MaterialInstanceComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaterialInstanceComponent")
            .field("source", &self.source)
            .field("bound", &self.bound.as_ref().map(|(id, version, _)| (id.0, version)))
            .finish()
    }
}

impl MaterialInstanceComponent {
    pub fn new(source: MaterialSource) -> Self {
        register_binding();
        Self {
            source,
            bound: None,
        }
    }

    pub fn source(&self) -> &MaterialSource {
        &self.source
    }

    /// The material asset bound into the instance, if any.
    pub fn bound(&self) -> Option<&Arc<dyn MaterialAsset>> {
        self.bound.as_ref().map(|(_, _, material)| material)
    }

    /// Whether the instance's material reads the frame clock (a graph with
    /// a `time` node), so what it draws animates.
    pub fn reads_time(&self) -> bool {
        self.bound().is_some_and(|material| material.reads_time())
    }

    /// What the instance draws in the scene `mirror` holds: the explicit
    /// surface, the bound asset resolved there, or the fallback.
    pub fn resolve(&self, mirror: Option<&GpuMirrorHandle>) -> ResolvedMaterial {
        if let Some(surface) = &self.source.surface {
            return ResolvedMaterial::surface(surface);
        }
        match &self.bound {
            Some((id, version, material)) => material
                .resolve(*id, *version, mirror, &self.source.fallback)
                .unwrap_or_else(|error| {
                    if mirror.is_some() {
                        log::warn!("material '{}' could not be resolved ({error}); drawing its fallback", self.source.asset);
                    }
                    ResolvedMaterial::surface(&self.source.fallback)
                }),
            None => ResolvedMaterial::surface(&self.source.fallback),
        }
    }
}

/// An instance's shading class and graph hash, at its entity row of
/// [`MATERIAL_KEYS`]: the draws using it sort and group on these.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "material_keys")]
pub struct MaterialKeyComponent {
    #[gpu]
    pub material_class: u32,
    #[gpu]
    pub graph_hash_lo: u32,
    #[gpu]
    pub graph_hash_hi: u32,
    /// 1 for a live instance: a zero row is no material.
    #[gpu]
    pub live: u32,
}

/// Register the instances' columns with a GPU mirror (initial capacities;
/// they grow). Whoever acquires instances registers them; registering twice
/// is harmless.
pub fn register_columns(
    store: &mut pulsar_scenedb::gpu::SceneGpuStore,
    device: &Arc<wgpu::Device>,
) {
    MaterialComponent::register_gpu_columns_growable(store, 256, device);
    MaterialKeyComponent::register_gpu_columns_growable(store, 256, device);
}

// ── The world's instances ─────────────────────────────────────────────────

/// The world's instances by source, with how many users hold each. Kept on
/// one entity of the world, so it lives and dies with the world.
#[derive(Default)]
struct MaterialInstances {
    by_source: HashMap<SourceKey, (Entity, u32)>,
    source_of: HashMap<Entity, SourceKey>,
}

fn registry(world: &mut World) -> Entity {
    if let Some((entity, _)) = world.query::<&MaterialInstances>().next() {
        return entity;
    }
    let entity = world.spawn();
    world.insert(entity, MaterialInstances::default());
    entity
}

/// The instance drawing `source`, held once more by the caller: an existing
/// instance of an equal source, or a new entity.
pub fn acquire(world: &mut World, source: &MaterialSource) -> Entity {
    let key = source.key();
    let registry = registry(world);
    let found = {
        let mut instances = world
            .get_mut::<MaterialInstances>(registry)
            .expect("registry entity");
        match instances.by_source.get_mut(&key) {
            Some((entity, users)) => {
                *users += 1;
                Some(*entity)
            }
            None => None,
        }
    };
    if let Some(entity) = found.filter(|entity| world.is_alive(*entity)) {
        return entity;
    }
    let entity = world.spawn();
    world.insert(entity, MaterialInstanceComponent::new(source.clone()));
    let mut instances = world
        .get_mut::<MaterialInstances>(registry)
        .expect("registry entity");
    instances.by_source.insert(key.clone(), (entity, 1));
    instances.source_of.insert(entity, key);
    entity
}

/// Drop one hold on `instance`; the last despawns it (its rows clear).
pub fn release(world: &mut World, instance: Entity) {
    let Some((registry, _)) = world.query::<&MaterialInstances>().next() else {
        return;
    };
    let last = {
        let mut instances = world
            .get_mut::<MaterialInstances>(registry)
            .expect("registry entity");
        let Some(key) = instances.source_of.get(&instance).cloned() else {
            return;
        };
        let last = match instances.by_source.get_mut(&key) {
            Some((_, users)) if *users > 1 => {
                *users -= 1;
                false
            }
            _ => true,
        };
        if last {
            instances.by_source.remove(&key);
            instances.source_of.remove(&instance);
        }
        last
    };
    if last {
        world.despawn(instance);
    }
}

/// The table row of `instance`, for draw rows: its entity row.
pub fn row_of(instance: Entity) -> u32 {
    instance.index()
}

// ── Assets: declared, bound, mirrored ──────────────────────────────────────

fn collect_instance_handles(value: *const (), out: &mut Vec<HandleId>) {
    // SAFETY: SceneDB reaches this only through the registration below,
    // with a pointer to a live `MaterialInstanceComponent`.
    let instance = unsafe { &*(value as *const MaterialInstanceComponent) };
    let id = instance.source.asset_id();
    if !id.is_zero() {
        out.push(id.handle());
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::handle_ledger::HandleLedgerRegistration {
        component_id: pulsar_scenedb::component::component_id::<MaterialInstanceComponent>,
        collect_from_value: collect_instance_handles,
    }
}

fn register_binding() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_binding::<MaterialInstanceComponent>(
            MATERIAL_ASSET_KIND,
            bind_instance_material,
        );
    });
}

/// How an instance takes in its loaded (or failed) material.
fn bind_instance_material(instance: &mut MaterialInstanceComponent, view: &AssetView) {
    if instance.source.asset_id() != view.id {
        return;
    }
    instance.bound = match view.payload::<MaterialPayload>() {
        Some(material) => {
            // What the material reads that a cache dropped (a graph's
            // textures) starts coming back now.
            material.0.warm();
            Some((view.id, view.version, material.0.clone()))
        }
        None => {
            log::warn!(
                "material '{}' could not be loaded ({:?}); its users draw the fallback surface",
                view.source,
                view.state
            );
            None
        }
    };
}

fn instance_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `MaterialInstanceComponent`'s
    // own `ComponentId`, with a pointer to a live value.
    let instance = unsafe { &*(data as *const MaterialInstanceComponent) };
    let resolved = instance.resolve(Some(mirror));
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &MaterialComponent::from(resolved.row),
        is_new_insert,
    );
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &MaterialKeyComponent {
            material_class: resolved.material_class,
            graph_hash_lo: resolved.graph_hash as u32,
            graph_hash_hi: (resolved.graph_hash >> 32) as u32,
            live: 1,
        },
        is_new_insert,
    );
}

fn instance_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<MaterialComponent>(mirror, row);
    pulsar_scenedb::gpu::clear_derived_row::<MaterialKeyComponent>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<MaterialInstanceComponent>,
        dispatch: instance_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<MaterialInstanceComponent>,
        clear: instance_clear,
    }
}
