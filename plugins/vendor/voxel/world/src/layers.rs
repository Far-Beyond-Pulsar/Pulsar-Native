//! Terrain as an ordered stack of layers (`helio.terrain`): every world,
//! from an Earth-like planet to a cratered moon or a flat block world, is a
//! list of layer kinds with parameters plus a material style. Presets
//! ([`TerrainLayers::earth`], [`TerrainLayers::moon`], [`TerrainLayers::flat`])
//! are just data; a game builds its own stacks (or randomizes them from a
//! seed) through the same settings.
//!
//! The stack compiles to one octave table sorted from coarsest to finest
//! (each octave tagged with its layer) that a single interpreter evaluates
//! on CPU and GPU (`landform.rs`, `landform.wgsl`): changing layers never
//! recompiles shaders, and erosion octaves see the slope of everything
//! coarser than themselves, in any layer, at every level.
//!
//! A stack's [`Climate`] and its `Rivers` layers bake the world's planet
//! maps ([`crate::planet_maps`]): drainage, rivers and lakes carved into
//! the terrain, temperature and moisture the materials and features read.
use crate::grid::{Grid, DOMAIN_UNIT};
use crate::landform::{
    noise_quantile, LandformConstants, LandformField, Octave, PackedRule, StackLayer, BASIN,
    CONTINENT, CRATER, CRATER_RADIUS_Q19, EROSION, GRAD_SHIFT, HILLS, LAYERS, OCTAVES, REGION,
    RIDGE, ROUGHNESS, RULES, STYLE_EARTHLIKE, STYLE_LAYERED, STYLE_LUNAR, STYLE_RULES, WARP,
    WARP_OCTAVES,
};
use crate::noise::{FINE_ONE, ONE};
use crate::terrain::{
    material, GeneratorInfo, TerrainField, TerrainGenerator, TerrainSource, HEIGHT_ONE,
};
use crate::volumes::{self, Caves, VolumeLayer, Volumes};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const ID: &str = "helio.terrain";
/// Output version, recorded with saved edits (one version is registered).
pub const VERSION: u32 = 1;

/// What a layer adds to the terrain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    /// Rolling fBm relief, warped with the domain (hills, highlands).
    #[default]
    Hills,
    /// Bends the domain of the layers after it (coastlines, ridge lines).
    /// At most one, before every other layer.
    Warp,
    /// Continents and ocean basins: `height_m` ocean depth, `base_m`
    /// lowland height; provides the land masks. At most one.
    Continents,
    /// Ridged mountain ranges inside regions (`coverage`, `region_km`).
    Mountains,
    /// Metre-scale fBm detail, not warped; `ratio` is its amplitude as a
    /// fraction of each wavelength.
    Roughness,
    /// Gullies down the slope of the coarser terrain, branching; full depth
    /// on slopes steeper than `ratio` (rise over run).
    Erosion,
    /// Crater octaves from `scale_km` (largest diameter) down; `coverage`
    /// is the density of the largest, `persistence` its growth per octave,
    /// `ratio` depth over diameter, `ratio2` rim over depth, `ratio3` the
    /// share of young craters with bright ejecta.
    Craters,
    /// Smooth low plains (maria, lakes beds) covering `coverage` of the
    /// surface, `height_m` deep.
    Basins,
    /// A constant height: `height_m` (a flat world is one plateau).
    Plateau,
    /// Escarpments: inside regions of `scale_km` covering `coverage`, the
    /// layers before it step into terraces `height_m` tall, flat treads
    /// joined by cliffs over the last `ratio` of each step (0.1: sheer).
    Cliffs,
    /// Rivers and lakes from the planet's drainage (its maps): valleys cut
    /// into the layers before it along every river, from a source draining
    /// `region_km` x `region_km` of wet land to the sea. At a source the
    /// valley sinks `height_m` under the filled terrain, its floodplain is
    /// `scale_km` wide with a channel `ratio2` of it, `base_m` deep; the
    /// floodplain and channel grow with the square root of the discharge,
    /// incision and depth with its fourth root. The valley walls slope
    /// `ratio` (rise over run), their shoulders rounded over `ratio3` m.
    /// `coverage` is how much the drainage has worn the planet (0: not at
    /// all): ranges lowered along their rivers, sediment filling basins
    /// into plains and fanning out where it leaves the ranges. `octaves`
    /// refines the drainage below the maps into streams that many levels
    /// (each four times finer, at most three: creeks).
    Rivers,
}

/// Tallest cliff step (m): terraces are evaluated in 32-bit integers.
pub const CLIFF_STEP_MAX_M: f64 = 1_000.0;

/// Where a layer applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerMask {
    #[default]
    Everywhere,
    /// Land only (rises from the coast; needs a Continents layer).
    Land,
    /// Everywhere but fading out under deep sea.
    AboveDeepSea,
}

/// One layer of a [`TerrainLayers`] stack. Fields mean what the layer's
/// kind says ([`LayerKind`]); unused ones are ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub kind: LayerKind,
    pub enabled: bool,
    pub mask: LayerMask,
    /// Main height (m): amplitude of the first octave, depth of basins or
    /// oceans, plateau height.
    pub height_m: f64,
    /// Secondary height (m): the lowlands of Continents.
    pub base_m: f64,
    /// Wavelength of the first octave or the largest feature (km).
    pub scale_km: f64,
    pub octaves: u32,
    /// Amplitude ratio from one octave to the next (craters: density growth).
    pub persistence: f64,
    /// Share of the surface covered (mountain regions, basins, crater density).
    pub coverage: f64,
    /// Size of the regions a layer occupies (mountain ranges), km.
    pub region_km: f64,
    pub ratio: f64,
    pub ratio2: f64,
    pub ratio3: f64,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            kind: LayerKind::Hills,
            enabled: true,
            mask: LayerMask::Everywhere,
            height_m: 100.0,
            base_m: 0.0,
            scale_km: 10.0,
            octaves: 4,
            persistence: 0.5,
            coverage: 0.5,
            region_km: 100.0,
            ratio: 0.0,
            ratio2: 0.0,
            ratio3: 0.0,
        }
    }
}

