//! Pluggable terrain generation.
//!
//! A [`TerrainGenerator`] turns a seed and its settings (JSON) into a
//! [`TerrainField`] for one world grid: the surface height and ground
//! material of every column. The field is evaluated twice, identically: on
//! the CPU for collision, ray casts, edits and gameplay queries, and by its
//! WGSL [`TerrainProgram`] on the GPU for streaming and rendering. Worlds
//! name their generator in a [`TerrainSource`]; generators register once
//! per process with [`register`], and [`generators`] lists them for editor
//! pickers.
//!
//! # Writing a generator
//!
//! The WGSL program defines its constants, its materials and a two-rate
//! column (`docs/two-rate-field.md`): its largest octaves evaluated at
//! nodes, the columns of a level whose indices are multiples of 16 on their
//! face, and blended. A column reads the bilinear blend of its four nodes'
//! words through the engine's `generation_node_word(slot)`,
//! `floor(sum w_c v_c / 256)` with weights `(16 - di)(16 - dj)`,
//! `di (16 - dj)`, `(16 - di) dj`, `di dj`, the high and low bytes of each
//! word summed apart (so every word blends exactly, wrapping). Generation
//! evaluates each tile's nodes once; a program without large octaves says
//! `terrain_has_nodes` false.
//!
//! ```wgsl
//! struct TerrainConstants { /* the bytes of TerrainProgram::constants */ }
//! fn ground_material(p: vec3<i32>, surface: u32, top_height: i32, depth: i32, slope: i32, layer: i32) -> u32
//! const TERRAIN_NODE_WORDS: u32 = /* words a node carries, at least 1 */;
//! fn terrain_has_nodes(level: u32) -> bool
//! fn terrain_node(p: vec3<i32>, level: u32) // a node at domain point p
//! fn terrain_node_word(slot: u32) -> i32 // of the node just evaluated
//! // (height, surface word); `display`: the program's coarse display
//! // variant, if it has one (may be ignored).
//! fn terrain_column_nodes(p: vec3<i32>, level: u32, display: bool) -> vec2<i32>
//! ```
//!
//! The engine has the column's nodes ready when `terrain_has_nodes`. The
//! surface word (8 bits, stored with each column cell) is whatever the
//! materials need besides height (biome, sediment, crater age), so shading
//! never runs the generator per pixel. It comes with the height
//! (`terrain_column_nodes(..).y`): nothing reads it apart.
//!
//! Its CPU field must blend the same way ([`TerrainField::height`] is by
//! [`Column`] for this).
//!
//! Volumetric shapes (caves, overhangs, ledges) are the engine's volume
//! layer over the field (`crate::volumes`, `docs/volumes.md`): a field
//! supplies its settings ([`TerrainField::volumes`]) and may gate cave
//! networks by its own inputs (planet maps, rock type) at a network's hub,
//! from the hub's reference column's surface word and height:
//!
//! ```wgsl
//! fn terrain_cave_gate(surface: u32, height: i32) -> i32 // Q8, 0..=256 (default 256)
//! ```
//!
//! The program reads its constants from the `terrain` uniform and its
//! tables (data too large for a uniform: a world's planet maps,
//! [`crate::planet_maps`]) from `terrain_tables` (a read-only storage
//! buffer of `vec4<u32>`, [`TerrainProgram::tables`]). It may call the
//! integer noise library (`shaders/noise.wgsl`, mirrored in
//! [`crate::noise`]) and the material ids (`M_*`, see [`material`]).
//!
//! A program with a climate defines
//!
//! ```wgsl
//! fn terrain_climate(p: vec3<i32>, height: i32) -> vec2<i32>
//! ```
//!
//! the temperature (1/128 °C) and moisture (Q16, 0 dry to 65535 wet) at
//! domain point `p` and height `height` (mm), as [`TerrainField::climate`]:
//! features grow by it (a spawn's climate ranges). Without it the climate is
//! 15 °C and half wet everywhere. A program with a drainage and erosion
//! (rivers, worn ranges, sediment: what caves near water or materials of
//! wet ground read) defines
//!
//! ```wgsl
//! fn terrain_erosion(p: vec3<i32>) -> vec2<i32> // (cut, fill level), mm
//! fn terrain_drainage(p: vec3<i32>) -> vec4<i32> // (log2 discharge, flags, river distance, bed)
//! ```
//!
//! as [`TerrainField::erosion`] and [`TerrainField::drainage`].
//! Heights are integer [`HEIGHT_ONE`] units (millimetres) above the datum;
//! `p` is the column centre in domain units ([`crate::grid::DOMAIN_UNIT`],
//! 1.25 cm) on the sphere of the planet's radius (a plane: its horizontal
//! position), independent of the world's voxel size, and `level` the
//! column's footprint, `2^level`
//! reference cells wide, so a field can omit detail finer than it. The CPU
//! and GPU functions must return equal values for every input:
//! `helio_pass_voxel_planet::engine::verify_field` compares them, and [`check_field`] checks
//! the declared bounds against sampled columns.
use crate::grid::{Grid, PLANE_FACE};
use crate::planet::Planet;
use crate::volumes::VolumeLayer;
use glam::IVec3;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Heights are integer millimetres above the datum (sea level).
pub const HEIGHT_ONE: i32 = 1000;
/// Climate of a field without one: 15 °C (1/128 °C), half wet (Q16).
pub const DEFAULT_CLIMATE: [i32; 2] = [15 * 128, 32_768];
/// Largest `TerrainConstants` uniform a program may declare.
pub const MAX_CONSTANT_BYTES: usize = 16384;

