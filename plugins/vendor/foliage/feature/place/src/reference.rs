//! CPU reference implementation of `cs_place`.
//!
//! A line-for-line transcription of the placement shader's candidate loop, so the
//! determinism contract and the placement rules (tufts, types by ground, densities and
//! patches, bands, cube grids) are enforced by tests that run in a headless container.
//!
//! # What "deterministic" has to mean here
//!
//! The same `(band, tile_coord, generation)` must produce a **byte-identical** record list
//! on any GPU. That is stronger than "the same set of tufts survives", and it is the
//! reason the shader compacts with a workgroup prefix sum instead of `atomicAdd`: with the
//! scan, a record's slab index is a pure function of its candidate index, and this
//! function can predict it.
//!
//! # Scope
//!
//! Models a flat tiling of world XZ under +Y (the built-in plane's chart): a candidate's
//! ground comes from the `ground` function the caller passes (the answer `cs_place` reads
//! from a hit).

use crate::{
    blade_seed, hash_to_unit, pack_blade, BladeParams, FoliageKind, GpuBladeInstance,
    GpuFoliageBand, GpuFoliageLayer, GpuFoliageType, BAND_LOD_BLADES,
};

use crate::uniforms::PlaceUniforms;

/// One candidate, before the accept/reject decision is applied.
///
/// Exposed so a failing determinism test can point at *which* candidate diverged rather
/// than only reporting that two record lists differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceCandidate {
    /// Index into the tile's candidates. This, not the output slab index, is what the
    /// seed is derived from.
    pub index: u32,
    /// [`blade_seed`] for this candidate.
    pub seed: u32,
    /// Tile-local position in `0.0..1.0` (`-1` where a cube grid gives the cell none).
    pub tile_uv: [f32; 2],
    /// Foliage type this candidate drew.
    pub type_id: u8,
    /// Blades its tuft holds.
    pub blades: u8,
    /// Acceptance probability.
    pub weight: f32,
    /// Whether the candidate became a tuft.
    pub accepted: bool,
}

/// Result of placing one tile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReferencePlacement {
    /// The records, in candidate order (a tuft's blades in order) — the exact byte layout
    /// the shader writes into the tile's slab.
    pub blades: Vec<GpuBladeInstance>,
    /// Candidates evaluated, i.e. `candidate_grid²`.
    pub candidates: u32,
    /// Tufts accepted.
    pub tufts: u32,
    /// Records that did not fit the slab. Mirrors [`crate::COUNTER_PLACEMENT_OVERFLOW`].
    pub dropped: u32,
}

/// Whether a world position lies inside at least one authored layer (`within_any_layer`
/// in the shader): an empty table carpets everything, an infinite-extent layer accepts
/// everything, otherwise the point must sit inside some layer's AABB.
pub fn within_any_layer(world: [f32; 3], layers: &[GpuFoliageLayer]) -> bool {
    if layers.is_empty() {
        return true;
    }
    layers.iter().any(|layer| {
        layer.bounds_max[3] > 0.5
            || (0..3).all(|a| world[a] >= layer.bounds_min[a] && world[a] <= layer.bounds_max[a])
    })
}

/// The ground a reference candidate stands on: its height along the tile's up, the
/// cosine of its slope, its ground id and its colour (RGBA8, 0: none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceGround {
    pub height: f32,
    pub slope_cos: f32,
    pub ground: u32,
    pub colour: u32,
}

/// The built-in plane: height 0, flat, ground id 0, no colour, everywhere.
pub fn flat_ground(_uv: [f32; 2]) -> Option<ReferenceGround> {
    Some(ReferenceGround {
        height: 0.0,
        slope_cos: 1.0,
        ground: 0,
        colour: 0,
    })
}