impl Layer {
    /// A layer of `kind` with that kind's defaults: added alone to a stack,
    /// it shows (craters with depth and rims, mountains with their ranges).
    /// The Earth preset is made of these.
    pub fn new(kind: LayerKind) -> Self {
        use LayerKind::*;
        let base = Self {
            kind,
            ..Self::default()
        };
        match kind {
            Hills => Self {
                height_m: 140.0,
                scale_km: 9.0,
                ..base
            },
            Warp => Self {
                scale_km: 40.0,
                ..base
            },
            Continents => Self {
                height_m: 2_400.0,
                base_m: 180.0,
                scale_km: 3_000.0,
                ..base
            },
            Mountains => Self {
                height_m: 2_400.0,
                scale_km: 20.0,
                octaves: 7,
                persistence: 0.47,
                region_km: 240.0,
                coverage: 0.45,
                ..base
            },
            Roughness => Self {
                scale_km: 0.512,
                octaves: 9,
                ratio: 0.035,
                ..base
            },
            Erosion => Self {
                height_m: 40.0,
                scale_km: 1.6,
                octaves: 6,
                ratio: 0.5,
                ..base
            },
            Craters => Self {
                scale_km: 40.0,
                octaves: 8,
                coverage: 0.3,
                persistence: 1.25,
                ratio: 0.2,
                ratio2: 0.3,
                ratio3: 0.15,
                ..base
            },
            Basins => Self {
                height_m: 1_200.0,
                scale_km: 900.0,
                coverage: 0.3,
                ..base
            },
            Plateau => Self {
                height_m: 400.0,
                ..base
            },
            Cliffs => Self {
                height_m: 30.0,
                scale_km: 25.0,
                coverage: 0.35,
                ratio: 0.12,
                ..base
            },
            Rivers => Self {
                height_m: 30.0,
                base_m: 2.0,
                scale_km: 0.25,
                region_km: 50.0,
                octaves: 3,
                ratio: 0.3,
                ratio2: 0.12,
                ratio3: 6.0,
                ..base
            },
        }
    }
}

/// How surface and buried cells get their materials.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialStyle {
    /// Meadows, dry lands (by the climate's moisture), rock outcrops,
    /// strata and snow above the climate's snowline;
    /// gravel in erosion gullies.
    #[default]
    Earthlike,
    /// Regolith over bedrock, dark mare plains, bright young ejecta.
    Lunar,
    /// A `surface` layer over `soil_depth_m` of `soil` over `rock`.
    Layered,
    /// The first of the [`MaterialRule`]s that holds, else `rock`: a game's
    /// own biomes (snow above a height, rock on steep slopes, sediment in
    /// gullies, strata, patches).
    Rules,
}

/// One material rule: the material of a ground cell where every condition
/// holds (ranges are inclusive). Rules are tried in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialRule {
    /// Material name (see [`material::NAMES`]).
    pub material: String,
    /// Column height above the datum (m).
    pub min_height_m: f64,
    pub max_height_m: f64,
    /// Ground slope, rise over run (1 is 45 degrees).
    pub min_slope: f64,
    pub max_slope: f64,
    /// Depth below the column top (m); 0 is the exposed surface.
    pub min_depth_m: f64,
    pub max_depth_m: f64,
    /// Moisture, 0 (dry) to 1 (wet): the climate's ([`Climate`]).
    pub min_moisture: f64,
    pub max_moisture: f64,
    /// Annual mean temperature at the cell (°C): the climate's, falling
    /// with height.
    pub min_temperature_c: f64,
    pub max_temperature_c: f64,
    /// Where river beds (the channels of a `Rivers` layer) count.
    pub river_bed: RiverBed,
    /// Erosion, -1 (gully floors) to 1 (the ribs between gullies).
    pub min_erosion: f64,
    pub max_erosion: f64,
    /// Patches: noise blobs of this size (km; 0 none) covering `patch_share`.
    pub patch_km: f64,
    pub patch_share: f64,
    /// Strata: bands of this thickness (m; 0 none), the odd ones or the even.
    pub band_m: f64,
    pub odd_bands: bool,
    /// Single-cell specks: this share of the cells (1 for all cells).
    pub speck_share: f64,
}

impl Default for MaterialRule {
    fn default() -> Self {
        Self {
            material: "Stone".into(),
            min_height_m: -1.0e6,
            max_height_m: 1.0e6,
            min_slope: 0.0,
            max_slope: 1.0e3,
            min_depth_m: 0.0,
            max_depth_m: 1.0e6,
            min_moisture: 0.0,
            max_moisture: 1.0,
            min_temperature_c: -1.0e3,
            max_temperature_c: 1.0e3,
            river_bed: RiverBed::Any,
            min_erosion: -1.0,
            max_erosion: 1.0,
            patch_km: 0.0,
            patch_share: 0.5,
            band_m: 0.0,
            odd_bands: false,
            speck_share: 1.0,
        }
    }
}

impl MaterialRule {
    /// `material` everywhere (narrow it with the fields).
    pub fn new(material: &str) -> Self {
        Self {
            material: material.into(),
            ..Self::default()
        }
    }

    fn validate(&self, index: usize) -> Result<(), String> {
        let values = [
            self.min_height_m,
            self.max_height_m,
            self.min_slope,
            self.max_slope,
            self.min_depth_m,
            self.max_depth_m,
            self.min_moisture,
            self.max_moisture,
            self.min_temperature_c,
            self.max_temperature_c,
            self.min_erosion,
            self.max_erosion,
            self.patch_km,
            self.patch_share,
            self.band_m,
            self.speck_share,
        ];
        if values.iter().any(|v| !v.is_finite()) || self.patch_km < 0.0 || self.band_m < 0.0 {
            return Err(format!(
                "material rule {index} has a non-finite or negative size"
            ));
        }
        if material::from_name(&self.material).is_none() {
            return Err(format!(
                "material rule {index}: unknown material {:?}",
                self.material
            ));
        }
        Ok(())
    }

    fn pack(&self, grid: &Grid) -> PackedRule {
        let mm = |m: f64| (m * f64::from(HEIGHT_ONE)).clamp(-2.0e9, 2.0e9).round() as i32;
        let eighths = |s: f64| (s * 8.0).clamp(-1.0e9, 1.0e9).floor() as i32;
        let cells = |m: f64| (m / grid.voxel_size()).clamp(-1.0e9, 1.0e9).round() as i32;
        let q16 = |v: f64| (v.clamp(0.0, 1.0) * 65_536.0).round() as i32;
        let byte = |v: f64| (v.clamp(-1.0, 1.0) * 126.0).round() as i32;
        let degrees = |c: f64| {
            (c * f64::from(crate::planet_maps::DEGREE))
                .clamp(-2.0e9, 2.0e9)
                .round() as i32
        };
        let patch = if self.patch_km > 0.0 {
            let shift = ((self.patch_km * 1_000.0 / DOMAIN_UNIT)
                .log2()
                .round()
                .clamp(1.0, 30.0)) as i32;
            (
                shift,
                (noise_quantile(1.0 - self.patch_share.clamp(0.0, 1.0)) * f64::from(ONE)).round()
                    as i32,
            )
        } else {
            (0, 0)
        };
        let speck = if self.speck_share >= 1.0 {
            0
        } else {
            ((self.speck_share.max(0.0) * 256.0).round() as i32).clamp(1, 255)
        };
        PackedRule {
            head: [
                material::from_name(&self.material).unwrap_or(material::STONE) as i32,
                speck,
                patch.0,
                patch.1,
            ],
            height_slope: [
                mm(self.min_height_m),
                mm(self.max_height_m),
                eighths(self.min_slope),
                eighths(self.max_slope),
            ],
            depth_moisture: [
                cells(self.min_depth_m),
                cells(self.max_depth_m),
                q16(self.min_moisture),
                q16(self.max_moisture),
            ],
            surface_bands: [
                byte(self.min_erosion),
                byte(self.max_erosion),
                mm(self.band_m).max(0),
                i32::from(self.odd_bands),
            ],
            climate: [
                degrees(self.min_temperature_c),
                degrees(self.max_temperature_c),
                match self.river_bed {
                    RiverBed::Any => 0,
                    RiverBed::Only => 1,
                    RiverBed::Never => 2,
                },
                0,
            ],
        }
    }
}

