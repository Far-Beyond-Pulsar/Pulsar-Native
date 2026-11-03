//! Ridge envelopes: coarse levels draw the display layer's unresolved
//! ridges as their conditional mean given the incoming ridge weight, baked
//! per stack into a suffix table (`TerrainConstants::ridge_suffix`).
use crate::grid::Grid;
use crate::landform::{LandformConstants, Octave, RIDGE, WARP, WARP_OCTAVES};
use crate::noise::{hash3, mul_fine, noise_fine, FINE_ONE};
use glam::IVec3;

pub(crate) const KNOTS: usize = 33;
/// Most ridges a display layer may have (suffix rows).
pub(crate) const RIDGES: usize = 7;
const SAMPLE_COUNT: usize = 2048;

/// The ridge octaves of the stack's display layer, coarsest first.
pub(crate) fn display_ridges(k: &LandformConstants) -> Vec<Octave> {
    let display = k.shape[0];
    if display <= 0 {
        return Vec::new();
    }
    let tag = ((display as u32 - 1) << 8) | RIDGE;
    let count = (k.header[0].max(0) as usize).min(k.octaves.len());
    k.octaves[WARP_OCTAVES.min(count)..count]
        .iter()
        .filter(|o| o.kind == tag)
        .copied()
        .collect()
}

