//! Foliage (`pulsar.foliage`): the component crate of the plugin that owns
//! scattered foliage and the level's wind.
//!
//! - The components: [`FoliageComponent`] (a foliage type grown over its
//!   layer) and [`WindComponent`] (the level's global wind, at most one a
//!   level).
//! - Their rows ([`FoliageSourceRow`], [`GlobalWindSourceRow`]): the foliage
//!   type, layer and wind rows SceneDB derives from the authored values; the
//!   plugin's row join places them (`helio_pass_foliage_place::scene_rows`).
//!   The wind rows are the foliage api's (`helio_foliage_api::
//!   WIND_SOURCES_BUFFER`), which water reads too.
//! - The registrations of the plugin's Helio features (placement and the
//!   G-buffer drawer, `helio_pass_foliage_gbuffer::FoliageFeature`), their
//!   settings (`helio_pass_foliage_place::FOLIAGE`), its scene rows, the
//!   one-wind level rule and the realtime redraw while foliage sways.
//!
//! Everything registers through `inventory` once the crate is linked: hosts
//! link it (the game: `extern crate foliage_component`; the editor through
//! the plugin's editor crate, `foliage_plugin`) and know no foliage type.

pub mod foliage;
pub mod wind;

pub use foliage::*;
pub use wind::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb::{Entity, World};
use pulsar_scenedb_derive::SceneStore;

/// Where the foliage components' source rows live.
pub const FOLIAGE_SOURCES_BUFFER: &str = "foliage_sources";

/// Foliage: the foliage type row (density, size, slope and altitude ranges,
/// LOD distances and wind response in `type_params`; the interaction
/// stiffness, material, density layer and kind/flags words in `type_ids`;
/// the impostor id, colour and ground mask in `type_tail`; blades a tuft,
/// lean, curve, fall, tip colour and patches in `type_tuft`), its layer
/// (half extent, infinite-extent flag, altitude min and max) and its wind
/// row. A disabled component derives a zero row, which the join skips (it
/// places only rows with a density).
///
/// The layer is a world-aligned square centred on the owner. The foliage
/// passes grow every type in every layer and read one wind row: the first
/// component that opts out of the global wind (its wind's `_pad[0]`, word
/// 46 of this row, is 1), else the level's global wind, else the first
/// component's own wind. So with several foliage components their types
/// share their layers and one wind applies to all of them.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "foliage_sources")]
pub struct FoliageSourceRow {
    #[gpu]
    pub type_params: [f32; 16],
    #[gpu]
    pub type_ids: [u32; 4],
    #[gpu]
    pub type_tail: [u32; 4],
    #[gpu]
    pub type_tuft: [u32; 8],
    #[gpu]
    pub layer: [f32; 4],
    #[gpu]
    pub wind: [f32; 12],
}

/// The level's global wind: the foliage passes' wind row
/// (`helio_pass_foliage_place::GpuWind`, bit for bit: direction and speed,
/// gusts, an unused clock and the global mark in word 10). A disabled
/// wind's row is zero, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "wind_sources")]
pub struct GlobalWindSourceRow {
    #[gpu]
    pub wind: [f32; 12],
}

/// `bytes`, which must be exactly a `T`, as a `T`.
fn read<T: bytemuck::Pod>(bytes: &[u8]) -> T {
    bytemuck::pod_read_unaligned(bytes)
}

impl FoliageSourceRow {
    pub fn of(foliage: &FoliageComponent) -> Self {
        if !foliage.general.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let foliage_type = foliage::runtime::gpu_type(foliage);
        let layer = [
            foliage.placement.layer_extent,
            if foliage.placement.has_infinite_extent {
                1.0
            } else {
                0.0
            },
            foliage.placement.altitude_min,
            foliage.placement.altitude_max,
        ];
        let wind = foliage::runtime::wind(foliage);
        read(
            &[
                bytemuck::bytes_of(&foliage_type),
                bytemuck::bytes_of(&layer),
                bytemuck::bytes_of(&wind),
            ]
            .concat(),
        )
    }
}