/// Whether type `foliage` grows on `ground` (`type_grows` in the shader).
pub fn type_grows(foliage: &GpuFoliageType, ground: &ReferenceGround) -> bool {
    let on_ground = foliage.ground_mask == 0
        || (ground.ground < 32 && (foliage.ground_mask >> ground.ground) & 1 != 0);
    foliage.density > 0.0
        && on_ground
        && ground.slope_cos.clamp(-1.0, 1.0) >= foliage.slope_range[0]
        && ground.slope_cos.clamp(-1.0, 1.0) <= foliage.slope_range[1]
        && ground.height >= foliage.altitude_range[0]
        && ground.height <= foliage.altitude_range[1]
}

/// Cubes across an `edge`-metre tile that blocky types stand on (the smallest cube of the
/// [`FoliageKind::Block`] types with a density), or 0 without any (`cube_cells`).
pub fn cube_cells(types: &[GpuFoliageType], edge: f32) -> u32 {
    let cube = types
        .iter()
        .filter(|t| {
            t.kind() == Some(FoliageKind::Block) && t.density > 0.0 && t.width_range[0] > 0.0
        })
        .map(|t| t.width_range[0])
        .fold(0.0_f32, |a, w| if a == 0.0 { w } else { a.min(w) });
    if cube <= 0.0 {
        0
    } else {
        (edge / cube).round().max(1.0) as u32
    }
}

/// The cube of `cubes` across that grid cell `cell` of `grid` stands on: one of those
/// whose centres fall in the cell, by `pick` in 0..1, or `None` when no centre does
/// (`owned_cube`).
pub fn owned_cube(cell: u32, grid: u32, cubes: u32, pick: f32) -> Option<u32> {
    // Cube k's centre (k + 1/2) / cubes is in cell floor((2k + 1) grid / (2 cubes)).
    let (g, n2, i) = (grid as i64, 2 * cubes as i64, cell as i64);
    let a = (i * n2 + g - 1).div_euclid(g);
    let b = ((i + 1) * n2 + g - 1).div_euclid(g);
    let (lo, hi) = (a.div_euclid(2), (b - 2).div_euclid(2));
    (hi >= lo).then(|| (lo + (pick * (hi - lo + 1) as f32) as i64).min(hi) as u32)
}

/// Candidate `candidate`'s cell in a tile of `band` (`candidate_cell`): probe block first,
/// then cluster block (`cluster_edge` across) within it, then row.
pub fn candidate_cell(candidate: u32, band: &GpuFoliageBand, cluster_edge: u32) -> [u32; 2] {
    let grid = band.candidate_grid.max(1);
    let blocks = band.probe_blocks.max(1);
    let per_side = (grid / blocks).max(1);
    let per_probe = per_side * per_side;
    let (probe, within) = (candidate / per_probe, candidate % per_probe);
    let edge = cluster_edge.max(1);
    let clusters_across = (per_side / edge).max(1);
    let (cluster, inside) = (within / (edge * edge), within % (edge * edge));
    [
        (probe % blocks) * per_side + (cluster % clusters_across) * edge + inside % edge,
        (probe / blocks) * per_side + (cluster / clusters_across) * edge + inside / edge,
    ]
}

