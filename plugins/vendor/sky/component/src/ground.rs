//! An atmosphere follows the ground it sits on: a component of the same
//! object that knows its ground (a voxel terrain, say) tells it the ground's
//! form and its air through [`AtmosphereComponent::follow_ground`], the
//! sky's own method, called by name. The sky decides what that air is: its
//! presets ([`AirPreset`]) are the sky's vocabulary, not the ground's.

use pulsar_scenedb::{Entity, World};

use crate::{AtmosphereComponent, AtmospherePlacement};
use helio_pass_sky::AtmosphereRow;

/// The air a ground has, by name (`follow_ground`'s `air`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirPreset {
    /// The authored sky stays as it is (only placed on the ground).
    Custom,
    /// No air: the sky is off.
    None,
    /// A thin, dusty air (Mars-like).
    Thin,
    /// Earth's air.
    Earthlike,
    /// A deep haze over a tall atmosphere (Titan-like).
    Dense,
}

impl AirPreset {
    /// `name` as `follow_ground` takes it (`"Earthlike"`, ...).
    pub fn from_name(name: &str) -> Result<Self, String> {
        Ok(match name {
            "Custom" => Self::Custom,
            "None" => Self::None,
            "Thin" => Self::Thin,
            "Earthlike" => Self::Earthlike,
            "Dense" => Self::Dense,
            other => return Err(format!("unknown air {other:?}")),
        })
    }
}

/// The sky `air` describes, on `sky`'s planet (placement, radius, ground and
/// sun kept): `None` for [`AirPreset::Custom`] (the authored sky stays);
/// [`AirPreset::None`] is a disabled sky. `density` scales every
/// coefficient, `tint` the molecular scattering per channel (the sky's
/// colour), `haze` the aerosols. The clouds stay as authored.
pub fn sky_for_air(
    sky: &AtmosphereComponent,
    air: AirPreset,
    density: f32,
    tint: [f32; 3],
    haze: f32,
) -> Option<AtmosphereComponent> {
    let preset = match air {
        AirPreset::Custom => return None,
        AirPreset::None => {
            return Some(AtmosphereComponent {
                enabled: false,
                ..sky.clone()
            });
        }
        AirPreset::Earthlike => AtmosphereComponent::from_row(&AtmosphereRow::earth()),
        AirPreset::Thin => AtmosphereComponent::from_row(&AtmosphereRow::mars()),
        AirPreset::Dense => {
            let earth = AtmosphereComponent::from_row(&AtmosphereRow::earth());
            AtmosphereComponent {
                thickness_km: 200.0,
                rayleigh_scattering: earth.rayleigh_scattering.map(|v| v * 1.5),
                rayleigh_scale_height_km: 20.0,
                mie_scattering: earth.mie_scattering.map(|v| v * 12.0),
                mie_absorption: [0.012, 0.018, 0.035],
                mie_scale_height_km: 6.0,
                mie_anisotropy: 0.6,
                ozone_absorption: [0.0; 3],
                ..earth
            }
        }
    };
    let density = density.max(0.0);
    let haze = haze.max(0.0);
    let times = |v: [f32; 3], k: [f32; 3]| [v[0] * k[0], v[1] * k[1], v[2] * k[2]];
    Some(AtmosphereComponent {
        enabled: density > 0.0,
        placement: sky.placement,
        planet_radius_km: sky.planet_radius_km,
        sun_angular_radius_deg: sky.sun_angular_radius_deg,
        ground_albedo: sky.ground_albedo,
        rayleigh_scattering: times(
            preset.rayleigh_scattering,
            tint.map(|t| t.max(0.0) * density),
        ),
        mie_scattering: preset.mie_scattering.map(|v| v * density * haze),
        mie_absorption: preset.mie_absorption.map(|v| v * density * haze),
        ozone_absorption: preset.ozone_absorption.map(|v| v * density),
        // The clouds stay as authored.
        clouds_enabled: sky.clouds_enabled,
        cloud_bottom_km: sky.cloud_bottom_km,
        cloud_top_km: sky.cloud_top_km,
        cloud_coverage: sky.cloud_coverage,
        cloud_type: sky.cloud_type,
        cloud_extinction_per_km: sky.cloud_extinction_per_km,
        cloud_weather_km: sky.cloud_weather_km,
        cloud_shape_km: sky.cloud_shape_km,
        cloud_detail_km: sky.cloud_detail_km,
        cloud_erosion: sky.cloud_erosion,
        cloud_wind: sky.cloud_wind,
        cloud_albedo: sky.cloud_albedo,
        cloud_voxels: sky.cloud_voxels,
        cloud_voxel_m: sky.cloud_voxel_m,
        cloud_voxel_fill: sky.cloud_voxel_fill,
        cloud_pole: sky.cloud_pole,
        cloud_circulation: sky.cloud_circulation,
        cloud_circulation_cells: sky.cloud_circulation_cells,
        cloud_synoptic_km: sky.cloud_synoptic_km,
        cloud_storminess: sky.cloud_storminess,
        cloud_mesoscale_km: sky.cloud_mesoscale_km,
        cloud_cirrus: sky.cloud_cirrus,
        cloud_cirrus_km: sky.cloud_cirrus_km,
        cloud_evolution: sky.cloud_evolution,
        cloud_seed: sky.cloud_seed,
        ..preset
    })
}

