use crate::{CellWord, ContentHash, MaterialId, PageKey, TerrainNodeSummary};

const NOISE_ONE: i64 = 1 << 15;

/// Canonical parameters for the smooth volumetric planet source.
///
/// All distances are expressed in the planet's LOD0 cells. The generator uses
/// integer arithmetic so a seed/configuration has one content identity on
/// every supported platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanetSdfConfig {
    pub seed: u64,
    pub surface_amplitude_cells: u32,
    pub surface_period_cells: u32,
    pub surface_octaves: u8,
    pub overhang_amplitude_cells: u32,
    pub overhang_period_cells: u32,
    pub cave_depth_cells: u32,
    pub cave_period_cells: u32,
    pub cave_width_cells: u32,
}

impl PlanetSdfConfig {
    /// Zero-relief scalar field used only to isolate hierarchy and extraction tests.
    pub const fn zero_relief_test_fixture() -> Self {
        Self {
            seed: 0,
            surface_amplitude_cells: 0,
            surface_period_cells: 1,
            surface_octaves: 0,
            overhang_amplitude_cells: 0,
            overhang_period_cells: 1,
            cave_depth_cells: 0,
            cave_period_cells: 1,
            cave_width_cells: 0,
        }
    }

    /// Earth-like defaults converted into the planet's explicit sample scale.
    pub fn earthlike(seed: u64, lod0_cell_size_mm: u32) -> Result<Self, PlanetSdfConfigError> {
        if lod0_cell_size_mm == 0 {
            return Err(PlanetSdfConfigError::CellSize);
        }
        let cells = |meters: u32| {
            meters
                .saturating_mul(1_000)
                .div_ceil(lod0_cell_size_mm)
                .max(1)
        };
        Ok(Self {
            seed,
            surface_amplitude_cells: cells(6_000),
            surface_period_cells: cells(400_000),
            surface_octaves: 6,
            overhang_amplitude_cells: cells(80),
            overhang_period_cells: cells(160),
            cave_depth_cells: cells(2_000),
            cave_period_cells: cells(120),
            cave_width_cells: cells(12),
        })
    }

    pub fn validate(self) -> Result<(), PlanetSdfConfigError> {
        if self.surface_octaves > 16 {
            return Err(PlanetSdfConfigError::SurfaceOctaves(self.surface_octaves));
        }
        if self.surface_amplitude_cells != 0 && self.surface_period_cells < 2 {
            return Err(PlanetSdfConfigError::SurfacePeriod(
                self.surface_period_cells,
            ));
        }
        if self.overhang_amplitude_cells != 0 && self.overhang_period_cells < 2 {
            return Err(PlanetSdfConfigError::OverhangPeriod(
                self.overhang_period_cells,
            ));
        }
        if self.cave_width_cells != 0 && (self.cave_depth_cells == 0 || self.cave_period_cells < 2)
        {
            return Err(PlanetSdfConfigError::CaveParameters);
        }
        Ok(())
    }

    pub fn maximum_displacement_cells(self) -> u64 {
        let mut amplitude = u64::from(self.surface_amplitude_cells);
        let mut total = 0_u64;
        for _ in 0..self.surface_octaves {
            total = total.saturating_add(amplitude);
            amplitude /= 2;
        }
        total.saturating_add(u64::from(self.overhang_amplitude_cells))
    }