/// Where a [`MaterialRule`] holds on river beds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiverBed {
    #[default]
    Any,
    /// Only in rivers' channels.
    Only,
    /// Never in them.
    Never,
}

/// A world's climate: temperature from latitude and height, moisture from
/// the rain belts, the way to the sea and rain shadows. With `maps` it is
/// baked over the whole planet ([`crate::planet_maps`]); without, the
/// climate of `latitude_deg` holds everywhere (heights still cool it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Climate {
    pub maps: bool,
    /// The planet's axis (world direction, any length): latitudes run
    /// around it. Earth's tilts 45 degrees from world up, as its weather
    /// does, so the top of a planet at the origin lies at mid-latitudes.
    pub pole: [f64; 3],
    /// Latitude of a flat world's origin (its north is -Z), and of an
    /// unmapped world's climate (degrees).
    pub latitude_deg: f64,
    /// Annual mean temperatures at sea level (°C).
    pub equator_c: f64,
    pub pole_c: f64,
    /// Cooling with height (°C per km).
    pub lapse_c_per_km: f64,
    /// Ground colder than this keeps its snow (°C).
    pub snow_c: f64,
    /// Scale of the rain (1: Earth's). An unmapped world's moisture is its
    /// latitude's rain belt times this.
    pub wetness: f64,
    /// How far inland from the sea, upwind, the rain halves (km).
    pub continentality_km: f64,
    /// Height of ground upwind that halves the rain behind it (m).
    pub rain_shadow_m: f64,
    /// Ground under the datum is sea (rivers end there, coasts are wet).
    pub ocean: bool,
    /// Size of the maps' texels at face centres (km).
    pub texel_km: f64,
}

impl Default for Climate {
    fn default() -> Self {
        Self::earth()
    }
}

impl Climate {
    /// Earth's: warm tropics and frozen poles, oceans, rain belts.
    pub fn earth() -> Self {
        Self {
            maps: true,
            // Normalized where it is used; whole numbers, so the
            // component's `[f32; 3]` holds it exactly.
            pole: [0.0, 1.0, 1.0],
            latitude_deg: 45.0,
            equator_c: 28.0,
            pole_c: -22.0,
            lapse_c_per_km: 6.5,
            snow_c: -3.0,
            wetness: 1.0,
            continentality_km: 2_000.0,
            rain_shadow_m: 2_000.0,
            ocean: true,
            texel_km: 25.0,
        }
    }

    /// The same everywhere: `latitude_deg`'s climate, unbaked.
    pub fn uniform(latitude_deg: f64, wetness: f64) -> Self {
        Self {
            maps: false,
            latitude_deg,
            wetness,
            ..Self::earth()
        }
    }

    fn spec(&self) -> crate::planet_maps::ClimateSpec {
        crate::planet_maps::ClimateSpec {
            maps: self.maps,
            pole: glam::DVec3::from_array(self.pole),
            latitude_deg: self.latitude_deg,
            equator_c: self.equator_c,
            pole_c: self.pole_c,
            lapse_c_per_km: self.lapse_c_per_km,
            snow_c: self.snow_c,
            wetness: self.wetness,
            continentality_km: self.continentality_km,
            rain_shadow_m: self.rain_shadow_m,
            ocean: self.ocean,
            texel_km: self.texel_km,
        }
    }

    fn validate(&self) -> Result<(), String> {
        let values = [
            self.latitude_deg,
            self.equator_c,
            self.pole_c,
            self.lapse_c_per_km,
            self.snow_c,
            self.wetness,
            self.continentality_km,
            self.rain_shadow_m,
            self.texel_km,
        ];
        if values.iter().chain(&self.pole).any(|v| !v.is_finite()) {
            return Err("climate settings must be finite".into());
        }
        if self.lapse_c_per_km < 0.0 || self.wetness < 0.0 || self.texel_km <= 0.0 {
            return Err("climate lapse rate, wetness and texel size must be positive".into());
        }
        if self.maps && self.pole.iter().all(|v| *v == 0.0) {
            return Err("a mapped climate needs a pole".into());
        }
        Ok(())
    }
}

/// A world's terrain: an ordered layer stack, volumetric features and a
/// material style (the `helio.terrain` settings JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainLayers {
    pub layers: Vec<Layer>,
    /// Caves and cliff shapes (`docs/volumes.md`).
    pub volumes: Volumes,
    pub materials: MaterialStyle,
    /// Temperature and moisture: the Earthlike snow and dry lands, rules'
    /// and features' climate ranges.
    pub climate: Climate,
    /// Depth of the soil (Earthlike, Layered) or regolith (Lunar).
    pub soil_depth_m: f64,
    /// Layered: material names (see [`material::NAMES`]); `rock` is also
    /// the Rules style's fallback.
    pub surface: String,
    pub soil: String,
    pub rock: String,
    /// Rules style: tried in order (at most 16).
    pub rules: Vec<MaterialRule>,
}

impl Default for TerrainLayers {
    fn default() -> Self {
        Self::earth()
    }
}

impl TerrainLayers {
    /// Continents and ocean basins, ridged mountain ranges with branching
    /// erosion gullies, hills, river valleys and lakes from the planet's
    /// drainage, and metre-scale roughness; karst cave networks, ledged
    /// cliffs; Earth's climate over meadows, dry lands, rock, strata and snow.
    pub fn earth() -> Self {
        use LayerKind::*;
        Self {
            layers: vec![
                Layer::new(Warp),
                Layer::new(Continents),
                Layer {
                    mask: LayerMask::Land,
                    ..Layer::new(Mountains)
                },
                Layer::new(Erosion),
                Self::earth_hills(),
                Layer::new(Rivers),
                Layer {
                    mask: LayerMask::AboveDeepSea,
                    ..Layer::new(Roughness)
                },
            ],
            volumes: Volumes::default(),
            materials: MaterialStyle::Earthlike,
            climate: Climate::earth(),
            soil_depth_m: 0.7,
            surface: "Grass".into(),
            soil: "Dirt".into(),
            rock: "Stone".into(),
            rules: Vec::new(),
        }
    }

    /// Earth's hills: 140 m over 9 km down to knolls of ~30 m over a
    /// kilometre, so the land near the eye has shape (four octaves at half
    /// persistence left it sloping ~3 %).
    fn earth_hills() -> Layer {
        Layer {
            mask: LayerMask::AboveDeepSea,
            octaves: 5,
            persistence: 0.6,
            ..Layer::new(LayerKind::Hills)
        }
    }

