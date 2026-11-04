//! Physically based atmosphere (Hillaire 2020, "A Scalable and Production
//! Ready Sky and Atmosphere Rendering Technique"), for any scene: a flat
//! world (the ground at the world origin) or a planet (its centre anywhere).
//!
//! An [`AtmosphereRow`] row describes the air: planet and atmosphere
//! radii, Rayleigh and Mie scattering, Mie and ozone absorption, ground
//! albedo. The scene's first directional light is its sun. Without a row
//! there is no atmosphere (space, a moon).
//!
//! [`AtmospherePass`] resolves the row and the sun on the GPU each frame and
//! builds the lookup tables: transmittance, multiple scattering, the sky
//! seen from the camera, the aerial-perspective volume (light scattered
//! between the camera and each pixel) and the sky's diffuse irradiance as
//! spherical harmonics. The tables that depend only on the row's
//! parameters (transmittance, multiple scattering and the sky irradiance
//! table over height and sun angle) are rebuilt only when the rows change;
//! the camera- and sun-dependent ones every frame. It publishes:
//!
//! - `atmosphere_frame` (buffer, `AtmosphereFrame` in
//!   `shaders/atmosphere_common.wgsl`): planet, sun, irradiance. Deferred
//!   lighting attenuates the sun through the air at each pixel and lights
//!   ambient with the sky's irradiance.
//! - `atmosphere_transmittance`, `atmosphere_sky_view`,
//!   `atmosphere_aerial` (textures).
//!
//! The row's cloud fields describe its clouds ([`CloudLayer`], the
//! [`crate::clouds`] module): the pass builds their noise and, each frame,
//! their shadow along the sun (`cloud_shadow_map`, `cloud_shadow_frame`).
//!
//! [`AtmosphereCompositePass`] runs after opaque lighting: the sky and the
//! sun's disk where no geometry is, aerial perspective where it is, and the
//! clouds ([`crate::clouds::CloudTracePass`]) in front of whichever lies
//! behind them.
//!
//! Positions stay exact in camera-relative frames: the planet centre is
//! rebased by the frame's world origin, split into an f32 and its
//! remainder.

use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult};
use pulsar_scenedb::gpu::BufferKey;

use crate::clouds::{
    CloudVolumes, ATMOSPHERE_CLOUDS, ATMOSPHERE_CLOUD_DEPTH, ATMOSPHERE_CLOUD_DETAIL,
    ATMOSPHERE_CLOUD_PLANET_WEATHER, ATMOSPHERE_CLOUD_SHAPE, ATMOSPHERE_CLOUD_WEATHER,
    CLOUD_SHADOW_FRAME, CLOUD_SHADOW_MAP,
};

pub use helio_frame_api::lighting::sky::{
    ATMOSPHERE_FRAME, ATMOSPHERE_FRAME_BYTES, ATMOSPHERE_MARKER, ATMOSPHERE_SKY_TABLE,
    ATMOSPHERE_SKY_TABLE_BYTES, ATMOSPHERE_SKY_TABLE_MARKER, ATMOSPHERE_SKY_TABLE_SNIPPET,
    ATMOSPHERE_SKY_VIEW, ATMOSPHERE_SNIPPET, SKY_TABLE_ENTRIES,
};
pub const ATMOSPHERE_TRANSMITTANCE: &str = "atmosphere_transmittance";
pub const ATMOSPHERE_MULTI_SCATTERING: &str = "atmosphere_multi_scattering";
pub const ATMOSPHERE_AERIAL: &str = "atmosphere_aerial";

/// Where an atmosphere's planet is.
pub mod placement {
    /// The ground is at the world origin; the planet's centre is straight
    /// below it. For flat worlds.
    pub const GROUND_AT_ORIGIN: u32 = 0;
    /// The planet's centre is at `AtmosphereRow::center` (world
    /// metres). For planets.
    pub const CENTER: u32 = 1;
}

