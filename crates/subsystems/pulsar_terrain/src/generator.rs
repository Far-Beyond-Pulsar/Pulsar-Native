use crate::{CellWord, ContentHash, MaterialId, PageKey, TerrainNodeSummary};

/// Deterministic canonical terrain source. Implementations must use integer or
/// fixed-point math and include every behavior-changing parameter in `hash`.
pub trait DeterministicGenerator: Send + Sync {
    fn hash(&self) -> ContentHash;
    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord;

    /// Conservatively summarize every canonical sample in one hierarchy
    /// region. Generators that cannot provide a tighter analytic bound remain
    /// correct by returning `unknown`, but cannot prune that region.
    fn summarize_region(&self, _key: PageKey) -> TerrainNodeSummary {
        TerrainNodeSummary::unknown(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedSphereGenerator {
    pub center_cell: [i64; 3],
    pub radius_cells: u64,
    pub material: MaterialId,
}

impl DeterministicGenerator for FixedSphereGenerator {
    fn hash(&self) -> ContentHash {
        let mut bytes = Vec::with_capacity(34);
        bytes.extend_from_slice(b"pulsar.fixed-sphere.v1");
        for axis in self.center_cell {
            bytes.extend_from_slice(&axis.to_le_bytes());
        }
        bytes.extend_from_slice(&self.radius_cells.to_le_bytes());
        bytes.push(self.material);
        ContentHash::of(&bytes)
    }

    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord {
        let delta = [
            i128::from(cell_xyz[0]) - i128::from(self.center_cell[0]),
            i128::from(cell_xyz[1]) - i128::from(self.center_cell[1]),
            i128::from(cell_xyz[2]) - i128::from(self.center_cell[2]),
        ];
        let distance_squared = delta
            .iter()
            .map(|axis| axis.saturating_mul(*axis) as u128)
            .sum::<u128>();
        let distance = integer_sqrt(distance_squared);
        let signed_distance = (distance as i128 - i128::from(self.radius_cells))
            .clamp(i128::from(i16::MIN), i128::from(i16::MAX)) as i16;
        let material = if signed_distance <= 0 {
            self.material
        } else {
            0
        };
        CellWord::new(signed_distance, material, 0)
    }

    fn summarize_region(&self, key: PageKey) -> TerrainNodeSummary {
        let Some(min) = key.lod0_cell_min() else {
            return TerrainNodeSummary::unknown(0);
        };
        let Some(span) = key.lod0_cell_span() else {
            return TerrainNodeSummary::unknown(0);
        };
        let Some(max_x) = min[0].checked_add(span - 1) else {
            return TerrainNodeSummary::unknown(0);
        };
        let Some(max_y) = min[1].checked_add(span - 1) else {
            return TerrainNodeSummary::unknown(0);
        };
        let Some(max_z) = min[2].checked_add(span - 1) else {
            return TerrainNodeSummary::unknown(0);
        };
        let max = [max_x, max_y, max_z];
        let (min_density, max_density) =
            sphere_signed_distance_bounds(self.center_cell, self.radius_cells, min, max);
        let error = if min_density > 0 || max_density <= 0 {
            0
        } else {
            1_u64.checked_shl(u32::from(key.lod)).unwrap_or(u64::MAX)
        };
        TerrainNodeSummary::new(min_density, max_density, error, 0)
            .expect("sphere bounds are ordered")
    }
}

pub(crate) fn sphere_signed_distance_bounds(
    center_cell: [i64; 3],
    radius_cells: u64,
    min_cell: [i64; 3],
    max_cell: [i64; 3],
) -> (i32, i32) {
    let mut minimum_distance_squared = 0_u128;
    let mut maximum_distance_squared = 0_u128;
    for axis in 0..3 {
        let center = i128::from(center_cell[axis]);
        let low = i128::from(min_cell[axis]);
        let high = i128::from(max_cell[axis]);
        let nearest = if center < low {
            low - center
        } else if center > high {
            center - high
        } else {
            0
        }
        .unsigned_abs();
        let farthest = (low - center)
            .unsigned_abs()
            .max((high - center).unsigned_abs());
        minimum_distance_squared =
            minimum_distance_squared.saturating_add(nearest.saturating_mul(nearest));
        maximum_distance_squared =
            maximum_distance_squared.saturating_add(farthest.saturating_mul(farthest));
    }
    let radius = i128::from(radius_cells);
    let minimum = (integer_sqrt(minimum_distance_squared) as i128 - radius)
        .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
    let maximum = (integer_sqrt(maximum_distance_squared) as i128 - radius)
        .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
    (minimum, maximum)
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut low = 1_u128;
    let mut high = 1_u128 << (128 - value.leading_zeros()).div_ceil(2);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if middle <= value / middle {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_sphere_is_stable_and_signed() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10,
            material: 7,
        };
        assert!(generator.sample_cell([0; 3]).is_solid());
        assert_eq!(generator.sample_cell([0; 3]).material(), 7);
        assert!(!generator.sample_cell([20, 0, 0]).is_solid());
        assert_eq!(generator.hash(), generator.hash());
    }

    #[test]
    fn fixed_sphere_summaries_never_reject_sampled_surface_values() {
        let generator = FixedSphereGenerator {
            center_cell: [-17, 29, -43],
            radius_cells: 1_003,
            material: 7,
        };
        for lod in [0, 1, 3, 5] {
            let span = PageKey::new(lod, [0; 3]).lod0_cell_span().unwrap();
            let radial_page = (generator.radius_cells as i64).div_euclid(span);
            for x in [
                -radial_page - 2,
                -radial_page - 1,
                radial_page,
                radial_page + 1,
            ] {
                for y in [-1, 0] {
                    for z in [-1, 0] {
                        let key = PageKey::new(lod, [x, y, z]);
                        let summary = generator.summarize_region(key);
                        let min = key.lod0_cell_min().unwrap();
                        for dz in [0, span / 4, span / 2, span * 3 / 4, span - 1] {
                            for dy in [0, span / 4, span / 2, span * 3 / 4, span - 1] {
                                for dx in [0, span / 4, span / 2, span * 3 / 4, span - 1] {
                                    let density = i32::from(
                                        generator
                                            .sample_cell([min[0] + dx, min[1] + dy, min[2] + dz])
                                            .density(),
                                    );
                                    assert!(
                                        summary.min_density() <= density
                                            && density <= summary.max_density(),
                                        "LOD{lod} {key:?} summary {summary:?} missed {density}"
                                    );
                                    if summary.is_uniform_air() {
                                        assert!(density > 0);
                                    }
                                    if summary.is_uniform_solid() {
                                        assert!(density <= 0);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
