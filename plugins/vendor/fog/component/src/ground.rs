//! A global fog follows the ground it sits on: a component of the same
//! object that knows its ground (a voxel terrain, say) tells it the ground's
//! form and its air through [`GlobalFogComponent::follow_ground`], the
//! fog's own method, called by name. The fog decides what haze that air
//! makes.

use pulsar_scenedb::{Entity, World};

use crate::{FogMode, GlobalFogComponent, MediumProps};

/// The ground haze `air` (`Custom`, `None`, `Thin`, `Earthlike`, `Dense`)
/// describes, over `fog`'s planet (`planet_center` and `planet_radius`
/// kept): `None` for `Custom` (the authored medium stays); `None` air is no
/// haze. A preset's haze thins with altitude over the ground, scattering
/// forward, of the air's aerosols' colour; `density` and `haze` scale its
/// extinction as they scale the sky's aerosols.
pub fn haze_for_air(
    fog: &GlobalFogComponent,
    air: &str,
    density: f32,
    haze: f32,
) -> Result<Option<GlobalFogComponent>, String> {
    // Extinction (1/m), its falloff with altitude (1/m), anisotropy and
    // the share it scatters, per channel.
    let (extinction, falloff, anisotropy, albedo) = match air {
        "Custom" => return Ok(None),
        "None" => {
            return Ok(Some(GlobalFogComponent {
                enabled: false,
                ..fog.clone()
            }));
        }
        // A clear day's boundary layer: kilometres of visibility,
        // thinning over a few hundred metres.
        "Earthlike" => (6.0e-4, 4.0e-3, 0.6, [1.0; 3]),
        // Fine butterscotch dust lofted high in thin air.
        "Thin" => (4.0e-4, 1.2e-3, 0.65, [0.95, 0.78, 0.6]),
        // A deep orange smog hugging the ground.
        "Dense" => (4.0e-3, 8.0e-4, 0.5, [0.9, 0.65, 0.35]),
        other => return Err(format!("unknown air {other:?}")),
    };
    let extinction = extinction * density.max(0.0) * haze.max(0.0);
    Ok(Some(GlobalFogComponent {
        enabled: extinction > 0.0,
        medium: MediumProps {
            mode: FogMode::HeightBased,
            extinction,
            albedo,
            emission: [0.0; 3],
            anisotropy,
            height_falloff: falloff,
            height: 0.0,
            ..fog.medium.clone()
        },
    }))
}

/// `fog` over a ground of `sphere` form and `planet_radius_m` with `air`: a
/// sphere's fog measures heights over it, a flat ground's along world Y.
pub fn haze_on_ground(
    fog: &GlobalFogComponent,
    sphere: bool,
    planet_radius_m: f64,
    air: &str,
    density: f32,
    haze: f32,
) -> Result<GlobalFogComponent, String> {
    let mut placed = fog.clone();
    placed.medium.planet_center = [0.0; 3];
    placed.medium.planet_radius = if sphere { planet_radius_m as f32 } else { 0.0 };
    Ok(haze_for_air(&placed, air, density, haze)?.unwrap_or(placed))
}

#[pulsar_scenedb::component_methods]
#[allow(clippy::too_many_arguments)]
impl GlobalFogComponent {
    /// Sit on the ground of the object it is on: `sphere` (else flat) of
    /// `planet_radius_m`, with `air` (`Custom`, `None`, `Thin`,
    /// `Earthlike`, `Dense`) scaled by `density` and `haze` (the arguments
    /// every ground dependent takes; a haze has no use for the air's
    /// `tint_r/g/b`, the sky's colour). Returns whether the fog changed.
    #[world_method(category = "Fog")]
    fn follow_ground(
        world: &mut World,
        entity: Entity,
        sphere: bool,
        planet_radius_m: f64,
        air: String,
        density: f64,
        _tint_r: f64,
        _tint_g: f64,
        _tint_b: f64,
        haze: f64,
    ) -> Result<bool, String> {
        let fog = world
            .get::<GlobalFogComponent>(entity)
            .ok_or("the object has no global fog")?;
        let wanted = haze_on_ground(
            fog,
            sphere,
            planet_radius_m,
            &air,
            density as f32,
            haze as f32,
        )?;
        if fog.enabled == wanted.enabled && fog.medium.to_medium() == wanted.medium.to_medium() {
            return Ok(false);
        }
        if let Some(mut fog) = world.get_mut::<GlobalFogComponent>(entity) {
            *fog = wanted;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Earth's air makes the haze Helio's voxel worlds fly through, over
    /// the planet the fog was placed on; the air's amounts scale it.
    #[test]
    fn a_worlds_air_derives_its_ground_haze() {
        use crate::{GlobalFogComponent, MediumProps};
        use helio_pass_volumetric_fog::GlobalFogRow as GlobalMedium;
        let placed = GlobalFogComponent {
            medium: MediumProps {
                planet_radius: 6_371_000.0,
                ..MediumProps::default()
            },
            ..GlobalFogComponent::default()
        };
        assert!(
            haze_for_air(&placed, "Custom", 1.0, 1.0).unwrap().is_none(),
            "custom keeps the authored medium"
        );
        assert!(
            !haze_for_air(&placed, "None", 1.0, 1.0)
                .unwrap()
                .unwrap()
                .enabled,
            "no air, no haze"
        );
        let earth = haze_for_air(&placed, "Earthlike", 1.0, 1.0)
            .unwrap()
            .unwrap();
        let flight = GlobalMedium {
            mode: 1,
            extinction: 0.0006,
            height_falloff: 0.004,
            anisotropy: 0.6,
            ..GlobalMedium::default()
        }
        .around_planet([0.0; 3], 6_371_000.0);
        assert!(earth.enabled);
        assert_eq!(earth.medium.to_medium(), flight);
        let hazy = haze_for_air(&placed, "Earthlike", 0.5, 4.0)
            .unwrap()
            .unwrap();
        assert!(
            (hazy.medium.extinction - 0.0012).abs() < 1e-9,
            "half the air, four times the haze"
        );
        assert!(
            !haze_for_air(&placed, "Earthlike", 1.0, 0.0)
                .unwrap()
                .unwrap()
                .enabled,
            "no haze at all"
        );
    }
}