/// The air around a planet (or above a flat world). Coefficients are per
/// kilometre, heights and radii in kilometres. The first enabled row is
/// used.
#[derive(Pod, Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct AtmosphereRow {
    /// Planet centre in world metres (with [`placement::CENTER`]). Planet
    /// centres are world anchors: unlike the other positions in pass rows,
    /// which are in the frame's space, the pass rebases them by the frame's
    /// world origin.
    pub center: [f32; 3],
    /// [`placement`].
    pub placement: u32,
    pub rayleigh_scattering: [f32; 3],
    pub rayleigh_scale_height: f32,
    pub mie_scattering: [f32; 3],
    pub mie_scale_height: f32,
    pub mie_absorption: [f32; 3],
    /// Mie phase asymmetry (forward scattering, the haze around the sun).
    pub mie_g: f32,
    pub ozone_absorption: [f32; 3],
    /// Height of the ozone layer's peak.
    pub ozone_center: f32,
    /// Diffuse reflectance of the ground, for light it bounces into the air.
    pub ground_albedo: [f32; 3],
    /// Thickness of the ozone layer (a tent around its peak).
    pub ozone_width: f32,
    /// Planet radius.
    pub bottom_radius: f32,
    /// Radius of the top of the atmosphere.
    pub top_radius: f32,
    /// Angular radius of the sun's disk (radians).
    pub sun_angular_radius: f32,
    /// Non-zero to use this atmosphere.
    pub enabled: u32,
    /// Bottom of the cloud layer above the surface (km).
    pub cloud_bottom: f32,
    /// Top of the cloud layer above the surface (km).
    pub cloud_top: f32,
    /// The planet's mean share of sky the clouds cover: 0 clear, 1
    /// overcast. The weather varies it from region to region.
    pub cloud_coverage: f32,
    /// The clouds' mean form: 0 flat stratus low in the layer, 1 cumulus
    /// towering through it. The weather's regimes vary it.
    pub cloud_type: f32,
    /// Extinction inside a cloud at full density (per km).
    pub cloud_extinction: f32,
    /// Size of the finest weather: the groups cumulus gather in (km).
    pub cloud_weather_scale: f32,
    /// Size of the clouds' billows (km).
    pub cloud_shape_scale: f32,
    /// Size of the wisps that erode their edges (km).
    pub cloud_detail_scale: f32,
    /// Wind carrying the clouds (world m/s).
    pub cloud_wind: [f32; 3],
    /// Non-zero to draw the clouds.
    pub clouds_enabled: u32,
    /// Share of the light a cloud scatters rather than absorbs, per channel.
    pub cloud_albedo: [f32; 3],
    /// How far the detail erodes the clouds' edges (0..1).
    pub cloud_erosion: f32,
    /// The planet's axis (world direction) the circulation's latitudes
    /// are measured from.
    pub cloud_pole: [f32; 3],
    /// How strongly the circulation bands the cover by latitude (0..1).
    pub cloud_circulation: f32,
    /// Circulation cells from the equator to each pole (Earth's three:
    /// Hadley, Ferrel, polar).
    pub cloud_circulation_cells: f32,
    /// Size of the weather systems: cyclones, fronts and highs (km).
    pub cloud_synoptic_scale: f32,
    /// How deep and frequent the cyclones are (0..1).
    pub cloud_storminess: f32,
    /// Size of the mesoscale cells and clusters (km).
    pub cloud_mesoscale_scale: f32,
    /// How much high cirrus the weather brings (0..1).
    pub cloud_cirrus: f32,
    /// Height of the cirrus sheet above the surface (km).
    pub cloud_cirrus_height: f32,
    /// How fast the weather evolves: 0 still, 1 real time, more a time
    /// lapse.
    pub cloud_evolution: f32,
    /// Which weather: each seed lays out other systems.
    pub cloud_seed: u32,
    /// How the clouds are drawn: 0 smooth volumes, otherwise voxels of this
    /// side (km), each cell filled or empty ([`CloudRepresentation`]).
    pub cloud_voxel_size: f32,
    /// The density (0..1) a voxel cell is filled from.
    pub cloud_voxel_fill: f32,
    /// Unused: the row stays a whole number of vec4s.
    pub cloud_reserved: [f32; 2],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for AtmosphereRow {}

/// The buffer the renderer reads [`AtmosphereRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const ATMOSPHERES: helio_core::BufferContract<AtmosphereRow> =
    helio_core::BufferContract::new("atmospheres");

const _: () = assert!(std::mem::size_of::<AtmosphereRow>() == 240);

impl Default for AtmosphereRow {
    fn default() -> Self {
        Self::earth()
    }
}

impl AtmosphereRow {
    /// Earth's atmosphere (Hillaire 2020's reference values), the ground at
    /// the world origin.
    pub fn earth() -> Self {
        Self {
            center: [0.0; 3],
            placement: placement::GROUND_AT_ORIGIN,
            rayleigh_scattering: [5.802e-3, 13.558e-3, 33.1e-3],
            rayleigh_scale_height: 8.0,
            mie_scattering: [3.996e-3; 3],
            mie_scale_height: 1.2,
            mie_absorption: [4.4e-3; 3],
            mie_g: 0.8,
            ozone_absorption: [0.65e-3, 1.881e-3, 0.085e-3],
            ozone_center: 25.0,
            ozone_width: 30.0,
            ground_albedo: [0.3; 3],
            bottom_radius: 6360.0,
            top_radius: 6460.0,
            sun_angular_radius: 0.004675,
            enabled: 1,
            ..Self::clear()
        }
    }

    /// The cloud fields of a sky without clouds ([`CloudLayer::cumulus`]'s
    /// shape, disabled); every other field zero.
    fn clear() -> Self {
        let mut row = Self::zeroed();
        CloudLayer::cumulus().write(&mut row);
        row.clouds_enabled = 0;
        row
    }

    /// This atmosphere with `clouds` (enabled).
    pub fn with_clouds(mut self, clouds: CloudLayer) -> Self {
        clouds.write(&mut self);
        self
    }

