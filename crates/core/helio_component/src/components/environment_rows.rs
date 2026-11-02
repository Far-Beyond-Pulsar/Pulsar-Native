//! The rows fog volumes, post-process volumes, camera post-process
//! settings, water volumes, foliage, the global wind, atmospheres, decals, particle emitters and 2D sprites cast, as the renderer's environment join reads them
//! (Pulsar-Native#1035, Phase 4).
//!
//! Each is a second GPU registration on its authored component: SceneDB
//! derives the row from the authored value on every insert, guarded write,
//! removal, despawn and mirror replay, keyed by the component instance
//! entity. A row is the pass's own row in the component's local space: the
//! graph-owned join (`helio-default-passes::environment_join`) gates it on
//! the instance's enabled state, its owner's generation and (for volumes)
//! visibility, and places volumes with the owner's transform, on the GPU.
//! A disabled component derives an inert row (its pass skips it).
//!
//! Layouts, in `u32` words (`shaders/environment_join.wgsl` copies the
//! payload after an optional local-size header):
//!
//! | Row | Words | Pass row it becomes |
//! |---|---|---|
//! | [`GlobalFogSourceRow`] | 20: the medium | `helio_pass_volumetric_fog::GlobalFogComponent` (20) |
//! | [`LocalFogSourceRow`] | 4 size + 24: medium, edge fade | `LocalFogVolumeComponent` (8 bounds + 24) |
//! | [`PostProcessVolumeSourceRow`] | 4 size + 140: the row after its bounds | `helio_pass_postprocess::PostProcessVolumeComponent` (8 bounds + 140) |
//! | [`CameraPostProcessSourceRow`] | 136: the row | `helio_pass_postprocess::CameraPostProcessComponent` (136) |
//! | [`WaterVolumeSourceRow`] | 4 size + 56: the row after its bounds | `helio_pass_water_sim::WaterVolumeComponent` (8 bounds + 56), packed into its leading rows |
//! | [`FoliageSourceRow`] | 32 type + 4 layer + 12 wind | `helio_pass_foliage_place`'s `FoliageTypeComponent` (32), `FoliageLayerComponent` (8) and `FoliageWindComponent` (12), each packed |
//! | [`AtmosphereSourceRow`] | 4 centre + 16 media + 4 ground + 4 shell + 16 clouds + 16 weather: the row, its centre in the owner's space | `helio_pass_sky::AtmosphereComponent` (60) |
//! | [`DecalSourceRow`] | 4 size + 32: the row, its transform left to the join | `helio_pass_decal::DecalComponent` (32), packed into its leading rows |
//! | [`GlobalWindSourceRow`] | 12: the wind row, marked global | `helio_pass_foliage_place`'s `FoliageWindComponent` (12), over the foliage components' own wind |
//! | [`CoronaEmitterSourceRow`] | 60: the row, its transform and pool range left to the join (word 46: the requested range) | `helio_pass_corona::CoronaEmitterComponent` (60), packed into its leading rows |
//! | [`SpriteSourceRow`] | 20: the row, its 2D position left to the join | `helio_pass_sprite_batch::SpriteComponent` (20) |

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb_derive::SceneStore;

use super::{
    AtmosphereComponent, CameraPostProcessComponent, DecalComponent, FoliageComponent,
    GlobalFogComponent, LocalFogVolumeComponent, ParticleEmitterComponent,
    PostProcessVolumeComponent, SpriteComponent, WaterVolumeComponent, WindComponent,
};

