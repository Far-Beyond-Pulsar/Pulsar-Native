//! Bands of resident tiles: foliage out to the horizon at a cost that does not grow with
//! the distance.
//!
//! A single ring of 8 m tiles cannot reach far: its tile count (and arena) grows as the
//! square of its radius. Foliage is resident in **bands** instead, like a clipmap. Band
//! `k` tiles the ground in tiles of `8 · 2^k` m, keeps a ring of
//! [`FOLIAGE_BAND_TILES`]² of them around the eye, and draws the distances between the
//! previous band's reach and its own ([`FOLIAGE_BAND_REACH`] of its tiles): 20, 40, 80,
//! 160, 320 m. Each band's ring costs the same, so the reach doubles per band at a
//! constant price.
//!
//! What thins it out is the tufts' **rank**. A tuft (a placement candidate) carries a
//! uniform rank; at distance `d` it is drawn while its rank is under [`keep`]`(d)` — whole
//! tufts drop out, never blades of one, and the ones left grow a little
//! ([`growth`]) so the meadow's cover holds. The share falls steeply near, where tufts
//! are many pixels and their blades the cost, and slowly past a knee, where a tuft is
//! one cheap shape and the cover is what reads: fallen as steeply to the range, the
//! middle distance went bare (the turf showing between a ring of near grass and the
//! far tufts). A band holds tufts only at the density its
//! nearest point needs (its *share*, `keep` at its inner edge), so the far bands' arenas
//! stay small. Between bands both draw across a fade, the near one shrinking out as the
//! far one grows in.
//!
//! The near band stores one record per blade (drawn blade by blade, L0 and L1); the
//! others one per tuft (L2 draws its blades in one instance, L3 the tuft as one shape).
//!
//! The table ([`GpuFoliageBands`]) is a uniform shared by placement, the cull and the
//! draw; `foliage_common.wgsl` mirrors it and its functions.

use bytemuck::{Pod, Zeroable};

use crate::FOLIAGE_TILE_SIZE_METERS;

/// Most bands a table holds.
pub const FOLIAGE_MAX_BANDS: usize = 6;

/// Tiles across each band's resident ring. Seven around the eye's tile reach three tiles
/// in every direction wherever the eye is in its own: past [`FOLIAGE_BAND_REACH`].
pub const FOLIAGE_BAND_TILES: u32 = 7;

/// How far a band draws, in its own tiles.
pub const FOLIAGE_BAND_REACH: f32 = 2.5;

/// Share of a boundary's radius over which the bands on either side of it fade.
pub const FOLIAGE_BAND_FADE: f32 = 0.25;

/// Side of the blocks big tiles ask the ground in (metres): each block's queries are
/// placed from its own reference point, so a ground's linearised mapping (a planet's)
/// stays exact to millimetres.
pub const FOLIAGE_PROBE_BLOCK_METERS: f32 = 16.0;

/// How a band's records are stored and drawn ([`GpuFoliageBand::lod`]).
pub const BAND_LOD_BLADES: u32 = 0;
/// One record per tuft, its blades drawn in one instance (L2).
pub const BAND_LOD_TUFT_BLADES: u32 = 2;
/// One record per tuft, drawn as one shape (L3).
pub const BAND_LOD_TUFT: u32 = 3;

/// One band (`foliage_common.wgsl`'s `FoliageBand`). 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuFoliageBand {
    /// First tile-table slot of the band's ring, and how many it has.
    pub slot_base: u32,
    pub slot_count: u32,
    /// First arena record of the band's slabs, and records a slab holds.
    pub arena_base: u32,
    pub slab: u32,
    /// Tile edge (metres), and the base (8 m) tiles across one.
    pub tile_size: f32,
    pub tile_span: u32,
    /// Distances the band draws: from `inner` (fading in over the `fade_in` under it) to
    /// `outer` (fading out over the `fade_out` under it).
    pub inner: f32,
    pub outer: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    /// Share of the full density the band holds (`keep` at its nearest).
    pub share: f32,
    /// Candidates (tufts) across a tile, and probe blocks across one.
    pub candidate_grid: u32,
    pub probe_blocks: u32,
    /// [`BAND_LOD_BLADES`], [`BAND_LOD_TUFT_BLADES`] or [`BAND_LOD_TUFT`].
    pub lod: u32,
    /// Candidates a square metre: the acceptance's denominator.
    pub grid_density: f32,
    pub _pad: u32,
}