impl GlobalWindSourceRow {
    pub fn of(wind: &WindComponent) -> Self {
        read(bytemuck::bytes_of(&wind.to_row()))
    }
}

macro_rules! derived_row {
    ($authored:ty, $row:ty, $dispatch:ident, $clear:ident) => {
        fn $dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
            // SAFETY: SceneDB reaches this only through the authored
            // component's own `ComponentId`, with a pointer to a live value.
            let authored = unsafe { &*(data as *const $authored) };
            pulsar_scenedb::gpu::write_derived_row(
                mirror,
                row,
                &<$row>::of(authored),
                is_new_insert,
            );
        }

        fn $clear(mirror: &GpuMirrorHandle, row: u32) {
            pulsar_scenedb::gpu::clear_derived_row::<$row>(mirror, row);
        }

        pulsar_scenedb::pulsar_reflection::inventory::submit! {
            pulsar_scenedb::gpu::GpuMirrorRegistration {
                component_id: pulsar_scenedb::component_id::<$authored>,
                dispatch: $dispatch,
            }
        }

        pulsar_scenedb::pulsar_reflection::inventory::submit! {
            pulsar_scenedb::gpu::GpuClearRegistration {
                component_id: pulsar_scenedb::component_id::<$authored>,
                clear: $clear,
            }
        }
    };
}

derived_row!(
    FoliageComponent,
    FoliageSourceRow,
    foliage_dispatch,
    foliage_clear
);
derived_row!(
    WindComponent,
    GlobalWindSourceRow,
    wind_dispatch,
    wind_clear
);

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    FoliageSourceRow::register_gpu_columns_growable(store, 8, device);
    GlobalWindSourceRow::register_gpu_columns_growable(store, 2, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    let key = helio_scene_join::BufferKey::of;
    Box::new(helio_pass_foliage_place::scene_rows::scene_rows(
        device,
        keys,
        helio_pass_foliage_place::FoliageRowSources {
            foliage: key(FOLIAGE_SOURCES_BUFFER),
            wind: key(helio_foliage_api::WIND_SOURCES_BUFFER),
        },
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        register_columns,
        derivation,
    }
}

/// Why adding a second global wind is refused.
pub const ONE_WIND: &str = "This level already has a global wind (a WindComponent); a level has one. Edit it in World Settings.";

/// Every attached [`WindComponent`] instance, enabled or not, in level
/// order.
pub fn winds(world: &World) -> Vec<Entity> {
    engine_backend::scene::level_rules::attached_in_level_order(
        world,
        world
            .query::<&WindComponent>()
            .map(|(instance, _)| instance),
    )
}

/// The level's global wind: its first [`WindComponent`] instance and that
/// instance's owner object.
pub fn level_wind(world: &World) -> Option<(Entity, Entity)> {
    let instance = *winds(world).first()?;
    Some((
        instance,
        pulsar_scene_model::attachments::owner_of(world, instance)?,
    ))
}

// A level has one global wind, edited in World Settings like the sky.
inventory::submit! {
    engine_backend::scene::level_rules::LevelSingletonRegistration {
        class_name: WIND_CLASS_NAME,
        refusal: ONE_WIND,
        instances: winds,
        is: |value| value.is::<WindComponent>(),
    }
}

/// The foliage passes' one wind (as the join picks it): a foliage
/// component's own wind when one opts out of the global wind, else the
/// level's global wind, else the components' own. Approximated by the
/// fastest candidate at the deciding level. Foliage in a wind faster than
/// calm keeps a realtime viewport drawing.
fn foliage_in_wind(world: &World) -> bool {
    use pulsar_scene_model::attachments;
    let foliage: Vec<&FoliageComponent> = world
        .query::<&FoliageComponent>()
        .filter(|(instance, foliage)| {
            foliage.general.enabled && attachments::is_enabled(world, *instance)
        })
        .map(|(_, foliage)| foliage)
        .collect();
    if foliage.is_empty() {
        return false;
    }
    let own_speed = |foliage: &FoliageComponent| {
        if foliage.wind.wind_enabled {
            foliage.wind.wind_speed
        } else {
            0.0
        }
    };
    let opted_out: Vec<f32> = foliage
        .iter()
        .filter(|foliage| !foliage.wind.use_global_wind)
        .map(|foliage| own_speed(foliage))
        .collect();
    if !opted_out.is_empty() {
        return opted_out.iter().any(|speed| *speed > 0.0);
    }
    let global = world
        .query::<&WindComponent>()
        .find(|(instance, wind)| wind.enabled && attachments::is_enabled(world, *instance))
        .map(|(_, wind)| wind.speed);
    match global {
        Some(speed) => speed > 0.0,
        None => foliage.iter().any(|foliage| own_speed(foliage) > 0.0),
    }
}

// The answer reads both components: re-checked when either changes.
inventory::submit! {
    engine_backend::subsystems::render::AnimatedContentRegistration {
        watch: |world| world.open_change_cursor::<FoliageComponent>(),
        animated: foliage_in_wind,
    }
}

inventory::submit! {
    engine_backend::subsystems::render::AnimatedContentRegistration {
        watch: |world| world.open_change_cursor::<WindComponent>(),
        animated: foliage_in_wind,
    }
}

/// Foliage's features: placement (before the G-buffer) and its drawer.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_foliage_gbuffer::FoliageFeature)
}