/// Engine material ids (`M_*` in WGSL). A ground material may carry
/// [`material::SPECK`].
pub mod material {
    pub const AIR: u32 = 0;
    pub const GRASS: u32 = 1;
    pub const DIRT: u32 = 2;
    pub const STONE: u32 = 3;
    pub const SAND: u32 = 4;
    pub const SNOW: u32 = 5;
    pub const WATER: u32 = 6;
    pub const GRAVEL: u32 = 7;
    pub const SANDSTONE: u32 = 8;
    pub const DARK_STONE: u32 = 9;
    pub const WOOD: u32 = 10;
    pub const LEAVES: u32 = 11;
    pub const CLAY: u32 = 12;
    pub const BRICK: u32 = 13;
    pub const PLANKS: u32 = 14;
    pub const COBBLE: u32 = 15;
    /// Lunar ground: fine regolith, dark mare plains, bright fresh ejecta,
    /// boulders, and the basalt and anorthosite bedrock under them.
    pub const REGOLITH: u32 = 16;
    pub const MARE: u32 = 17;
    pub const EJECTA: u32 = 18;
    pub const BOULDER: u32 = 19;
    pub const BASALT: u32 = 20;
    pub const ANORTHOSITE: u32 = 21;
    /// Ids in use, air included (every terrain program shares them, so a
    /// painted material means the same on any world).
    pub const COUNT: u32 = 22;
    /// Flag on a ground material: a single-voxel fleck of a surface (mud
    /// in a meadow) that blends into grass once its cell is about a pixel
    /// wide instead of tinting the distant ground.
    pub const SPECK: u32 = 0x100;
    /// Bits of the material id.
    pub const ID: u32 = 0xff;

    /// Names of the solid materials, in id order after air.
    pub const NAMES: [&str; COUNT as usize - 1] = [
        "Grass",
        "Dirt",
        "Stone",
        "Sand",
        "Snow",
        "Water",
        "Gravel",
        "Sandstone",
        "DarkStone",
        "Wood",
        "Leaves",
        "Clay",
        "Brick",
        "Planks",
        "Cobble",
        "Regolith",
        "Mare",
        "Ejecta",
        "Boulder",
        "Basalt",
        "Anorthosite",
    ];

    /// The material id of a name in [`NAMES`] (ignoring case and `_`).
    pub fn from_name(name: &str) -> Option<u32> {
        let key: String = name
            .chars()
            .filter(|c| *c != '_')
            .collect::<String>()
            .to_ascii_lowercase();
        NAMES
            .iter()
            .position(|n| n.to_ascii_lowercase() == key)
            .map(|i| i as u32 + 1)
    }
}

