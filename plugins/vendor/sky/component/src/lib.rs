//! Sky (`pulsar.sky`): the component crate of the plugin that owns the
//! level's sky.
//!
//! - The component ([`component`]): [`AtmosphereComponent`], a planet's air
//!   and its clouds; a level has one ([`level_atmosphere`]).
//! - Its row ([`AtmosphereSourceRow`]): the atmosphere row SceneDB derives
//!   from the authored value; the plugin's row join places a planet at its
//!   owner (`helio_pass_sky::scene_rows`).
//! - The registrations of the plugin's Helio feature
//!   (`helio_pass_sky::SkyFeature`: the atmosphere's tables, the clouds and
//!   the sky composite; it publishes the frame's `sky`, `sun_visibility`
//!   and `sun_transmittance`), its scene rows, and its level rule (one sky)
//!   with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate sky_component`; the editor through the
//! plugin's editor crate, `sky_plugin`) and know no sky type.

pub mod component;

pub use component::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb::{Entity, World};
use pulsar_scenedb_derive::SceneStore;

/// Where the atmospheres' source rows live.
pub const ATMOSPHERE_SOURCES_BUFFER: &str = "atmosphere_sources";

/// An atmosphere (`helio_pass_sky::AtmosphereRow` pass row, bit for
/// bit): its centre and placement (`center`; the centre in the owner's
/// space, zero: the join adds the owner's position to a planet placed at its
/// owner), Rayleigh and Mie scattering, Mie and ozone absorption (`media`),
/// ground albedo and ozone width (`ground`), and planet and atmosphere
/// radii, sun size and enabled flag (`shell`; `enabled` 0 when the authored
/// component is disabled), the cloud layer (`clouds`) and its weather
/// (`weather`), followed by the night-sky controls (`night`). The placement
/// and enabled words are `u32`s, kept as bits.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "atmosphere_sources")]
pub struct AtmosphereSourceRow {
    #[gpu]
    pub center: [f32; 4],
    #[gpu]
    pub media: [f32; 16],
    #[gpu]
    pub ground: [f32; 4],
    #[gpu]
    pub shell: [f32; 4],
    #[gpu]
    pub clouds: [f32; 16],
    #[gpu]
    pub weather: [f32; 16],
    #[gpu]
    pub night: [f32; 16],
}

impl AtmosphereSourceRow {
    pub fn of(atmosphere: &AtmosphereComponent) -> Self {
        bytemuck::pod_read_unaligned(bytemuck::bytes_of(&atmosphere.to_row()))
    }
}

fn atmosphere_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through the authored component's own
    // `ComponentId`, with a pointer to a live value.
    let authored = unsafe { &*(data as *const AtmosphereComponent) };
    // The pass reads the row on the GPU; the CPU needs the sky texture to keep
    // resident (a level has one sky), so announce it alongside the row.
    {
        // Diagnostic: one line whenever the sky texture's authored state changes.
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
        let state = (authored.sky_texture_enabled as u64)
            | ((authored.enabled as u64) << 1)
            | ((authored.sky_texture_source() as u64) << 2);
        if LAST.swap(state, std::sync::atomic::Ordering::Relaxed) != state {
            tracing::info!(
                "atmosphere row derived: enabled={}, sky_texture_enabled={}, preset={:?}, source={:#x}, intensity={}, opacity={}",
                authored.enabled,
                authored.sky_texture_enabled,
                authored.sky_texture_preset,
                authored.sky_texture_source(),
                authored.sky_texture_intensity,
                authored.sky_texture_opacity,
            );
        }
    }
    helio_pass_sky::sky_texture::set_requested(if authored.enabled {
        authored.sky_texture_source()
    } else {
        helio_pass_sky::sky_texture::source::NONE
    });
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &AtmosphereSourceRow::of(authored),
        is_new_insert,
    );
}

