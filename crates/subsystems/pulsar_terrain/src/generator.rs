use crate::{CellWord, ContentHash, MaterialId};

/// Deterministic canonical terrain source. Implementations must use integer or
/// fixed-point math and include every behavior-changing parameter in `hash`.
pub trait DeterministicGenerator: Send + Sync {
    fn hash(&self) -> ContentHash;
    fn sample_cell(&self, cell_xyz: [i64; 3]) -> CellWord;
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
}