    fn unresolved_error_cells(self, lod: u8) -> u64 {
        let spacing = 1_u64.checked_shl(u32::from(lod)).unwrap_or(u64::MAX);
        let mut error = spacing;
        let mut amplitude = u64::from(self.surface_amplitude_cells);
        let mut period = u64::from(self.surface_period_cells);
        for _ in 0..self.surface_octaves {
            if period < spacing.saturating_mul(2) {
                error = error.saturating_add(amplitude);
            }
            amplitude /= 2;
            period = (period / 2).max(1);
        }
        if u64::from(self.overhang_period_cells) < spacing.saturating_mul(2) {
            error = error.saturating_add(u64::from(self.overhang_amplitude_cells));
        }
        if self.cave_width_cells != 0
            && u64::from(self.cave_period_cells) < spacing.saturating_mul(2)
        {
            error = error.saturating_add(u64::from(self.cave_width_cells));
        }
        error
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanetSdfConfigError {
    #[error("planet LOD0 cell size must be non-zero")]
    CellSize,
    #[error("surface octave count must be <= 16, got {0}")]
    SurfaceOctaves(u8),
    #[error("surface period must be >= 2 cells when enabled, got {0}")]
    SurfacePeriod(u32),
    #[error("overhang period must be >= 2 cells when enabled, got {0}")]
    OverhangPeriod(u32),
    #[error("enabled caves require a nonzero depth and a period of at least 2 cells")]
    CaveParameters,
}

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

/// Pole-free canonical smooth planet SDF.
///
/// Relief and caves are evaluated in Cartesian planet space, so there is no
/// latitude/longitude seam or polar singularity. The scalar is negative in
/// solid terrain and positive in air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanetSdfGenerator {
    pub center_cell: [i64; 3],
    pub radius_cells: u64,
    pub material: MaterialId,
    pub config: PlanetSdfConfig,
}

impl PlanetSdfGenerator {
    pub fn new(
        center_cell: [i64; 3],
        radius_cells: u64,
        material: MaterialId,
        config: PlanetSdfConfig,
    ) -> Result<Self, PlanetSdfConfigError> {
        config.validate()?;
        Ok(Self {
            center_cell,
            radius_cells,
            material,
            config,
        })
    }

    #[doc(hidden)]
    pub const fn zero_relief_test_fixture(
        center_cell: [i64; 3],
        radius_cells: u64,
        material: MaterialId,
    ) -> Self {
        Self {
            center_cell,
            radius_cells,
            material,
            config: PlanetSdfConfig::zero_relief_test_fixture(),
        }
    }

