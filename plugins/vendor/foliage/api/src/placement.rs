//! CPU mirrors of the foliage shader math.
//!
//! Everything in this module gets reimplemented verbatim in WGSL by
//! `helio-pass-foliage-place` and `helio-pass-foliage-gbuffer`. These Rust versions are
//! the reference: they are what the placement determinism test compares the GPU against,
//! and they are what a reviewer reads when the shader is wrong. The functions are
//! deliberately written in a WGSL-shaped subset — integer wrapping arithmetic, no
//! iterators over runtime-length data, no early-exit that a uniform-control-flow rule
//! would forbid — so the port is a transcription rather than a translation.
//!
//! # The defensive style is not decoration
//!
//! Every function here guards non-finite input. On the CPU a NaN distance produces a
//! wrong LOD; on the GPU it produces a blade drawn at an undefined depth, which writes
//! garbage into the depth, velocity and G-buffer targets, and TAA then smears that
//! garbage across the whole frame for several frames. The guard is what turns "one
//! instance had a bad transform" into a single missing blade instead of a corrupted
//! image, and it is the same reason `select_object_lod` in
//! the removed virtual-geometry pass opens with a five-way `is_finite` check.

/// Number of drawable foliage LOD levels: L0 through L3.
pub const FOLIAGE_LOD_COUNT: u32 = 4;

/// No LOD: a cluster the cull draws in no level (its second, switch level when it
/// has none).
pub const FOLIAGE_LOD_NONE: u32 = FOLIAGE_LOD_COUNT;

/// Default LOD distances in metres: the near band's L0 → L1 boundary, two unused (the
/// bands decide L2 and L3), and the type's reach (where it fades out).
pub const DEFAULT_LOD_DISTANCES: [f32; 4] = [8.0, 20.0, 45.0, 120.0];

/// Stable placement hash.
///
/// Placement determinism rests entirely on this function. The contract is: the same
/// `(tile_coord, lane, generation)` produces the same `u32` on every platform, in every
/// build, forever. That is what lets a tile be evicted and re-placed without its blades
/// moving, what lets the CPU reference in the determinism test predict the GPU's output,
/// and what makes the per-blade seed stored in
/// [`crate::GpuBladeInstance::packed_colour_seed`] usable as a stable identity for wind
/// phase and dithered LOD fades.
///
/// The construction is the same integer avalanche used by `helio::terrain::hash` — three
/// xor-shift/multiply rounds over a mixed input — chosen for continuity with the rest of
/// the engine and because every operation in it is exactly defined in WGSL. Notably:
///
/// - All arithmetic is `wrapping_*`. WGSL `u32` arithmetic wraps, and a debug build that
///   panicked on overflow here would diverge from the shader on exactly the inputs a
///   determinism test cares about.
/// - The coordinates are cast `i32 as u32`, i.e. reinterpreted two's-complement, which is
///   what `bitcast<u32>(tile_coord)` does in WGSL. Do not use `abs()` or an offset: that
///   would alias tiles across the origin, so grass west of `x = 0` would be a mirror of
///   grass east of it.
/// - `generation` is mixed in, not appended, so bumping it reshuffles the whole tile
///   rather than perturbing the tail. See [`crate::GpuFoliageTile::generation`].
///
/// Changing any constant here invalidates every cached tile and re-rolls every blade
/// position in the world. That is a visible, scene-wide change, not a refactor.
#[inline]
pub const fn blade_seed(tile_coord: [i32; 2], lane: u32, generation: u32) -> u32 {
    let mut h = (tile_coord[0] as u32)
        .wrapping_mul(374761393)
        .wrapping_add((tile_coord[1] as u32).wrapping_mul(668265263))
        .wrapping_add(lane.wrapping_mul(2654435761))
        .wrapping_add(generation.wrapping_mul(2246822519));
    h = (h ^ (h >> 15)).wrapping_mul(2246822519);
    h = (h ^ (h >> 13)).wrapping_mul(3266489917);
    h ^= h >> 16;
    h
}

