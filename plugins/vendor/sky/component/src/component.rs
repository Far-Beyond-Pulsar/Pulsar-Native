//! A planet's air: the sky, the sun's colour through it, the sky's light on
//! surfaces, aerial perspective and the clouds. Any scene may have one: a
//! planet (centred on the owner) or flat ground at the world's origin. The
//! sun is the scene's first directional light.
//!
//! SceneDB derives an [`crate::AtmosphereSourceRow`] from the authored
//! value; the sky's row join places it as the sky pass's
//! `helio_pass_sky::AtmosphereRow` row in `"atmospheres"`.
use engine_class_derive::{engine_class, register_world_component};
use helio_pass_sky::AtmosphereRow;
use helio_pass_sky::atmosphere::placement;
use helio_pass_sky::sky_texture::{self, source as sky_source};
use helio_pass_sky::{CloudLayer, CloudRepresentation, PlanetWeather};
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

/// Where the atmosphere's planet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum AtmospherePlacement {
    /// Flat ground at the world's origin: the planet's centre is straight
    /// below it, `planet_radius_km` down.
    GroundAtOrigin,
    /// A planet centred on the owner's position.
    PlanetAtOwner,
}

impl Default for AtmospherePlacement {
    fn default() -> Self {
        Self::GroundAtOrigin
    }
}

/// Which panorama the sky texture layer shows.
///
/// The built-in panoramas are the galaxy and nebula layers of Toolbench's
/// Deep-Space Skybox, CC0 1.0 (public domain; free for commercial use, no
/// credit required). They are starless: the procedural stars, shooting stars
/// and directional-light moon draw over them. Credits and license text:
/// `plugins/vendor/sky/feature/assets/deep-space/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum SkyTexturePreset {
    /// Galaxy band with faint blue and red nebula clouds. Deep-Space Skybox
    /// by Toolbench, CC0 1.0.
    GalaxyAndNebula,
    /// The dusty galaxy band alone. Deep-Space Skybox by Toolbench, CC0 1.0.
    Galaxy,
    /// Faint nebula clouds alone. Deep-Space Skybox by Toolbench, CC0 1.0.
    Nebula,
    /// Your own equirectangular panorama: set Texture File.
    CustomFile,
}

impl Default for SkyTexturePreset {
    fn default() -> Self {
        Self::GalaxyAndNebula
    }
}