    /// A dry world of dunes and mesas, with materials from rules: dune sand
    /// on gentle ground, sandstone and clay strata on slopes, gravel in the
    /// gullies, scattered stone patches and dark stone specks; a dry
    /// subtropical climate everywhere.
    pub fn desert() -> Self {
        use LayerKind::*;
        let mut earth = Self::earth();
        earth
            .layers
            .retain(|l| l.kind != Continents && l.kind != Rivers);
        // The desert's hills stay gentle under its dunes.
        for l in earth.layers.iter_mut().filter(|l| l.kind == Hills) {
            *l = Layer {
                mask: l.mask,
                ..Layer::new(Hills)
            };
        }
        for l in &mut earth.layers {
            l.mask = LayerMask::Everywhere;
        }
        earth.layers.insert(1, Layer::new(Plateau));
        let rule = |material: &str, f: &dyn Fn(&mut MaterialRule)| {
            let mut r = MaterialRule::new(material);
            f(&mut r);
            r
        };
        Self {
            materials: MaterialStyle::Rules,
            rules: vec![
                rule("Gravel", &|r| {
                    r.max_depth_m = 0.3;
                    r.max_erosion = -0.4;
                    r.min_slope = 0.2;
                }),
                rule("Sand", &|r| {
                    r.max_depth_m = 1.5;
                    r.max_slope = 0.6;
                }),
                rule("DarkStone", &|r| {
                    r.max_depth_m = 0.0;
                    r.speck_share = 0.06;
                }),
                rule("Stone", &|r| {
                    r.patch_km = 0.05;
                    r.patch_share = 0.15;
                }),
                rule("Clay", &|r| {
                    r.band_m = 2.5;
                    r.odd_bands = true;
                }),
            ],
            rock: "Sandstone".into(),
            climate: Climate::uniform(25.0, 0.2),
            // Drier karst, more of it open; sandstone ledges and undercuts.
            volumes: Volumes {
                caves: Caves {
                    share: 0.35,
                    entrances: 0.5,
                    ..Caves::default()
                },
                cliffs: volumes::Cliffs {
                    depth_m: 6.0,
                    strata_m: 2.0,
                    noise: 0.3,
                    share: 0.7,
                    ..volumes::Cliffs::default()
                },
            },
            ..earth
        }
    }

    /// Cratered highlands and dark basalt plains over regolith, lava tubes
    /// under them (skylights for entrances).
    /// The basins come after the highlands, which they flatten.
    pub fn moon() -> Self {
        use LayerKind::*;
        Self {
            layers: vec![
                Layer {
                    kind: Hills,
                    height_m: 1_500.0,
                    scale_km: 250.0,
                    octaves: 3,
                    persistence: 0.5,
                    ..Layer::default()
                },
                Layer {
                    kind: Basins,
                    height_m: 1_200.0,
                    scale_km: 900.0,
                    coverage: 0.3,
                    ..Layer::default()
                },
                Layer {
                    kind: Craters,
                    scale_km: 40.0,
                    octaves: 10,
                    coverage: 0.3,
                    persistence: 1.25,
                    ratio: 0.2,
                    ratio2: 0.3,
                    ratio3: 0.15,
                    ..Layer::default()
                },
            ],
            volumes: Volumes {
                caves: Caves::lava_tubes(),
                ..Volumes::none()
            },
            materials: MaterialStyle::Lunar,
            climate: Climate::uniform(0.0, 0.0),
            soil_depth_m: 4.0,
            ..Self::earth()
        }
    }

    /// Level ground: one plateau, grass over a metre of dirt over stone.
    pub fn flat() -> Self {
        Self::flat_at(0.0)
    }

    /// [`Self::flat`] at `height_m` (rounded down to whole voxel layers).
    pub fn flat_at(height_m: f64) -> Self {
        Self {
            layers: vec![Layer {
                kind: LayerKind::Plateau,
                height_m,
                ..Layer::default()
            }],
            volumes: Volumes::none(),
            materials: MaterialStyle::Layered,
            climate: Climate::uniform(45.0, 1.0),
            soil_depth_m: 1.0,
            ..Self::earth()
        }
    }

    /// Without caves and cliff shapes: a pure heightfield.
    pub fn heightfield(mut self) -> Self {
        self.volumes = Volumes::none();
        self
    }

    /// The stack's errors, before it is compiled.
    pub fn validate(&self) -> Result<(), String> {
        let enabled: Vec<&Layer> = self.layers.iter().filter(|l| l.enabled).collect();
        if enabled.len() > LAYERS {
            return Err(format!("a terrain has at most {LAYERS} enabled layers"));
        }
        for (index, l) in enabled.iter().enumerate() {
            let values = [
                l.height_m,
                l.base_m,
                l.scale_km,
                l.persistence,
                l.coverage,
                l.region_km,
                l.ratio,
                l.ratio2,
                l.ratio3,
            ];
            if values.iter().any(|v| !v.is_finite()) {
                return Err(format!(
                    "layer {index} ({:?}) has a non-finite value",
                    l.kind
                ));
            }
            if l.kind != LayerKind::Plateau && l.scale_km <= 0.0 {
                return Err(format!(
                    "layer {index} ({:?}) needs a positive scale",
                    l.kind
                ));
            }
            if l.height_m.abs() > 1.0e5 || l.base_m.abs() > 1.0e5 || l.scale_km > 1.0e5 {
                return Err(format!("layer {index} ({:?}) exceeds 100 km", l.kind));
            }
            if l.kind == LayerKind::Warp && index != 0 {
                return Err("a Warp layer must come first".into());
            }
            if l.mask != LayerMask::Everywhere
                && !enabled.iter().any(|o| o.kind == LayerKind::Continents)
            {
                return Err(format!(
                    "layer {index} ({:?}) is masked by land without a Continents layer",
                    l.kind
                ));
            }
        }
        if enabled
            .iter()
            .any(|l| l.kind == LayerKind::Rivers && l.mask != LayerMask::Everywhere)
        {
            return Err(
                "a Rivers layer is not masked: rivers carve land and end at the sea".into(),
            );
        }
        for kind in [LayerKind::Warp, LayerKind::Continents, LayerKind::Rivers] {
            if enabled.iter().filter(|l| l.kind == kind).count() > 1 {
                return Err(format!("a terrain has at most one {kind:?} layer"));
            }
        }
        self.volumes.validate()?;
        if !self.soil_depth_m.is_finite() || self.soil_depth_m < 0.0 {
            return Err("soil depth must be finite and non-negative".into());
        }
        self.climate.validate()?;
        for name in [&self.surface, &self.soil, &self.rock] {
            if material::from_name(name).is_none() {
                return Err(format!("unknown material {name:?}"));
            }
        }
        if self.rules.len() > RULES {
            return Err(format!("a terrain has at most {RULES} material rules"));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            rule.validate(index)?;
        }
        Ok(())
    }