    /// The cloud layer this row describes, if it draws one.
    pub fn clouds(&self) -> Option<CloudLayer> {
        (self.clouds_enabled != 0).then(|| CloudLayer {
            bottom_km: self.cloud_bottom,
            top_km: self.cloud_top,
            coverage: self.cloud_coverage,
            kind: self.cloud_type,
            extinction: self.cloud_extinction,
            weather_km: self.cloud_weather_scale,
            shape_km: self.cloud_shape_scale,
            detail_km: self.cloud_detail_scale,
            wind: self.cloud_wind,
            albedo: self.cloud_albedo,
            erosion: self.cloud_erosion,
            representation: if self.cloud_voxel_size > 0.0 {
                CloudRepresentation::Voxels {
                    cell_m: self.cloud_voxel_size * 1000.0,
                    fill: self.cloud_voxel_fill,
                }
            } else {
                CloudRepresentation::Volumetric
            },
            weather: PlanetWeather {
                pole: self.cloud_pole,
                circulation: self.cloud_circulation,
                circulation_cells: self.cloud_circulation_cells,
                synoptic_km: self.cloud_synoptic_scale,
                storminess: self.cloud_storminess,
                mesoscale_km: self.cloud_mesoscale_scale,
                cirrus: self.cloud_cirrus,
                cirrus_km: self.cloud_cirrus_height,
                evolution: self.cloud_evolution,
                seed: self.cloud_seed,
            },
        })
    }

    /// A thin, dusty Martian atmosphere: butterscotch by day, blue around
    /// the setting sun.
    pub fn mars() -> Self {
        Self {
            rayleigh_scattering: [0.19e-3, 1.0e-3, 2.0e-3],
            rayleigh_scale_height: 11.1,
            mie_scattering: [11.0e-3, 8.0e-3, 5.0e-3],
            mie_scale_height: 11.1,
            mie_absorption: [4.0e-3, 6.5e-3, 9.0e-3],
            mie_g: 0.75,
            ozone_absorption: [0.0; 3],
            ground_albedo: [0.35, 0.22, 0.12],
            bottom_radius: 3389.5,
            top_radius: 3489.5,
            sun_angular_radius: 0.0031,
            ..Self::earth()
        }
    }

    /// This atmosphere around a planet of `radius_m` centred at `center_m`
    /// (world metres); the atmosphere keeps its thickness.
    pub fn around_planet(mut self, center_m: [f32; 3], radius_m: f64) -> Self {
        let thickness = self.top_radius - self.bottom_radius;
        self.placement = placement::CENTER;
        self.center = center_m;
        self.bottom_radius = (radius_m * 0.001) as f32;
        self.top_radius = self.bottom_radius + thickness;
        self
    }
}

/// A layer of clouds in the atmosphere (the cloud fields of
/// [`AtmosphereRow`]): a band of altitudes over the planet's surface
/// where clouds form, gathered by the weather ([`PlanetWeather`] and the
/// cumulus groups), shaped by billows and eroded into wisps, carried by the
/// wind; and a sheet of cirrus above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudLayer {
    /// The band, above the surface (km).
    pub bottom_km: f32,
    pub top_km: f32,
    /// The planet's mean share of sky covered: 0 clear, 1 overcast. The
    /// weather varies it from region to region.
    pub coverage: f32,
    /// The mean form: 0 stratus, 1 cumulus. The weather's regimes vary it.
    pub kind: f32,
    /// Extinction inside a cloud at full density (per km).
    pub extinction: f32,
    /// Sizes of the cumulus groups (the finest weather), of the billows and
    /// of the wisps (km).
    pub weather_km: f32,
    pub shape_km: f32,
    pub detail_km: f32,
    /// World m/s.
    pub wind: [f32; 3],
    /// Single-scattering albedo per channel.
    pub albedo: [f32; 3],
    /// How far the wisps erode the edges (0..1).
    pub erosion: f32,
    /// How the clouds are drawn.
    pub representation: CloudRepresentation,
    /// The weather's larger scales.
    pub weather: PlanetWeather,
}

/// How a [`CloudLayer`]'s clouds are drawn, from the same weather and
/// density: their trace, shadow, the sun's visibility and the fog's shafts
/// all read the one density, so every representation shades alike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CloudRepresentation {
    /// Smooth volumes: billows eroded into wisps.
    Volumetric,
    /// Blocky clouds of cubic cells `cell_m` metres on a side (on a lattice
    /// fixed to the planet), each filled where the smooth density at its
    /// centre reaches `fill` (0..1) and empty elsewhere. Cells too small to
    /// see at a distance blend back into the smooth density's cover.
    Voxels { cell_m: f32, fill: f32 },
}

impl Default for CloudRepresentation {
    fn default() -> Self {
        Self::Volumetric
    }
}