/// The features' settings from the project's foliage settings.
fn configure_renderer(config: helio::RendererConfig) -> helio::RendererConfig {
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    config.with_feature(helio_pass_foliage_place::FoliageSettings::from_settings(
        &project_settings::resolved(&helio_pass_foliage_place::FOLIAGE),
    ))
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        features: render_features,
        configure: configure_renderer,
    }
}

inventory::submit! {
    pulsar_settings::project::rendering::SettingsGroupRegistration {
        group: &helio_pass_foliage_place::FOLIAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_have_the_documented_word_counts() {
        assert_eq!(std::mem::size_of::<FoliageSourceRow>(), 48 * 4);
        assert_eq!(std::mem::size_of::<GlobalWindSourceRow>(), 12 * 4);
        assert_eq!(
            std::mem::size_of::<helio_pass_foliage_place::components::FoliageTypeRow>(),
            32 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_foliage_place::components::FoliageWindRow>(),
            12 * 4
        );
        assert_eq!(
            std::mem::size_of::<FoliageSourceRow>() as u64,
            helio_pass_foliage_place::scene_rows::FOLIAGE_SOURCE_ROW_BYTES
        );
        assert_eq!(
            std::mem::size_of::<GlobalWindSourceRow>() as u64,
            helio_foliage_api::WIND_SOURCE_ROW_BYTES
        );
    }

    #[test]
    fn a_foliage_row_holds_its_type_layer_and_wind() {
        let mut foliage = FoliageComponent::default();
        foliage.general.enabled = true;
        foliage.general.density = 12.0;
        foliage.placement.layer_extent = 25.0;
        let row = FoliageSourceRow::of(&foliage);
        let foliage_type = foliage::runtime::gpu_type(&foliage);
        let type_bytes = bytemuck::bytes_of(&foliage_type);
        assert_eq!(&bytemuck::bytes_of(&row)[..type_bytes.len()], type_bytes);
        assert_eq!(row.type_params[0], 12.0, "density first: the join's gate");
        assert_eq!(row.layer[0], 25.0);
        foliage.general.enabled = false;
        let row = FoliageSourceRow::of(&foliage);
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn registers_its_features_settings_and_level_rule() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.foliage"),
            "features: {:?}",
            features.ids()
        );
        assert!(
            pulsar_settings::project::rendering::groups()
                .iter()
                .any(|group| std::ptr::eq(*group, &helio_pass_foliage_place::FOLIAGE))
        );
        assert!(
            inventory::iter::<engine_backend::scene::level_rules::LevelSingletonRegistration>
                .into_iter()
                .any(|rule| rule.class_name == WIND_CLASS_NAME)
        );
    }
}
