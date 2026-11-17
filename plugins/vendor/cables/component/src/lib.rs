//! Authored cable definitions and connectors for `pulsar.cables`.
//!
//! A cable definition describes appearance and simulation for one nonzero ID.
//! Connectors sharing that ID are paired each frame on the CPU (`sync`, as
//! portals pair by Link ID) and published as the frame's cable spans; the
//! feature draws them with the definition's GPU row.

pub mod sync;
pub use sync::{CableProblem, CableSync, Pairing, pair_cables};

use engine_class_derive::{engine_class, world_component};
use pulsar_reflection::{Reflectable, TextureSrc};
use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;
use serde::{Deserialize, Serialize};

pub const CABLE_CLASS_NAME: &str = "CableComponent";
pub const CONNECTOR_CLASS_NAME: &str = "CableConnectorComponent";

/// Selects the geometry path used by a cable definition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum CableRenderMode {
    /// Camera-facing textured ribbon.
    #[default]
    TwoD,
    /// A round cable section rendered along the simulated path.
    ThreeD,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum CableSurface {
    SolidColor,
    #[default]
    TileTexture,
}

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Cable", category_color = "#D6A85A")]
#[category("Simulation", category_color = "#74A8D8")]
#[category("Appearance", category_color = "#9D83C7")]
#[serde(default)]
pub struct CableComponent {
    /// Shared matching key. Connectors with this ID form every unique pair.
    #[property(min = 1.0, max = 4_294_967_295.0, step = 1.0, category = "Cable")]
    pub id: u32,
    #[property(category = "Cable")]
    pub enabled: bool,
    #[property(category = "Appearance")]
    pub render_mode: CableRenderMode,
    /// Resolved by the cable asset bindings when using 2D mode.
    #[property(category = "Appearance")]
    pub tile_texture: TextureSrc,
    #[property(category = "Appearance")]
    pub surface: CableSurface,
    #[property(category = "Appearance")]
    pub color: [f32; 4],
    /// Runtime binding for `tile_texture`; populated by the texture asset
    /// lifecycle and deliberately excluded from serialized scene data.
    #[serde(skip)]
    pub tile_image: helio_component::texture_assets::BoundTexture,
    #[property(min = 0.001, max = 100.0, step = 0.001, category = "Appearance")]
    pub radius: f32,
    /// Number of parallel strands (1 disables bundling).
    #[property(min = 1.0, max = 64.0, step = 1.0, category = "Appearance")]
    pub bundle_count: u32,
    #[property(min = 0.0, max = 100.0, step = 0.001, category = "Appearance")]
    pub bundle_spacing: f32,
    /// Rest-shape sag as a fraction of span length.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Simulation")]
    pub droop: f32,
    /// Wind direction in world space. Zero uses the project's wind direction.
    #[property(category = "Simulation")]
    pub wind_direction: [f32; 3],
    #[property(min = 0.0, max = 100.0, step = 0.01, category = "Simulation")]
    pub wind_strength: f32,
    #[property(min = 0.0, max = 100.0, step = 0.01, category = "Simulation")]
    pub wind_speed: f32,
    /// Incrementing this restarts the GPU strand from a straight span.
    #[property(min = 0.0, max = 4_294_967_295.0, step = 1.0, category = "Simulation")]
    pub reset_revision: u32,
}

impl Default for CableComponent {
    fn default() -> Self {
        Self {
            id: 1,
            enabled: true,
            render_mode: CableRenderMode::TwoD,
            tile_texture: TextureSrc::default(),
            surface: CableSurface::TileTexture,
            color: [0.58, 0.52, 0.42, 1.0],
            tile_image: Default::default(),
            radius: 0.025,
            bundle_count: 1,
            bundle_spacing: 0.05,
            droop: 0.04,
            wind_direction: [1.0, 0.0, 0.0],
            wind_strength: 0.0,
            wind_speed: 1.0,
            reset_revision: 0,
        }
    }
}

impl CableComponent {
    fn tile_path(&self) -> Option<std::path::PathBuf> {
        if let Some(reference) = self.tile_texture.asset_ref() {
            let root = std::path::PathBuf::from(engine_state::get_project_path()?);
            let index = engine_fs::open_project_file_index(&root).ok().flatten()?;
            let path = index.path_for_asset_id_kind("texture", reference.as_str()).ok().flatten()?;
            return Some(root.join(path));
        }
        let path = self.tile_texture.legacy_path()?.trim();
        (!path.is_empty()).then(|| std::path::PathBuf::from(path))
    }

    /// Reset the next GPU simulation initialization to a straight span.
    /// The feature observes this authored revision; it never reads state back.
    pub fn reset_simulation(&mut self) {
        self.reset_revision = self.reset_revision.wrapping_add(1);
    }

    /// Project the authored settings into the feature's GPU contract row.
    pub fn gpu_row(&self, texture_slot: u32) -> cables_api::CableDefinitionRow {
        cables_api::CableDefinitionRow {
            id: self.id,
            enabled: u32::from(self.enabled && self.id != 0),
            mode: match self.render_mode {
                CableRenderMode::TwoD => 0,
                CableRenderMode::ThreeD => 1,
            },
            bundle_count: self.bundle_count.clamp(1, 64),
            radius: self.radius.max(0.001),
            bundle_spacing: self.bundle_spacing.max(0.0),
            droop: self.droop.clamp(0.0, 1.0),
            wind_strength: self.wind_strength.max(0.0),
            wind_direction_speed: [
                self.wind_direction[0],
                self.wind_direction[1],
                self.wind_direction[2],
                self.wind_speed.max(0.0),
            ],
            reset_revision: self.reset_revision,
            texture_slot,
            color_rgba8: u32::from_le_bytes(
                self.color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8),
            ),
            _padding: 0,
        }
    }
}