/// The weather's scales larger than the cumulus groups, over the whole
/// planet (`shaders/cloud_planet_weather.wgsl`): the circulation's bands by
/// latitude, the synoptic systems (cyclones spiralling the cover with their
/// fronts, highs clearing it), regimes from stratocumulus decks to open
/// cumulus cells, the mesoscale's cells, and cirrus. A flat world lies at
/// one place of its planet (the ground at the origin), under that place's
/// weather. Planets too small to hold a few systems have none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetWeather {
    /// The planet's axis (world direction): the latitudes the circulation
    /// bands. Earth's default tilts it 45 degrees from world up, so a flat
    /// world and the top of a planet at the origin lie at mid-latitudes.
    pub pole: [f32; 3],
    /// How strongly the circulation bands the cover by latitude (0..1).
    pub circulation: f32,
    /// Circulation cells from the equator to each pole (Earth's three).
    pub circulation_cells: f32,
    /// Size of the synoptic systems (km).
    pub synoptic_km: f32,
    /// How deep and frequent the cyclones are (0..1).
    pub storminess: f32,
    /// Size of the mesoscale cells (km).
    pub mesoscale_km: f32,
    /// How much cirrus the weather brings (0..1), and the height of their
    /// sheet above the surface (km).
    pub cirrus: f32,
    pub cirrus_km: f32,
    /// How fast the weather evolves: 0 still, 1 real time (systems living a
    /// few days, drifting with the westerlies and the trades), more a time
    /// lapse.
    pub evolution: f32,
    /// Which weather: each seed lays out other systems.
    pub seed: u32,
}

impl Default for PlanetWeather {
    fn default() -> Self {
        Self::earth()
    }
}

impl PlanetWeather {
    /// Earth's: three cells a hemisphere, systems 1500 km across, mesoscale
    /// cells of tens of kilometres, cirrus at 9 km; still.
    pub fn earth() -> Self {
        Self {
            pole: [
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            circulation: 1.0,
            circulation_cells: 3.0,
            synoptic_km: 1500.0,
            storminess: 0.6,
            mesoscale_km: 80.0,
            cirrus: 0.5,
            cirrus_km: 9.0,
            evolution: 0.0,
            seed: 0,
        }
    }

    /// No weather systems or bands: the layer's coverage everywhere, its
    /// cells and groups alone; no cirrus.
    pub fn calm() -> Self {
        Self {
            circulation: 0.0,
            storminess: 0.0,
            cirrus: 0.0,
            ..Self::earth()
        }
    }
}

impl Default for CloudLayer {
    fn default() -> Self {
        Self::cumulus()
    }
}

impl CloudLayer {
    /// Fair-weather cumulus: scattered heaps between 1.5 and 4 km.
    pub fn cumulus() -> Self {
        Self {
            bottom_km: 1.5,
            top_km: 4.0,
            coverage: 0.45,
            kind: 0.85,
            extinction: 40.0,
            weather_km: 24.0,
            shape_km: 6.0,
            detail_km: 0.8,
            wind: [8.0, 0.0, 3.0],
            albedo: [0.99; 3],
            erosion: 0.45,
            representation: CloudRepresentation::Volumetric,
            weather: PlanetWeather::earth(),
        }
    }

    /// A low, flat, grey deck.
    pub fn overcast() -> Self {
        Self {
            bottom_km: 1.0,
            top_km: 2.2,
            coverage: 0.95,
            kind: 0.2,
            extinction: 30.0,
            ..Self::cumulus()
        }
    }

    fn write(&self, row: &mut AtmosphereRow) {
        row.cloud_bottom = self.bottom_km.max(0.0);
        row.cloud_top = self.top_km.max(self.bottom_km + 0.01);
        row.cloud_coverage = self.coverage.clamp(0.0, 1.0);
        row.cloud_type = self.kind.clamp(0.0, 1.0);
        row.cloud_extinction = self.extinction.max(0.0);
        row.cloud_weather_scale = self.weather_km.max(0.01);
        row.cloud_shape_scale = self.shape_km.max(0.01);
        row.cloud_detail_scale = self.detail_km.max(0.001);
        row.cloud_wind = self.wind;
        row.clouds_enabled = 1;
        row.cloud_albedo = self.albedo.map(|a| a.clamp(0.0, 1.0));
        row.cloud_erosion = self.erosion.clamp(0.0, 1.0);
        let w = &self.weather;
        row.cloud_pole = w.pole;
        row.cloud_circulation = w.circulation.clamp(0.0, 1.0);
        row.cloud_circulation_cells = w.circulation_cells.round().max(1.0);
        row.cloud_synoptic_scale = w.synoptic_km.max(1.0);
        row.cloud_storminess = w.storminess.clamp(0.0, 1.0);
        row.cloud_mesoscale_scale = w.mesoscale_km.max(0.01);
        row.cloud_cirrus = w.cirrus.clamp(0.0, 1.0);
        row.cloud_cirrus_height = w.cirrus_km.max(row.cloud_top + 0.1);
        row.cloud_evolution = w.evolution.max(0.0);
        row.cloud_seed = w.seed;
        (row.cloud_voxel_size, row.cloud_voxel_fill) = match self.representation {
            CloudRepresentation::Volumetric => (0.0, 0.5),
            CloudRepresentation::Voxels { cell_m, fill } => {
                ((cell_m * 0.001).max(1e-4), fill.clamp(0.0, 1.0))
            }
        };
    }
}

const TRANSMITTANCE: [u32; 2] = [256, 64];
const MULTI_SCATTERING: u32 = 32;
const SKY_VIEW: [u32; 2] = [192, 108];
const AERIAL: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ResolveUniform {
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
    /// x: the frame clock's advance (s).
    clock: [f32; 4],
}

fn lut(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 3],
    dimension: wgpu::TextureDimension,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: size[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn storage_texture(
    binding: u32,
    dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba16Float,
            view_dimension: dimension,
        },
        count: None,
    }
}

