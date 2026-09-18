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

/// Deterministic source for a finite, axis-aligned flat voxel world.
///
/// The solid region is the intersection of two half-space families: the
/// volume's own axis-aligned box, and everything at or below the ground plane
/// `origin_cell[1]`. That gives exactly what "create a flat world" should
/// produce — a level ground surface at the origin plane with solid rock under
/// it, bounded on every side — expressed in the *same* signed density field
/// planets use, so every brush, page, summary and snapshot path is shared.
///
/// All of it is integer math on canonical LOD0 cells, so the source is
/// bit-reproducible exactly like [`FixedSphereGenerator`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedVolumeGenerator {
    /// Canonical LOD0 cell at the centre of the volume's ground plane.
    pub origin_cell: [i64; 3],
    /// Half-extent from `origin_cell` along each axis, in LOD0 cells.
    pub half_extent_cells: [i64; 3],
    pub material: MaterialId,
}

impl FixedVolumeGenerator {
    /// Signed distance, in cells, to the volume's solid region.
    fn signed_distance(&self, cell_xyz: [i64; 3]) -> i32 {
        let box_distance =
            box_signed_distance(self.origin_cell, self.half_extent_cells, cell_xyz);
        let below_ground = (i128::from(cell_xyz[1]) - i128::from(self.origin_cell[1]))
            .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        box_distance.max(below_ground)
    }
}

impl DeterministicGenerator for FixedVolumeGenerator {
    fn hash(&self) -> ContentHash {
        let mut bytes = Vec::with_capacity(56);
        bytes.extend_from_slice(b"pulsar.fixed-volume.v1");
        for axis in self.origin_cell {
            bytes.extend_from_slice(&axis.to_le_bytes());
        }
        for axis in self.half_extent_cells {
            bytes.extend_from_slice(&axis.to_le_bytes());
        }
        bytes.push(self.material);
        ContentHash::of(&bytes)
    }

    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord {
        let signed_distance = i64::from(self.signed_distance(cell_xyz))
            .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        let material = if signed_distance <= 0 {
            self.material
        } else {
            0
        };
        CellWord::new(signed_distance, material, 0)
    }

    fn summarize_region(&self, key: PageKey) -> TerrainNodeSummary {
        let Some((min, max)) = region_cell_bounds(key) else {
            return TerrainNodeSummary::unknown(0);
        };
        let (box_min, box_max) =
            box_signed_distance_bounds(self.origin_cell, self.half_extent_cells, min, max);
        // The ground half-space is monotone in y, so its extremes over the
        // region are its values at the region's y bounds.
        let ground_min = (i128::from(min[1]) - i128::from(self.origin_cell[1]))
            .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        let ground_max = (i128::from(max[1]) - i128::from(self.origin_cell[1]))
            .clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
        // `max` of two functions: the upper bound is exact, the lower bound is
        // conservative (never above the true minimum), which is all a summary
        // has to promise.
        let min_density = box_min.max(ground_min);
        let max_density = box_max.max(ground_max);
        let error = if min_density > 0 || max_density <= 0 {
            0
        } else {
            1_u64.checked_shl(u32::from(key.lod)).unwrap_or(u64::MAX)
        };
        TerrainNodeSummary::new(min_density, max_density, error, 0)
            .expect("volume bounds are ordered")
    }
}

/// The canonical terrain source, one variant per [`crate::TerrainShape`].
///
/// The runtime instantiates [`crate::TerrainCore`] with exactly this type, so
/// planets and volumes share one hierarchy, one page builder, one edit log and
/// one snapshot format. Shape is a property of the *source*, not of any
/// machinery above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainGenerator {
    Sphere(FixedSphereGenerator),
    Volume(FixedVolumeGenerator),
}

impl DeterministicGenerator for TerrainGenerator {
    fn hash(&self) -> ContentHash {
        match self {
            Self::Sphere(generator) => generator.hash(),
            Self::Volume(generator) => generator.hash(),
        }
    }

    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord {
        match self {
            Self::Sphere(generator) => generator.sample_cell(cell_xyz),
            Self::Volume(generator) => generator.sample_cell(cell_xyz),
        }
    }

    fn summarize_region(&self, key: PageKey) -> TerrainNodeSummary {
        match self {
            Self::Sphere(generator) => generator.summarize_region(key),
            Self::Volume(generator) => generator.summarize_region(key),
        }
    }
}

/// Inclusive canonical LOD0 cell bounds of one hierarchy region.
fn region_cell_bounds(key: PageKey) -> Option<([i64; 3], [i64; 3])> {
    let min = key.lod0_cell_min()?;
    let span = key.lod0_cell_span()?;
    let max = [
        min[0].checked_add(span - 1)?,
        min[1].checked_add(span - 1)?,
        min[2].checked_add(span - 1)?,
    ];
    Some((min, max))
}