/// Defaults are Earth's air (Hillaire 2020). Coefficients are per kilometre,
/// heights and radii in kilometres.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Atmosphere", category_color = "#6F9FD1")]
#[category("Planet", category_color = "#8F8F8F")]
#[category("Scattering", category_color = "#9FB7D1")]
#[category("Clouds", category_color = "#C9D3DE")]
#[category("Weather", category_color = "#A9B8C8")]
#[category("Night Sky", category_color = "#A5BCE8")]
#[category("Sky Texture", category_color = "#8FA8E0")]
#[category("Moon", category_color = "#C8C4B8")]
#[category("Shooting Stars", category_color = "#E6C68D")]
#[serde(default)]
pub struct AtmosphereComponent {
    #[property(category = "Atmosphere")]
    pub enabled: bool,
    #[property(category = "Planet")]
    pub placement: AtmospherePlacement,
    /// Radius of the planet's surface.
    #[property(min = 0.001, max = 1000000.0, step = 1.0, category = "Planet")]
    pub planet_radius_km: f32,
    /// Height of the top of the air above the surface.
    #[property(min = 0.001, max = 10000.0, step = 1.0, category = "Planet")]
    pub thickness_km: f32,
    /// Light the ground reflects back into the air.
    #[property(category = "Planet")]
    pub ground_albedo: [f32; 3],
    /// Half the sun's apparent diameter.
    #[property(min = 0.01, max = 20.0, step = 0.01, category = "Atmosphere")]
    pub sun_angular_radius_deg: f32,
    /// Molecular (Rayleigh) scattering at the surface: what makes the sky blue.
    #[property(category = "Scattering")]
    pub rayleigh_scattering: [f32; 3],
    #[property(min = 0.01, max = 1000.0, step = 0.1, category = "Scattering")]
    pub rayleigh_scale_height_km: f32,
    /// Aerosol (Mie) scattering at the surface: haze and the glow around the sun.
    #[property(category = "Scattering")]
    pub mie_scattering: [f32; 3],
    #[property(category = "Scattering")]
    pub mie_absorption: [f32; 3],
    #[property(min = 0.01, max = 1000.0, step = 0.1, category = "Scattering")]
    pub mie_scale_height_km: f32,
    /// Forward scattering of aerosols (0 isotropic, towards 1 forward).
    #[property(min = -0.999, max = 0.999, step = 0.001, category = "Scattering")]
    pub mie_anisotropy: f32,
    /// Ozone absorption at the layer's peak: the zenith's blue at dusk.
    #[property(category = "Scattering")]
    pub ozone_absorption: [f32; 3],
    #[property(min = 0.0, max = 1000.0, step = 0.1, category = "Scattering")]
    pub ozone_center_km: f32,
    #[property(min = 0.0, max = 1000.0, step = 0.1, category = "Scattering")]
    pub ozone_width_km: f32,
    /// A layer of clouds over the surface.
    #[property(category = "Clouds")]
    pub clouds_enabled: bool,
    /// The layer's bottom and top above the surface.
    #[property(min = 0.0, max = 100.0, step = 0.05, category = "Clouds")]
    pub cloud_bottom_km: f32,
    #[property(min = 0.0, max = 100.0, step = 0.05, category = "Clouds")]
    pub cloud_top_km: f32,
    /// The planet's mean share of sky covered: 0 clear, 1 overcast (the
    /// weather varies it from region to region).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Clouds")]
    pub cloud_coverage: f32,
    /// The clouds' mean form: 0 flat stratus, 1 towering cumulus (the
    /// weather's regimes vary it).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Clouds")]
    pub cloud_type: f32,
    /// How dense a cloud is inside: extinction per kilometre.
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "Clouds")]
    pub cloud_extinction_per_km: f32,
    /// Size of the groups cumulus gather in: the finest weather.
    #[property(min = 0.01, max = 10000.0, step = 0.5, category = "Clouds")]
    pub cloud_weather_km: f32,
    /// Size of the clouds' billows.
    #[property(min = 0.01, max = 1000.0, step = 0.1, category = "Clouds")]
    pub cloud_shape_km: f32,
    /// Size of the wisps eroding their edges.
    #[property(min = 0.001, max = 100.0, step = 0.01, category = "Clouds")]
    pub cloud_detail_km: f32,
    /// How far the wisps erode the edges.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Clouds")]
    pub cloud_erosion: f32,
    /// The wind carrying the clouds (world metres per second).
    #[property(category = "Clouds")]
    pub cloud_wind: [f32; 3],
    /// Share of light the clouds scatter rather than absorb.
    #[property(category = "Clouds")]
    pub cloud_albedo: [f32; 3],
    /// Blocky clouds: cubic cells, each filled or empty, instead of smooth
    /// volumes (the same weather, shadows and fog).
    #[property(category = "Clouds")]
    pub cloud_voxels: bool,
    /// The voxel cells' side.
    #[property(min = 0.1, max = 10000.0, step = 1.0, category = "Clouds")]
    pub cloud_voxel_m: f32,
    /// The density a voxel cell is filled from.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Clouds")]
    pub cloud_voxel_fill: f32,
    /// The planet's axis (world direction) the weather's latitudes are
    /// measured from.
    #[property(category = "Weather")]
    pub cloud_pole: [f32; 3],
    /// How strongly the circulation bands the cover by latitude: the
    /// tropical convergence and the storm tracks cloudy, the subtropics
    /// clear.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Weather")]
    pub cloud_circulation: f32,
    /// Circulation cells from the equator to each pole (Earth's three).
    #[property(min = 1.0, max = 12.0, step = 1.0, category = "Weather")]
    pub cloud_circulation_cells: f32,
    /// Size of the weather systems: cyclones, fronts and highs.
    #[property(min = 1.0, max = 100000.0, step = 10.0, category = "Weather")]
    pub cloud_synoptic_km: f32,
    /// How deep and frequent the cyclones are.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Weather")]
    pub cloud_storminess: f32,
    /// Size of the mesoscale cells the clouds organise in.
    #[property(min = 0.01, max = 10000.0, step = 1.0, category = "Weather")]
    pub cloud_mesoscale_km: f32,
    /// How much high cirrus the weather brings.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Weather")]
    pub cloud_cirrus: f32,
    /// Height of the cirrus sheet above the surface.
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Weather")]
    pub cloud_cirrus_km: f32,
    /// How fast the weather evolves: 0 still, 1 real time, more a time
    /// lapse.
    #[property(min = 0.0, max = 100000.0, step = 1.0, category = "Weather")]
    pub cloud_evolution: f32,
    /// Which weather: each seed lays out other systems.
    #[property(min = 0.0, max = 65535.0, step = 1.0, category = "Weather")]
    pub cloud_seed: u32,
    /// Draw stars and textured directional emitters while the sun is below the horizon.
    #[property(category = "Night Sky")]
    pub night_sky_enabled: bool,
    /// Approximate number of stars over the full sky.
    #[property(min = 0.0, max = 20000.0, step = 100.0, category = "Night Sky")]
    pub star_count: f32,
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Night Sky")]
    pub star_brightness: f32,
    /// Fraction of the stars that twinkle.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Night Sky")]
    pub star_twinkle_fraction: f32,
    #[property(min = 0.0, max = 10.0, step = 0.1, category = "Night Sky")]
    pub star_twinkle_speed: f32,
    #[property(min = 0.0, max = 65535.0, step = 1.0, category = "Night Sky")]
    pub star_seed: u32,
    #[property(category = "Shooting Stars")]
    pub shooting_stars_enabled: bool,
    /// Events per minute, visible hemisphere.
    #[property(min = 0.0, max = 20.0, step = 0.01, category = "Shooting Stars")]
    pub shooting_star_rate: f32,
    #[property(min = 0.05, max = 20.0, step = 0.05, category = "Shooting Stars")]
    pub shooting_star_speed: f32,
    /// Apparent trail length in radians.
    #[property(min = 0.005, max = 0.2, step = 0.005, category = "Shooting Stars")]
    pub shooting_star_length: f32,
    /// Show a 360 degree sky panorama behind the atmosphere, clouds, stars,
    /// moon and shooting stars. It does not replace or disable any of them,
    /// and it only decorates the sky: scene lighting comes from the
    /// directional lights, not from the texture. It fades out in daylight.
    #[property(category = "Sky Texture")]
    pub sky_texture_enabled: bool,
    /// The panorama: a built-in galaxy/nebula preset (CC0, Deep-Space Skybox
    /// by Toolbench) or your own file.
    #[property(category = "Sky Texture")]
    pub sky_texture_preset: SkyTexturePreset,
    /// For Custom File: an equirectangular (2:1) .hdr, .exr, .png or .jpg,
    /// relative to the project or absolute. .hdr/.exr are used as linear
    /// light; other formats as sRGB. Wider than 4096 px is halved to fit.
    #[property(category = "Sky Texture")]
    pub sky_texture_file: String,
    /// Brightness of the panorama. The built-in layers are physically faint,
    /// so the default is well above 1.
    #[property(min = 0.0, max = 1000.0, step = 0.5, category = "Sky Texture")]
    pub sky_texture_intensity: f32,
    /// How much of the panorama shows: 0 hides it, 1 adds it at full strength.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Sky Texture")]
    pub sky_texture_opacity: f32,
    /// Turns the panorama around the vertical axis, in degrees.
    #[property(min = -180.0, max = 180.0, step = 0.5, category = "Sky Texture")]
    pub sky_texture_yaw_deg: f32,
    /// Tilts the panorama toward or away from the horizon, in degrees.
    #[property(min = -180.0, max = 180.0, step = 0.5, category = "Sky Texture")]
    pub sky_texture_pitch_deg: f32,
    /// Rolls the panorama about the view axis, in degrees.
    #[property(min = -180.0, max = 180.0, step = 0.5, category = "Sky Texture")]
    pub sky_texture_roll_deg: f32,
}

