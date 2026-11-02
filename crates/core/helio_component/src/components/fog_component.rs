//! Physical media authoring. The authored values reach the volumetric fog
//! pass through their derived rows ([`super::environment_rows`]) and the
//! graph's environment join; nothing here writes pass rows.
use super::FogMode;
use engine_class_derive::{engine_class, register_world_component};
use helio_pass_volumetric_fog::GlobalFogComponent as GlobalMedium;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Medium", category_color = "#D18F6F")]
#[serde(default)]
pub struct MediumProps {
    #[property(category = "Medium")]
    pub mode: FogMode,
    /// Absorption plus scattering, in inverse metres. One world unit is one metre.
    #[property(min = 0.0, max = 100.0, step = 0.001, category = "Medium")]
    pub extinction: f32,
    /// Fraction of extinction that scatters, per color channel.
    #[property(category = "Medium")]
    pub albedo: [f32; 3],
    /// Scene-linear radiance emitted per metre; independent of extinction.
    #[property(category = "Medium")]
    pub emission: [f32; 3],
    #[property(min = -0.999, max = 0.999, step = 0.001, category = "Medium")]
    pub anisotropy: f32,
    #[property(min = 0.0, max = 100.0, step = 0.001, category = "Medium")]
    pub height_falloff: f32,
    /// Reference height in metres: along world Y, or an altitude over the
    /// planet when `planet_radius` is set.
    #[property(category = "Medium")]
    pub height: f32,
    /// The planet heights are measured above: its centre (world metres).
    #[property(category = "Medium")]
    pub planet_center: [f32; 3],
    /// The planet's radius in metres; 0: heights along world Y.
    #[property(min = 0.0, step = 1.0, category = "Medium")]
    pub planet_radius: f32,
}

impl Default for MediumProps {
    fn default() -> Self {
        let value = GlobalMedium::default();
        Self {
            mode: FogMode::Uniform,
            extinction: value.extinction,
            albedo: value.albedo,
            emission: value.emission,
            anisotropy: value.anisotropy,
            height_falloff: value.height_falloff,
            height: value.height,
            planet_center: [value.planet[0], value.planet[1], value.planet[2]],
            planet_radius: value.planet[3],
        }
    }
}

impl MediumProps {
    pub fn to_medium(&self) -> GlobalMedium {
        GlobalMedium {
            enabled: 1,
            mode: match self.mode {
                FogMode::Uniform => 0,
                FogMode::HeightBased => 1,
                FogMode::Smoke => 2,
            },
            extinction: self.extinction,
            albedo: self.albedo,
            emission: self.emission,
            anisotropy: self.anisotropy,
            height_falloff: self.height_falloff,
            height: self.height,
            planet: [self.planet_center[0], self.planet_center[1], self.planet_center[2], self.planet_radius],
            ..Default::default()
        }
    }
}

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Medium", category_color = "#D18F6F")]
#[category("Volume", category_color = "#8F8F8F")]
#[serde(default)]
pub struct GlobalFogComponent {
    #[property(category = "Medium")]
    pub enabled: bool,
    #[sub_props]
    #[serde(flatten)]
    pub medium: MediumProps,
}

impl Default for GlobalFogComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            medium: MediumProps::default(),
        }
    }
}

impl GlobalFogComponent {
    /// The ground haze a voxel world's `air` settings describe, over its
    /// planet (`planet_center` and `planet_radius` kept): `None` for
    /// `Custom` (the authored medium stays); `VoxelAir::None` is no haze. A
    /// preset's haze thins with altitude over the ground, scattering
    /// forward, of the air's aerosols' colour; `density` and `haze` scale
    /// its extinction as they scale the sky's aerosols
    /// ([`super::AtmosphereComponent::for_air`]).
    pub fn for_air(&self, air: crate::VoxelAir, density: f32, haze: f32) -> Option<Self> {
        use crate::VoxelAir;
        // Extinction (1/m), its falloff with altitude (1/m), anisotropy and
        // the share it scatters, per channel.
        let (extinction, falloff, anisotropy, albedo) = match air {
            VoxelAir::Custom => return None,
            VoxelAir::None => {
                return Some(Self {
                    enabled: false,
                    ..self.clone()
                });
            }
            // A clear day's boundary layer: kilometres of visibility,
            // thinning over a few hundred metres.
            VoxelAir::Earthlike => (6.0e-4, 4.0e-3, 0.6, [1.0; 3]),
            // Fine butterscotch dust lofted high in thin air.
            VoxelAir::Thin => (4.0e-4, 1.2e-3, 0.65, [0.95, 0.78, 0.6]),
            // A deep orange smog hugging the ground.
            VoxelAir::Dense => (4.0e-3, 8.0e-4, 0.5, [0.9, 0.65, 0.35]),
        };
        let extinction = extinction * density.max(0.0) * haze.max(0.0);
        Some(Self {
            enabled: extinction > 0.0,
            medium: MediumProps {
                mode: FogMode::HeightBased,
                extinction,
                albedo,
                emission: [0.0; 3],
                anisotropy,
                height_falloff: falloff,
                height: 0.0,
                ..self.medium.clone()
            },
        })
    }
}

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Medium", category_color = "#D18F6F")]
#[category("Volume", category_color = "#8F8F8F")]
#[serde(default)]
pub struct LocalFogVolumeComponent {
    #[property(category = "Medium")]
    pub enabled: bool,
    /// Full local extent in metres. The owner's scale and rotation produce
    /// the world AABB (the environment join, on the GPU).
    #[property(category = "Volume")]
    pub size: [f32; 3],
    /// Inward fade from the world AABB boundary, in metres.
    #[property(min = 0.0, max = 1000.0, step = 0.1, category = "Volume")]
    pub edge_fade: f32,
    #[sub_props]
    #[serde(flatten)]
    pub medium: MediumProps,
}

impl Default for LocalFogVolumeComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            size: [10.0; 3],
            edge_fade: 0.0,
            medium: MediumProps::default(),
        }
    }
}

#[register_world_component]
impl GlobalFogComponent {}

#[register_world_component]
impl LocalFogVolumeComponent {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VoxelAir;

    /// Earth's air makes the haze Helio's voxel worlds fly through, over
    /// the planet the fog was placed on; the air's amounts scale it.
    #[test]
    fn a_worlds_air_derives_its_ground_haze() {
        let placed = GlobalFogComponent {
            medium: MediumProps {
                planet_radius: 6_371_000.0,
                ..MediumProps::default()
            },
            ..GlobalFogComponent::default()
        };
        assert!(placed.for_air(VoxelAir::Custom, 1.0, 1.0).is_none(), "custom keeps the authored medium");
        assert!(!placed.for_air(VoxelAir::None, 1.0, 1.0).unwrap().enabled, "no air, no haze");
        let earth = placed.for_air(VoxelAir::Earthlike, 1.0, 1.0).unwrap();
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
        let hazy = placed.for_air(VoxelAir::Earthlike, 0.5, 4.0).unwrap();
        assert!((hazy.medium.extinction - 0.0012).abs() < 1e-9, "half the air, four times the haze");
        assert!(!placed.for_air(VoxelAir::Earthlike, 1.0, 0.0).unwrap().enabled, "no haze at all");
    }
}