    /// The interpreter's constants and volume layer on `grid`.
    pub fn compile(
        &self,
        grid: &Grid,
        seed: u32,
    ) -> Result<(LandformConstants, Option<VolumeLayer>), String> {
        self.validate()?;
        let units = |metres: f64| (metres * f64::from(HEIGHT_ONE)).round() as i32;
        let shift = |metres: f64| ((metres / DOMAIN_UNIT).log2().round().clamp(1.0, 30.0)) as u32;
        let mut state = seed.wrapping_mul(0x9E37_79B9);
        let mut next_seed = || {
            state = state.wrapping_add(0x6D2B_79F5);
            state
        };
        let mut warp = [Octave::default(); WARP_OCTAVES];
        for (index, w) in warp.iter_mut().enumerate() {
            // Zero-amplitude placeholders without a Warp layer.
            *w = Octave {
                shift: 20,
                amplitude: 0,
                seed: 0,
                kind: WARP + index as u32 / 2,
            };
        }
        let mut octaves: Vec<Octave> = Vec::new();
        let mut layers = [StackLayer::default(); LAYERS];
        let mut erosion_sum = 0i32;
        let mut display = 0;
        let mut continents = 0;
        let mut warps = 0;
        for (index, l) in self.layers.iter().filter(|l| l.enabled).enumerate() {
            let tag = (index as u32) << 8;
            let mask = match l.mask {
                LayerMask::Everywhere => 0,
                LayerMask::Land => 1,
                LayerMask::AboveDeepSea => 2,
            };
            let metres = l.scale_km * 1_000.0;
            let octave = |o: u32| metres / f64::from(1u32 << o);
            let mut layer = StackLayer {
                kind: 0,
                mask,
                a: 0,
                b: 0,
            };
            match l.kind {
                LayerKind::Warp => {
                    // Two octaves per axis, amplitude 15% of the wavelength.
                    let amplitude = metres * 0.15 / DOMAIN_UNIT;
                    for axis in 0..3 {
                        for o in 0..2 {
                            warp[axis * 2 + o] = Octave {
                                shift: shift(octave(o as u32)),
                                amplitude: (amplitude / f64::from(1u32 << o)).round() as i32,
                                seed: next_seed(),
                                kind: WARP + axis as u32,
                            };
                        }
                    }
                    warps = WARP_OCTAVES as i32;
                    layer.kind = StackLayer::WARP;
                }
                LayerKind::Continents => {
                    layer.kind = StackLayer::CONTINENTS;
                    layer.a = units(-l.height_m.abs());
                    layer.b = units(l.base_m);
                    continents = index as i32 + 1;
                    for o in 0..4 {
                        octaves.push(Octave {
                            shift: shift(octave(o)),
                            amplitude: ONE >> o,
                            seed: next_seed(),
                            kind: tag | CONTINENT,
                        });
                    }
                }
                LayerKind::Mountains => {
                    layer.kind = StackLayer::MOUNTAINS;
                    // Region mask: two fine octaves; the bias keeps `coverage`.
                    layer.a =
                        (noise_quantile(1.0 - l.coverage.clamp(0.0, 1.0)) * f64::from(ONE) * 1.2)
                            .round() as i32;
                    let region = l.region_km * 1_000.0;
                    for o in 0..2 {
                        octaves.push(Octave {
                            shift: shift(region / f64::from(1u32 << o)),
                            amplitude: ONE >> o,
                            seed: next_seed(),
                            kind: tag | REGION,
                        });
                    }
                    let mut amplitude = l.height_m;
                    for o in 0..l.octaves.min(12) {
                        octaves.push(Octave {
                            shift: shift(octave(o)),
                            amplitude: units(amplitude),
                            seed: next_seed(),
                            kind: tag | RIDGE,
                        });
                        amplitude *= l.persistence;
                    }
                    // The first mountain layer with a bakeable ridge chain
                    // draws coarse levels from its ridge envelope.
                    if display == 0
                        && (1..=crate::ridge_envelope::RIDGES as u32).contains(&l.octaves)
                    {
                        display = index as i32 + 1;
                    }
                }
                LayerKind::Hills | LayerKind::Roughness => {
                    let (kind, class) = if l.kind == LayerKind::Hills {
                        (StackLayer::HILLS, HILLS)
                    } else {
                        (StackLayer::ROUGHNESS, ROUGHNESS)
                    };
                    layer.kind = kind;
                    let mut amplitude = l.height_m;
                    for o in 0..l.octaves.min(16) {
                        let w = octave(o);
                        let a = if l.kind == LayerKind::Roughness {
                            w * l.ratio
                        } else {
                            amplitude
                        };
                        octaves.push(Octave {
                            shift: shift(w),
                            amplitude: units(a),
                            seed: next_seed(),
                            kind: tag | class,
                        });
                        amplitude *= l.persistence;
                    }
                }
                LayerKind::Erosion => {
                    layer.kind = StackLayer::EROSION;
                    layer.a = ((l.ratio.max(0.01)
                        * f64::from(1u32 << GRAD_SHIFT)
                        * DOMAIN_UNIT
                        * 1_000.0)
                        .round() as i32)
                        .clamp(32, 1 << 26);
                    let mut amplitude = l.height_m;
                    for o in 0..l.octaves.min(8) {
                        if amplitude > 0.0 && octave(o) > 1.0 {
                            octaves.push(Octave {
                                shift: shift(octave(o)),
                                amplitude: units(amplitude),
                                seed: next_seed(),
                                kind: tag | EROSION,
                            });
                            erosion_sum += units(amplitude);
                        }
                        amplitude *= l.persistence;
                    }
                }
                LayerKind::Craters => {
                    layer.kind = StackLayer::CRATERS;
                    layer.a = (l.ratio2.clamp(0.0, 4.0) * 65_536.0).round() as i32;
                    layer.b = (l.ratio3.clamp(0.0, 1.0) * 65_536.0).round() as i32;
                    for o in 0..l.octaves.min(12) {
                        let diameter = octave(o);
                        // A cell holds one crater of at most 0.3 cells radius.
                        let cell = diameter / (2.0 * f64::from(CRATER_RADIUS_Q19) / 524_288.0);
                        if cell < DOMAIN_UNIT * 1024.0 {
                            break;
                        }
                        let depth =
                            l.ratio.max(0.0) * diameter * (15_000.0 / diameter).min(1.0).powf(0.7);
                        let density = (l.coverage * l.persistence.powi(o as i32)).clamp(0.0, 0.95);
                        // Crater octaves carry their density in the seed's
                        // low 16 bits (the hash seed is the rest).
                        let seed = (next_seed() & !0xffff) | ((density * 65_535.0) as u32 & 0xffff);
                        octaves.push(Octave {
                            shift: shift(cell),
                            amplitude: units(depth),
                            seed,
                            kind: tag | CRATER,
                        });
                    }
                }
                LayerKind::Basins => {
                    layer.kind = StackLayer::BASINS;
                    layer.a = units(l.height_m.abs());
                    layer.b = ((noise_quantile(1.0 - l.coverage.clamp(0.0, 1.0))
                        * f64::from(FINE_ONE))
                    .round()) as i32;
                    octaves.push(Octave {
                        shift: shift(metres),
                        amplitude: ONE,
                        seed: next_seed(),
                        kind: tag | BASIN,
                    });
                }
                LayerKind::Cliffs => {
                    // Whole layers tall, so treads lie on cell boundaries.
                    let layer_mm = grid.layer_mm() as i32;
                    let step = (units(l.height_m.abs().min(CLIFF_STEP_MAX_M)) / layer_mm).max(1)
                        * layer_mm;
                    let riser = (l.ratio.clamp(1.0 / 1024.0, 1.0) * 1024.0).round() as u32;
                    layer.kind = StackLayer::CLIFFS | (riser << 16);
                    layer.a = step;
                    layer.b = ((noise_quantile(1.0 - l.coverage.clamp(0.0, 1.0))
                        * f64::from(FINE_ONE))
                    .round()) as i32;
                    octaves.push(Octave {
                        shift: shift(metres),
                        amplitude: ONE,
                        seed: next_seed(),
                        kind: tag | BASIN,
                    });
                }
                LayerKind::Rivers => {
                    // Its bounds come with the maps (`LandformField::new`).
                    layer.kind = StackLayer::RIVERS;
                }
                LayerKind::Plateau => {
                    layer.kind = StackLayer::PLATEAU;
                    layer.a = units(l.height_m).div_euclid(grid.layer_mm() as i32)
                        * grid.layer_mm() as i32;
                }
            }
            layers[index] = layer;
        }
        // Coarsest first, stable: each octave follows only coarser terrain
        // (erosion is steered by what precedes it), and the ridge weight
        // chains keep their order.
        octaves.sort_by(|a, b| b.shift.cmp(&a.shift));
        if WARP_OCTAVES + octaves.len() > OCTAVES {
            return Err(format!(
                "the layers have {} octaves; at most {}",
                octaves.len(),
                OCTAVES - WARP_OCTAVES
            ));
        }
        let mut table = [Octave::default(); OCTAVES];
        table[..WARP_OCTAVES].copy_from_slice(&warp);
        table[WARP_OCTAVES..WARP_OCTAVES + octaves.len()].copy_from_slice(&octaves);
        let soil = ((self.soil_depth_m / grid.voxel_size()).round() as i32).max(1);
        let id = |name: &str| material::from_name(name).unwrap_or(material::STONE) as i32;
        let style = match self.materials {
            MaterialStyle::Earthlike => STYLE_EARTHLIKE,
            MaterialStyle::Lunar => STYLE_LUNAR,
            MaterialStyle::Layered => STYLE_LAYERED,
            MaterialStyle::Rules => STYLE_RULES,
        };
        let mut rules = [PackedRule::default(); RULES];
        for (packed, rule) in rules.iter_mut().zip(&self.rules) {
            *packed = rule.pack(grid);
        }
        let constants = LandformConstants {
            header: [
                (WARP_OCTAVES + octaves.len()) as i32,
                grid.layer_mm() as i32,
                soil,
                seed as i32,
            ],
            levels: [0, continents, 0, units(-8.0)],
            shape: [display, 16, 0, i32::from(grid.is_plane())],
            style: [style, id(&self.surface), id(&self.soil), id(&self.rock)],
            stack: [
                self.layers.iter().filter(|l| l.enabled).count() as i32,
                erosion_sum.max(1),
                grid.sphere_constants()[2] as i32,
                warps,
            ],
            materials: [
                self.rules.len() as i32,
                id(&self.rock),
                octaves
                    .iter()
                    .filter(|o| o.class() == EROSION)
                    .map(|o| o.shift as i32)
                    .max()
                    .unwrap_or(-1),
                0,
            ],
            maps: crate::planet_maps::climate_header(&self.climate.spec()),
            layers,
            octaves: table,
            rules,
        };
        let volumes = VolumeLayer::new(&self.volumes, grid, constants.header[3] as u32);
        Ok((
            constants,
            (volumes.caves() || volumes.cliffs()).then_some(volumes),
        ))
    }
}

