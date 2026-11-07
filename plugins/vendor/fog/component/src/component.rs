//! Physical media authoring. The authored values reach the volumetric fog
//! pass through their derived rows ([`crate::GlobalFogSourceRow`],
//! [`crate::LocalFogSourceRow`]) and the plugin's row join; nothing here
//! writes pass rows.
use engine_class_derive::{engine_class, register_world_component};
use helio_pass_volumetric_fog::GlobalFogRow as GlobalMedium;
use helio_pass_volumetric_fog::components::fog_mode;
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

/// How a medium's density varies through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum FogMode {
    /// Constant density everywhere inside the medium.
    Uniform,
    /// Density decays exponentially with height above the medium's base.
    HeightBased,
    /// Animated world-space billows with height falloff.
    Smoke,
}

impl Default for FogMode {
    fn default() -> Self {
        Self::Uniform
    }
}

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
                FogMode::Uniform => fog_mode::UNIFORM,
                FogMode::HeightBased => fog_mode::HEIGHT,
                FogMode::Smoke => fog_mode::SMOKE,
            },
            extinction: self.extinction,
            albedo: self.albedo,
            emission: self.emission,
            anisotropy: self.anisotropy,
            height_falloff: self.height_falloff,
            height: self.height,
            planet: [
                self.planet_center[0],
                self.planet_center[1],
                self.planet_center[2],
                self.planet_radius,
            ],
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
