//! A meadow for voxel worlds from the engine's foliage: tufts of blocky blades
//! (`FoliageKind::Block`: staircases of 5 cm cubes on one grid, fanning out from a tight
//! base) of short and tall grass, leafy undergrowth and flower clusters, in bare and
//! dense patches, on the grass material. Placed on the terrain the chunk pass draws (its
//! foliage ground), gone where it is dug, and coloured by the turf under each tuft, so
//! the turf past the last blades already reads as the meadow.
//!
//! Hosts spawn [`Meadow::types`] and [`Meadow::wind`] as SceneDB rows and size the
//! renderer's foliage with [`Meadow::tufts_per_m2`], [`Meadow::blades_per_tuft`] and
//! [`Meadow::range`]. Bands of tiles in the frame of the cube face under the eye carry it
//! out to its range at the same cost on any planet.

use helio_foliage_api::components::{FoliageTypeRow, FoliageWindRow};
use helio_foliage_api::{
    pack_kind_and_flags, FoliageKind, FoliageMaterial, GpuFoliageType, FOLIAGE_FLAG_GROUND_COLOUR,
    FOLIAGE_FLAG_RECEIVES_INTERACTION,
};

use crate::terrain::material;

/// The grass's cube edge, metres.
pub const CUBE_M: f32 = 0.05;

/// A meadow's foliage rows and the renderer settings it is drawn with.
pub struct Meadow {
    /// The types, one SceneDB row each.
    pub types: Vec<FoliageTypeRow>,
    /// The wind, one SceneDB row.
    pub wind: FoliageWindRow,
    /// Tufts a square metre at the densest (every type's together):
    /// `RendererConfig::foliage_tufts_per_m2`.
    pub tufts_per_m2: f32,
    /// Blades a tuft averages: `RendererConfig::foliage_blades_per_tuft`.
    pub blades_per_tuft: f32,
    /// Metres the meadow reaches: `RendererConfig::foliage_range`.
    pub range: f32,
}

/// One type's shape.
struct Kind {
    /// Tufts a square metre.
    density: f32,
    /// Blades a tuft.
    blades: [u32; 2],
    /// Blade heights and widths (metres; a width over the cube is a leaf's plate).
    height: [f32; 2],
    width: [f32; 2],
    /// Lean at the tuft's centre and rim, extra lean at the tip, share lying over.
    lean: [f32; 2],
    curve: f32,
    fall: f32,
    /// The turf's colour times twice this (0.5: the turf's own).
    colour: [f32; 3],
    /// A flower head's colour, sRGB.
    tip: Option<[u8; 3]>,
    /// Patches: metres across, how bare between them.
    patch: [f32; 2],
    /// Where the type fades out, metres (0: the meadow's range).
    reach: f32,
}

fn row(ground: u32, k: Kind) -> FoliageTypeRow {
    let mut t = GpuFoliageType {
        density: k.density,
        height_range: k.height,
        width_range: k.width,
        // Any slope the grass voxels lie on (their tops are level steps).
        slope_range: [-1.0, 1.0],
        altitude_range: [f32::MIN, f32::MAX],
        lod_distances: [6.0, 0.0, 0.0, k.reach],
        // Swaying whole, tips most (cubes move whole), a little flutter and jitter.
        wind_response: [0.7, 0.35, 0.1],
        // Lies flat under a foot (`FoliageInteractorRow`).
        interaction_stiffness: 1.0,
        kind_and_flags: pack_kind_and_flags(
            FoliageKind::Block,
            FOLIAGE_FLAG_RECEIVES_INTERACTION | FOLIAGE_FLAG_GROUND_COLOUR,
        ),
        ground_mask: 1 << ground,
        lean_range: k.lean,
        curve: k.curve,
        fall_chance: k.fall,
        patch_scale: k.patch[0],
        patch_strength: k.patch[1],
        ..GpuFoliageType::default()
    };
    t.set_blades_per_tuft(k.blades);
    t.set_material(Some(FoliageMaterial {
        base_color: k.colour,
        roughness: 0.9,
        metallic: 0.0,
    }));
    t.set_tip_color(k.tip.map(|c| c.map(|v| (f32::from(v) / 255.0).powf(2.2))));
    t.into()
}