impl TerrainLayers {
    /// What the stack's planet maps are made of (`None`: it bakes none).
    pub fn map_spec(&self, seed: u32) -> Option<crate::planet_maps::MapSpec> {
        let rivers = self
            .layers
            .iter()
            .find(|l| l.enabled && l.kind == LayerKind::Rivers)
            .map(|l| crate::planet_maps::RiverSpec {
                source_km2: l.region_km * l.region_km,
                incision_m: l.height_m.max(0.0),
                floodplain_m: l.scale_km.max(0.0) * 1_000.0,
                channel_ratio: l.ratio2.clamp(0.0, 1.0),
                depth_m: l.base_m.max(0.0),
                wall: l.ratio,
                rounding_m: l.ratio3,
                erosion: l.coverage.clamp(0.0, 4.0),
                streams: l.octaves.min(crate::streams::MAX_LEVELS),
            });
        let spec = crate::planet_maps::MapSpec {
            climate: self.climate.spec(),
            rivers,
            seed,
        };
        spec.baked().then_some(spec)
    }

    /// The compiled field on `grid`, with its planet maps (baked once per
    /// process and world).
    pub fn field(&self, grid: &Grid, seed: u32) -> Result<LandformField, String> {
        let (constants, volumes) = self.compile(grid, seed)?;
        let maps = self
            .map_spec(seed)
            .map(|spec| crate::planet_maps::maps(&spec, &constants, grid));
        Ok(LandformField::new(grid, constants, volumes, maps))
    }

    /// A world terrain source using this stack.
    pub fn source(&self, seed: u64) -> TerrainSource {
        TerrainSource {
            generator: ID.into(),
            version: 0,
            seed,
            settings: serde_json::to_string(self).expect("terrain layers serialize"),
        }
    }
}

/// Builds [`LandformField`]s from [`TerrainLayers`] settings.
pub struct LayersGenerator;