fn sampled_texture(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

fn buffer_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

struct Kernel {
    pipeline: wgpu::ComputePipeline,
    group: Option<wgpu::BindGroup>,
    groups: [u32; 3],
    /// Reads only the atmosphere's parameters (not the camera, the sun or
    /// the frame's origin), so its output holds until the rows change.
    params_only: bool,
}

/// Resolves the scene's atmosphere and builds its lookup tables (see the
/// module docs).
pub struct AtmospherePass {
    shared_layout: wgpu::BindGroupLayout,
    shared_group: Option<wgpu::BindGroup>,
    shared_key: Option<(wgpu::Buffer, wgpu::Buffer, wgpu::Buffer)>,
    resolve: wgpu::ComputePipeline,
    kernels: Vec<Kernel>,
    frame: wgpu::Buffer,
    uniform: wgpu::Buffer,
    no_rows: wgpu::Buffer,
    sampler: wgpu::Sampler,
    transmittance: wgpu::TextureView,
    multi_scattering: wgpu::TextureView,
    sky_view: wgpu::TextureView,
    aerial: wgpu::TextureView,
    sky_table: wgpu::Buffer,
    rows: helio_core::SceneBufferLiveness,
    active: bool,
    xr_mode: bool,
    /// `(epoch, content_generation)` of the atmosphere rows the
    /// parameter-only tables (transmittance, multiple scattering, the sky
    /// irradiance table) were last built from; `None` rebuilds them.
    params_tables: Option<(u64, u64)>,
    /// `HELIO_ATMOSPHERE_TRACE=1`: the resolved frame's head (planet, sun,
    /// illuminance, eye) read back and printed each frame (diagnostics).
    trace: Option<Vec<(u64, wgpu::Buffer)>>,
    frame_index: u64,
    clouds: CloudVolumes,
}

impl AtmospherePass {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = helio_core::shader::module_with(
            device,
            "Atmosphere LUTs",
            helio_core::include_wgsl!("shaders/atmosphere_luts.wgsl"),
            &[ATMOSPHERE_SNIPPET],
        );
        let compute = wgpu::ShaderStages::COMPUTE;
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let shared_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Atmosphere shared"),
            entries: &[
                buffer_entry(0, compute, wgpu::BufferBindingType::Uniform),
                buffer_entry(1, compute, storage(true)),
                buffer_entry(2, compute, storage(true)),
                buffer_entry(3, compute, storage(true)),
                buffer_entry(4, compute, storage(false)),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let d2 = wgpu::TextureViewDimension::D2;
        let transmittance = lut(
            device,
            "Atmosphere transmittance",
            [TRANSMITTANCE[0], TRANSMITTANCE[1], 1],
            wgpu::TextureDimension::D2,
        );
        let multi = lut(
            device,
            "Atmosphere multiple scattering",
            [MULTI_SCATTERING, MULTI_SCATTERING, 1],
            wgpu::TextureDimension::D2,
        );
        let multi_scattering = multi.clone();
        let sky_view = lut(
            device,
            "Atmosphere sky view",
            [SKY_VIEW[0], SKY_VIEW[1], 1],
            wgpu::TextureDimension::D2,
        );
        let aerial = lut(
            device,
            "Atmosphere aerial perspective",
            [AERIAL; 3],
            wgpu::TextureDimension::D3,
        );
        let sky_table = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atmosphere sky table"),
            size: ATMOSPHERE_SKY_TABLE_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let pipeline = |layout: Option<&wgpu::BindGroupLayout>, entry: &str| {
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = match layout {
                Some(layout) => vec![Some(&shared_layout), Some(layout)],
                None => vec![Some(&shared_layout)],
            };
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            helio_core::shader::create_compute_pipeline(
                &device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        // Each kernel binds only its own outputs and inputs: a texture may
        // not be written and sampled in one dispatch.
        let kernel = |entry: &str,
                      entries: &[(wgpu::BindGroupLayoutEntry, wgpu::BindingResource)],
                      groups: [u32; 3]| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(entry),
                entries: &entries.iter().map(|(e, _)| *e).collect::<Vec<_>>(),
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(entry),
                layout: &layout,
                entries: &entries
                    .iter()
                    .map(|(e, r)| wgpu::BindGroupEntry {
                        binding: e.binding,
                        resource: r.clone(),
                    })
                    .collect::<Vec<_>>(),
            });
            Kernel {
                pipeline: pipeline(Some(&layout), entry),
                group: Some(group),
                groups,
                params_only: false,
            }
        };
        let view = wgpu::BindingResource::TextureView;
        let params_only = |kernel: Kernel| Kernel {
            params_only: true,
            ..kernel
        };
        let kernels = vec![
            params_only(kernel(
                "transmittance_lut_kernel",
                &[(storage_texture(0, d2), view(&transmittance))],
                [
                    TRANSMITTANCE[0].div_ceil(8),
                    TRANSMITTANCE[1].div_ceil(8),
                    1,
                ],
            )),
            params_only(kernel(
                "multi_scattering_kernel",
                &[
                    (sampled_texture(1, compute, d2), view(&transmittance)),
                    (storage_texture(2, d2), view(&multi)),
                ],
                [
                    MULTI_SCATTERING.div_ceil(8),
                    MULTI_SCATTERING.div_ceil(8),
                    1,
                ],
            )),
            kernel(
                "sky_view_kernel",
                &[
                    (sampled_texture(1, compute, d2), view(&transmittance)),
                    (sampled_texture(3, compute, d2), view(&multi)),
                    (storage_texture(4, d2), view(&sky_view)),
                ],
                [SKY_VIEW[0].div_ceil(8), SKY_VIEW[1].div_ceil(8), 1],
            ),
            kernel(
                "aerial_kernel",
                &[
                    (sampled_texture(1, compute, d2), view(&transmittance)),
                    (sampled_texture(3, compute, d2), view(&multi)),
                    (
                        storage_texture(6, wgpu::TextureViewDimension::D3),
                        view(&aerial),
                    ),
                ],
                [AERIAL.div_ceil(8), AERIAL.div_ceil(8), 1],
            ),
            kernel(
                "irradiance_kernel",
                &[(sampled_texture(5, compute, d2), view(&sky_view))],
                [1, 1, 1],
            ),
            params_only(kernel(
                "sky_table_kernel",
                &[
                    (sampled_texture(1, compute, d2), view(&transmittance)),
                    (sampled_texture(3, compute, d2), view(&multi)),
                    (
                        buffer_entry(7, compute, storage(false)),
                        sky_table.as_entire_binding(),
                    ),
                ],
                [SKY_TABLE_ENTRIES as u32, 1, 1],
            )),
        ];
        let resolve = pipeline(None, "resolve");
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atmosphere frame"),
            size: ATMOSPHERE_FRAME_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let clouds = CloudVolumes::new(device, &frame);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atmosphere resolve"),
            size: std::mem::size_of::<ResolveUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Bound while a scene has no atmosphere or light rows yet.
        let no_rows = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atmosphere no rows"),
            size: 128,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Atmosphere LUT"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            shared_layout,
            shared_group: None,
            shared_key: None,
            resolve,
            kernels,
            frame,
            uniform,
            no_rows,
            sampler,
            transmittance,
            multi_scattering,
            sky_view,
            aerial,
            sky_table,
            rows: helio_core::SceneBufferLiveness::with_row_predicate(|row| {
                // `enabled` is the last word.
                row.len() >= 112 && row[108..112].iter().any(|&b| b != 0)
            }),
            active: false,
            xr_mode: false,
            params_tables: None,
            trace: std::env::var_os("HELIO_ATMOSPHERE_TRACE").map(|_| Vec::new()),
            frame_index: 0,
            clouds,
        }
    }

    /// The pass that composites this atmosphere over the lit scene, in
    /// `format` (the lighting target's).
    pub fn composite(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> AtmosphereCompositePass {
        AtmosphereCompositePass::new(device, format)
    }
}