/// The GPU half of a [`TerrainField`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainProgram {
    /// Identity of `wgsl`: equal keys must mean equal source. Pipelines are
    /// compiled once per key, so a settings change that only changes
    /// `constants` never recompiles shaders.
    pub key: Cow<'static, str>,
    /// Defines `TerrainConstants`, `ground_material` and the two-rate column
    /// (see the module doc).
    pub wgsl: Cow<'static, str>,
    /// Bytes of the `TerrainConstants` uniform (WGSL uniform layout, at
    /// most [`MAX_CONSTANT_BYTES`]).
    pub constants: Vec<u8>,
    /// Words of the `terrain_tables` storage buffer (may be empty), shared
    /// by every program of the world.
    pub tables: Arc<[u32]>,
    /// Data the program reads by region, made on demand (`terrain_regions`;
    /// `None`: it reads none).
    pub regions: Option<Regions>,
}

/// Data a terrain program reads by region, too large to bake whole (a
/// planet's streams, [`crate::streams`]): fixed-size word blocks keyed by
/// two words, which the field makes on the CPU and the engine keeps
/// resident on the GPU wherever it evaluates the program
/// (`terrain_regions`, binding 41).
///
/// The program finds a region with `terrain_region(key: vec2<u32>) -> i32`
/// (its slot, -1 when not resident) and reads its words with
/// `terrain_region_word(slot: u32, i: u32) -> u32`. The engine keeps every
/// region [`TerrainRegions::keys`] names for a footprint resident while it
/// generates it, so a program reading only those reads what its CPU field
/// reads.
pub trait TerrainRegions: Send + Sync {
    /// Words of every region.
    fn words(&self) -> usize;
    /// Equal for sources that make the same regions (programs compare by
    /// it).
    fn identity(&self) -> u64;
    /// The regions the field reads at the columns `[i0, i1] x [j0, j1]` of
    /// `face` at grid level `level` of `grid` (and a margin around them of
    /// at least the program's reach at that level), appended to `out`.
    fn keys(
        &self,
        grid: &Grid,
        face: u8,
        level: u32,
        i: [i32; 2],
        j: [i32; 2],
        out: &mut Vec<[u32; 2]>,
    );
    /// The words of region `key` into `out` (`words()` of them).
    fn region(&self, key: [u32; 2], out: &mut [u32]);
}

/// A program's [`TerrainRegions`] (programs compare equal when they share
/// the same source).
#[derive(Clone)]
pub struct Regions(pub Arc<dyn TerrainRegions>);

impl std::fmt::Debug for Regions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Regions({} words)", self.0.words())
    }
}

impl PartialEq for Regions {
    fn eq(&self, other: &Self) -> bool {
        self.0.identity() == other.0.identity()
    }
}

impl Eq for Regions {}

/// A column of the world grid: its face, cell indices and level (grid
/// levels; its footprint is `2^(level + level_offset)` reference cells).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Column {
    pub face: u8,
    pub i: i32,
    pub j: i32,
    pub level: u32,
}