/// The band table (`foliage_common.wgsl`'s `FoliageBands`). 416 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuFoliageBands {
    pub count: u32,
    /// [`keep`]: full density within `keep_distance` metres, falling as
    /// `(keep_distance / d)^keep_power` past it, and as `d^-keep_far_power` past
    /// `keep_knee` metres.
    pub keep_distance: f32,
    pub keep_power: f32,
    /// [`growth`]: surviving tufts grow by `keep^-growth_power`, at most `growth_max`.
    pub growth_power: f32,
    pub growth_max: f32,
    /// Where the last band ends (metres).
    pub range: f32,
    pub keep_knee: f32,
    pub keep_far_power: f32,
    pub bands: [GpuFoliageBand; FOLIAGE_MAX_BANDS],
}

const _: () = {
    assert!(std::mem::size_of::<GpuFoliageBand>() == 64);
    assert!(std::mem::size_of::<GpuFoliageBands>() == 32 + 64 * FOLIAGE_MAX_BANDS);
};

/// What a pass's bands are planned from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoliageRange {
    /// Metres the foliage reaches (the last band ends here).
    pub range: f32,
    /// Densest ground in tufts a square metre (sizes the candidate grids).
    pub tufts_per_m2: f32,
    /// Blades a tuft averages (sizes the near band's slabs).
    pub blades_per_tuft: f32,
    /// [`GpuFoliageBands::keep_distance`], `keep_knee` and the powers.
    pub keep_distance: f32,
    pub keep_power: f32,
    pub keep_knee: f32,
    pub keep_far_power: f32,
    pub growth_power: f32,
    pub growth_max: f32,
    /// Candidate-grid granularity (the cull's cluster block edge).
    pub cluster_edge: u32,
}

impl Default for FoliageRange {
    fn default() -> Self {
        Self {
            range: 200.0,
            // The engine meadow's (`helio_pass_voxel_planet::meadow`): a host that sizes
            // nothing (an editor's project settings) still grows it at its density.
            tufts_per_m2: 16.0,
            blades_per_tuft: 10.0,
            keep_distance: 14.0,
            keep_power: 1.5,
            keep_knee: 20.0,
            keep_far_power: 1.1,
            growth_power: 0.3,
            growth_max: 2.0,
            cluster_edge: 4,
        }
    }
}

/// Share of the tufts drawn at `d` metres (`foliage_keep` in the shaders).
pub fn keep(bands: &GpuFoliageBands, d: f32) -> f32 {
    let near = |d: f32| {
        (bands.keep_distance / d.max(1.0e-3))
            .powf(bands.keep_power)
            .min(1.0)
    };
    if d <= bands.keep_knee {
        return near(d);
    }
    near(bands.keep_knee) * (bands.keep_knee / d).powf(bands.keep_far_power)
}

/// How much a surviving tuft grows at `d` metres.
pub fn growth(bands: &GpuFoliageBands, d: f32) -> f32 {
    keep(bands, d)
        .powf(-bands.growth_power)
        .clamp(1.0, bands.growth_max.max(1.0))
}

/// The band's weight at `d` metres: 1 inside it, fading across its boundaries, 0 past.
pub fn band_weight(band: &GpuFoliageBand, d: f32) -> f32 {
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a).max(1.0e-6)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let fade_in = if band.inner > 0.0 {
        smooth(band.inner - band.fade_in, band.inner, d)
    } else {
        1.0
    };
    fade_in * (1.0 - smooth(band.outer - band.fade_out, band.outer, d))
}

/// The scale a tuft of `rank` draws at in `band` at `d` metres (0: not drawn): the band's
/// weight, the tuft shrinking out as the kept share falls to its rank, and its growth
/// (`foliage_tuft_scale`).
pub fn tuft_scale(bands: &GpuFoliageBands, band: &GpuFoliageBand, d: f32, rank: f32) -> f32 {
    let w = band_weight(band, d);
    if w <= 0.0 {
        return 0.0;
    }
    let rel = keep(bands, d) / band.share.max(1.0e-6);
    let fade = ((rel - rank) / (0.2 * rel).max(1.0e-3)).clamp(0.0, 1.0);
    w * fade * growth(bands, d)
}