impl Default for AtmosphereComponent {
    fn default() -> Self {
        Self::from_row(&AtmosphereRow::earth())
    }
}

impl AtmosphereComponent {
    /// The component whose row is `row` (its authored values; placed on
    /// flat ground).
    pub fn from_row(row: &AtmosphereRow) -> Self {
        Self {
            enabled: true,
            placement: AtmospherePlacement::GroundAtOrigin,
            planet_radius_km: row.bottom_radius,
            thickness_km: row.top_radius - row.bottom_radius,
            ground_albedo: row.ground_albedo,
            sun_angular_radius_deg: row.sun_angular_radius.to_degrees(),
            rayleigh_scattering: row.rayleigh_scattering,
            rayleigh_scale_height_km: row.rayleigh_scale_height,
            mie_scattering: row.mie_scattering,
            mie_absorption: row.mie_absorption,
            mie_scale_height_km: row.mie_scale_height,
            mie_anisotropy: row.mie_g,
            ozone_absorption: row.ozone_absorption,
            ozone_center_km: row.ozone_center,
            ozone_width_km: row.ozone_width,
            clouds_enabled: row.clouds_enabled != 0,
            cloud_bottom_km: row.cloud_bottom,
            cloud_top_km: row.cloud_top,
            cloud_coverage: row.cloud_coverage,
            cloud_type: row.cloud_type,
            cloud_extinction_per_km: row.cloud_extinction,
            cloud_weather_km: row.cloud_weather_scale,
            cloud_shape_km: row.cloud_shape_scale,
            cloud_detail_km: row.cloud_detail_scale,
            cloud_erosion: row.cloud_erosion,
            cloud_wind: row.cloud_wind,
            cloud_albedo: row.cloud_albedo,
            cloud_voxels: row.cloud_voxel_size > 0.0,
            cloud_voxel_m: if row.cloud_voxel_size > 0.0 {
                row.cloud_voxel_size * 1000.0
            } else {
                32.0
            },
            cloud_voxel_fill: row.cloud_voxel_fill,
            cloud_pole: row.cloud_pole,
            cloud_circulation: row.cloud_circulation,
            cloud_circulation_cells: row.cloud_circulation_cells,
            cloud_synoptic_km: row.cloud_synoptic_scale,
            cloud_storminess: row.cloud_storminess,
            cloud_mesoscale_km: row.cloud_mesoscale_scale,
            cloud_cirrus: row.cloud_cirrus,
            cloud_cirrus_km: row.cloud_cirrus_height,
            cloud_evolution: row.cloud_evolution,
            cloud_seed: row.cloud_seed,
            night_sky_enabled: row.night_sky_enabled != 0,
            star_count: row.star_count,
            star_brightness: row.star_brightness,
            star_twinkle_fraction: row.star_twinkle_fraction,
            star_twinkle_speed: row.star_twinkle_speed,
            star_seed: row.star_seed,
            shooting_stars_enabled: row.shooting_stars_enabled != 0,
            shooting_star_rate: row.shooting_star_rate,
            shooting_star_speed: row.shooting_star_speed,
            shooting_star_length: row.shooting_star_length,
            sky_texture_enabled: row.sky_texture_source != sky_source::NONE,
            sky_texture_preset: match row.sky_texture_source {
                sky_source::GALAXY => SkyTexturePreset::Galaxy,
                sky_source::NEBULA => SkyTexturePreset::Nebula,
                code if code & sky_source::CUSTOM != 0 => SkyTexturePreset::CustomFile,
                _ => SkyTexturePreset::GalaxyAndNebula,
            },
            sky_texture_file: sky_texture::custom_path(row.sky_texture_source)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sky_texture_intensity: row.sky_texture_intensity,
            sky_texture_opacity: row.sky_texture_opacity,
            sky_texture_yaw_deg: row.sky_texture_yaw.to_degrees(),
            sky_texture_pitch_deg: row.sky_texture_pitch.to_degrees(),
            sky_texture_roll_deg: row.sky_texture_roll.to_degrees(),
        }
    }