impl RenderPass for AtmospherePass {
    fn set_frame_inputs(&mut self, inputs: &helio_core::RenderFrameInputs<'_>) {
        self.xr_mode = inputs.xr_mode;
    }

    fn scene_buffers(&self) -> &'static [&'static str] {
        &["atmospheres", "scene_lights"]
    }

    fn name(&self) -> &'static str {
        "Atmosphere"
    }

    fn writes(&self) -> &'static [&'static str] {
        &[
            ATMOSPHERE_FRAME,
            ATMOSPHERE_SKY_TABLE,
            ATMOSPHERE_TRANSMITTANCE,
            ATMOSPHERE_MULTI_SCATTERING,
            ATMOSPHERE_SKY_VIEW,
            ATMOSPHERE_AERIAL,
            CLOUD_SHADOW_MAP,
            CLOUD_SHADOW_FRAME,
            ATMOSPHERE_CLOUD_PLANET_WEATHER,
            ATMOSPHERE_CLOUD_WEATHER,
            ATMOSPHERE_CLOUD_SHAPE,
            ATMOSPHERE_CLOUD_DETAIL,
        ]
    }

    fn declare_resources(&self, _builder: &mut ResourceBuilder) {}

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        let rows = ctx.scene_buffers.get(BufferKey::of("atmospheres"));
        self.rows.update(ctx.device, ctx.queue, rows);
        let active = rows.is_some_and(|handle| self.rows.maybe_live(handle));
        if !active && self.active {
            // Consumers read "no atmosphere" from the frame, and no clouds'
            // shadow.
            ctx.write_buffer(&self.frame, 0, &[0u8; 16]);
            ctx.write_buffer(self.clouds.shadow_frame(), 0, &[0u8; 16]);
        }
        if active {
            self.clouds.prepare(
                ctx,
                rows.map(|handle| (handle.epoch, handle.content_generation)),
            );
        } else {
            self.params_tables = None;
            self.clouds.forget_weather();
        }
        self.active = active;
        let origin = ctx.world_origin;
        let hi = origin.as_vec3();
        let lo = (origin - hi.as_dvec3()).as_vec3();
        let uniform = ResolveUniform {
            origin_hi: hi.extend(0.0).to_array(),
            origin_lo: lo.extend(0.0).to_array(),
            clock: [ctx.time_delta, f32::from(self.xr_mode), 0.0, 0.0],
        };
        ctx.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.active {
            return Ok(());
        }
        let atmospheres = ctx
            .scene_buffers
            .get(BufferKey::of("atmospheres"))
            .map_or(&self.no_rows, |h| &h.buffer);
        let lights = ctx
            .scene_buffers
            .get(BufferKey::of("scene_lights"))
            .map_or(&self.no_rows, |h| &h.buffer);
        let key = (atmospheres.clone(), lights.clone(), ctx.camera.clone());
        if self.shared_key.as_ref() != Some(&key) {
            self.shared_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Atmosphere shared"),
                layout: &self.shared_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: atmospheres.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: lights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.frame.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
            self.shared_key = Some(key);
        }
        let mut cmds = ctx.graphics_cmds();
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Atmosphere"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, self.shared_group.as_ref(), &[]);
        pass.set_pipeline(&self.resolve);
        pass.dispatch_workgroups(1, 1, 1);
        // The atmosphere's parameters come only from its rows (`resolve`
        // picks the first enabled one), so the tables built from them alone
        // are rebuilt only when those rows change.
        let rows = ctx
            .scene_buffers
            .get(BufferKey::of("atmospheres"))
            .map(|handle| (handle.epoch, handle.content_generation));
        let rebuild_params_tables = rows.is_none() || self.params_tables != rows;
        self.params_tables = rows;
        for kernel in &self.kernels {
            if kernel.params_only && !rebuild_params_tables {
                continue;
            }
            pass.set_pipeline(&kernel.pipeline);
            pass.set_bind_group(1, kernel.group.as_ref(), &[]);
            pass.dispatch_workgroups(kernel.groups[0], kernel.groups[1], kernel.groups[2]);
        }
        // In XR the sky-view LUT provides the baked cloudless sky. Avoid
        // generating cloud weather, shadows and volumes for each eye.
        if !self.xr_mode {
            self.clouds.record(&mut pass);
        }
        drop(pass);
        self.frame_index += 1;
        if let Some(trace) = &mut self.trace {
            // Map last frame's copy (submitted by now), copy this frame's.
            for (index, buffer) in trace.drain(..) {
                let probe = buffer.clone();
                buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                    if result.is_err() { return; }
                    let bytes = probe.slice(..).get_mapped_range().unwrap().to_vec();
                    let f: &[f32] = bytemuck::cast_slice(&bytes);
                    eprintln!(
                        "ATMOSPHERE_TRACE frame {index} planet {:?} sun {:?} illuminance {:?} eye {:?} r {:.3}",
                        &f[0..4], &f[4..8], &f[8..12], &f[12..16],
                        (f[12] * f[12] + f[13] * f[13] + f[14] * f[14]).sqrt(),
                    );
                });
            }
            let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Atmosphere trace"),
                size: 64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            cmds.copy_buffer_to_buffer(&self.frame, 0, &staging, 0, 64);
            trace.push((self.frame_index, staging));
        }
        Ok(())
    }

    fn publish<'a>(&self, frame: &mut helio_core::ResourceRegistry<'a>) {
        // Pass-owned resources, handed over for the frame.
        let buffer: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.frame) };
        frame.write_buffer(ResourceKey::new(ATMOSPHERE_FRAME), buffer, "Atmosphere");
        let table: &'a wgpu::Buffer = unsafe { std::mem::transmute(&self.sky_table) };
        frame.write_buffer(ResourceKey::new(ATMOSPHERE_SKY_TABLE), table, "Atmosphere");
        for (key, view) in [
            (ATMOSPHERE_TRANSMITTANCE, &self.transmittance),
            (ATMOSPHERE_MULTI_SCATTERING, &self.multi_scattering),
            (ATMOSPHERE_SKY_VIEW, &self.sky_view),
            (ATMOSPHERE_AERIAL, &self.aerial),
        ] {
            let view: &'a wgpu::TextureView = unsafe { std::mem::transmute(view) };
            frame.write_texture_view(ResourceKey::new(key), view, "Atmosphere");
        }
        let active: bool = self.active;
        frame.write(ResourceKey::new("atmosphere_active"), active, "Atmosphere");
        self.clouds.publish(frame);
    }
}