pub const GLOBAL_FOG_SOURCES_BUFFER: &str = "global_fog_sources";
pub const LOCAL_FOG_SOURCES_BUFFER: &str = "local_fog_sources";
pub const POST_PROCESS_VOLUME_SOURCES_BUFFER: &str = "post_process_volume_sources";
pub const CAMERA_POST_PROCESS_SOURCES_BUFFER: &str = "camera_postprocess_sources";
pub const WATER_VOLUME_SOURCES_BUFFER: &str = "water_volume_sources";
pub const FOLIAGE_SOURCES_BUFFER: &str = "foliage_sources";
pub const ATMOSPHERE_SOURCES_BUFFER: &str = "atmosphere_sources";
pub const DECAL_SOURCES_BUFFER: &str = "decal_sources";
pub const CORONA_EMITTER_SOURCES_BUFFER: &str = "corona_emitter_sources";
pub const WIND_SOURCES_BUFFER: &str = "wind_sources";
pub const SPRITE_SOURCES_BUFFER: &str = "sprite_sources";

/// A 2D sprite: the sprite passes' row
/// (`helio_pass_sprite_batch::SpriteComponent`, bit for bit) at the 2D
/// origin, which the join moves to its owner's X and Y, turns by the
/// owner's roll and scales by its X and Y scale. Its image is registered in
/// the scene's texture store as the row is derived (its slot in
/// `atlas_layer`). A disabled sprite's row is zero, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "sprite_sources")]
pub struct SpriteSourceRow {
    /// Position, size, rotation, depth, padding, UV rectangle and tint.
    #[gpu]
    pub sprite: [f32; 16],
    /// The image's texture store slot and padding.
    #[gpu]
    pub texture: [u32; 4],
}

impl SpriteSourceRow {
    pub fn of(sprite: &SpriteComponent, texture_slot: u32) -> Self {
        read(bytemuck::bytes_of(&sprite.to_row(texture_slot)))
    }
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

impl GlobalWindSourceRow {
    pub fn of(wind: &WindComponent) -> Self {
        read(bytemuck::bytes_of(&wind.to_row()))
    }
}

/// A particle emitter: the Corona pass row
/// (`helio_pass_corona::GpuCoronaEmitter`, bit for bit) with a zero
/// transform, which the join replaces with the owner's, and the requested
/// pool range in `particle_count` (word 46), which the join replaces with
/// the allocated range (`particle_offset`, word 45). A disabled emitter's
/// row is zero, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "corona_emitter_sources")]
pub struct CoronaEmitterSourceRow {
    /// The pass row's transform: zero here, the join's to write.
    #[gpu]
    pub transform: [f32; 16],
    /// Emission (rate, lifetime and its variation, gravity), start and end
    /// size, start and end colour.
    #[gpu]
    pub params: [f32; 16],
    /// Velocity, its variation, shape (type, radius, -, active).
    #[gpu]
    pub motion: [f32; 12],
    /// Sprite, particle offset (the join's), requested particles, spawn
    /// cursor (unused: the pass keeps its own).
    #[gpu]
    pub range: [u32; 4],
    #[gpu]
    pub reserved: [f32; 12],
}

impl CoronaEmitterSourceRow {
    pub fn of(emitter: &ParticleEmitterComponent) -> Self {
        read(bytemuck::bytes_of(&emitter.to_row()))
    }
}

/// A decal: its box's local size (`xyz`, full extent before the owner's
/// scale), then the decal pass row with a zero transform, which the join
/// replaces with the world-to-decal transform of the owner-placed box. A
/// disabled decal has a zero size, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "decal_sources")]
pub struct DecalSourceRow {
    #[gpu]
    pub size: [f32; 4],
    /// The pass row's transform: zero here, the join's to write.
    #[gpu]
    pub transform: [f32; 16],
    /// The rest of the pass row: colour and opacity, texture slots, blend,
    /// type, fade, age and normal adaptation.
    #[gpu]
    pub params: [f32; 16],
}

impl DecalSourceRow {
    /// The row of `decal` with its albedo texture at `albedo_slot`
    /// (`u32::MAX`: none).
    pub fn of(decal: &DecalComponent, albedo_slot: u32) -> Self {
        if !decal.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let size = [decal.size[0], decal.size[1], decal.size[2], 0.0];
        read(
            &[
                bytemuck::bytes_of(&size),
                bytemuck::bytes_of(&decal.to_row(albedo_slot)),
            ]
            .concat(),
        )
    }
}

/// An atmosphere (`helio_pass_sky::AtmosphereComponent` pass row, bit for
/// bit): its centre and placement (`center`; the centre in the owner's
/// space, zero: the join adds the owner's position to a planet placed at its
/// owner), Rayleigh and Mie scattering, Mie and ozone absorption (`media`),
/// ground albedo and ozone width (`ground`), and planet and atmosphere
/// radii, sun size and enabled flag (`shell`; `enabled` 0 when the authored
/// component is disabled), the cloud layer (`clouds`) and its weather
/// (`weather`, the 16 words after it). The placement and enabled words are
/// `u32`s, kept as bits.
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
}