/// The bands reaching `range.range`, their slots and slabs laid out from slot and record 0.
pub fn plan_bands(range: &FoliageRange) -> GpuFoliageBands {
    let mut table = GpuFoliageBands {
        keep_distance: range.keep_distance.max(0.1),
        keep_power: range.keep_power.max(0.0),
        keep_knee: range.keep_knee.max(range.keep_distance).max(0.1),
        keep_far_power: range.keep_far_power.clamp(0.0, range.keep_power.max(0.0)),
        growth_power: range.growth_power.max(0.0),
        growth_max: range.growth_max.max(1.0),
        range: range.range.max(1.0),
        ..Default::default()
    };
    let edge = range.cluster_edge.max(1);
    let (mut slot, mut record, mut inner, mut fade_in) = (0u32, 0u32, 0.0f32, 0.0f32);
    for k in 0..FOLIAGE_MAX_BANDS {
        let span = 1u32 << k;
        let tile = FOLIAGE_TILE_SIZE_METERS * span as f32;
        let outer = (FOLIAGE_BAND_REACH * tile).min(table.range);
        let is_last = outer >= table.range || k + 1 == FOLIAGE_MAX_BANDS;
        // The last band fades out at the range over at most half its width, clear of the
        // fade it enters by.
        let fade_out = if is_last {
            (0.2 * outer).min(0.5 * (outer - inner))
        } else {
            FOLIAGE_BAND_FADE * outer
        };
        // The density the band's nearest drawn point needs.
        let nearest = (inner - fade_in).max(0.0);
        let share = keep(&table, nearest.max(1.0e-3)).min(1.0);
        let blocks = ((tile / FOLIAGE_PROBE_BLOCK_METERS).round() as u32).max(1);
        let step = blocks * edge;
        let wanted = (range.tufts_per_m2.max(0.01) * share * tile * tile).sqrt();
        let grid = ((wanted / step as f32).ceil() as u32).max(1) * step;
        let grid = grid.min((isqrt(crate::MAX_CANDIDATES_PER_TILE) / step).max(1) * step);
        let candidates = grid * grid;
        // The tufts the grid can hold at full density decide the share it achieves.
        let achieved = (candidates as f32 / (range.tufts_per_m2.max(0.01) * tile * tile))
            .min(share.max(1.0e-3));
        let lod = match k {
            0 => BAND_LOD_BLADES,
            1 => BAND_LOD_TUFT_BLADES,
            _ => BAND_LOD_TUFT,
        };
        let per_tuft = if lod == BAND_LOD_BLADES {
            range.blades_per_tuft.max(1.0) * 1.3
        } else {
            1.0
        };
        let slab =
            ((candidates as f32 * per_tuft).ceil() as u32).clamp(1, crate::MAX_BLADES_PER_TILE);
        let slots = FOLIAGE_BAND_TILES * FOLIAGE_BAND_TILES;
        table.bands[k] = GpuFoliageBand {
            slot_base: slot,
            slot_count: slots,
            arena_base: record,
            slab,
            tile_size: tile,
            tile_span: span,
            inner,
            outer,
            fade_in,
            fade_out,
            share: achieved,
            candidate_grid: grid,
            probe_blocks: blocks,
            lod,
            grid_density: candidates as f32 / (tile * tile),
            _pad: 0,
        };
        table.count = k as u32 + 1;
        slot += slots;
        record += slots * slab;
        if is_last {
            break;
        }
        inner = outer;
        fade_in = fade_out;
    }
    table
}

impl GpuFoliageBands {
    /// The bands in use.
    pub fn bands(&self) -> &[GpuFoliageBand] {
        &self.bands[..self.count as usize]
    }

    /// Tile-table slots over every band.
    pub fn slots(&self) -> u32 {
        self.bands().iter().map(|b| b.slot_count).sum()
    }

    /// Arena records over every band.
    pub fn records(&self) -> u64 {
        self.bands()
            .iter()
            .map(|b| u64::from(b.slot_count) * u64::from(b.slab))
            .sum()
    }

    /// The band a slot belongs to.
    pub fn band_of_slot(&self, slot: u32) -> Option<usize> {
        self.bands()
            .iter()
            .position(|b| slot >= b.slot_base && slot < b.slot_base + b.slot_count)
    }
}