/// Surface height and ground material of every column of one world grid.
pub trait TerrainField: Send + Sync + 'static {
    /// Surface height ([`HEIGHT_ONE`] units above the datum) of `column`.
    /// A field may share work between neighbouring columns of a level
    /// (the two-rate field's nodes): it is a column's, not a point's.
    fn height(&self, column: Column) -> i32;
    /// A height at least `column`'s, cheaper where the field can leave out
    /// terms that only lower it (a landform's streams): what bounds want
    /// without the exact surface.
    fn height_bound(&self, column: Column) -> i32 {
        self.height(column)
    }
    /// Material of a solid ground cell: `surface` is its column cell's
    /// surface word ([`Self::surface`]), `top_height` its column's top
    /// ([`HEIGHT_ONE`] units, a whole number of voxel layers), `depth` the
    /// cells below the column top (0 is the exposed top cell), `slope` the
    /// ground slope in eighths of a cell per cell (`Planet::material_slope`),
    /// and `layer` the cell's layer index (0 is the layer just above the
    /// datum); `p` is the cell's volume point.
    fn ground_material(
        &self,
        p: IVec3,
        surface: u32,
        top_height: i32,
        depth: i32,
        slope: i32,
        layer: i32,
    ) -> u32;
    /// Surface word (8 bits) of `column`, whose height is `height`
    /// (`terrain_column_nodes(..).y` in WGSL, stored per column cell at generation):
    /// inputs of the materials besides height. 0 when the program has none.
    fn surface(&self, _column: Column, _height: i32) -> u32 {
        0
    }
    /// Climate at domain point `p` and height `height` (mm): temperature in
    /// 1/128 °C and moisture in Q16 (`terrain_climate` in WGSL; 15 °C and
    /// half wet without one).
    fn climate(&self, _p: IVec3, _height: i32) -> (i32, i32) {
        (DEFAULT_CLIMATE[0], DEFAULT_CLIMATE[1])
    }
    /// The worn terrain at domain point `p` (mm): how far erosion cut it
    /// down and the level its sediment fills up to (`terrain_erosion` in
    /// WGSL; `(0, i32::MIN)` without: nothing worn, no sediment).
    fn erosion(&self, _p: IVec3) -> (i32, i32) {
        (0, i32::MIN)
    }
    /// The drainage at domain point `p` (`terrain_drainage` in WGSL;
    /// [`crate::planet_maps::drainage`]): the wet area draining through it
    /// (`log2(1 + km²)` in 1/2048), its flags (lake, ocean, river, dry
    /// sink), the nearest river's distance (domain units, `i32::MAX`:
    /// none) and its bed (mm).
    fn drainage(&self, _p: IVec3) -> [i32; 4] {
        [0, 0, i32::MAX, 0]
    }
    /// Lowest and highest height any column can have at any level.
    fn height_range(&self) -> (i32, i32);
    /// Per grid level `L >= 1`: how many level-`L` cells the top of any
    /// finer column inside a level-`L` column may rise above that column's
    /// own top, plus two. Coarse levels bound the terrain with this, so it
    /// must be conservative; [`check_field`] samples it.
    fn bound_margins(&self) -> [i32; 24];
    /// The terrain's volumetric shapes (caves, cliff shapes), compiled for
    /// this field's grid (`docs/volumes.md`).
    fn volumes(&self) -> Option<&VolumeLayer> {
        None
    }
    /// Share (Q8, 0 to 256) of cave networks allowed at a network's hub,
    /// by its reference column's surface word and height
    /// (`terrain_cave_gate` in WGSL).
    fn cave_gate(&self, _surface: u32, _height: i32) -> i32 {
        256
    }
    /// The generator's own material table and detail (the renderer's
    /// appearance unless the world overrides it).
    fn appearance(&self) -> TerrainAppearance {
        TerrainAppearance::default()
    }
    fn program(&self) -> TerrainProgram;
}

/// How one terrain material looks. Shading knows materials only through
/// these properties (no material is special), so any generator can define
/// its own: regolith and basalt on a moon, coloured sands elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialAppearance {
    /// sRGB colour in [0, 1] and perceptual roughness.
    pub colour: [f32; 4],
    /// Dry, middle and lush world-space patch colours (sRGB), when the
    /// material varies by patches (turf).
    pub patches: Option<[[f32; 3]; 3]>,
    /// Material showing on the sides of this material's surface cells below
    /// a lip (soil under turf).
    pub lip: Option<u8>,
    /// Material of this one's single-voxel flecks and their share, averaged
    /// into its colour once flecks are below a pixel.
    pub fleck: Option<(u8, f32)>,
    /// Material a filtered single-voxel speck of this one blends into.
    pub speck_host: Option<u8>,
    /// The share of light on one side of a cell of this material that
    /// leaves the other, tinted by its colour (foliage lets some through;
    /// 0: opaque). Such a material is two-sided foliage: the sun from
    /// behind lights it through itself and the cells past it, and global
    /// illumination lights the shade under canopies with it.
    pub transmission: f32,
    /// Light the material gives off: its colour times this (scene-linear;
    /// 0: none). It glows, and global illumination lights its
    /// surroundings with it.
    pub emission: f32,
}