impl AtmosphereSourceRow {
    pub fn of(atmosphere: &AtmosphereComponent) -> Self {
        read(bytemuck::bytes_of(&atmosphere.to_row()))
    }
}

/// A global fog medium (`GlobalFogComponent` pass row, bit for bit;
/// `enabled` 0 when the authored component is disabled).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "global_fog_sources")]
pub struct GlobalFogSourceRow {
    #[gpu]
    pub medium: [f32; 16],
    /// The medium's `planet` (its last four words).
    #[gpu]
    pub planet: [f32; 4],
}

/// A local fog volume: its local size (`xyz`, full extent before the owner's
/// scale), then the pass row's medium and edge fade (`x`).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "local_fog_sources")]
pub struct LocalFogSourceRow {
    #[gpu]
    pub size: [f32; 4],
    #[gpu]
    pub medium: [f32; 16],
    /// The medium's `planet` (its last four words).
    #[gpu]
    pub planet: [f32; 4],
    #[gpu]
    pub edge_fade: [f32; 4],
}

/// A post-process volume: its local size, then the pass row after its
/// bounds: priority, blend radius, blend weight, unbound flag (`params`, as
/// bits), override mask, settings and lens blocks.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "post_process_volume_sources")]
pub struct PostProcessVolumeSourceRow {
    #[gpu]
    pub size: [f32; 4],
    #[gpu]
    pub params: [u32; 4],
    #[gpu]
    pub mask: [u32; 4],
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

/// A camera's post-process baseline (`CameraPostProcessComponent` pass row,
/// bit for bit: view id and enabled flag in `header`; `enabled` 0 when the
/// authored component is disabled).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_postprocess_sources")]
pub struct CameraPostProcessSourceRow {
    #[gpu]
    pub header: [u32; 4],
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

/// A water volume: its local size (`xyz`, full extent before the owner's
/// scale; `w`, the surface height above the owner's position), then the
/// water volume row after its bounds: waves (wave params, direction,
/// colour, extinction), optics (reflection and refraction, caustics, fog,
/// surface params), shading (shadow, sun direction, SSR, simulation
/// dynamics), wind and a reserved vec4. A disabled component has a zero
/// size, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "water_volume_sources")]
pub struct WaterVolumeSourceRow {
    #[gpu]
    pub size: [f32; 4],
    #[gpu]
    pub waves: [f32; 16],
    #[gpu]
    pub optics: [f32; 16],
    #[gpu]
    pub shading: [f32; 16],
    #[gpu]
    pub wind: [f32; 4],
    #[gpu]
    pub reserved: [f32; 4],
}

impl WaterVolumeSourceRow {
    pub fn of(water: &WaterVolumeComponent) -> Self {
        if !water.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let size = [
            water.size[0],
            water.size[1],
            water.size[2],
            water.surface_height_offset,
        ];
        let gpu = water.local_gpu();
        // The pass row after its two bound vec4s.
        let after_bounds = &bytemuck::bytes_of(&gpu)[32..];
        read(&[bytemuck::bytes_of(&size), after_bounds].concat())
    }
}

/// Foliage: the foliage type row (density, size, slope and altitude ranges,
/// LOD distances and wind response in `type_params`; the interaction
/// stiffness, material, density layer and kind/flags words in `type_ids`;
/// the impostor id, colour and ground mask in `type_tail`; blades a tuft,
/// lean, curve, fall, tip colour and patches in `type_tuft`), its layer
/// (half extent, infinite-extent flag, altitude min and max) and its wind
/// row. A disabled
/// component derives a zero row, which the join skips (it places only rows
/// with a density).
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

impl FoliageSourceRow {
    pub fn of(foliage: &FoliageComponent) -> Self {
        if !foliage.general.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let foliage_type = super::foliage_component::runtime::gpu_type(foliage);
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
        let wind = super::foliage_component::runtime::wind(foliage);
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

/// `bytes`, which must be exactly a `T`, as a `T`.
fn read<T: bytemuck::Pod>(bytes: &[u8]) -> T {
    bytemuck::pod_read_unaligned(bytes)
}

impl GlobalFogSourceRow {
    pub fn of(fog: &GlobalFogComponent) -> Self {
        let mut medium = fog.medium.to_medium();
        medium.enabled = u32::from(fog.enabled);
        read(bytemuck::bytes_of(&medium))
    }
}

impl LocalFogSourceRow {
    pub fn of(fog: &LocalFogVolumeComponent) -> Self {
        let mut medium = fog.medium.to_medium();
        medium.enabled = u32::from(fog.enabled);
        let words: [f32; 20] = bytemuck::cast(medium);
        Self {
            size: [fog.size[0], fog.size[1], fog.size[2], 0.0],
            medium: read(bytemuck::cast_slice(&words[..16])),
            planet: read(bytemuck::cast_slice(&words[16..])),
            edge_fade: [fog.edge_fade.max(0.0), 0.0, 0.0, 0.0],
        }
    }
}

impl PostProcessVolumeSourceRow {
    pub fn of(volume: &PostProcessVolumeComponent) -> Self {
        let size = [volume.size[0], volume.size[1], volume.size[2], 0.0];
        let mut row: Self = bytemuck::Zeroable::zeroed();
        if volume.enabled {
            let pass = helio_pass_postprocess::PostProcessVolumeComponent::from(
                volume.local_descriptor().to_gpu(),
            );
            // The pass row after its two bound vec4s.
            let after_bounds = &bytemuck::bytes_of(&pass)[32..];
            row = read(&[bytemuck::bytes_of(&size), after_bounds].concat());
        }
        row.size = size;
        row
    }
}

impl CameraPostProcessSourceRow {
    pub fn of(camera: &CameraPostProcessComponent) -> Self {
        let mut row = helio_pass_postprocess::CameraPostProcessComponent::new(
            camera.view_id,
            &camera.settings.to_settings(),
        );
        row.enabled = u32::from(camera.enabled);
        read(bytemuck::bytes_of(&row))
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
    GlobalFogComponent,
    GlobalFogSourceRow,
    global_fog_dispatch,
    global_fog_clear
);
derived_row!(
    LocalFogVolumeComponent,
    LocalFogSourceRow,
    local_fog_dispatch,
    local_fog_clear
);
derived_row!(
    PostProcessVolumeComponent,
    PostProcessVolumeSourceRow,
    post_process_volume_dispatch,
    post_process_volume_clear
);
derived_row!(
    CameraPostProcessComponent,
    CameraPostProcessSourceRow,
    camera_post_process_dispatch,
    camera_post_process_clear
);
derived_row!(
    FoliageComponent,
    FoliageSourceRow,
    foliage_dispatch,
    foliage_clear
);
derived_row!(
    AtmosphereComponent,
    AtmosphereSourceRow,
    atmosphere_dispatch,
    atmosphere_clear
);
derived_row!(
    WindComponent,
    GlobalWindSourceRow,
    wind_dispatch,
    wind_clear
);
derived_row!(
    ParticleEmitterComponent,
    CoronaEmitterSourceRow,
    particle_emitter_dispatch,
    particle_emitter_clear
);
// A decal's image is registered in the scene's texture store as its row is
// derived, so the row carries its bindless slot. The world loaded the image
// (a texture asset) and bound it into the decal; nothing reads the file here.
fn decal_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `DecalComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let decal = unsafe { &*(data as *const DecalComponent) };
    let albedo_slot = crate::texture_assets::texture_slot(
        &decal.albedo_image,
        &decal.albedo_texture,
        mirror,
        "decal",
    );
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &DecalSourceRow::of(decal, albedo_slot),
        is_new_insert,
    );
}

// A sprite's image is registered in the scene's texture store as its row is
// derived, so the row carries its bindless slot.
fn sprite_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `SpriteComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let sprite = unsafe { &*(data as *const SpriteComponent) };
    let slot = if sprite.enabled {
        crate::texture_assets::texture_slot(&sprite.image, &sprite.texture, mirror, "sprite")
    } else {
        u32::MAX
    };
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &SpriteSourceRow::of(sprite, slot),
        is_new_insert,
    );
}

fn sprite_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<SpriteSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<SpriteComponent>,
        dispatch: sprite_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<SpriteComponent>,
        clear: sprite_clear,
    }
}

fn decal_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<DecalSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<DecalComponent>,
        dispatch: decal_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<DecalComponent>,
        clear: decal_clear,
    }
}