fn atmosphere_clear(mirror: &GpuMirrorHandle, row: u32) {
    helio_pass_sky::sky_texture::set_requested(helio_pass_sky::sky_texture::source::NONE);
    pulsar_scenedb::gpu::clear_derived_row::<AtmosphereSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<AtmosphereComponent>,
        dispatch: atmosphere_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<AtmosphereComponent>,
        clear: atmosphere_clear,
    }
}

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    AtmosphereSourceRow::register_gpu_columns_growable(store, 4, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_sky::scene_rows::scene_rows(
        device,
        keys,
        helio_scene_join::BufferKey::of(ATMOSPHERE_SOURCES_BUFFER),
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        register_columns,
        derivation,
    }
}

/// The sky's feature: the atmosphere's tables, the clouds and the sky
/// composite.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_sky::SkyFeature)
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: |config| config,
    }
}

/// The sky's class name.
pub const ATMOSPHERE_CLASS: &str = "AtmosphereComponent";

/// Why adding a second atmosphere is refused.
pub const ONE_ATMOSPHERE: &str =
    "This level already has a sky (an AtmosphereComponent); a level has one. Edit it in World Settings.";

/// Every attached [`AtmosphereComponent`] instance, enabled or not, in
/// level order (a level file's order).
pub fn atmospheres(world: &World) -> Vec<Entity> {
    let instances: Vec<Entity> = world
        .query::<&AtmosphereComponent>()
        .map(|(instance, _)| instance)
        .collect();
    engine_backend::scene::level_rules::attached_in_level_order(world, instances.into_iter())
}

/// The level's sky: its first [`AtmosphereComponent`] instance and that
/// instance's owner object.
pub fn level_atmosphere(world: &World) -> Option<(Entity, Entity)> {
    let instance = *atmospheres(world).first()?;
    Some((
        instance,
        pulsar_scene_model::attachments::owner_of(world, instance)?,
    ))
}

// A level has one sky (#1057): the atmosphere pass draws the first enabled
// row, so a second would only fight the first.
inventory::submit! {
    engine_backend::scene::level_rules::LevelSingletonRegistration {
        class_name: ATMOSPHERE_CLASS,
        refusal: ONE_ATMOSPHERE,
        instances: atmospheres,
        is: |value| value.is::<AtmosphereComponent>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_have_the_documented_word_counts() {
        assert_eq!(std::mem::size_of::<AtmosphereSourceRow>(), 76 * 4);
        assert_eq!(std::mem::size_of::<helio_pass_sky::AtmosphereRow>(), 76 * 4);
        assert_eq!(
            std::mem::size_of::<AtmosphereSourceRow>() as u64,
            helio_pass_sky::scene_rows::ATMOSPHERE_SOURCE_ROW_BYTES
        );
    }

    #[test]
    fn a_disabled_atmosphere_derives_an_inert_row() {
        let mut atmosphere = AtmosphereComponent::default();
        assert_eq!(AtmosphereSourceRow::of(&atmosphere).shell[3].to_bits(), 1);
        atmosphere.enabled = false;
        assert_eq!(AtmosphereSourceRow::of(&atmosphere).shell[3].to_bits(), 0);
    }

    #[test]
    fn an_atmosphere_row_is_the_pass_row_in_its_owners_space() {
        let atmosphere = AtmosphereComponent {
            placement: AtmospherePlacement::PlanetAtOwner,
            planet_radius_km: 6360.0,
            ..Default::default()
        };
        let row = AtmosphereSourceRow::of(&atmosphere);
        assert_eq!(
            bytemuck::bytes_of(&row),
            bytemuck::bytes_of(&atmosphere.to_row())
        );
        assert_eq!(
            &row.center[..3],
            &[0.0; 3],
            "the join adds the owner's position"
        );
        assert_eq!(
            row.center[3].to_bits(),
            helio_pass_sky::atmosphere::placement::CENTER
        );
    }

    #[test]
    fn registers_its_feature_and_its_rule() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.sky"),
            "features: {:?}",
            features.ids()
        );
        assert!(
            inventory::iter::<engine_backend::scene::level_rules::LevelSingletonRegistration>
                .into_iter()
                .any(|rule| rule.class_name == ATMOSPHERE_CLASS)
        );
    }
}
