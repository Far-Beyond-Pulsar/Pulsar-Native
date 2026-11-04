//! The atmosphere row as a SceneDB component, for standalone hosts
//! (examples, tests): it carries its contract row's exact fields
//! (`same_layout!`), so SceneDB mirrors it into the buffer the passes read.
//! Engines derive the row from their own component instead. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// The air around a planet (or above a flat world). Coefficients are per
/// kilometre, heights and radii in kilometres. The first enabled row is
/// used.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "atmospheres")]
pub struct AtmosphereComponent {
    /// Planet centre in world metres (with [`crate::atmosphere::placement::CENTER`]). Planet
    /// centres are world anchors: unlike the other positions in pass rows,
    /// which are in the frame's space, the pass rebases them by the frame's
    /// world origin.
    #[gpu]
    pub center: [f32; 3],
    /// [`crate::atmosphere::placement`].
    #[gpu]
    pub placement: u32,
    #[gpu]
    pub rayleigh_scattering: [f32; 3],
    #[gpu]
    pub rayleigh_scale_height: f32,
    #[gpu]
    pub mie_scattering: [f32; 3],
    #[gpu]
    pub mie_scale_height: f32,
    #[gpu]
    pub mie_absorption: [f32; 3],
    /// Mie phase asymmetry (forward scattering, the haze around the sun).
    #[gpu]
    pub mie_g: f32,
    #[gpu]
    pub ozone_absorption: [f32; 3],
    /// Height of the ozone layer's peak.
    #[gpu]
    pub ozone_center: f32,
    /// Diffuse reflectance of the ground, for light it bounces into the air.
    #[gpu]
    pub ground_albedo: [f32; 3],
    /// Thickness of the ozone layer (a tent around its peak).
    #[gpu]
    pub ozone_width: f32,
    /// Planet radius.
    #[gpu]
    pub bottom_radius: f32,
    /// Radius of the top of the atmosphere.
    #[gpu]
    pub top_radius: f32,
    /// Angular radius of the sun's disk (radians).
    #[gpu]
    pub sun_angular_radius: f32,
    /// Non-zero to use this atmosphere.
    #[gpu]
    pub enabled: u32,
    /// Bottom of the cloud layer above the surface (km).
    #[gpu]
    pub cloud_bottom: f32,
    /// Top of the cloud layer above the surface (km).
    #[gpu]
    pub cloud_top: f32,
    /// The planet's mean share of sky the clouds cover: 0 clear, 1
    /// overcast. The weather varies it from region to region.
    #[gpu]
    pub cloud_coverage: f32,
    /// The clouds' mean form: 0 flat stratus low in the layer, 1 cumulus
    /// towering through it. The weather's regimes vary it.
    #[gpu]
    pub cloud_type: f32,
    /// Extinction inside a cloud at full density (per km).
    #[gpu]
    pub cloud_extinction: f32,
    /// Size of the finest weather: the groups cumulus gather in (km).
    #[gpu]
    pub cloud_weather_scale: f32,
    /// Size of the clouds' billows (km).
    #[gpu]
    pub cloud_shape_scale: f32,
    /// Size of the wisps that erode their edges (km).
    #[gpu]
    pub cloud_detail_scale: f32,
    /// Wind carrying the clouds (world m/s).
    #[gpu]
    pub cloud_wind: [f32; 3],
    /// Non-zero to draw the clouds.
    #[gpu]
    pub clouds_enabled: u32,
    /// Share of the light a cloud scatters rather than absorbs, per channel.
    #[gpu]
    pub cloud_albedo: [f32; 3],
    /// How far the detail erodes the clouds' edges (0..1).
    #[gpu]
    pub cloud_erosion: f32,
    /// The planet's axis (world direction) the circulation's latitudes
    /// are measured from.
    #[gpu]
    pub cloud_pole: [f32; 3],
    /// How strongly the circulation bands the cover by latitude (0..1).
    #[gpu]
    pub cloud_circulation: f32,
    /// Circulation cells from the equator to each pole (Earth's three:
    /// Hadley, Ferrel, polar).
    #[gpu]
    pub cloud_circulation_cells: f32,
    /// Size of the weather systems: cyclones, fronts and highs (km).
    #[gpu]
    pub cloud_synoptic_scale: f32,
    /// How deep and frequent the cyclones are (0..1).
    #[gpu]
    pub cloud_storminess: f32,
    /// Size of the mesoscale cells and clusters (km).
    #[gpu]
    pub cloud_mesoscale_scale: f32,
    /// How much high cirrus the weather brings (0..1).
    #[gpu]
    pub cloud_cirrus: f32,
    /// Height of the cirrus sheet above the surface (km).
    #[gpu]
    pub cloud_cirrus_height: f32,
    /// How fast the weather evolves: 0 still, 1 real time, more a time
    /// lapse.
    #[gpu]
    pub cloud_evolution: f32,
    /// Which weather: each seed lays out other systems.
    #[gpu]
    pub cloud_seed: u32,
    /// How the clouds are drawn: 0 smooth volumes, otherwise voxels of this
    /// side (km), each cell filled or empty ([`crate::CloudRepresentation`]).
    #[gpu]
    pub cloud_voxel_size: f32,
    /// The density (0..1) a voxel cell is filled from.
    #[gpu]
    pub cloud_voxel_fill: f32,
    /// Unused: the row stays a whole number of vec4s.
    #[gpu]
    pub cloud_reserved: [f32; 2],
}
same_layout!(AtmosphereComponent, crate::AtmosphereRow, crate::ATMOSPHERES, "atmospheres"; center, placement, rayleigh_scattering, rayleigh_scale_height, mie_scattering, mie_scale_height, mie_absorption, mie_g, ozone_absorption, ozone_center, ground_albedo, ozone_width, bottom_radius, top_radius, sun_angular_radius, enabled, cloud_bottom, cloud_top, cloud_coverage, cloud_type, cloud_extinction, cloud_weather_scale, cloud_shape_scale, cloud_detail_scale, cloud_wind, clouds_enabled, cloud_albedo, cloud_erosion, cloud_pole, cloud_circulation, cloud_circulation_cells, cloud_synoptic_scale, cloud_storminess, cloud_mesoscale_scale, cloud_cirrus, cloud_cirrus_height, cloud_evolution, cloud_seed, cloud_voxel_size, cloud_voxel_fill, cloud_reserved);

impl Default for AtmosphereComponent {
    fn default() -> Self {
        crate::AtmosphereRow::default().into()
    }
}

impl AtmosphereComponent {
    pub fn around_planet(self, center_m: [f32; 3], radius_m: f64) -> Self {
        crate::AtmosphereRow::from(self)
            .around_planet(center_m, radius_m)
            .into()
    }

    pub fn with_clouds(self, clouds: crate::CloudLayer) -> Self {
        crate::AtmosphereRow::from(self).with_clouds(clouds).into()
    }
}