/// Short and tall grass, leafy undergrowth and yellow and white flower clusters on
/// [`material::GRASS`].
pub fn meadow() -> Meadow {
    let grass = material::GRASS;
    let cube = [CUBE_M; 2];
    let types = vec![
        // Short grass everywhere, a touch lighter than the turf.
        row(
            grass,
            Kind {
                density: 6.0,
                blades: [6, 11],
                height: [0.12, 0.3],
                width: cube,
                lean: [0.03, 0.35],
                curve: 0.15,
                fall: 0.05,
                colour: [0.58, 0.62, 0.5],
                tip: None,
                patch: [12.0, 0.35],
                reach: 0.0,
            },
        ),
        // Tall grass over most of the meadow, thicker in patches, yellower at the tips.
        row(
            grass,
            Kind {
                density: 5.0,
                blades: [8, 14],
                height: [0.3, 0.6],
                width: cube,
                lean: [0.03, 0.3],
                curve: 0.2,
                fall: 0.1,
                colour: [0.62, 0.62, 0.45],
                tip: None,
                patch: [22.0, 0.5],
                reach: 0.0,
            },
        ),
        // Leafy undergrowth: broad plates fanned low, darker.
        row(
            grass,
            Kind {
                density: 0.7,
                blades: [4, 7],
                height: [0.1, 0.2],
                width: [CUBE_M, 3.0 * CUBE_M],
                lean: [0.9, 1.6],
                curve: 0.3,
                fall: 0.0,
                colour: [0.4, 0.52, 0.38],
                tip: None,
                patch: [9.0, 0.9],
                reach: 140.0,
            },
        ),
        // Flower clusters, yellow and white, in their own patches.
        row(
            grass,
            Kind {
                density: 0.35,
                blades: [3, 6],
                height: [0.22, 0.4],
                width: cube,
                lean: [0.05, 0.3],
                curve: 0.2,
                fall: 0.0,
                colour: [0.45, 0.55, 0.4],
                tip: Some([236, 206, 58]),
                patch: [7.0, 0.95],
                reach: 90.0,
            },
        ),
        row(
            grass,
            Kind {
                density: 0.25,
                blades: [3, 5],
                height: [0.18, 0.34],
                width: cube,
                lean: [0.05, 0.3],
                curve: 0.2,
                fall: 0.0,
                colour: [0.45, 0.55, 0.4],
                tip: Some([238, 236, 226]),
                patch: [6.0, 0.95],
                reach: 90.0,
            },
        ),
    ];
    Meadow {
        types,
        wind: FoliageWindRow {
            direction_speed: [1.0, 0.0, 0.3, 1.5],
            gust: [0.5, 0.2, 0.0, 0.05],
            time_prev_time: [0.0, 0.0],
            _pad: [0.0; 2],
        },
        tufts_per_m2: 13.0,
        blades_per_tuft: 9.5,
        range: 200.0,
    }
}

impl Meadow {
    /// Stands of tall grass, knee to waist high and yellower, in some places (large
    /// patches with bare meadow between).
    pub fn with_tall_stands(mut self) -> Self {
        let stand = row(
            material::GRASS,
            Kind {
                density: 4.0,
                blades: [7, 13],
                height: [0.6, 1.1],
                width: [CUBE_M; 2],
                lean: [0.04, 0.28],
                curve: 0.25,
                fall: 0.08,
                colour: [0.66, 0.64, 0.42],
                tip: None,
                patch: [34.0, 0.97],
                reach: 0.0,
            },
        );
        self.types.push(stand);
        self.tufts_per_m2 += 3.0;
        self.blades_per_tuft += 0.5;
        self
    }

    /// Wind at `speed` m/s along `direction` (across the ground), in gusts of `gust`
    /// times the base sway.
    pub fn with_wind(mut self, direction: [f32; 2], speed: f32, gust: f32) -> Self {
        let length = direction[0].hypot(direction[1]).max(1.0e-6);
        self.wind.direction_speed = [direction[0] / length, 0.0, direction[1] / length, speed];
        self.wind.gust = [gust, 0.15, 0.0, 0.06];
        self
    }
}