fn isqrt(value: u32) -> u32 {
    let mut r = (value as f64).sqrt() as u32;
    while r * r > value {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= value {
        r += 1;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_reach_the_range_with_rings_of_equal_size() {
        let table = plan_bands(&FoliageRange {
            range: 250.0,
            ..FoliageRange::default()
        });
        let bands = table.bands();
        assert!(bands.len() >= 4, "{} bands", bands.len());
        assert_eq!(bands.last().unwrap().outer, 250.0);
        for pair in bands.windows(2) {
            // Each band starts where the last ends, and overlaps it by the same fade.
            assert_eq!(pair[1].inner, pair[0].outer);
            assert_eq!(pair[1].fade_in, pair[0].fade_out);
            assert_eq!(pair[1].tile_size, 2.0 * pair[0].tile_size);
            assert_eq!(pair[1].slot_base, pair[0].slot_base + pair[0].slot_count);
            assert_eq!(
                pair[1].arena_base,
                pair[0].arena_base + pair[0].slot_count * pair[0].slab
            );
        }
        for b in bands {
            // A ring of seven tiles holds the band's reach wherever the eye is in its tile.
            assert!(
                b.outer <= 3.0 * b.tile_size,
                "band reaching {} m over tiles of {} m",
                b.outer,
                b.tile_size
            );
            assert_eq!(b.candidate_grid % (b.probe_blocks * 4), 0);
            assert!(b.candidate_grid * b.candidate_grid <= crate::MAX_CANDIDATES_PER_TILE);
            assert!(b.share > 0.0 && b.share <= 1.0);
        }
        assert_eq!(bands[0].lod, BAND_LOD_BLADES);
        assert_eq!(bands[0].share, 1.0);
    }

    #[test]
    fn neighbouring_bands_sum_to_one_across_their_fade() {
        let table = plan_bands(&FoliageRange::default());
        let bands = table.bands();
        for pair in bands.windows(2) {
            let mut d = pair[0].outer - pair[0].fade_out - 1.0;
            while d < pair[0].outer + 1.0 {
                let sum = band_weight(&pair[0], d) + band_weight(&pair[1], d);
                assert!((sum - 1.0).abs() < 1.0e-4, "weights sum to {sum} at {d} m");
                d += 0.37;
            }
        }
    }

    #[test]
    fn the_share_falls_slower_past_the_knee() {
        let table = plan_bands(&FoliageRange::default());
        let knee = table.keep_knee;
        // Continuous at the knee.
        assert!((keep(&table, knee - 1.0e-3) - keep(&table, knee + 1.0e-3)).abs() < 1.0e-3);
        // Past it the share halves over a longer distance than before it.
        let steep = (table.keep_distance / (2.0 * table.keep_distance)).powf(table.keep_power);
        let slow = keep(&table, 2.0 * knee) / keep(&table, knee);
        assert!(slow > steep, "{slow} past the knee, {steep} before");
        // The middle distance keeps a cover: about a tenth of the tufts at 100 m (a
        // twentieth with one power to the range).
        assert!(keep(&table, 100.0) > 0.09, "{}", keep(&table, 100.0));
    }

    #[test]
    fn the_kept_share_falls_and_tufts_left_grow_a_little() {
        let table = plan_bands(&FoliageRange::default());
        assert_eq!(keep(&table, 5.0), 1.0);
        assert!(keep(&table, 100.0) < keep(&table, 50.0));
        assert_eq!(growth(&table, 5.0), 1.0);
        assert!(growth(&table, 150.0) > 1.0 && growth(&table, 150.0) <= table.growth_max);
        // Every band holds the density its nearest drawn point needs.
        for b in table.bands() {
            let nearest = (b.inner - b.fade_in).max(1.0e-3);
            assert!(
                b.share + 1.0e-4 >= keep(&table, nearest).min(b.share),
                "band at {} m holds {}",
                b.inner,
                b.share
            );
        }
        // A tuft drawn in its band at full scale near, gone once its rank is past the share
        // (in the bands holding more than their middle keeps; the farthest may hold less,
        // its grid full).
        let mut thinned = 0;
        for b in table.bands() {
            let d = (b.inner + b.outer) * 0.5;
            assert!(tuft_scale(&table, b, d, 0.0) > 0.0);
            if keep(&table, d) / b.share < 0.9 {
                assert_eq!(tuft_scale(&table, b, d, 0.999), 0.0);
                thinned += 1;
            }
        }
        assert!(thinned >= 2, "{thinned} bands thin out");
    }
}