#[world_component]
impl CableComponent {}

/// An endpoint whose transform is connected to the cable definition with the
/// same nonzero ID. Two or more connectors can share an ID.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Cable", category_color = "#D6A85A")]
#[serde(default)]
pub struct CableConnectorComponent {
    #[property(min = 1.0, max = 4_294_967_295.0, step = 1.0, category = "Cable")]
    pub id: u32,
    #[property(category = "Cable")]
    pub enabled: bool,
}

impl Default for CableConnectorComponent {
    fn default() -> Self {
        Self { id: 1, enabled: true }
    }
}

#[world_component]
impl CableConnectorComponent {}

/// SceneDB-owned source columns for cable definitions. Fields marked `#[gpu]`
/// are mirrored on component edits; rendering never walks component storage.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "cable_definition_sources")]
pub struct CableDefinitionSourceRow {
    #[gpu]
    pub id: u32,
    #[gpu]
    pub enabled: u32,
    #[gpu]
    pub mode: u32,
    #[gpu]
    pub bundle_count: u32,
    #[gpu]
    pub radius: f32,
    #[gpu]
    pub bundle_spacing: f32,
    #[gpu]
    pub droop: f32,
    #[gpu]
    pub wind_strength: f32,
    #[gpu]
    pub wind_direction_speed: [f32; 4],
    #[gpu]
    pub reset_revision: u32,
    #[gpu]
    pub texture_slot: u32,
    #[gpu]
    pub color_rgba8: u32,
    #[gpu]
    pub _padding: u32,
}

impl CableDefinitionSourceRow {
    fn of(c: &CableComponent, texture_slot: u32) -> Self {
        let row = c.gpu_row(texture_slot);
        bytemuck::pod_read_unaligned(bytemuck::bytes_of(&row))
    }
}

fn definition_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new: bool) {
    // SAFETY: SceneDB invokes this registration with the live component value.
    let component = unsafe { &*(data as *const CableComponent) };
    let texture_slot = if component.enabled && component.surface == CableSurface::TileTexture {
        component.tile_path().map_or(u32::MAX, |path| {
            helio_component::texture_assets::texture_slot(
                &component.tile_image,
                &path.to_string_lossy(),
                mirror,
                "cable",
            )
        })
    } else {
        u32::MAX
    };
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &CableDefinitionSourceRow::of(component, texture_slot),
        is_new,
    );
}

fn definition_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<CableDefinitionSourceRow>(mirror, row);
}

fn collect_cable_handles(
    value: *const (),
    out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>,
) {
    // SAFETY: SceneDB calls this for the CableComponent attachment only.
    let cable = unsafe { &*(value as *const CableComponent) };
    let path = cable.tile_path();
    let path = path.as_deref().and_then(std::path::Path::to_str).unwrap_or_default();
    helio_component::texture_assets::collect_texture(path, out);
}

/// Resolve the authored tile texture through the shared texture asset flow.
pub fn bind_cable_texture(cable: &mut CableComponent, view: &pulsar_scenedb::AssetView) {
    let Some(path) = cable.tile_path() else { return; };
    helio_component::texture_assets::bind_texture(
        &mut cable.tile_image,
        &path.to_string_lossy(),
        view,
        "cable",
    );
}

/// Registers the CableComponent texture asset binding with SceneDB.
pub fn register_texture_binding() {
    pulsar_scenedb::register_binding::<CableComponent>(
        helio_component::texture_assets::TEXTURE_ASSET_KIND,
        bind_cable_texture,
    );
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::handle_ledger::HandleLedgerRegistration {
        component_id: pulsar_scenedb::component_id::<CableComponent>,
        collect_from_value: collect_cable_handles,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<CableComponent>,
        dispatch: definition_dispatch,
    }
}
pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<CableComponent>,
        clear: definition_clear,
    }
}

/// Registers growable SceneDB columns for authored cable definitions.
pub fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    CableDefinitionSourceRow::register_gpu_columns_growable(store, 16, device);
}

/// Packs the enabled cable definitions into the rows the feature draws from.
///
/// A source row sits at its owning object's scene slot, so a definition on
/// the 1,700th object is row ~1,700; the join packs the enabled ones into
/// the leading rows, as every kind's `*_sources` column is joined.
pub fn row_derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    use helio_scene_join::{BufferKey, RowJoin, RowTable};
    /// `CableDefinitionSourceRow::enabled` (word 1).
    const ENABLED_WORD: u32 = 1;
    /// Enabled definitions the draw can address.
    const CAPACITY: u32 = 256;
    Box::new(RowJoin::new(
        device,
        "CableDefinitionRows",
        keys,
        vec![RowTable::new(
            "Cable Definition Rows",
            BufferKey::of("cable_definitions"),
            BufferKey::of("cable_definition_sources"),
            std::mem::size_of::<CableDefinitionSourceRow>() as u64,
            (std::mem::size_of::<CableDefinitionSourceRow>() / 4) as u32,
            0,
        )
        .packed(CAPACITY)
        .gated_on(ENABLED_WORD)],
    ))
}

/// The cable plugin's Helio render feature set.
pub fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(cables_feature::CablesFeature)
}