derived_row!(
    WaterVolumeComponent,
    WaterVolumeSourceRow,
    water_volume_dispatch,
    water_volume_clear
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_have_the_documented_word_counts() {
        assert_eq!(std::mem::size_of::<GlobalFogSourceRow>(), 20 * 4);
        assert_eq!(std::mem::size_of::<LocalFogSourceRow>(), 28 * 4);
        assert_eq!(std::mem::size_of::<PostProcessVolumeSourceRow>(), 144 * 4);
        assert_eq!(std::mem::size_of::<CameraPostProcessSourceRow>(), 136 * 4);
        assert_eq!(std::mem::size_of::<WaterVolumeSourceRow>(), 60 * 4);
        assert_eq!(std::mem::size_of::<FoliageSourceRow>(), 48 * 4);
        assert_eq!(std::mem::size_of::<AtmosphereSourceRow>(), 60 * 4);
        assert_eq!(std::mem::size_of::<DecalSourceRow>(), 36 * 4);
        assert_eq!(std::mem::size_of::<CoronaEmitterSourceRow>(), 60 * 4);
        assert_eq!(std::mem::size_of::<GlobalWindSourceRow>(), 12 * 4);
        assert_eq!(std::mem::size_of::<SpriteSourceRow>(), 20 * 4);
        assert_eq!(
            std::mem::size_of::<helio_pass_sprite_batch::SpriteComponent>(),
            20 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_corona::GpuCoronaEmitter>(),
            60 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_decal::DecalComponent>(),
            32 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_sky::AtmosphereComponent>(),
            60 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_foliage_place::components::FoliageTypeComponent>(),
            32 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_foliage_place::components::FoliageWindComponent>(),
            12 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_water_sim::WaterVolumeComponent>(),
            64 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_volumetric_fog::GlobalFogComponent>(),
            20 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_volumetric_fog::LocalFogVolumeComponent>(),
            32 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_postprocess::PostProcessVolumeComponent>(),
            148 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_postprocess::CameraPostProcessComponent>(),
            136 * 4
        );
    }

    #[test]
    fn disabled_components_derive_inert_rows() {
        let enabled = |row: &GlobalFogSourceRow| row.medium[0].to_bits();
        let mut fog = GlobalFogComponent::default();
        assert_eq!(enabled(&GlobalFogSourceRow::of(&fog)), 1);
        fog.enabled = false;
        assert_eq!(enabled(&GlobalFogSourceRow::of(&fog)), 0);

        let mut camera = CameraPostProcessComponent::default();
        camera.view_id = 7;
        let row = CameraPostProcessSourceRow::of(&camera);
        assert_eq!((row.header[0], row.header[1]), (7, 1));
        camera.enabled = false;
        assert_eq!(CameraPostProcessSourceRow::of(&camera).header[1], 0);

        let mut volume = PostProcessVolumeComponent::default();
        volume.enabled = false;
        let row = PostProcessVolumeSourceRow::of(&volume);
        assert!(bytemuck::bytes_of(&row)[16..].iter().all(|byte| *byte == 0));

        let mut water = WaterVolumeComponent::default();
        water.enabled = false;
        let row = WaterVolumeSourceRow::of(&water);
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));

        let mut decal = DecalComponent::default();
        assert_eq!(
            DecalSourceRow::of(&decal, u32::MAX).size,
            [1.0, 1.0, 1.0, 0.0]
        );
        decal.enabled = false;
        let row = DecalSourceRow::of(&decal, u32::MAX);
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));

        let emitter = ParticleEmitterComponent {
            max_particles: 300,
            ..Default::default()
        };
        // The join's gate word (the requested range) and the range itself.
        assert_eq!(CoronaEmitterSourceRow::of(&emitter).range[2], 300);
        let row = CoronaEmitterSourceRow::of(&ParticleEmitterComponent {
            enabled: false,
            ..emitter
        });
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));

        let mut atmosphere = AtmosphereComponent::default();
        assert_eq!(AtmosphereSourceRow::of(&atmosphere).shell[3].to_bits(), 1);
        atmosphere.enabled = false;
        assert_eq!(AtmosphereSourceRow::of(&atmosphere).shell[3].to_bits(), 0);
    }

    #[test]
    fn an_atmosphere_row_is_the_pass_row_in_its_owners_space() {
        let atmosphere = AtmosphereComponent {
            placement: super::super::AtmospherePlacement::PlanetAtOwner,
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
    fn a_foliage_row_holds_its_type_layer_and_wind() {
        let mut foliage = FoliageComponent::default();
        foliage.general.enabled = true;
        foliage.general.density = 12.0;
        foliage.placement.layer_extent = 25.0;
        let row = FoliageSourceRow::of(&foliage);
        let foliage_type = super::super::foliage_component::runtime::gpu_type(&foliage);
        let type_bytes = bytemuck::bytes_of(&foliage_type);
        assert_eq!(&bytemuck::bytes_of(&row)[..type_bytes.len()], type_bytes);
        assert_eq!(row.type_params[0], 12.0, "density first: the join's gate");
        assert_eq!(row.layer[0], 25.0);
        foliage.general.enabled = false;
        let row = FoliageSourceRow::of(&foliage);
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_water_row_is_the_pass_row_after_its_bounds() {
        let mut water = WaterVolumeComponent::default();
        water.surface_height_offset = 2.5;
        let row = WaterVolumeSourceRow::of(&water);
        assert_eq!(
            &bytemuck::bytes_of(&row)[16..],
            &bytemuck::bytes_of(&water.local_gpu())[32..]
        );
        assert_eq!(row.size, [200.0, 60.0, 200.0, 2.5]);
    }

    #[test]
    fn a_post_process_volume_row_is_the_pass_row_after_its_bounds() {
        let volume = PostProcessVolumeComponent::default();
        let row = PostProcessVolumeSourceRow::of(&volume);
        let pass = helio_pass_postprocess::PostProcessVolumeComponent::from(
            volume.local_descriptor().to_gpu(),
        );
        assert_eq!(
            &bytemuck::bytes_of(&row)[16..],
            &bytemuck::bytes_of(&pass)[32..]
        );
        assert_eq!(row.size[..3], volume.size);
    }
}