    /// The sky texture source the row carries: nothing when the layer is
    /// off (or a custom file is unnamed), otherwise the preset or the file,
    /// resolved against the project folder when relative.
    pub fn sky_texture_source(&self) -> u32 {
        if !self.sky_texture_enabled {
            return sky_source::NONE;
        }
        match self.sky_texture_preset {
            SkyTexturePreset::GalaxyAndNebula => sky_source::GALAXY_NEBULA,
            SkyTexturePreset::Galaxy => sky_source::GALAXY,
            SkyTexturePreset::Nebula => sky_source::NEBULA,
            SkyTexturePreset::CustomFile => {
                let file = self.sky_texture_file.trim();
                if file.is_empty() {
                    return sky_source::NONE;
                }
                let path = std::path::Path::new(file);
                let resolved = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    match engine_state::get_project_path() {
                        Some(root) => std::path::PathBuf::from(root).join(path),
                        None => path.to_path_buf(),
                    }
                };
                sky_texture::custom_source(&resolved)
            }
        }
    }

    /// The authored cloud layer, whether or not it is enabled.
    pub fn cloud_layer(&self) -> CloudLayer {
        CloudLayer {
            bottom_km: self.cloud_bottom_km,
            top_km: self.cloud_top_km,
            coverage: self.cloud_coverage,
            kind: self.cloud_type,
            extinction: self.cloud_extinction_per_km,
            weather_km: self.cloud_weather_km,
            shape_km: self.cloud_shape_km,
            detail_km: self.cloud_detail_km,
            wind: self.cloud_wind,
            albedo: self.cloud_albedo,
            erosion: self.cloud_erosion,
            representation: if self.cloud_voxels {
                CloudRepresentation::Voxels {
                    cell_m: self.cloud_voxel_m,
                    fill: self.cloud_voxel_fill,
                }
            } else {
                CloudRepresentation::Volumetric
            },
            weather: PlanetWeather {
                pole: self.cloud_pole,
                circulation: self.cloud_circulation,
                circulation_cells: self.cloud_circulation_cells,
                synoptic_km: self.cloud_synoptic_km,
                storminess: self.cloud_storminess,
                mesoscale_km: self.cloud_mesoscale_km,
                cirrus: self.cloud_cirrus,
                cirrus_km: self.cloud_cirrus_km,
                evolution: self.cloud_evolution,
                seed: self.cloud_seed,
            },
        }
    }

    /// The pass row in the owner's space: a planet placed at its owner has
    /// a zero centre, which the environment join moves to the owner's world
    /// position ([`crate::AtmosphereSourceRow`]).
    pub fn to_row(&self) -> AtmosphereRow {
        let bottom = self.planet_radius_km.max(0.001);
        let placement = match self.placement {
            AtmospherePlacement::GroundAtOrigin => placement::GROUND_AT_ORIGIN,
            AtmospherePlacement::PlanetAtOwner => placement::CENTER,
        };
        let row = AtmosphereRow {
            center: [0.0; 3],
            placement,
            rayleigh_scattering: self.rayleigh_scattering,
            rayleigh_scale_height: self.rayleigh_scale_height_km.max(0.01),
            mie_scattering: self.mie_scattering,
            mie_scale_height: self.mie_scale_height_km.max(0.01),
            mie_absorption: self.mie_absorption,
            mie_g: self.mie_anisotropy.clamp(-0.999, 0.999),
            ozone_absorption: self.ozone_absorption,
            ozone_center: self.ozone_center_km,
            ground_albedo: self.ground_albedo,
            ozone_width: self.ozone_width_km.max(0.0),
            bottom_radius: bottom,
            top_radius: bottom + self.thickness_km.max(0.001),
            sun_angular_radius: self.sun_angular_radius_deg.to_radians(),
            enabled: u32::from(self.enabled),
            night_sky_enabled: u32::from(self.night_sky_enabled),
            star_count: self.star_count.clamp(0.0, 20000.0),
            star_brightness: self.star_brightness.max(0.0),
            star_twinkle_fraction: self.star_twinkle_fraction.clamp(0.0, 1.0),
            star_twinkle_speed: self.star_twinkle_speed.max(0.0),
            star_seed: self.star_seed,
            shooting_stars_enabled: u32::from(self.shooting_stars_enabled),
            shooting_star_rate: self.shooting_star_rate.max(0.0),
            shooting_star_speed: self.shooting_star_speed.max(0.05),
            shooting_star_length: self.shooting_star_length.clamp(0.005, 0.2),
            // The words once reserved for a moon carry the sky texture layer.
            // Sky emitters (the moon) are authored by LightComponents.
            sky_texture_source: self.sky_texture_source(),
            sky_texture_intensity: self.sky_texture_intensity.max(0.0),
            sky_texture_opacity: self.sky_texture_opacity.clamp(0.0, 1.0),
            sky_texture_yaw: self.sky_texture_yaw_deg.to_radians(),
            sky_texture_pitch: self.sky_texture_pitch_deg.to_radians(),
            sky_texture_roll: self.sky_texture_roll_deg.to_radians(),
            ..AtmosphereRow::earth()
        }
        .with_clouds(self.cloud_layer());
        AtmosphereRow {
            clouds_enabled: u32::from(self.clouds_enabled),
            ..row
        }
    }
}