/// Signed distance, in cells, from a point to an axis-aligned box.
///
/// Standard exact box SDF: the Euclidean distance to the surface outside, and
/// the (negative) distance to the nearest face inside.
pub(crate) fn box_signed_distance(
    center_cell: [i64; 3],
    half_extent_cells: [i64; 3],
    cell_xyz: [i64; 3],
) -> i32 {
    let per_axis = std::array::from_fn(|axis| {
        (i128::from(cell_xyz[axis]) - i128::from(center_cell[axis])).abs()
            - i128::from(half_extent_cells[axis])
    });
    box_distance_from_axis_deltas(per_axis)
}

/// Conservative (here: exact) signed-distance interval of a box over an
/// inclusive cell AABB.
///
/// The region is a product of independent per-axis intervals and the box SDF
/// is monotone non-decreasing in every per-axis delta, so evaluating it at the
/// per-axis delta minima and maxima yields the true extremes.
pub(crate) fn box_signed_distance_bounds(
    center_cell: [i64; 3],
    half_extent_cells: [i64; 3],
    min_cell: [i64; 3],
    max_cell: [i64; 3],
) -> (i32, i32) {
    let mut nearest = [0_i128; 3];
    let mut farthest = [0_i128; 3];
    for axis in 0..3 {
        let center = i128::from(center_cell[axis]);
        let low = i128::from(min_cell[axis]);
        let high = i128::from(max_cell[axis]);
        let half = i128::from(half_extent_cells[axis]);
        let closest = if center < low {
            low - center
        } else if center > high {
            center - high
        } else {
            0
        };
        nearest[axis] = closest - half;
        farthest[axis] = (low - center).abs().max((high - center).abs()) - half;
    }
    (
        box_distance_from_axis_deltas(nearest),
        box_distance_from_axis_deltas(farthest),
    )
}

/// `outside_distance + inside_distance` for per-axis `|p - c| - half` deltas.
fn box_distance_from_axis_deltas(deltas: [i128; 3]) -> i32 {
    let outside_squared = deltas
        .iter()
        .map(|delta| {
            let clamped = (*delta).max(0);
            clamped.saturating_mul(clamped) as u128
        })
        .sum::<u128>();
    let outside = integer_sqrt(outside_squared).min(i32::MAX as u128) as i128;
    let inside = deltas.iter().copied().fold(i128::MIN, i128::max).min(0);
    (outside + inside).clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
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

    fn volume() -> FixedVolumeGenerator {
        FixedVolumeGenerator {
            origin_cell: [0; 3],
            half_extent_cells: [64, 64, 64],
            material: 5,
        }
    }

    #[test]
    fn a_flat_volume_is_solid_below_its_ground_plane_and_air_above() {
        let generator = volume();
        assert!(generator.sample_cell([0, 0, 0]).is_solid(), "ground plane");
        assert_eq!(generator.sample_cell([0, 0, 0]).material(), 5);
        assert!(generator.sample_cell([10, -20, -10]).is_solid());
        assert!(!generator.sample_cell([0, 1, 0]).is_solid(), "above ground");
        assert_eq!(generator.sample_cell([0, 1, 0]).material(), 0);
    }

    #[test]
    fn a_flat_volume_is_air_outside_its_extent() {
        let generator = volume();
        assert!(!generator.sample_cell([65, -10, 0]).is_solid());
        assert!(!generator.sample_cell([0, -10, -65]).is_solid());
        assert!(!generator.sample_cell([0, -65, 0]).is_solid(), "below floor");
        assert!(generator.sample_cell([64, -64, 64]).is_solid(), "corner");
    }

    #[test]
    fn a_flat_volumes_density_grows_with_height_above_the_ground() {
        let generator = volume();
        assert_eq!(generator.sample_cell([0, 4, 0]).density(), 4);
        assert_eq!(generator.sample_cell([0, -4, 0]).density(), -4);
    }

    #[test]
    fn flat_volume_summaries_never_reject_sampled_values() {
        let generator = FixedVolumeGenerator {
            origin_cell: [-13, 37, 5],
            half_extent_cells: [200, 200, 200],
            material: 2,
        };
        for lod in [0, 1, 3, 5] {
            let span = PageKey::new(lod, [0; 3]).lod0_cell_span().unwrap();
            for x in [-2, -1, 0, 1, 2] {
                for y in [-2, -1, 0, 1] {
                    for z in [-1, 0, 1] {
                        let key = PageKey::new(lod, [x, y, z]);
                        let summary = generator.summarize_region(key);
                        let min = key.lod0_cell_min().unwrap();
                        for dz in [0, span / 2, span - 1] {
                            for dy in [0, span / 2, span - 1] {
                                for dx in [0, span / 2, span - 1] {
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
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_shape_axis_delegates_without_changing_either_source() {
        let sphere = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10,
            material: 7,
        };
        let flat = volume();
        assert_eq!(
            TerrainGenerator::Sphere(sphere).sample_cell([0; 3]),
            sphere.sample_cell([0; 3])
        );
        assert_eq!(
            TerrainGenerator::Volume(flat).sample_cell([0; 3]),
            flat.sample_cell([0; 3])
        );
        assert_ne!(
            TerrainGenerator::Sphere(sphere).hash(),
            TerrainGenerator::Volume(flat).hash(),
            "a planet and a volume must never share a generator identity"
        );
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