impl Default for MaterialAppearance {
    fn default() -> Self {
        Self {
            colour: [1.0, 0.0, 1.0, 0.9],
            patches: None,
            lip: None,
            fleck: None,
            speck_host: None,
            transmission: 0.0,
            emission: 0.0,
        }
    }
}

/// Material slots of a world's appearance table (ids 0..MATERIALS; the
/// built-in ones are [`material::NAMES`]).
pub const MATERIALS: usize = 32;

/// Art controls, independent of occupancy, terrain recipes and edit journals.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainAppearance {
    /// Per material id (see [`MaterialAppearance`]).
    pub materials: [MaterialAppearance; MATERIALS],
    /// Patch contrast, voxel pigment contrast, edge darkening, step
    /// softness (share of the slope's normal in natural voxel steps' light).
    pub detail: [f32; 4],
    /// How smooth surfaces are lit, 0 to 1: at 0 voxel faces are lit by
    /// their own planes (natural steps by the slope they stand for), at 1
    /// every hit is lit by the normal of the smooth surface the voxels
    /// approximate (a quadratic B-spline density over the cells around it,
    /// continuous across cells), edits, caves, overhangs and features
    /// included, with the voxels' edge darkening and pigment mosaic faded.
    /// The voxels themselves (collision, ray casts, edits) are unchanged.
    /// A host's per-world setting (the voxel component's `smoothness`), not
    /// part of an appearance file.
    #[serde(skip)]
    pub smoothing: f32,
}

impl Default for TerrainAppearance {
    /// The built-in generators' materials (`terrain::material`).
    fn default() -> Self {
        use crate::terrain::material::*;
        // Air and unused slots are magenta (a missing material shows).
        let colours: [[u8; 3]; COUNT as usize] = [
            [200, 0, 200],
            [106, 126, 68],
            [128, 96, 66],
            [133, 139, 142],
            [203, 188, 151],
            [217, 228, 236],
            [28, 72, 92],
            [116, 111, 102],
            [185, 142, 104],
            [82, 88, 95],
            [101, 75, 53],
            [59, 102, 52],
            [155, 113, 89],
            [148, 77, 63],
            [158, 119, 79],
            [121, 126, 130],
            [142, 140, 135],
            [84, 84, 86],
            [196, 194, 188],
            [112, 110, 106],
            [58, 59, 63],
            [168, 166, 158],
        ];
        let roughness = [
            0.9, 0.94, 0.96, 0.84, 0.93, 0.78, 0.35, 0.9, 0.88, 0.82, 0.97, 0.94, 0.92, 0.86, 0.86,
            0.85, 0.95, 0.95, 0.93, 0.85, 0.8, 0.85,
        ];
        let unit = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        let mut materials: [MaterialAppearance; MATERIALS] =
            std::array::from_fn(|i| match colours.get(i) {
                Some(&c) => {
                    let [r, g, b] = unit(c);
                    MaterialAppearance {
                        colour: [r, g, b, roughness[i]],
                        ..Default::default()
                    }
                }
                None => MaterialAppearance::default(),
            });
        let turf = &mut materials[GRASS as usize];
        turf.patches = Some([
            unit([160, 150, 96]),
            unit([106, 126, 68]),
            unit([62, 98, 56]),
        ]);
        turf.lip = Some(DIRT as u8);
        for speck in [DIRT, SAND] {
            materials[speck as usize].speck_host = Some(GRASS as u8);
        }
        for rock in [STONE, DARK_STONE, SANDSTONE] {
            materials[rock as usize].fleck = Some((DIRT as u8, 0.125));
        }
        // Weathered and fresh stone: lichen-warm, the rock's grey, darker.
        materials[STONE as usize].patches = Some([
            unit([160, 157, 149]),
            unit([133, 139, 142]),
            unit([98, 104, 110]),
        ]);
        // Dark stone alternates with stone in 4.5 m strata on cliffs: a shade
        // darker, not a band of another colour (zebra stripes on outcrops).
        materials[DARK_STONE as usize].patches = Some([
            unit([140, 138, 131]),
            unit([110, 115, 119]),
            unit([82, 87, 94]),
        ]);
        // Lunar ground varies in maturity (fresh is brighter); fresh ejecta
        // and boulders thin out into the regolith around them.
        materials[REGOLITH as usize].patches = Some([
            unit([158, 156, 150]),
            unit([142, 140, 135]),
            unit([118, 117, 113]),
        ]);
        materials[MARE as usize].patches =
            Some([unit([98, 98, 99]), unit([84, 84, 86]), unit([68, 68, 71])]);
        materials[BOULDER as usize].patches = Some([
            unit([130, 128, 123]),
            unit([112, 110, 106]),
            unit([90, 89, 86]),
        ]);
        materials[EJECTA as usize].speck_host = Some(REGOLITH as u8);
        // A cell of leaves (a cluster with gaps) passes about two thirds of
        // the light on it.
        materials[LEAVES as usize].transmission = 0.65;
        // Natural steps are lit mostly by the slope they stand for (step
        // softness 0.75): lit by their own faces, every 10 cm step of a
        // gentle slope showed its riser and the shadow it casts as a dark
        // contour line ("sound waves" over meadows). Edges and corner
        // occlusion still draw the voxels.
        Self {
            materials,
            detail: [0.3, 0.4, 0.08, 0.75],
            smoothing: 0.0,
        }
    }
}

