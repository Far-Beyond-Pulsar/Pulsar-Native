use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type MaterialId = u8;
pub const LOD0_CELL_SIZE_METERS: f64 = 0.1;
pub const PAGE_EDGE_CELLS: i64 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanetId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanetPosition {
    pub sector: [i64; 3],
    pub offset_m: [f64; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageKey {
    pub lod: u8,
    pub page_xyz: [i64; 3],
}

impl PageKey {
    pub const fn new(lod: u8, page_xyz: [i64; 3]) -> Self {
        Self { lod, page_xyz }
    }

    pub fn parent(self) -> Option<Self> {
        Some(Self {
            lod: self.lod.checked_add(1)?,
            page_xyz: self.page_xyz.map(|axis| axis.div_euclid(2)),
        })
    }

    pub fn lod0_min(self) -> Option<[i64; 3]> {
        let scale = 1_i64.checked_shl(u32::from(self.lod))?;
        Some([
            self.page_xyz[0].checked_mul(scale)?,
            self.page_xyz[1].checked_mul(scale)?,
            self.page_xyz[2].checked_mul(scale)?,
        ])
    }

    /// Number of canonical LOD0 cells covered by one axis of this page.
    pub fn lod0_cell_span(self) -> Option<i64> {
        PAGE_EDGE_CELLS.checked_shl(u32::from(self.lod))
    }

    /// Canonical minimum LOD0-cell coordinate covered by this page.
    pub fn lod0_cell_min(self) -> Option<[i64; 3]> {
        let span = self.lod0_cell_span()?;
        Some([
            self.page_xyz[0].checked_mul(span)?,
            self.page_xyz[1].checked_mul(span)?,
            self.page_xyz[2].checked_mul(span)?,
        ])
    }

    /// Convert a canonical LOD0-cell coordinate into a page and its 0..32
    /// cell coordinate at the requested LOD. Euclidean division keeps the
    /// mapping continuous and unambiguous across every negative-axis boundary.
    pub fn address_lod0_cell(lod: u8, cell_xyz: [i64; 3]) -> Option<(Self, [u8; 3])> {
        let scale = 1_i64.checked_shl(u32::from(lod))?;
        let span = PAGE_EDGE_CELLS.checked_mul(scale)?;
        let page_xyz = cell_xyz.map(|axis| axis.div_euclid(span));
        let local = cell_xyz.map(|axis| (axis.rem_euclid(span) / scale) as u8);
        Some((Self { lod, page_xyz }, local))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in self.0 {
            output.push(HEX[usize::from(byte >> 4)] as char);
            output.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        output
    }
}

pub type PageId = ContentHash;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Air,
    Solid(MaterialId),
    Procedural(ContentHash),
    Branch,
    Page(PageId),
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellWord(pub u32);

impl CellWord {
    pub const AIR: Self = Self::new(i16::MAX, 0, 0);

    pub const fn new(density: i16, material: MaterialId, flags: u8) -> Self {
        Self((density as u16 as u32) | ((material as u32) << 16) | ((flags as u32) << 24))
    }

    pub const fn density(self) -> i16 {
        self.0 as u16 as i16
    }

    pub const fn material(self) -> MaterialId {
        (self.0 >> 16) as u8
    }

    pub const fn flags(self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub const fn is_solid(self) -> bool {
        self.density() <= 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_cell_to_page_conversion_is_exact_at_every_boundary() {
        for lod in 0..=30 {
            let scale = 1_i64 << lod;
            let span = PAGE_EDGE_CELLS * scale;
            let boundaries = [
                -span - 1,
                -span,
                -span + 1,
                -1,
                0,
                1,
                span - 1,
                span,
                span + 1,
            ];
            for coordinate in boundaries {
                let cell = [coordinate, -coordinate, coordinate.saturating_sub(1)];
                let (key, local) = PageKey::address_lod0_cell(lod, cell).unwrap();
                let page_min = key.lod0_cell_min().unwrap();
                for axis in 0..3 {
                    assert!(local[axis] < PAGE_EDGE_CELLS as u8);
                    let reconstructed = page_min[axis] + i64::from(local[axis]) * scale;
                    assert!(reconstructed <= cell[axis]);
                    assert!(cell[axis] < reconstructed + scale);
                }
            }
        }
    }

    #[test]
    fn page_parent_uses_euclidean_division_on_negative_axes() {
        assert_eq!(
            PageKey::new(0, [-1, -2, -3]).parent(),
            Some(PageKey::new(1, [-1, -1, -2]))
        );
    }
}