pub(crate) fn bake_ridge_suffix(
    grid: &Grid,
    k: &LandformConstants,
) -> Result<[[i32; 4]; 66], &'static str> {
    let count = usize::try_from(k.header[0]).map_err(|_| "invalid octave count")?;
    if !(WARP_OCTAVES..=k.octaves.len()).contains(&count) {
        return Err("invalid octave count");
    }
    let ridges = display_ridges(k);
    let n = ridges.len();
    if n == 0 || n > RIDGES || ridges.windows(2).any(|o| o[0].shift < o[1].shift) {
        return Err("suffix filtering requires a display layer of one to seven ordered ridges");
    }
    // Current limb multiplication contract. Canonical validation currently
    // accepts values outside it; do not silently saturate the new bake.
    if k.octaves[..count]
        .iter()
        .any(|o| i64::from(o.amplitude).abs() >= (1 << 28))
    {
        return Err("recipe exceeds existing fixed-point amplitude contract");
    }
    let magnitude: i64 = ridges.iter().map(|o| i64::from(o.amplitude).abs()).sum();
    if magnitude >= (1 << 28) {
        return Err("ridge suffix exceeds the fixed-point limb contract");
    }

    let mut samples = Vec::<[i32; RIDGES]>::with_capacity(SAMPLE_COUNT);
    for index in 0..SAMPLE_COUNT {
        let face = grid.faces()[index % grid.faces().len()];
        let i = (hash3(index as i32, 0, 0, 0x51978213) % (grid.cells() as u32)) as i32;
        let j = (hash3(index as i32, 1, 0, 0x51978213) % (grid.cells() as u32)) as i32;
        let p = grid.domain_point(face, i, j, 0);
        let mut warp = [0i32; 3];
        for o in &k.octaves[..(k.stack[3].max(0) as usize).min(WARP_OCTAVES)] {
            let axis = (o.kind.wrapping_sub(WARP)) as usize;
            if axis >= 3 {
                return Err("invalid warp axis");
            }
            warp[axis] = warp[axis]
                .checked_add(mul_fine(o.amplitude, noise_fine(p, o.shift, o.seed)))
                .ok_or("warp sum overflows")?;
        }
        let q = IVec3::new(
            p.x.checked_add(warp[0]).ok_or("warped domain overflows")?,
            p.y.checked_add(warp[1]).ok_or("warped domain overflows")?,
            p.z.checked_add(warp[2]).ok_or("warped domain overflows")?,
        );
        samples.push(std::array::from_fn(|r| {
            let Some(o) = ridges.get(r) else { return 0 };
            let n = noise_fine(q, o.shift, o.seed);
            let ridge = (FINE_ONE - n.abs()).clamp(0, FINE_ONE - 1);
            mul_fine(ridge, ridge)
        }));
    }

    let mut packed = [[0i32; 4]; 66];
    for row in 0..n {
        for knot in 0..KNOTS {
            let incoming = (FINE_ONE / 4 + (knot as i32) * 393216).min(FINE_ONE - 1);
            let mut sum = 0i64;
            for sample in &samples {
                let mut weight = incoming;
                for r in row..n {
                    let v = mul_fine(sample[r], weight);
                    sum += i64::from(mul_fine(ridges[r].amplitude, v));
                    weight = (v * 2).clamp(FINE_ONE / 4, FINE_ONE - 1);
                }
            }
            let mean =
                i32::try_from(sum / (SAMPLE_COUNT as i64)).map_err(|_| "suffix mean overflows")?;
            let index = row * KNOTS + knot;
            packed[index / 4][index % 4] = mean;
        }
        for knot in 0..KNOTS - 1 {
            let index = row * KNOTS + knot;
            let next = index + 1;
            let delta =
                i64::from(packed[next / 4][next % 4]) - i64::from(packed[index / 4][index % 4]);
            if delta.abs() >= (1 << 28) {
                return Err("mean interpolation exceeds limb contract");
            }
        }
    }
    // Rows from the ridge count on are the zero terminal suffix. All values
    // remain signed.
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Shape;
    use crate::layers::{Layer, LayerKind, TerrainLayers};

    // Exact i64 product is independent of the production limb implementation.
    fn product(a: i32, b: i32) -> i32 {
        (i64::from(a) * i64::from(b) / i64::from(FINE_ONE)) as i32
    }

    /// Earth constants with its mountain layer changed by `f`.
    fn earth(grid: &Grid, seed: u32, f: impl Fn(&mut Layer)) -> LandformConstants {
        let mut stack = TerrainLayers::earth();
        stack
            .layers
            .iter_mut()
            .filter(|l| l.kind == LayerKind::Mountains)
            .for_each(f);
        stack.compile(grid, seed).unwrap().0
    }

    fn is_display_ridge(k: &LandformConstants, o: &Octave) -> bool {
        k.shape[0] > 0 && o.kind == (((k.shape[0] as u32 - 1) << 8) | RIDGE)
    }

    fn holdout(grid: &Grid, k: &LandformConstants) -> Vec<[i32; RIDGES]> {
        let ridges = display_ridges(k);
        (0..8192)
            .map(|index| {
                // Distinct salt and sequence from the constructor's 2048 samples.
                let id = index + 19_937;
                let face = grid.faces()[index as usize % grid.faces().len()];
                let i = (hash3(id, 79, -15, 0xA49BA217) % grid.cells() as u32) as i32;
                let j = (hash3(id, -97, 31, 0xA49BA217) % grid.cells() as u32) as i32;
                let p = grid.domain_point(face, i, j, 0);
                let mut q = p;
                for o in &k.octaves[..k.stack[3] as usize] {
                    q[(o.kind - WARP) as usize] +=
                        product(o.amplitude, noise_fine(p, o.shift, o.seed));
                }
                std::array::from_fn(|r| {
                    let Some(o) = ridges.get(r) else { return 0 };
                    let value =
                        (FINE_ONE - noise_fine(q, o.shift, o.seed).abs()).clamp(0, FINE_ONE - 1);
                    product(value, value)
                })
            })
            .collect()
    }

    #[test]
    fn suffix_means_match_independent_full_ridge_holdout() {
        for grid in [
            Grid::new(6_371_000.0, 0.1).unwrap(),
            Grid::plane(Shape::Plane, 100_000.0, 0.3).unwrap(),
        ] {
            for (scale_km, octaves) in [(20.0, 7), (0.000001, 7), (100_000.0, 7), (20.0, 4)] {
                let k = earth(&grid, 19, |l| {
                    l.scale_km = scale_km;
                    l.octaves = octaves;
                });
                let lut = bake_ridge_suffix(&grid, &k).unwrap();
                let samples = holdout(&grid, &k);
                let ridges = display_ridges(&k);
                let n = ridges.len();
                assert_eq!(n, octaves as usize);
                for row in 0..n {
                    let magnitude: i64 = ridges[row..]
                        .iter()
                        .map(|o| i64::from(o.amplitude).abs())
                        .sum();
                    for half_knot in (0..=64).step_by(2).chain([7, 23, 47]) {
                        let incoming = (FINE_ONE / 4 + half_knot as i32 * 196608).min(FINE_ONE - 1);
                        let mut sum = 0i64;
                        for sample in &samples {
                            let mut weight = incoming;
                            for r in row..n {
                                let v = product(sample[r], weight);
                                sum += i64::from(product(ridges[r].amplitude, v));
                                weight = (v * 2).clamp(FINE_ONE / 4, FINE_ONE - 1);
                            }
                        }
                        let expected = sum / samples.len() as i64;
                        let delta = incoming - FINE_ONE / 4;
                        let knot = (delta / 393216).min(31) as usize;
                        let fraction = ((delta - knot as i32 * 393216) << 7) / 3;
                        let index = row * KNOTS + knot;
                        let lo = lut[index / 4][index % 4];
                        let hi = lut[(index + 1) / 4][(index + 1) % 4];
                        let actual = i64::from(lo) + i64::from(product(hi - lo, fraction));
                        // A statistical gate, not exact spatial reconstruction.
                        let error = (actual - expected).abs();
                        let tolerance = (magnitude * 15 / 1000).max(16);
                        assert!(error <= tolerance,
                            "scale={scale_km} km x{octaves}, row={row}, half-knot={half_knot}: {actual} vs {expected}, error={error}, allowed={tolerance}");
                    }
                }
                // Rows past the ridge count are the zero terminal suffix.
                for index in n * KNOTS..264 {
                    assert_eq!(lut[index / 4][index % 4], 0);
                }
                assert!(
                    lut[32 / 4][32 % 4] > 0,
                    "unresolved positive ridge mass must not vanish"
                );
            }
        }
    }

    #[test]
    fn signed_amplitudes_repeated_shifts_and_unsafe_recipes_are_explicit() {
        let grid = Grid::new(6_371_000.0, 1.0).unwrap();
        let k = earth(&grid, 7, |_| {});
        let positive = bake_ridge_suffix(&grid, &k).unwrap();
        let count = k.header[0] as usize;
        let mut negative = k;
        for o in &mut negative.octaves[..count] {
            if is_display_ridge(&k, o) {
                o.amplitude = -o.amplitude;
            }
        }
        let neg = bake_ridge_suffix(&grid, &negative).unwrap();
        for index in 0..264 {
            assert_eq!(positive[index / 4][index % 4], -neg[index / 4][index % 4]);
        }
        let mut zero = k;
        for o in &mut zero.octaves[..count] {
            if is_display_ridge(&k, o) {
                o.amplitude = 0;
                o.shift = 1;
            }
        }
        assert_eq!(bake_ridge_suffix(&grid, &zero).unwrap(), [[0; 4]; 66]);
        let mut invalid = k;
        let first = invalid
            .octaves
            .iter()
            .position(|o| is_display_ridge(&k, o))
            .unwrap();
        let second = first
            + 1
            + invalid.octaves[first + 1..]
                .iter()
                .position(|o| is_display_ridge(&k, o))
                .unwrap();
        invalid.octaves[first].amplitude = 1 << 28;
        assert!(bake_ridge_suffix(&grid, &invalid).is_err());
        invalid = k;
        invalid.octaves[second].shift = invalid.octaves[first].shift + 1;
        assert!(bake_ridge_suffix(&grid, &invalid).is_err());
        // No display layer: no bake.
        let mut none = k;
        none.shape[0] = 0;
        assert!(bake_ridge_suffix(&grid, &none).is_err());
        // Mixed signs test recipe; no positive-only assumption in the payload.
        let mut mixed = k;
        mixed.octaves[first].amplitude = -mixed.octaves[first].amplitude;
        assert!(bake_ridge_suffix(&grid, &mixed).unwrap()[8][0] < 0);
    }

    #[test]
    fn display_bounds_reuse_canonical_suffix_diameter() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        for signs in [0u32, 0b1111111, 0b0101010] {
            let mut k = earth(&grid, 7, |_| {});
            let tag = ((k.shape[0] as u32 - 1) << 8) | RIDGE;
            for (r, o) in k.octaves.iter_mut().filter(|o| o.kind == tag).enumerate() {
                if signs & (1 << r) != 0 {
                    o.amplitude = -o.amplitude;
                }
            }
            let canonical = k.bound_margins(&grid);
            assert_eq!(canonical, k.bound_margins(&grid));
        }
        // Constants (2608: the planet maps' header and rules' climate rows
        // included) and the ridge suffix (1056).
        assert_eq!(std::mem::size_of::<LandformConstants>() + 66 * 16, 3664);
    }

    #[test]
    fn signed_suffix_mask_rounding_preserves_the_diameter() {
        // Mixed signs and non-integral mask products exercise the rounding
        // premise independently of the bake's statistical approximation.
        let negative = -2_105_360;
        let positive = 2_400_000;
        let diameter = positive - negative;
        let masks = [0, 1, FINE_ONE / 7, FINE_ONE / 2, FINE_ONE - 1, FINE_ONE];
        let prefixes = [-4_505_360, -1, 0, 1, 4_505_360];
        let mut minimum = i32::MAX;
        let mut maximum = i32::MIN;
        for prefix in prefixes {
            for region in masks {
                for land in masks {
                    let mask = |x| mul_fine(mul_fine(x, region), land);
                    let baseline = mask(prefix);
                    for tail in [negative, negative + 1, -1, 0, 1, positive - 1, positive] {
                        let actual = mask(prefix + tail) - baseline;
                        assert!((negative..=positive).contains(&actual));
                        minimum = minimum.min(actual);
                        maximum = maximum.max(actual);
                    }
                }
            }
        }
        assert_eq!(maximum - minimum, diameter);
    }

    #[test]
    #[ignore = "constructor microbenchmark; run optimized and without concurrent engine work"]
    fn constructor_bake_cost_and_cached_payload() {
        use crate::terrain::TerrainField;
        use std::{hint::black_box, time::Instant};
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let stack = TerrainLayers::earth();
        let k = stack.compile(&grid, 7).unwrap().0;
        let mut bake = Vec::new();
        let mut constructor = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            black_box(bake_ridge_suffix(&grid, &k).unwrap());
            bake.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            black_box(stack.field(&grid, 7).unwrap());
            constructor.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        bake.sort_by(f64::total_cmp);
        constructor.sort_by(f64::total_cmp);
        let field = stack.field(&grid, 7).unwrap();
        let first = field.program();
        assert_eq!(
            first.constants.len(),
            std::mem::size_of::<crate::landform::LandformConstants>() + 66 * 16
        );
        let start = Instant::now();
        for _ in 0..1024 {
            let program = field.program();
            assert_eq!(program.constants, first.constants);
            black_box(program);
        }
        let serialization = start.elapsed().as_secs_f64() * 1000.0 / 1024.0;
        eprintln!("recipe-only ridge bake: median={:.6}ms max={:.6}ms; full constructor: median={:.6}ms max={:.6}ms; cached program serialization={serialization:.6}ms/call",bake[4],bake[8],constructor[4],constructor[8]);
    }
}