/// What a generator is, for registration and editor pickers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratorInfo {
    /// Stable id stored in worlds, e.g. `helio.terrain`.
    pub id: String,
    /// Output version: a change that alters any generated column is a new
    /// version, so saved worlds keep the terrain they were edited against.
    pub version: u32,
    pub name: String,
    pub description: String,
    /// Engine class name of the component that holds this generator's
    /// settings, if it has one; its serialized form is the settings JSON.
    pub settings_component: Option<String>,
}

pub trait TerrainGenerator: Send + Sync + 'static {
    fn info(&self) -> GeneratorInfo;
    /// The field of `grid` for `seed` and `settings` (JSON; empty means
    /// the generator's defaults).
    fn build(
        &self,
        grid: &Grid,
        seed: u64,
        settings: &str,
    ) -> Result<Arc<dyn TerrainField>, String>;
}

/// The generator a world uses: its id, version, seed and settings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainSource {
    pub generator: String,
    /// Generator output version; 0 accepts the registered version, which
    /// `Planet::recipe` then names.
    pub version: u32,
    pub seed: u64,
    /// Generator settings (JSON); empty means its defaults.
    pub settings: String,
}

impl Default for TerrainSource {
    fn default() -> Self {
        Self {
            generator: crate::layers::ID.into(),
            version: 0,
            seed: 7,
            settings: String::new(),
        }
    }
}

type Registry = RwLock<BTreeMap<String, Arc<dyn TerrainGenerator>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map: BTreeMap<String, Arc<dyn TerrainGenerator>> = BTreeMap::new();
        for generator in [Arc::new(crate::layers::LayersGenerator) as Arc<dyn TerrainGenerator>] {
            map.insert(generator.info().id, generator);
        }
        RwLock::new(map)
    })
}

/// Make a generator available to every world in this process.
pub fn register(generator: Arc<dyn TerrainGenerator>) -> Result<(), String> {
    let info = generator.info();
    if info.id.is_empty() || info.version == 0 {
        return Err("a terrain generator needs an id and a nonzero version".into());
    }
    let mut map = registry()
        .write()
        .map_err(|_| "terrain registry poisoned")?;
    if map.contains_key(&info.id) {
        return Err(format!(
            "terrain generator {} is already registered",
            info.id
        ));
    }
    map.insert(info.id, generator);
    Ok(())
}