#[register_world_component]
impl AtmosphereComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_earths_air_on_flat_ground() {
        let row = AtmosphereComponent::default().to_row();
        let earth = AtmosphereRow::earth();
        assert_eq!(row.placement, placement::GROUND_AT_ORIGIN);
        assert_eq!(row.center, [0.0; 3]);
        assert!((row.bottom_radius - earth.bottom_radius).abs() < 1e-3);
        assert!((row.top_radius - earth.top_radius).abs() < 1e-3);
        assert!((row.sun_angular_radius - earth.sun_angular_radius).abs() < 1e-6);
        assert_eq!(row.rayleigh_scattering, earth.rayleigh_scattering);
        assert_eq!(row.enabled, 1);
    }

    #[test]
    fn a_planet_is_placed_at_its_owner_by_the_join() {
        let component = AtmosphereComponent {
            placement: AtmospherePlacement::PlanetAtOwner,
            planet_radius_km: 1737.0,
            thickness_km: 40.0,
            ..Default::default()
        };
        let row = component.to_row();
        assert_eq!(row.placement, placement::CENTER);
        assert_eq!(
            row.center, [0.0; 3],
            "local: the join adds the owner's position"
        );
        assert_eq!((row.bottom_radius, row.top_radius), (1737.0, 1777.0));
        let disabled = AtmosphereComponent {
            enabled: false,
            ..component
        };
        assert_eq!(disabled.to_row().enabled, 0);
    }

    #[test]
    fn clouds_are_authored_on_the_atmosphere() {
        let row = AtmosphereComponent::default().to_row();
        assert_eq!(row.clouds_enabled, 0, "no clouds by default");
        let clouds = AtmosphereComponent {
            clouds_enabled: true,
            cloud_coverage: 0.7,
            cloud_bottom_km: 1.0,
            cloud_top_km: 3.0,
            cloud_wind: [5.0, 0.0, -2.0],
            ..Default::default()
        };
        let row = clouds.to_row();
        assert_eq!(row.clouds_enabled, 1);
        assert_eq!(
            (row.cloud_bottom, row.cloud_top, row.cloud_coverage),
            (1.0, 3.0, 0.7)
        );
        assert_eq!(row.cloud_wind, [5.0, 0.0, -2.0]);
        assert_eq!(
            AtmosphereComponent::from_row(&row).to_row().clouds(),
            row.clouds(),
            "the clouds round-trip"
        );
    }
}