    fn sample_density_i64(self, cell_xyz: [i64; 3]) -> i64 {
        let local = [
            cell_xyz[0].saturating_sub(self.center_cell[0]),
            cell_xyz[1].saturating_sub(self.center_cell[1]),
            cell_xyz[2].saturating_sub(self.center_cell[2]),
        ];
        let distance = integer_sqrt(
            local
                .iter()
                .map(|axis| i128::from(*axis).unsigned_abs().saturating_pow(2))
                .sum::<u128>(),
        );
        let radial = i128::try_from(distance)
            .unwrap_or(i128::MAX)
            .saturating_sub(i128::from(self.radius_cells));
        let relief = fbm_displacement(
            local,
            self.config.seed,
            self.config.surface_amplitude_cells,
            self.config.surface_period_cells,
            self.config.surface_octaves,
        )
        .saturating_add(scaled_noise(
            rotate(local, 5),
            self.config.overhang_period_cells,
            self.config.overhang_amplitude_cells,
            self.config.seed ^ 0x6a09_e667_f3bc_c909,
        ));
        let surface = radial
            .saturating_sub(i128::from(relief))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;

        if self.config.cave_width_cells == 0 {
            return surface;
        }
        let depth = surface.saturating_neg();
        let cave_depth = i64::from(self.config.cave_depth_cells);
        let cave_width = i64::from(self.config.cave_width_cells);
        if depth < -cave_width || depth > cave_depth.saturating_add(cave_width) {
            return surface;
        }
        let first = value_noise_q15(
            rotate(local, 1),
            self.config.cave_period_cells,
            self.config.seed ^ 0xbb67_ae85_84ca_a73b,
        )
        .unsigned_abs() as i64;
        let second = value_noise_q15(
            rotate(local, 3),
            self.config.cave_period_cells,
            self.config.seed ^ 0x3c6e_f372_fe94_f82b,
        )
        .unsigned_abs() as i64;
        let threshold = NOISE_ONE / 5;
        let tunnel = threshold
            .saturating_sub(first.max(second))
            .saturating_mul(cave_width)
            / threshold;
        let band = depth
            .saturating_add(cave_width)
            .min(cave_depth.saturating_add(cave_width).saturating_sub(depth));
        surface.max(tunnel.min(band))
    }
}

impl DeterministicGenerator for PlanetSdfGenerator {
    fn hash(&self) -> ContentHash {
        let mut bytes = Vec::with_capacity(96);
        bytes.extend_from_slice(b"pulsar.planet-sdf.v1");
        for axis in self.center_cell {
            bytes.extend_from_slice(&axis.to_le_bytes());
        }
        bytes.extend_from_slice(&self.radius_cells.to_le_bytes());
        bytes.push(self.material);
        bytes.extend_from_slice(&self.config.seed.to_le_bytes());
        bytes.extend_from_slice(&self.config.surface_amplitude_cells.to_le_bytes());
        bytes.extend_from_slice(&self.config.surface_period_cells.to_le_bytes());
        bytes.push(self.config.surface_octaves);
        bytes.extend_from_slice(&self.config.overhang_amplitude_cells.to_le_bytes());
        bytes.extend_from_slice(&self.config.overhang_period_cells.to_le_bytes());
        bytes.extend_from_slice(&self.config.cave_depth_cells.to_le_bytes());
        bytes.extend_from_slice(&self.config.cave_period_cells.to_le_bytes());
        bytes.extend_from_slice(&self.config.cave_width_cells.to_le_bytes());
        ContentHash::of(&bytes)
    }

    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord {
        let signed_distance = self
            .sample_density_i64(cell_xyz)
            .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        CellWord::new(
            signed_distance,
            if signed_distance <= 0 {
                self.material
            } else {
                0
            },
            0,
        )
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
        let (base_min, base_max) = sphere_signed_distance_bounds(
            self.center_cell,
            self.radius_cells,
            min,
            [max_x, max_y, max_z],
        );
        let displacement = self
            .config
            .maximum_displacement_cells()
            .min(i32::MAX as u64) as i32;
        let min_density = base_min.saturating_sub(displacement);
        let surface_max = base_max.saturating_add(displacement);
        let caves_possible = self.config.cave_width_cells != 0
            && min_density <= self.config.cave_width_cells.min(i32::MAX as u32) as i32
            && surface_max
                >= -(self
                    .config
                    .cave_depth_cells
                    .saturating_add(self.config.cave_width_cells)
                    .min(i32::MAX as u32) as i32);
        let max_density = if caves_possible {
            surface_max.max(self.config.cave_width_cells.min(i32::MAX as u32) as i32)
        } else {
            surface_max
        };
        let error = if min_density > 0 || (max_density <= 0 && !caves_possible) {
            0
        } else {
            self.config.unresolved_error_cells(key.lod)
        };
        TerrainNodeSummary::new(min_density, max_density, error, 0)
            .expect("planet SDF bounds are ordered")
    }
}

fn fbm_displacement(
    point: [i64; 3],
    seed: u64,
    mut amplitude: u32,
    mut period: u32,
    octaves: u8,
) -> i64 {
    let mut total = 0_i64;
    for octave in 0..octaves {
        if amplitude == 0 || period < 2 {
            break;
        }
        total = total.saturating_add(scaled_noise(
            rotate(point, octave),
            period,
            amplitude,
            seed ^ u64::from(octave).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        ));
        amplitude /= 2;
        period = (period / 2).max(1);
    }
    total
}

fn scaled_noise(point: [i64; 3], period: u32, amplitude: u32, seed: u64) -> i64 {
    if amplitude == 0 || period < 2 {
        return 0;
    }
    i64::from(value_noise_q15(point, period, seed)).saturating_mul(i64::from(amplitude)) / NOISE_ONE
}

fn value_noise_q15(point: [i64; 3], period: u32, seed: u64) -> i32 {
    let period = i64::from(period.max(2));
    let lattice = point.map(|axis| axis.div_euclid(period));
    let fraction = point.map(|axis| {
        let remainder = axis.rem_euclid(period);
        ((i128::from(remainder) * i128::from(NOISE_ONE)) / i128::from(period)) as i64
    });
    let fade = fraction.map(fade_q15);
    let mut corners = [0_i64; 8];
    for (index, output) in corners.iter_mut().enumerate() {
        *output = i64::from(lattice_value(
            [
                lattice[0] + (index & 1) as i64,
                lattice[1] + ((index >> 1) & 1) as i64,
                lattice[2] + ((index >> 2) & 1) as i64,
            ],
            seed,
        ));
    }
    let x00 = lerp_q15(corners[0], corners[1], fade[0]);
    let x10 = lerp_q15(corners[2], corners[3], fade[0]);
    let x01 = lerp_q15(corners[4], corners[5], fade[0]);
    let x11 = lerp_q15(corners[6], corners[7], fade[0]);
    let y0 = lerp_q15(x00, x10, fade[1]);
    let y1 = lerp_q15(x01, x11, fade[1]);
    lerp_q15(y0, y1, fade[2]) as i32
}

fn fade_q15(value: i64) -> i64 {
    // 6t^5 - 15t^4 + 10t^3, evaluated in Q15.
    let t2 = value.saturating_mul(value) / NOISE_ONE;
    let t3 = t2.saturating_mul(value) / NOISE_ONE;
    let polynomial = 10_i64
        .saturating_mul(NOISE_ONE)
        .saturating_sub(15_i64.saturating_mul(value))
        .saturating_add(6_i64.saturating_mul(t2));
    (t3.saturating_mul(polynomial) / NOISE_ONE).clamp(0, NOISE_ONE)
}

fn lerp_q15(left: i64, right: i64, weight: i64) -> i64 {
    left.saturating_add(right.saturating_sub(left).saturating_mul(weight) / NOISE_ONE)
}

fn lattice_value(point: [i64; 3], seed: u64) -> i32 {
    let mut value = seed;
    for (axis, prime) in point.into_iter().zip([
        0x9e37_79b9_7f4a_7c15_u64,
        0xbf58_476d_1ce4_e5b9,
        0x94d0_49bb_1331_11eb,
    ]) {
        value ^= (axis as u64).wrapping_mul(prime);
        value = splitmix64(value);
    }
    ((value >> 48) as u16 as i32) - 32_768
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn rotate(point: [i64; 3], octave: u8) -> [i64; 3] {
    match octave % 6 {
        0 => point,
        1 => [point[1], point[2], point[0]],
        2 => [point[2], point[0], point[1]],
        3 => [point[0], point[2].saturating_neg(), point[1]],
        4 => [point[1].saturating_neg(), point[0], point[2]],
        _ => [point[2], point[1], point[0].saturating_neg()],
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

    fn volumetric_fixture() -> PlanetSdfGenerator {
        PlanetSdfGenerator::new(
            [0; 3],
            1_000,
            7,
            PlanetSdfConfig {
                seed: 0x5eed,
                surface_amplitude_cells: 120,
                surface_period_cells: 512,
                surface_octaves: 4,
                overhang_amplitude_cells: 24,
                overhang_period_cells: 48,
                cave_depth_cells: 240,
                cave_period_cells: 64,
                cave_width_cells: 12,
            },
        )
        .unwrap()
    }

    #[test]
    fn earthlike_defaults_keep_the_same_physical_scale_at_different_cell_sizes() {
        let meter = PlanetSdfConfig::earthlike(7, 1_000).unwrap();
        let decimeter = PlanetSdfConfig::earthlike(7, 100).unwrap();

        assert_eq!(
            decimeter.surface_amplitude_cells,
            meter.surface_amplitude_cells * 10
        );
        assert_eq!(
            decimeter.surface_period_cells,
            meter.surface_period_cells * 10
        );
        assert_eq!(
            decimeter.overhang_amplitude_cells,
            meter.overhang_amplitude_cells * 10
        );
        assert_eq!(decimeter.cave_width_cells, meter.cave_width_cells * 10);
        assert_eq!(
            PlanetSdfConfig::earthlike(7, 0),
            Err(PlanetSdfConfigError::CellSize)
        );
    }

    #[test]
    fn volumetric_planet_is_deterministic_and_config_is_content_addressed() {
        let generator = volumetric_fixture();
        let coordinates = [
            [-1_111, -997, -31],
            [-1_024, 17, 33],
            [0, 0, 0],
            [991, -129, 73],
            [1_117, 1_009, -47],
        ];
        let first = coordinates.map(|point| generator.sample_cell(point));
        let second = coordinates.map(|point| generator.sample_cell(point));
        assert_eq!(first, second);

        let mut changed = generator;
        changed.config.seed ^= 1;
        assert_ne!(generator.hash(), changed.hash());
        assert_ne!(
            generator.hash(),
            PlanetSdfGenerator::zero_relief_test_fixture(
                generator.center_cell,
                generator.radius_cells,
                generator.material
            )
            .hash()
        );
    }

    #[test]
    fn volumetric_planet_has_relief_and_canonical_cave_air() {
        let generator = volumetric_fixture();
        let mut surface_radii = Vec::new();
        for direction in [[1, 0, 0], [0, 1, 0], [0, 0, 1], [-1, 0, 0]] {
            let radius = (700_i64..=1_300)
                .find(|radius| {
                    !generator
                        .sample_cell(direction.map(|axis| i64::from(axis) * radius))
                        .is_solid()
                })
                .expect("fixture surface must cross the radial trace");
            surface_radii.push(radius);
        }
        assert!(surface_radii.iter().min() != surface_radii.iter().max());

        let cave_air = (-1_200_i64..=1_200).any(|x| {
            (-1_200_i64..=1_200).step_by(12).any(|y| {
                let point = [x, y, 0];
                let radial = integer_sqrt(
                    point
                        .iter()
                        .map(|axis| i128::from(*axis).unsigned_abs().saturating_pow(2))
                        .sum(),
                ) as i64;
                radial < 980 && radial > 760 && !generator.sample_cell(point).is_solid()
            })
        });
        assert!(
            cave_air,
            "the configured underground band must contain cave air"
        );
    }

    #[test]
    fn volumetric_summaries_conservatively_contain_samples_across_signed_pages() {
        let generator = volumetric_fixture();
        for lod in [0, 1, 3, 5] {
            for page_x in -3..=2 {
                for page_y in -2..=1 {
                    for page_z in -2..=1 {
                        let key = PageKey::new(lod, [page_x, page_y, page_z]);
                        let summary = generator.summarize_region(key);
                        let min = key.lod0_cell_min().unwrap();
                        let span = key.lod0_cell_span().unwrap();
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
                                        "{key:?} summary {summary:?} missed {density}"
                                    );
                                    assert!(!summary.is_uniform_air() || density > 0);
                                    assert!(!summary.is_uniform_solid() || density <= 0);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zero_relief_sdf_is_stable_and_signed() {
        let generator = PlanetSdfGenerator::zero_relief_test_fixture([0; 3], 10, 7);
        assert!(generator.sample_cell([0; 3]).is_solid());
        assert_eq!(generator.sample_cell([0; 3]).material(), 7);
        assert!(!generator.sample_cell([20, 0, 0]).is_solid());
        assert_eq!(generator.hash(), generator.hash());
    }

    #[test]
    fn zero_relief_sdf_summaries_never_reject_sampled_surface_values() {
        let generator = PlanetSdfGenerator::zero_relief_test_fixture([-17, 29, -43], 1_003, 7);
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
