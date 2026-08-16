use crate::{PlanetId, PlanetSdfConfig, PlanetSdfConfigError, PlanetSdfGenerator};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanetDefinition {
    pub planet_id: PlanetId,
    pub center_cell: [i64; 3],
    pub radius_cells: u64,
    pub material: u8,
    pub lod0_cell_size_mm: u32,
    pub sdf: PlanetSdfConfig,
    pub root_lod: u8,
    pub max_resident_pages: usize,
}

impl PlanetDefinition {
    pub fn lod0_cell_size_m(&self) -> f64 {
        f64::from(self.lod0_cell_size_mm) / 1_000.0
    }

    pub fn generator(&self) -> Result<PlanetSdfGenerator, PlanetSdfConfigError> {
        PlanetSdfGenerator::new(self.center_cell, self.radius_cells, self.material, self.sdf)
    }

    /// Whether every discrete cell touched by the spherical source fits the
    /// centered sparse hierarchy root represented by `root_lod`.
    pub fn fits_centered_root(&self) -> bool {
        if !(1..=62).contains(&self.root_lod) {
            return false;
        }
        let half_span = i128::from(crate::PAGE_EDGE_CELLS) << (self.root_lod - 1);
        let radius = i128::from(
            self.radius_cells
                .saturating_add(self.sdf.maximum_displacement_cells()),
        );
        self.center_cell.iter().all(|axis| {
            let center = i128::from(*axis);
            center - radius >= -half_span && center + radius < half_span
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_root_contains_the_entire_planet() {
        let definition = PlanetDefinition {
            planet_id: PlanetId([1; 16]),
            center_cell: [0; 3],
            radius_cells: 63_710_000,
            material: 1,
            lod0_cell_size_mm: 100,
            sdf: PlanetSdfConfig::zero_relief_test_fixture(),
            root_lod: 22,
            max_resident_pages: 8_192,
        };
        assert!(definition.fits_centered_root());

        let outside = PlanetDefinition {
            center_cell: [1_i64 << 62, 0, 0],
            ..definition
        };
        assert!(!outside.fits_centered_root());
    }
}