/// The PCG output permutation (`foliage_hash`).
pub fn foliage_hash(x: u32) -> u32 {
    let state = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

fn lattice(i: [i32; 2], salt: u32) -> f32 {
    hash_to_unit(foliage_hash(
        (i[0] as u32).wrapping_mul(73_856_093) ^ (i[1] as u32).wrapping_mul(19_349_663) ^ salt,
    ))
}

/// Value noise in `[0, 1]` over the chart (`chart_noise`): the point `uv` of the band
/// tile `coord` (`span` base tiles across), lattice cells `2^e` base tiles across, the
/// lattice index in integer arithmetic.
pub fn chart_noise(coord: [i32; 2], uv: [f32; 2], span: u32, e: i32, salt: u32) -> f32 {
    let base = coord.map(|c| c.wrapping_mul(span as i32));
    let local = uv.map(|u| u * span as f32);
    let (mut i, mut f) = ([0i32; 2], [0f32; 2]);
    for a in 0..2 {
        if e <= 0 {
            let at = local[a] * (1u32 << (-e) as u32) as f32;
            i[a] = base[a]
                .wrapping_mul(1i32 << (-e))
                .wrapping_add(at.floor() as i32);
            f[a] = at - at.floor();
        } else {
            let mask = (1i32 << e) - 1;
            let at = ((base[a] & mask) as f32 + local[a]) / (1u32 << e as u32) as f32;
            i[a] = (base[a] >> e).wrapping_add(at.floor() as i32);
            f[a] = at - at.floor();
        }
    }
    let s = f.map(|f| f * f * (3.0 - 2.0 * f));
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let a = mix(lattice(i, salt), lattice([i[0] + 1, i[1]], salt), s[0]);
    let b = mix(
        lattice([i[0], i[1] + 1], salt),
        lattice([i[0] + 1, i[1] + 1], salt),
        s[0],
    );
    mix(a, b, s[1])
}

/// Type `t`'s patch weight at a candidate (`patch_weight`): `0..1`, bare where its noise
/// is low, by its strength.
pub fn patch_weight(ty: &GpuFoliageType, t: u32, coord: [i32; 2], uv: [f32; 2], span: u32) -> f32 {
    if ty.patch_scale <= 0.0 || ty.patch_strength <= 0.0 {
        return 1.0;
    }
    let e = ((ty.patch_scale / 8.0).log2().round() as i32).clamp(-8, 16);
    let salt = t.wrapping_mul(0x27d4_eb2f).wrapping_add(0x1656_67b1);
    let n = 0.7 * chart_noise(coord, uv, span, e, salt)
        + 0.3 * chart_noise(coord, uv, span, e - 2, salt ^ 0x9e37_79b9);
    let x = ((n - 0.38) / 0.24).clamp(0.0, 1.0);
    let f = x * x * (3.0 - 2.0 * x);
    let strength = ty.patch_strength.clamp(0.0, 1.0);
    (1.0 - strength) + strength * f
}

/// The blades a tuft of `ty` holds, by `pick` in 0..1 (`tuft_blades`).
pub fn tuft_blades(ty: &GpuFoliageType, pick: f32) -> u32 {
    let lo = (ty.blades_per_tuft & 0xffff).clamp(1, 15);
    let hi = (ty.blades_per_tuft >> 16).clamp(lo, 15);
    (lo + (pick * (hi - lo + 1) as f32) as u32).min(hi)
}

/// The seed generation of band `band`'s tiles (`band_generation`).
pub fn band_generation(generation: u32, band: u32) -> u32 {
    generation ^ band.wrapping_mul(0x9e37_79b9)
}

/// The world point of `uv` at height `h` on band tile `coord` of a flat tiling.
fn flat_point(band: &GpuFoliageBand, coord: [i32; 2], uv: [f32; 2], h: f32) -> [f32; 3] {
    let size = band.tile_size;
    [
        coord[0] as f32 * size + uv[0] * size,
        h,
        coord[1] as f32 * size + uv[1] * size,
    ]
}

/// Evaluate candidate `index` of band `band_index`'s tile `coord` over `ground` (a
/// tile-local `(u, v)` to its answer, `None` where there is no ground).
#[allow(clippy::too_many_arguments)]
pub fn reference_candidate(
    uniforms: &PlaceUniforms,
    band: &GpuFoliageBand,
    band_index: u32,
    types: &[GpuFoliageType],
    layers: &[GpuFoliageLayer],
    coord: [i32; 2],
    generation: u32,
    index: u32,
    ground: &dyn Fn([f32; 2]) -> Option<ReferenceGround>,
) -> (ReferenceCandidate, Option<ReferenceGround>) {
    let grid = band.candidate_grid.max(1);
    let seed = blade_seed(coord, index, band_generation(generation, band_index));
    let cell = candidate_cell(index, band, uniforms.cluster_edge);
    // On the stored unorm16 grid, so the ground answered is the ground under the tuft;
    // with blocky types, on the centre of a cube the cell owns.
    let snap = |x: f32| {
        helio_foliage_api::packing::unpack_unorm16(helio_foliage_api::packing::pack_unorm16(x))
    };
    let pick = [hash_to_unit(seed), hash_to_unit(seed.rotate_left(11))];
    let cubes = cube_cells(types, band.tile_size);
    let mut candidate = ReferenceCandidate {
        index,
        seed,
        tile_uv: [-1.0; 2],
        type_id: 0,
        blades: 0,
        weight: 0.0,
        accepted: false,
    };
    if cubes > 0 {
        let (Some(ku), Some(kv)) = (
            owned_cube(cell[0], grid, cubes, pick[0]),
            owned_cube(cell[1], grid, cubes, pick[1]),
        ) else {
            return (candidate, None);
        };
        candidate.tile_uv = [
            snap((ku as f32 + 0.5) / cubes as f32),
            snap((kv as f32 + 0.5) / cubes as f32),
        ];
    } else {
        candidate.tile_uv = [
            snap((cell[0] as f32 + pick[0]) / grid as f32),
            snap((cell[1] as f32 + pick[1]) / grid as f32),
        ];
    }
    let uv = candidate.tile_uv;
    let Some(g) = ground(uv) else {
        return (candidate, None);
    };
    if !within_any_layer(flat_point(band, coord, uv, g.height), layers) {
        return (candidate, Some(g));
    }
    // The types growing here by their densities and patches: a tuft with probability
    // `sum · share / grid_density`, of each type by its share of the sum.
    let live = types
        .iter()
        .rposition(|t| t.density > 0.0)
        .map_or(0, |n| n + 1)
        .min(uniforms.type_count as usize);
    let weight =
        |t: usize| types[t].density * patch_weight(&types[t], t as u32, coord, uv, band.tile_span);
    let growing: Vec<usize> = (0..live).filter(|&t| type_grows(&types[t], &g)).collect();
    let sum: f32 = growing.iter().map(|&t| weight(t)).sum();
    let wanted = sum * uniforms.density_multiplier * band.share;
    candidate.weight = (wanted / band.grid_density.max(1.0e-6)).clamp(0.0, 1.0);
    let roll = hash_to_unit(seed.rotate_left(17)) * band.grid_density.max(1.0e-6);
    if roll < wanted {
        let pick = hash_to_unit(seed.rotate_left(5)) * sum;
        let mut running = 0.0;
        for &t in &growing {
            candidate.type_id = t as u8;
            running += weight(t);
            if pick < running {
                break;
            }
        }
        candidate.blades = tuft_blades(
            &types[candidate.type_id as usize],
            hash_to_unit(seed.rotate_left(3)),
        ) as u8;
        candidate.accepted = true;
    }
    (candidate, Some(g))
}

/// Place band `band_index`'s tile `coord` over `ground`, returning the records the
/// placement shader must reproduce byte-for-byte.
///
/// Heights are stored as f16 offsets from the centre of the tile's ground range (every
/// candidate's ground, accepted or not), so the reference takes the same centre.
#[allow(clippy::too_many_arguments)]
pub fn place_tile_reference(
    uniforms: &PlaceUniforms,
    band: &GpuFoliageBand,
    band_index: u32,
    types: &[GpuFoliageType],
    layers: &[GpuFoliageLayer],
    coord: [i32; 2],
    generation: u32,
    ground: &dyn Fn([f32; 2]) -> Option<ReferenceGround>,
) -> ReferencePlacement {
    let grid = band.candidate_grid.max(1);
    let candidates = grid * grid;
    let mut placement = ReferencePlacement {
        candidates,
        ..Default::default()
    };
    let evaluated: Vec<_> = (0..candidates)
        .map(|index| {
            reference_candidate(
                uniforms, band, band_index, types, layers, coord, generation, index, ground,
            )
        })
        .collect();
    let heights = evaluated.iter().filter_map(|(_, g)| g.map(|g| g.height));
    let (low, high) = heights.fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), h| {
        (a.min(h), b.max(h))
    });
    let center = if low <= high { (low + high) * 0.5 } else { 0.0 };

    for (candidate, g) in evaluated {
        if !candidate.accepted {
            continue;
        }
        placement.tufts += 1;
        let seed = candidate.seed;
        let colour = g.map_or(0, |g| g.colour);
        let tuft = BladeParams {
            tile_uv: candidate.tile_uv,
            height_offset: g.map_or(0.0, |g| g.height) - center,
            blade: 0,
            blades: candidate.blades,
            rank: hash_to_unit(seed.rotate_left(23)),
            height_scale: hash_to_unit(seed.rotate_left(29)),
            aux: ((seed >> 9) & 0xff) as u8,
            type_id: candidate.type_id,
            variant: ((seed >> 30) & 3) as u8,
            colour: (colour != 0)
                .then(|| [0, 8, 16].map(|s| ((colour >> s) & 0xff) as f32 / 255.0)),
            seed: seed as u16,
        };
        let records = if band.lod == BAND_LOD_BLADES {
            candidate.blades
        } else {
            1
        };
        for blade in 0..records {
            if placement.blades.len() as u32 >= band.slab {
                placement.dropped += 1;
                continue;
            }
            placement
                .blades
                .push(pack_blade(BladeParams { blade, ..tuft }));
        }
    }
    placement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plan_bands, unpack_blade, FoliageRange, GpuFoliageBands, BAND_LOD_TUFT};

    fn bands() -> GpuFoliageBands {
        plan_bands(&FoliageRange {
            range: 160.0,
            tufts_per_m2: 6.0,
            ..FoliageRange::default()
        })
    }

    fn uniforms(types: &[GpuFoliageType]) -> PlaceUniforms {
        PlaceUniforms {
            cluster_edge: 4,
            density_multiplier: 1.0,
            type_count: types.len() as u32,
            ..PlaceUniforms::default()
        }
    }

    fn grass(density: f32, blades: [u32; 2]) -> GpuFoliageType {
        let mut t = GpuFoliageType {
            density,
            slope_range: [-1.0, 1.0],
            ..GpuFoliageType::default()
        };
        t.set_blades_per_tuft(blades);
        t
    }

    /// Flat ground whose normal and tile up are a hair apart dots to just over 1: a type
    /// growing on any slope grows there (whole tiles of the Earth meadow grew nothing).
    #[test]
    fn flat_ground_a_rounding_over_one_still_grows() {
        let ground = ReferenceGround {
            height: 0.0,
            slope_cos: 1.0 + 4.0 * f32::EPSILON,
            ground: 0,
            colour: 0,
        };
        assert!(type_grows(&grass(1.0, [1, 1]), &ground));
    }

    fn place(
        types: &[GpuFoliageType],
        band: usize,
        coord: [i32; 2],
        generation: u32,
    ) -> ReferencePlacement {
        let table = bands();
        place_tile_reference(
            &uniforms(types),
            &table.bands[band],
            band as u32,
            types,
            &[],
            coord,
            generation,
            &flat_ground,
        )
    }

    #[test]
    fn placing_the_same_tile_twice_is_byte_identical() {
        let types = [grass(6.0, [5, 12])];
        let first = place(&types, 0, [4, -9], 7);
        assert!(!first.blades.is_empty());
        assert_eq!(first, place(&types, 0, [4, -9], 7));
        // A generation bump and a neighbour are independent draws.
        let shared = |other: &ReferencePlacement| {
            first
                .blades
                .iter()
                .zip(&other.blades)
                .filter(|(a, b)| a.packed_pos == b.packed_pos)
                .count()
        };
        assert!(shared(&place(&types, 0, [4, -9], 8)) * 20 < first.blades.len());
        assert!(shared(&place(&types, 0, [5, -9], 7)) * 20 < first.blades.len());
    }

    #[test]
    fn the_near_band_writes_a_record_per_blade_and_far_bands_one_per_tuft() {
        let types = [grass(6.0, [5, 12])];
        let near = place(&types, 0, [0, 0], 1);
        let mut blades_seen = 0;
        let mut index = 0;
        while index < near.blades.len() {
            let tuft = unpack_blade(&near.blades[index]);
            assert!((5..=12).contains(&tuft.blades), "{} blades", tuft.blades);
            // A tuft's blades are consecutive, numbered, and alike but for their index.
            for j in 0..tuft.blades as usize {
                let blade = unpack_blade(&near.blades[index + j]);
                assert_eq!(blade.blade as usize, j);
                assert_eq!(
                    BladeParams { blade: 0, ..blade },
                    BladeParams { blade: 0, ..tuft }
                );
            }
            blades_seen += tuft.blades as usize;
            index += tuft.blades as usize;
        }
        assert_eq!(blades_seen, near.blades.len());

        let table = bands();
        let far_band = (0..table.count as usize)
            .find(|&b| table.bands[b].lod == BAND_LOD_TUFT)
            .unwrap();
        let far = place(&types, far_band, [0, 0], 1);
        assert_eq!(far.blades.len(), far.tufts as usize);
        assert!(far.blades.iter().all(|r| unpack_blade(r).blade == 0));
    }

    #[test]
    fn tufts_follow_the_density_times_the_bands_share() {
        let types = [grass(6.0, [1, 1])];
        let table = bands();
        for band in 0..table.count as usize {
            let b = &table.bands[band];
            let tiles = 4;
            let tufts: u32 = (0..tiles)
                .map(|i| place(&types, band, [i, 3], 2).tufts)
                .sum();
            let per_m2 = tufts as f32 / (tiles as f32 * b.tile_size * b.tile_size);
            let expected = 6.0 * b.share;
            assert!(
                (per_m2 - expected).abs() < 0.15 * expected + 0.02,
                "band {band}: {per_m2} tufts/m² for {expected}"
            );
        }
    }

    #[test]
    fn stratification_spreads_tufts_over_the_whole_tile() {
        // A rejection sampler without the stratified grid clumps, and clumps read as bald
        // patches: every 2 m square of a dense tile holds tufts.
        let types = [grass(8.0, [1, 1])];
        let tile = place(&types, 0, [0, 0], 3);
        let mut cells = [[0u32; 4]; 4];
        for record in &tile.blades {
            let uv = unpack_blade(record).tile_uv;
            cells[(uv[0] * 4.0).min(3.0) as usize][(uv[1] * 4.0).min(3.0) as usize] += 1;
        }
        assert!(cells.iter().flatten().all(|&n| n > 0), "{cells:?}");
    }

    #[test]
    fn candidates_are_ordered_by_probe_block_then_cluster() {
        let table = bands();
        for b in table.bands() {
            let grid = b.candidate_grid;
            let mut seen = std::collections::HashSet::new();
            let per_block = (grid / b.probe_blocks).pow(2);
            for c in 0..grid * grid {
                let cell = candidate_cell(c, b, 4);
                assert!(cell[0] < grid && cell[1] < grid);
                assert!(seen.insert(cell), "cell {cell:?} twice");
                // Every candidate of a probe block lies in that block.
                let block = c / per_block;
                let side = grid / b.probe_blocks;
                assert_eq!(
                    [cell[0] / side, cell[1] / side],
                    [block % b.probe_blocks, block / b.probe_blocks]
                );
            }
        }
    }

    #[test]
    fn types_grow_only_on_their_ground_and_by_their_densities() {
        let mut a = grass(4.0, [3, 3]);
        a.ground_mask = 1 << 1;
        let mut b = grass(2.0, [3, 3]);
        b.ground_mask = 1 << 4;
        let types = [a, b];
        let table = bands();
        let ground = |uv: [f32; 2]| {
            let id = if uv[1] > 0.75 {
                7
            } else if uv[0] < 0.5 {
                1
            } else {
                4
            };
            Some(ReferenceGround {
                height: 0.0,
                slope_cos: 1.0,
                ground: id,
                colour: 0,
            })
        };
        let mut counts = [0usize; 2];
        for tile in 0..6 {
            let p = place_tile_reference(
                &uniforms(&types),
                &table.bands[0],
                0,
                &types,
                &[],
                [tile, 1],
                0,
                &ground,
            );
            for record in p.blades {
                let r = unpack_blade(&record);
                assert!(
                    r.tile_uv[1] <= 0.75 + 1e-4,
                    "a tuft on ground 7 at {:?}",
                    r.tile_uv
                );
                assert_eq!(r.type_id, if r.tile_uv[0] < 0.5 { 0 } else { 1 });
                if r.blade == 0 {
                    counts[r.type_id as usize] += 1;
                }
            }
        }
        let area = 6.0 * 64.0 * 0.375;
        let (a, b) = (counts[0] as f32 / area, counts[1] as f32 / area);
        assert!(
            (a - 4.0).abs() < 0.6 && (b - 2.0).abs() < 0.4,
            "{a} and {b} tufts/m²"
        );
    }

    #[test]
    fn records_stand_on_the_answered_ground_and_carry_its_colour() {
        let types = [grass(6.0, [2, 2])];
        let table = bands();
        let ground = |uv: [f32; 2]| {
            Some(ReferenceGround {
                height: 2.0 + uv[0],
                slope_cos: 1.0,
                ground: 0,
                colour: 0xff_20_80_40,
            })
        };
        let p = place_tile_reference(
            &uniforms(&types),
            &table.bands[0],
            0,
            &types,
            &[],
            [0, 0],
            0,
            &ground,
        );
        assert!(!p.blades.is_empty());
        for record in &p.blades {
            let r = unpack_blade(record);
            // The centre is the middle of the tile's ground range: 2.0..3.0.
            assert!(
                (r.height_offset + 2.5 - (2.0 + r.tile_uv[0])).abs() < 2e-3,
                "{r:?}"
            );
            let c = r.colour.expect("the ground's colour");
            assert!(
                (c[0] - 0x40 as f32 / 255.0).abs() < 0.04
                    && (c[1] - 0x80 as f32 / 255.0).abs() < 0.02,
                "{c:?}"
            );
        }
    }

    #[test]
    fn patches_leave_bare_ground_only_where_they_are_asked_to() {
        let mut patchy = grass(8.0, [1, 1]);
        patchy.patch_scale = 16.0;
        patchy.patch_strength = 1.0;
        let uniform = grass(8.0, [1, 1]);
        // Count 2 m squares without a tuft over 8 x 8 tiles.
        let bare = |t: &GpuFoliageType| {
            let mut empty = 0;
            for i in 0..8 {
                for j in 0..8 {
                    let mut cells = [false; 16];
                    for record in place(std::slice::from_ref(t), 0, [i, j], 5).blades {
                        let uv = unpack_blade(&record).tile_uv;
                        cells[(uv[0] * 4.0).min(3.0) as usize * 4
                            + (uv[1] * 4.0).min(3.0) as usize] = true;
                    }
                    empty += cells.iter().filter(|c| !**c).count();
                }
            }
            empty
        };
        assert_eq!(bare(&uniform), 0);
        let empty = bare(&patchy);
        assert!(
            empty > 64 * 16 / 6 && empty < 64 * 16 * 5 / 6,
            "{empty} of 1024 squares bare"
        );
    }

    #[test]
    fn chart_noise_runs_on_across_tiles_and_bands() {
        // The same chart point in a near tile and in a far tile spanning it reads the same
        // noise: the lattice is the chart's, not the tile's.
        for e in [-2, 0, 1, 3] {
            for (coord, uv) in [([3, -2], [0.25f32, 0.75f32]), ([-17, 40], [0.9, 0.1])] {
                let near = chart_noise(coord, uv, 1, e, 9);
                // The far tile (span 4) holding base tile `coord`.
                let far_coord = [coord[0].div_euclid(4), coord[1].div_euclid(4)];
                let far_uv = [0, 1].map(|a| ((coord[a] - far_coord[a] * 4) as f32 + uv[a]) / 4.0);
                let far = chart_noise(far_coord, far_uv, 4, e, 9);
                assert!((near - far).abs() < 1e-4, "e {e}: {near} vs {far}");
            }
        }
    }

    #[test]
    fn cubes_belong_to_one_cell_each() {
        // Every cube centre is owned by exactly one cell, whatever the two grids.
        for (grid, cubes) in [
            (80, 160),
            (80, 80),
            (80, 100),
            (96, 160),
            (100, 64),
            (64, 100),
        ] {
            let mut owner = vec![None; cubes as usize];
            for cell in 0..grid {
                for pick in [0.0, 0.3, 0.999] {
                    if let Some(k) = owned_cube(cell, grid, cubes, pick) {
                        let centre = (k as f64 + 0.5) / cubes as f64;
                        assert_eq!(
                            (centre * grid as f64).floor() as u32,
                            cell,
                            "cube {k} of {cubes} in cell {cell} of {grid}"
                        );
                        assert!(owner[k as usize].is_none_or(|c| c == cell));
                        owner[k as usize] = Some(cell);
                    }
                }
            }
        }
    }

    #[test]
    fn blocky_tufts_stand_on_cube_centres() {
        let mut t = grass(8.0, [1, 1]);
        t.kind_and_flags = crate::pack_kind_and_flags(FoliageKind::Block, 0);
        t.width_range = [0.05, 0.05];
        let types = [t];
        let table = bands();
        let cubes = cube_cells(&types, table.bands[0].tile_size);
        let mut seen = std::collections::HashSet::new();
        for record in place(&types, 0, [3, -2], 0).blades {
            let uv = unpack_blade(&record).tile_uv;
            let k = uv.map(|x| x * cubes as f32 - 0.5);
            assert!(
                k.iter().all(|k| (k - k.round()).abs() < 0.01),
                "{uv:?} is not a cube centre"
            );
            assert!(
                seen.insert(k.map(|k| k.round() as i32)),
                "two tufts on one cube"
            );
        }
    }

    #[test]
    fn a_full_slab_drops_the_tail_and_counts_it() {
        let types = [grass(8.0, [15, 15])];
        let mut table = bands();
        table.bands[0].slab = 64;
        let p = place_tile_reference(
            &uniforms(&types),
            &table.bands[0],
            0,
            &types,
            &[],
            [0, 0],
            0,
            &flat_ground,
        );
        assert_eq!(p.blades.len(), 64);
        assert_eq!(p.dropped as usize, p.tufts as usize * 15 - 64);
    }

    #[test]
    fn layers_gate_the_tufts() {
        let types = [grass(6.0, [1, 1])];
        let table = bands();
        let b = &table.bands[0];
        let far = [GpuFoliageLayer {
            bounds_min: [1000.0, -1.0, 1000.0, 0.0],
            bounds_max: [1010.0, 1.0, 1010.0, 0.0],
        }];
        assert!(place_tile_reference(
            &uniforms(&types),
            b,
            0,
            &types,
            &far,
            [0, 0],
            0,
            &flat_ground
        )
        .blades
        .is_empty());
        let infinite = [GpuFoliageLayer {
            bounds_min: [0.0; 4],
            bounds_max: [0.0, 0.0, 0.0, 1.0],
        }];
        assert!(!place_tile_reference(
            &uniforms(&types),
            b,
            0,
            &types,
            &infinite,
            [0, 0],
            0,
            &flat_ground
        )
        .blades
        .is_empty());
    }
}