/// Composites the atmosphere over the lit scene in place (`pre_aa`): the
/// sky and the sun where no geometry is, aerial perspective where it is.
/// Draws nothing without an atmosphere.
pub struct AtmosphereCompositePass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Its bind groups: one per set of inputs (the cloud trace's alternate
    /// frame to frame), kept while they are bound.
    cache: helio_core::gpu_cache::GpuCache,
    active: bool,
    /// A 1x1 zero texture: no clouds (no opacity, no light), bound while no
    /// cloud pass publishes any.
    no_clouds: wgpu::TextureView,
}

impl AtmosphereCompositePass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = helio_core::shader::module_with(
            device,
            "Atmosphere composite",
            helio_core::include_wgsl!("shaders/atmosphere_composite.wgsl"),
            &[ATMOSPHERE_SNIPPET],
        );
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let read_only = wgpu::BufferBindingType::Storage { read_only: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Atmosphere composite"),
            entries: &[
                buffer_entry(0, fragment, read_only),
                buffer_entry(1, fragment, read_only),
                sampled_texture(2, fragment, wgpu::TextureViewDimension::D2),
                sampled_texture(3, fragment, wgpu::TextureViewDimension::D2),
                sampled_texture(4, fragment, wgpu::TextureViewDimension::D3),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                sampled_texture(7, fragment, wgpu::TextureViewDimension::D2),
                sampled_texture(8, fragment, wgpu::TextureViewDimension::D2),
                // The clouds' depths: Rg32Float, loaded.
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Atmosphere composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // dst * alpha + src: alpha 0 replaces (sky), alpha T attenuates.
        let blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = helio_core::shader::create_render_pipeline(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Atmosphere composite"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_composite"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: blend,
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Zero,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Atmosphere composite"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_clouds = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("No clouds"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            pipeline,
            layout,
            sampler,
            cache: helio_core::gpu_cache::GpuCache::new(),
            active: false,
            no_clouds,
        }
    }
}