/// The generator registered as `id`, if it has `version` (0 accepts the
/// registered version). One version of each generator is registered.
pub fn find(id: &str, version: u32) -> Option<Arc<dyn TerrainGenerator>> {
    let generator = registry().read().ok()?.get(id).cloned()?;
    (version == 0 || generator.info().version == version).then_some(generator)
}

/// Every registered generator, by name.
pub fn generators() -> Vec<GeneratorInfo> {
    let mut list: Vec<_> = registry()
        .read()
        .map(|map| map.values().map(|g| g.info()).collect())
        .unwrap_or_default();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Build the field of `source` for `grid`.
pub fn build(source: &TerrainSource, grid: &Grid) -> Result<Arc<dyn TerrainField>, String> {
    let generator = find(&source.generator, source.version).ok_or_else(|| {
        format!(
            "unknown terrain generator {} v{}",
            source.generator, source.version
        )
    })?;
    let field = generator.build(grid, source.seed, &source.settings)?;
    let (lo, hi) = field.height_range();
    let program = field.program();
    if lo > hi || program.constants.len() > MAX_CONSTANT_BYTES || program.key.is_empty() {
        return Err(format!(
            "terrain generator {} built an invalid field",
            source.generator
        ));
    }
    Ok(field)
}

/// First air layer above a column of `height` at `level` (level cells).
#[inline]
pub fn top_cells(grid: &Grid, height: i32, level: u32) -> i32 {
    height.div_euclid(grid.layer_mm() as i32) >> level
}

/// Heightfield terrain kind at a level cell before edits: 0 air, 1 solid.
#[inline]
pub fn terrain_kind(top: i32, k: i32) -> u32 {
    u32::from(k < top)
}

/// Check a world's field against its declared bounds at `samples` random
/// columns: every height inside [`TerrainField::height_range`], and every
/// finer column top within [`TerrainField::bound_margins`] of the coarser
/// column containing it. Returns the worst margin use per level.
pub fn check_field(planet: &Planet, samples: u32) -> Result<Vec<i32>, String> {
    let grid = *planet.grid();
    let field = planet.field();
    let margins = field.bound_margins();
    let (lo, hi) = field.height_range();
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut worst = vec![i32::MIN; grid.levels() as usize];
    for _ in 0..samples {
        let level = 1 + (next() % u64::from(grid.levels() - 1)) as u32;
        let cells = (grid.cells() >> level).max(1) as u64;
        let face = if grid.is_plane() {
            PLANE_FACE
        } else {
            (next() % 6) as u8
        };
        let (i, j) = ((next() % cells) as i32, (next() % cells) as i32);
        let height = planet.column_height(face, i, j, level);
        if height < lo || height > hi {
            return Err(format!(
                "level {level} column ({face}, {i}, {j}): height {height} outside {lo}..={hi}"
            ));
        }
        let top = top_cells(&grid, height, level);
        for _ in 0..6 {
            let finer = (next() % u64::from(level)) as u32;
            let shift = level - finer;
            let fi = (i << shift) + (next() % (1u64 << shift)) as i32;
            let fj = (j << shift) + (next() % (1u64 << shift)) as i32;
            let fine = planet.column_top(face, fi, fj, finer);
            let excess = (((fine - 1) >> shift) + 1) - top;
            worst[level as usize] = worst[level as usize].max(excess);
            if excess > margins[level as usize] {
                return Err(format!(
                    "level {level} column ({face}, {i}, {j}): a level {finer} top rises {excess} cells, margin {}",
                    margins[level as usize]
                ));
            }
        }
    }
    Ok(worst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_generators_are_listed_and_duplicates_rejected() {
        let ids: Vec<_> = generators().into_iter().map(|g| g.id).collect();
        assert!(ids.contains(&crate::layers::ID.to_string()));
        assert!(register(Arc::new(crate::layers::LayersGenerator)).is_err());
        let grid = Grid::plane(crate::grid::Shape::Plane, 1024.0, 0.1).unwrap();
        let unknown = TerrainSource {
            generator: "example.none".into(),
            ..TerrainSource::default()
        };
        assert!(build(&unknown, &grid).is_err());
    }
}