impl TerrainGenerator for LayersGenerator {
    fn info(&self) -> GeneratorInfo {
        GeneratorInfo {
            id: ID.into(),
            version: VERSION,
            name: "Terrain layers".into(),
            description: "An ordered stack of terrain layers (continents, mountains, erosion, hills, craters, basins, plateaus) with cave networks, cliff shapes and a material style.".into(),
            settings_component: Some("VoxelTerrainLayersComponent".into()),
        }
    }
    fn build(
        &self,
        grid: &Grid,
        seed: u64,
        settings: &str,
    ) -> Result<Arc<dyn TerrainField>, String> {
        let layers: TerrainLayers = if settings.trim().is_empty() {
            TerrainLayers::default()
        } else {
            serde_json::from_str(settings).map_err(|e| format!("invalid terrain layers: {e}"))?
        };
        Ok(Arc::new(layers.field(grid, (seed ^ (seed >> 32)) as u32)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Shape;
    use crate::planet::{Planet, PlanetRecipe};
    use crate::terrain::check_field;
    use glam::IVec3;

    fn planet(shape: Shape, radius_m: f64, stack: &TerrainLayers) -> Planet {
        Planet::new(PlanetRecipe {
            shape,
            radius_m,
            plane_size_m: 4_000.0,
            terrain: stack.source(7),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn flat_ground_is_one_whole_layer_with_its_materials() {
        let grid = Grid::plane(Shape::Plane, 1024.0, 0.1).unwrap();
        let stack = TerrainLayers {
            soil_depth_m: 0.5,
            surface: "Sand".into(),
            rock: "dark_stone".into(),
            ..TerrainLayers::flat_at(2.34)
        };
        let field = LayersGenerator
            .build(&grid, 0, &serde_json::to_string(&stack).unwrap())
            .unwrap();
        assert_eq!(
            field.height(crate::terrain::Column {
                face: crate::grid::PLANE_FACE,
                i: 0,
                j: 0,
                level: 0
            }),
            2_300
        );
        assert_eq!(field.height_range(), (2_300, 2_300));
        assert_eq!(field.bound_margins(), [2; 24]);
        assert_eq!(
            field.ground_material(IVec3::ZERO, 0, 2_300, 0, 0, 22),
            material::SAND
        );
        assert_eq!(
            field.ground_material(IVec3::ZERO, 0, 2_300, 5, 0, 17),
            material::DIRT
        );
        assert_eq!(
            field.ground_material(IVec3::ZERO, 0, 2_300, 6, 0, 16),
            material::DARK_STONE
        );
        assert!(LayersGenerator
            .build(&grid, 0, r#"{"rock": "Air"}"#)
            .is_err());
    }

    /// Craters exist at every size, basins set the surface word, and the
    /// bounds hold on samples, on spheres and planes.
    #[test]
    fn a_moon_is_craters_and_basins_within_its_bounds() {
        let stack = TerrainLayers::moon();
        for shape in [Shape::Sphere, Shape::Plane] {
            let planet = planet(shape, 1_737_400.0, &stack);
            check_field(&planet, 2_000).unwrap();
            let g = planet.grid();
            let n = g.cells();
            let (mut lo, mut hi, mut fresh, mut mare) = (i32::MAX, i32::MIN, 0, 0);
            for t in 0..4_000 {
                let face = if g.is_plane() {
                    crate::grid::PLANE_FACE
                } else {
                    2
                };
                let column = crate::terrain::Column {
                    face,
                    i: n / 4 + t * 3,
                    j: n / 2,
                    level: 0,
                };
                let h = planet.field().height(column);
                let s = planet.field().surface(column, h);
                lo = lo.min(h);
                hi = hi.max(h);
                fresh += usize::from(s & 0x7f > 0);
                mare += usize::from(s & 0x80 != 0);
            }
            assert!(hi - lo > 1_000, "{shape:?}: relief {lo}..{hi}");
            eprintln!("{shape:?}: relief {lo}..{hi} mm, {fresh} samples on fresh ejecta, {mare} on basins");
            assert_eq!(
                planet.field().appearance(),
                crate::landform::lunar_appearance()
            );
        }
    }

    /// Cliffs step the terrain into terraces: inside their regions most
    /// heights sit on a tread (a whole step), the rises between them are
    /// steep, and the field stays within its bounds on spheres and planes.
    #[test]
    fn cliffs_step_the_terrain_into_terraces_within_its_bounds() {
        let mut stack = TerrainLayers::earth();
        stack.layers.push(Layer {
            coverage: 1.0,
            ..Layer::new(LayerKind::Cliffs)
        });
        for shape in [Shape::Sphere, Shape::Plane] {
            let planet = planet(shape, 6_371_000.0, &stack);
            check_field(&planet, 3_000).unwrap_or_else(|e| panic!("{shape:?}: {e}"));
            let g = planet.grid();
            let n = g.cells();
            let face = if g.is_plane() {
                crate::grid::PLANE_FACE
            } else {
                2
            };
            let step = (crate::terrain::HEIGHT_ONE as f64 * 30.0) as i32;
            let (mut on_tread, mut samples, mut steep) = (0, 0, 0);
            // Land columns across the face (the ocean floor is flat anyway).
            for t in 0..40_000 {
                let (i, j) = (
                    ((t * 7_919) % 199) * (n / 200),
                    (t.wrapping_mul(104_729) % 197) * (n / 200) + t % 64,
                );
                let h = planet.field().height(crate::terrain::Column {
                    face: face,
                    i: i,
                    j: j,
                    level: 0,
                });
                if h <= 50 * crate::terrain::HEIGHT_ONE {
                    continue;
                }
                let h1 = planet.field().height(crate::terrain::Column {
                    face: face,
                    i: i + 1,
                    j: j,
                    level: 0,
                });
                samples += 1;
                on_tread += usize::from(h.rem_euclid(step) == 0);
                steep += usize::from((h1 - h).abs() >= g.layer_mm() as i32 * 2);
            }
            eprintln!("{shape:?}: {on_tread}/{samples} land columns on a tread, {steep} steep");
            assert!(samples > 500, "{shape:?}: {samples} land samples");
            assert!(
                on_tread * 2 > samples,
                "{shape:?}: {on_tread}/{samples} on a tread"
            );
            assert!(steep > 0, "{shape:?}: no cliffs");
        }
    }

    /// The terrace function: flat treads, a riser over the given share of
    /// each step, continuous and monotonic, also below zero.
    #[test]
    fn terraces_are_flat_treads_and_monotonic_risers() {
        use crate::landform::terrace;
        let step = 30_000;
        let mut last = terrace(-100_000, step, 123);
        for h in (-100_000..100_000).step_by(37) {
            let t = terrace(h, step, 123);
            assert!(t >= last, "monotonic at {h}");
            assert!((t - h).abs() < step, "within a step at {h}");
            last = t;
        }
        assert_eq!(terrace(5_000, step, 123), 0, "a tread");
        assert_eq!(terrace(-25_000, step, 123), -30_000, "a tread below zero");
        assert_eq!(terrace(59_999, step, 123), 59_970, "the top of a riser");
    }

    /// No steps between neighbouring columns (the bowls and rims are
    /// continuous; the rim's crease is a slope change, not a step).
    #[test]
    fn the_moon_has_no_steps_between_neighbouring_columns() {
        let planet = planet(Shape::Sphere, 1_737_400.0, &TerrainLayers::moon());
        let g = planet.grid();
        let n = g.cells();
        let mut worst = 0;
        for t in 0..20_000 {
            let (i, j) = (n / 5 + t * 131, n / 3 + t * 17);
            let h = |d: i32| {
                planet.field().height(crate::terrain::Column {
                    face: 2,
                    i: i + d,
                    j: j,
                    level: 0,
                })
            };
            worst = worst.max((h(0) - 2 * h(1) + h(2)).abs());
        }
        assert!(worst < 100, "worst second difference {worst} mm");
    }

    /// Stacks the presets never make: two mountain layers, craters on land,
    /// basins flattening a plateau, layers masked by the coast. Their
    /// bounds hold all the same.
    #[test]
    fn custom_stacks_keep_their_bounds() {
        use LayerKind::*;
        let base = TerrainLayers::earth();
        let mut alien = base.clone();
        alien.layers.insert(
            3,
            Layer {
                kind: Mountains,
                mask: LayerMask::AboveDeepSea,
                height_m: 900.0,
                scale_km: 4.0,
                octaves: 5,
                region_km: 60.0,
                coverage: 0.3,
                ..Layer::default()
            },
        );
        alien.layers.push(Layer {
            kind: Craters,
            mask: LayerMask::Land,
            scale_km: 12.0,
            octaves: 4,
            coverage: 0.4,
            persistence: 1.3,
            ratio: 0.15,
            ratio2: 0.4,
            ratio3: 0.5,
            ..Layer::default()
        });
        alien.layers.retain(|l| l.kind != Roughness);
        let mesa = TerrainLayers {
            layers: vec![
                Layer {
                    kind: Plateau,
                    height_m: 300.0,
                    ..Layer::default()
                },
                Layer {
                    kind: Hills,
                    height_m: 80.0,
                    scale_km: 2.0,
                    octaves: 5,
                    persistence: 0.55,
                    ..Layer::default()
                },
                Layer {
                    kind: Basins,
                    height_m: 250.0,
                    scale_km: 20.0,
                    coverage: 0.4,
                    ..Layer::default()
                },
                Layer {
                    kind: Erosion,
                    height_m: 15.0,
                    scale_km: 0.8,
                    octaves: 4,
                    persistence: 0.5,
                    ratio: 0.3,
                    ..Layer::default()
                },
            ],
            materials: MaterialStyle::Layered,
            surface: "Sand".into(),
            soil: "Clay".into(),
            rock: "Sandstone".into(),
            ..base.clone()
        };
        for (name, stack) in [("alien", alien), ("mesa", mesa)] {
            for shape in [Shape::Sphere, Shape::Plane] {
                let planet = planet(shape, 2_000_000.0, &stack);
                let worst =
                    check_field(&planet, 3_000).unwrap_or_else(|e| panic!("{name} {shape:?}: {e}"));
                eprintln!("{name} {shape:?}: worst {worst:?}");
            }
        }
    }

    /// Rules apply in order: the first that holds picks the material.
    #[test]
    fn material_rules_pick_the_first_rule_that_holds() {
        use crate::landform::ground_material;
        let grid = Grid::new(1_000_000.0, 0.1).unwrap();
        let (k, _) = TerrainLayers::desert().compile(&grid, 7).unwrap();
        let maps = crate::planet_maps::MapView {
            header: &k.maps,
            tables: &[],
            streams: None,
        };
        let gully = (-100i32 as u32) & 0xff;
        let id = |m: u32| m & material::ID;
        let mut seen = std::collections::BTreeMap::new();
        for n in 0..4_000 {
            let p = grid.domain_point(2, 1_000 + n * 977, 2_000 + n * 131, 0);
            // A gully floor on a slope: gravel, the first rule.
            assert_eq!(
                ground_material(&k, maps, p, gully, 500_000, 0, 4, 5_000),
                material::GRAVEL
            );
            // Gentle ground near the surface: sand.
            assert_eq!(
                ground_material(&k, maps, p, 0, 500_000, 3, 2, 5_000),
                material::SAND
            );
            // Deeper: stone patches, clay bands or the sandstone fallback.
            let deep = id(ground_material(
                &k,
                maps,
                p,
                0,
                500_000,
                40,
                2,
                4_960 + n % 40,
            ));
            assert!(
                [material::STONE, material::CLAY, material::SANDSTONE].contains(&deep),
                "{deep}"
            );
            *seen.entry(deep).or_insert(0) += 1;
            // Steep exposed ground: dark stone specks among the others.
            *seen
                .entry(1000 + id(ground_material(&k, maps, p, 0, 500_000, 0, 10, 5_000)))
                .or_insert(0) += 1;
        }
        eprintln!("{seen:?}");
        assert!(
            seen.len() >= 5,
            "patches, bands, fallback and specks all occur: {seen:?}"
        );
        let mut many = TerrainLayers::desert();
        many.rules = vec![MaterialRule::default(); 17];
        assert!(many.compile(&grid, 7).is_err());
        many.rules = vec![MaterialRule::new("Lava")];
        assert!(many.compile(&grid, 7).is_err());
    }

    #[test]
    fn invalid_stacks_are_rejected() {
        use LayerKind::*;
        let grid = Grid::new(1_000_000.0, 0.1).unwrap();
        let with = |layers: Vec<Layer>| TerrainLayers {
            layers,
            ..TerrainLayers::earth()
        };
        let hills = Layer {
            kind: Hills,
            ..Layer::default()
        };
        assert!(with(vec![hills.clone(); LAYERS + 1])
            .compile(&grid, 1)
            .is_err());
        assert!(with(vec![
            hills.clone(),
            Layer {
                kind: Warp,
                ..Layer::default()
            }
        ])
        .compile(&grid, 1)
        .is_err());
        assert!(with(vec![Layer {
            mask: LayerMask::Land,
            ..hills.clone()
        }])
        .compile(&grid, 1)
        .is_err());
        assert!(with(vec![Layer {
            scale_km: f64::NAN,
            ..hills.clone()
        }])
        .compile(&grid, 1)
        .is_err());
        assert!(
            with(vec![
                Layer {
                    kind: Roughness,
                    octaves: 16,
                    ..Layer::default()
                };
                4
            ])
            .compile(&grid, 1)
            .is_err(),
            "too many octaves"
        );
        // Disabled layers do not count.
        let mut many = vec![
            Layer {
                enabled: false,
                ..hills.clone()
            };
            LAYERS + 4
        ];
        many.push(hills);
        assert!(with(many).compile(&grid, 1).is_ok());
    }

    /// Settings round-trip through JSON; empty settings are the Earth preset.
    #[test]
    fn presets_round_trip_and_empty_settings_are_the_earth() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        for stack in [
            TerrainLayers::earth(),
            TerrainLayers::moon(),
            TerrainLayers::flat(),
        ] {
            let json = serde_json::to_string(&stack).unwrap();
            assert_eq!(serde_json::from_str::<TerrainLayers>(&json).unwrap(), stack);
        }
        let empty = LayersGenerator.build(&grid, 7, "").unwrap();
        let earth = TerrainLayers::earth().field(&grid, 7).unwrap();
        assert_eq!(empty.program(), earth.program());
    }
}