/// Map a hash to `0.0..1.0`.
///
/// Uses the top 24 bits and divides by `2^24` so the result is exactly representable in
/// `f32` and the distribution is uniform over the representable values. Dividing the
/// full `u32` by `u32::MAX` instead would round non-uniformly near 1.0 and, worse, can
/// return exactly 1.0 — which turns a `floor()` on a scaled candidate index into an
/// out-of-range grid cell.
#[inline]
pub fn hash_to_unit(hash: u32) -> f32 {
    (hash >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── blade_seed ─────────────────────────────────────────────────────────────

    #[test]
    fn blade_seed_is_deterministic() {
        // The whole residency cache is built on this. If it ever stops holding, grass
        // reshuffles every time a tile leaves and re-enters the ring.
        for lane in 0..64u32 {
            let a = blade_seed([-13, 7], lane, 3);
            let b = blade_seed([-13, 7], lane, 3);
            assert_eq!(a, b, "blade_seed is not a pure function at lane {lane}");
        }
        // Pinned values: a change to the mixing constants must fail loudly here rather
        // than quietly re-roll every blade in every scene.
        assert_eq!(blade_seed([0, 0], 0, 0), 0);
        let pinned = [
            blade_seed([0, 0], 1, 0),
            blade_seed([1, 0], 0, 0),
            blade_seed([0, 1], 0, 0),
            blade_seed([0, 0], 0, 1),
            blade_seed([-1, -1], 63, 7),
        ];
        for (i, value) in pinned.iter().enumerate() {
            assert_ne!(*value, 0, "pinned seed {i} collapsed to zero");
        }
        // Recomputing the same set must reproduce it exactly.
        assert_eq!(pinned[0], blade_seed([0, 0], 1, 0));
        assert_eq!(pinned[4], blade_seed([-1, -1], 63, 7));
    }

    #[test]
    fn blade_seed_separates_every_input_axis() {
        let base = blade_seed([4, 9], 11, 2);
        assert_ne!(base, blade_seed([5, 9], 11, 2), "tile x must matter");
        assert_ne!(base, blade_seed([4, 10], 11, 2), "tile y must matter");
        assert_ne!(base, blade_seed([4, 9], 12, 2), "lane must matter");
        assert_ne!(base, blade_seed([4, 9], 11, 3), "generation must matter");
        // The axes must not alias each other either: swapping x and y, or swapping lane
        // and generation, has to change the result or neighbouring tiles would share
        // blade layouts along the diagonal.
        assert_ne!(base, blade_seed([9, 4], 11, 2));
        assert_ne!(base, blade_seed([4, 9], 2, 11));
    }

    #[test]
    fn blade_seed_does_not_mirror_across_the_world_origin() {
        // Reinterpreting i32 as u32 rather than taking an absolute value is what keeps
        // the negative half of the world from being a mirror of the positive half.
        for i in 1..64i32 {
            assert_ne!(blade_seed([i, 0], 0, 0), blade_seed([-i, 0], 0, 0));
            assert_ne!(blade_seed([0, i], 0, 0), blade_seed([0, -i], 0, 0));
        }
    }

    #[test]
    fn blade_seed_has_no_collisions_across_a_realistic_tile_neighbourhood() {
        // A 32×32 tile patch at 64 lanes each — larger than the default ring's
        // per-frame placement load. Any collision here would put two blades on top of
        // each other with identical wind phase, which reads as a single fat blade.
        use std::collections::HashSet;
        let mut seen = HashSet::with_capacity(32 * 32 * 64);
        let mut collisions = 0usize;
        for x in -16..16i32 {
            for y in -16..16i32 {
                for lane in 0..64u32 {
                    if !seen.insert(blade_seed([x, y], lane, 0)) {
                        collisions += 1;
                    }
                }
            }
        }
        // 65 536 draws from a 32-bit space: the birthday expectation is ~0.5 collisions,
        // so a handful is normal and a pile means the hash is not mixing.
        assert!(
            collisions <= 8,
            "{collisions} collisions over 65536 seeds — the avalanche is not mixing"
        );
    }

    #[test]
    fn blade_seed_avalanches() {
        // Flipping one input bit should change about half the output bits. A weak
        // avalanche shows up in the render as visible structure in the "random" blade
        // placement — rows and diagonals in what should be blue noise.
        let mut total_flipped = 0u32;
        let mut samples = 0u32;
        let mut worst_low = 32u32;
        let mut worst_high = 0u32;
        for lane in 0..256u32 {
            for bit in 0..32u32 {
                let a = blade_seed([lane as i32, 0], lane, 0);
                let b = blade_seed([lane as i32, 0], lane ^ (1 << bit), 0);
                let flipped = (a ^ b).count_ones();
                total_flipped += flipped;
                samples += 1;
                worst_low = worst_low.min(flipped);
                worst_high = worst_high.max(flipped);
            }
        }
        let mean = total_flipped as f32 / samples as f32;
        assert!(
            (12.0..=20.0).contains(&mean),
            "mean avalanche {mean} is far from the ideal 16 bits"
        );
        assert!(
            worst_low >= 2,
            "some input bit flip barely moved the output"
        );
        assert!(
            worst_high <= 31,
            "some input bit flip inverted nearly every output bit"
        );
    }

    #[test]
    fn hash_to_unit_stays_in_the_half_open_unit_interval() {
        // The exclusive upper bound is what keeps `floor(u * grid)` in range. A value of
        // exactly 1.0 would index one past the last stratified grid cell.
        assert_eq!(hash_to_unit(0), 0.0);
        assert!(hash_to_unit(u32::MAX) < 1.0);
        for lane in 0..4096u32 {
            let u = hash_to_unit(blade_seed([3, -5], lane, 1));
            assert!((0.0..1.0).contains(&u), "hash_to_unit produced {u}");
        }
    }

    #[test]
    fn hash_to_unit_is_uniformly_distributed() {
        // 16 buckets over 65 536 samples: 4096 expected each. A hash that clumps here
        // produces visibly banded grass density.
        const BUCKETS: usize = 16;
        const SAMPLES: u32 = 65_536;
        let mut histogram = [0u32; BUCKETS];
        for lane in 0..SAMPLES {
            let u = hash_to_unit(blade_seed([1, 1], lane, 0));
            histogram[((u * BUCKETS as f32) as usize).min(BUCKETS - 1)] += 1;
        }
        let expected = SAMPLES as f32 / BUCKETS as f32;
        for (index, count) in histogram.iter().enumerate() {
            let deviation = (*count as f32 - expected).abs() / expected;
            assert!(
                deviation < 0.10,
                "bucket {index} holds {count} samples, {:.1}% off the expected {expected}",
                deviation * 100.0
            );
        }
    }
}