impl RenderPass for AtmosphereCompositePass {
    fn name(&self) -> &'static str {
        "AtmosphereComposite"
    }

    fn reads(&self) -> &'static [&'static str] {
        &[
            "pre_aa",
            "depth",
            ATMOSPHERE_FRAME,
            ATMOSPHERE_TRANSMITTANCE,
            ATMOSPHERE_MULTI_SCATTERING,
            ATMOSPHERE_SKY_VIEW,
            ATMOSPHERE_AERIAL,
            ATMOSPHERE_CLOUDS,
            ATMOSPHERE_CLOUD_DEPTH,
        ]
    }

    fn writes(&self) -> &'static [&'static str] {
        &["pre_aa"]
    }

    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read("pre_aa");
    }

    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        self.active = ctx
            .registry
            .get::<bool>(ResourceKey::new("atmosphere_active"))
            .unwrap_or(false);
        Ok(())
    }

    fn render_pass_descriptor_with_storage<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        resources: &'a helio_core::ResourceRegistry<'a>,
        storage: &'a mut helio_core::RenderFrameStorage,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        if !self.active {
            return None;
        }
        let target = resources.texture_view(ResourceKey::new("pre_aa"))?;
        let color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>] = storage
            .retain_boxed_slice(Box::new([Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })]));
        Some(wgpu::RenderPassDescriptor {
            label: Some("Atmosphere composite"),
            color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if !self.active {
            return Ok(());
        }
        let Some(mut pass) = ctx.render_cmds() else {
            return Ok(());
        };
        let registry = ctx.registry;
        let (
            Some(frame),
            Some(transmittance),
            Some(multi_scattering),
            Some(sky_view),
            Some(aerial),
        ) = (
            registry.get::<&wgpu::Buffer>(ResourceKey::new(ATMOSPHERE_FRAME)),
            registry.get::<&wgpu::TextureView>(ResourceKey::new(ATMOSPHERE_TRANSMITTANCE)),
            registry.get::<&wgpu::TextureView>(ResourceKey::new(ATMOSPHERE_MULTI_SCATTERING)),
            registry.get::<&wgpu::TextureView>(ResourceKey::new(ATMOSPHERE_SKY_VIEW)),
            registry.get::<&wgpu::TextureView>(ResourceKey::new(ATMOSPHERE_AERIAL)),
        )
        else {
            return Ok(());
        };
        let clouds = registry
            .texture_binding(ATMOSPHERE_CLOUDS)
            .unwrap_or(&self.no_clouds);
        let cloud_depth = registry
            .texture_binding(ATMOSPHERE_CLOUD_DEPTH)
            .unwrap_or(&self.no_clouds);
        let group = self.cache.bind_group(
            ctx.device,
            &wgpu::BindGroupDescriptor {
                label: Some("Atmosphere composite"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ctx.camera.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: frame.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(transmittance),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(sky_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(aerial),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(ctx.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(multi_scattering),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(clouds),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(cloud_depth),
                    },
                ],
            },
        );
        self.cache.end_frame();
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    fn on_resize(&mut self, _device: &wgpu::Device, _size: helio_core::RenderSize) {
        self.cache.clear();
    }
}