/// `sky` on a ground of `sphere` form and `planet_radius_m` with `air`: a
/// sphere's atmosphere is centred on its owner at the planet's radius; a
/// flat ground's sits at the origin, at the Earth's radius unless it was
/// already flat (its curvature is the author's).
pub fn sky_on_ground(
    sky: &AtmosphereComponent,
    sphere: bool,
    planet_radius_m: f64,
    air: AirPreset,
    density: f32,
    tint: [f32; 3],
    haze: f32,
) -> AtmosphereComponent {
    let (placement, radius_km) = if sphere {
        (
            AtmospherePlacement::PlanetAtOwner,
            (planet_radius_m / 1000.0) as f32,
        )
    } else if sky.placement == AtmospherePlacement::GroundAtOrigin {
        (AtmospherePlacement::GroundAtOrigin, sky.planet_radius_km)
    } else {
        (
            AtmospherePlacement::GroundAtOrigin,
            AtmosphereComponent::default().planet_radius_km,
        )
    };
    let placed = AtmosphereComponent {
        placement,
        planet_radius_km: radius_km,
        ..sky.clone()
    };
    sky_for_air(&placed, air, density, tint, haze).unwrap_or(placed)
}

/// Whether two skies are the same air (every property).
pub fn same_sky(a: &AtmosphereComponent, b: &AtmosphereComponent) -> bool {
    a.enabled == b.enabled && a.placement == b.placement && a.to_row() == b.to_row()
}

#[pulsar_scenedb::component_methods]
#[allow(clippy::too_many_arguments)]
impl AtmosphereComponent {
    /// Sit on the ground of the object it is on: `sphere` (else flat) of
    /// `planet_radius_m`, with `air` (`Custom`, `None`, `Thin`,
    /// `Earthlike`, `Dense`) scaled by `density`, tinted by
    /// `tint_r/g/b` and hazed by `haze`. Returns whether the sky changed.
    #[world_method(category = "Atmosphere")]
    fn follow_ground(
        world: &mut World,
        entity: Entity,
        sphere: bool,
        planet_radius_m: f64,
        air: String,
        density: f64,
        tint_r: f64,
        tint_g: f64,
        tint_b: f64,
        haze: f64,
    ) -> Result<bool, String> {
        let air = AirPreset::from_name(&air)?;
        let sky = world
            .get::<AtmosphereComponent>(entity)
            .ok_or("the object has no atmosphere")?;
        let wanted = sky_on_ground(
            sky,
            sphere,
            planet_radius_m,
            air,
            density as f32,
            [tint_r as f32, tint_g as f32, tint_b as f32],
            haze as f32,
        );
        if same_sky(sky, &wanted) {
            return Ok(false);
        }
        if let Some(mut sky) = world.get_mut::<AtmosphereComponent>(entity) {
            *sky = wanted;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ground's air derives its sky on the sky's planet; the clouds stay
    /// as authored.
    #[test]
    fn a_worlds_air_derives_its_sky() {
        use crate::{AtmosphereComponent, AtmospherePlacement};
        use helio_pass_sky::AtmosphereRow;
        let sky = AtmosphereComponent {
            placement: AtmospherePlacement::PlanetAtOwner,
            planet_radius_km: 1737.0,
            clouds_enabled: true,
            cloud_coverage: 0.7,
            ..Default::default()
        };
        assert!(
            sky_for_air(&sky, AirPreset::Custom, 1.0, [1.0; 3], 1.0).is_none(),
            "custom keeps the authored sky"
        );
        assert!(
            !sky_for_air(&sky, AirPreset::None, 1.0, [1.0; 3], 1.0)
                .unwrap()
                .enabled,
            "an airless world has no sky"
        );
        let earth = sky_for_air(&sky, AirPreset::Earthlike, 1.0, [1.0; 3], 1.0).unwrap();
        assert!(earth.enabled);
        assert_eq!(
            (earth.placement, earth.planet_radius_km),
            (AtmospherePlacement::PlanetAtOwner, 1737.0),
            "the planet stays"
        );
        assert_eq!(
            earth.rayleigh_scattering,
            AtmosphereRow::earth().rayleigh_scattering
        );
        let thin = sky_for_air(&sky, AirPreset::Earthlike, 0.5, [1.0, 0.5, 1.0], 2.0).unwrap();
        assert!((thin.rayleigh_scattering[1] - earth.rayleigh_scattering[1] * 0.25).abs() < 1e-9);
        assert!(
            (thin.mie_scattering[0] - earth.mie_scattering[0]).abs() < 1e-9,
            "half the air, twice the haze"
        );
        assert!(
            !sky_for_air(&sky, AirPreset::Earthlike, 0.0, [1.0; 3], 1.0)
                .unwrap()
                .enabled,
            "no air at all"
        );
        let air = sky_for_air(&sky, AirPreset::Thin, 1.0, [1.0; 3], 1.0).unwrap();
        assert!(
            air.clouds_enabled && air.cloud_coverage == 0.7,
            "a world's air keeps its clouds"
        );
    }
}
