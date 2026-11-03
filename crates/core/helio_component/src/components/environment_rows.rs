//! The rows atmospheres cast, as the renderer's environment join reads them
//! (Pulsar-Native#1035, Phase 4). (Fog, post-process volumes, water,
//! foliage and the wind are their plugins' now: each plugin's component
//! crate derives its own rows.)
//!
//! A row is a second GPU registration on its authored component: SceneDB
//! derives the row from the authored value on every insert, guarded write,
//! removal, despawn and mirror replay, keyed by the component instance
//! entity. A row is the pass's own row in the component's local space: the
//! graph-owned join gates it on the instance's enabled state and its owner's
//! generation, and places it with the owner's transform, on the GPU. A
//! disabled component derives an inert row (its pass skips it).
//!
//! | Row | Words | Pass row it becomes |
//! |---|---|---|
//! | [`AtmosphereSourceRow`] | 4 centre + 16 media + 4 ground + 4 shell + 16 clouds + 16 weather: the row, its centre in the owner's space | `helio_pass_sky::AtmosphereRow` (60) |

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb_derive::SceneStore;

use super::AtmosphereComponent;

pub const ATMOSPHERE_SOURCES_BUFFER: &str = "atmosphere_sources";

/// An atmosphere (`helio_pass_sky::AtmosphereRow` pass row, bit for
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

/// `bytes`, which must be exactly a `T`, as a `T`.
fn read<T: bytemuck::Pod>(bytes: &[u8]) -> T {
    bytemuck::pod_read_unaligned(bytes)
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
    AtmosphereComponent,
    AtmosphereSourceRow,
    atmosphere_dispatch,
    atmosphere_clear
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_have_the_documented_word_counts() {
        assert_eq!(std::mem::size_of::<AtmosphereSourceRow>(), 60 * 4);
        assert_eq!(std::mem::size_of::<helio_pass_sky::AtmosphereRow>(), 60 * 4);
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
}
