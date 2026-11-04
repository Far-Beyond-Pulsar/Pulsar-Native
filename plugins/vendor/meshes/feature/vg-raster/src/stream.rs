//! Test/bench helper: lays plain clusters out as ONE real-format page (the
//! format `helio-vg-build` writes: page header, `GpuCluster` records, asset
//! lattice position blocks, u8 index stream, attribute stream) so the raster
//! kernels can be driven with the same bytes the shared decode
//! (`helio_vg_format::DECODE_WGSL`) reads in production. The position
//! encoding itself lives in `helio_vg_format::testing` and is validated
//! against the builder's decoder by `helio-vg-stream`'s `decode_gpu` test.

use helio_vg_format::testing::{TestCluster, TestPage};
use helio_vg_format::{GpuAssetQuant, GpuVisibleCluster};

/// f32 -> f16 bits (round to nearest even is not needed for test uvs: truncating
/// the mantissa with rounding of the dropped bit).
pub fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let man = bits & 0x007f_ffff;
    if value == 0.0 {
        return sign;
    }
    if exp >= 0x1f {
        return sign | 0x7bff;
    }
    if exp <= 0 {
        let shift = (14 - exp) as u32;
        if shift > 24 {
            return sign;
        }
        let m = man | 0x0080_0000;
        return sign | ((m >> shift) as u16);
    }
    sign | ((exp as u16) << 10) | (man >> 13) as u16
}

/// One cluster to place in the pool.
#[derive(Clone, Debug)]
pub struct PoolEntry {
    pub positions: Vec<[f32; 3]>,
    pub tris: Vec<[u8; 3]>,
    pub material: u32,
    pub instance: u32,
    /// uv0 per vertex (stored as f16 in the attribute stream), if any.
    pub uvs: Option<Vec<[f32; 2]>>,
}

/// Collects clusters, then [`PagePool::build`] writes the page.
#[derive(Default)]
pub struct PagePool {
    pub entries: Vec<PoolEntry>,
}

/// The built page plus the rows the kernels need.
pub struct BuiltPool {
    pub page: TestPage,
    /// The page pool buffer image (pool slot 0 holds the page).
    pub bytes: Vec<u8>,
    /// Asset lattice, to upload as `vg_asset_quant[0]`.
    pub quant: GpuAssetQuant,
    /// One visible-list entry per pushed cluster, in push order.
    pub visible: Vec<GpuVisibleCluster>,
}

impl PagePool {
    /// Append a cluster for `instance`; returns its index (visible-list position).
    pub fn push(
        &mut self,
        positions: &[[f32; 3]],
        tris: &[[u8; 3]],
        material: u32,
        instance: u32,
    ) -> u32 {
        self.entries.push(PoolEntry {
            positions: positions.to_vec(),
            tris: tris.to_vec(),
            material,
            instance,
            uvs: None,
        });
        (self.entries.len() - 1) as u32
    }

    /// As [`Self::push`], with uv0 per vertex (for the alpha-masked kernels).
    pub fn push_with_uvs(
        &mut self,
        positions: &[[f32; 3]],
        tris: &[[u8; 3]],
        material: u32,
        instance: u32,
        uvs: &[[f32; 2]],
    ) -> u32 {
        assert_eq!(positions.len(), uvs.len());
        let index = self.push(positions, tris, material, instance);
        self.entries[index as usize].uvs = Some(uvs.to_vec());
        index
    }

    pub fn build(&self) -> BuiltPool {
        let attrs: Vec<Option<Vec<[u8; 16]>>> = self
            .entries
            .iter()
            .map(|e| {
                e.uvs.as_ref().map(|uvs| {
                    uvs.iter()
                        .map(|uv| {
                            let mut row = [0u8; 16];
                            let (u, v) =
                                (f16_bits(uv[0]).to_le_bytes(), f16_bits(uv[1]).to_le_bytes());
                            row[8..10].copy_from_slice(&u);
                            row[10..12].copy_from_slice(&v);
                            row
                        })
                        .collect()
                })
            })
            .collect();
        let clusters: Vec<TestCluster<'_>> = self
            .entries
            .iter()
            .zip(&attrs)
            .map(|(e, a)| TestCluster {
                positions: &e.positions,
                tris: &e.tris,
                material: e.material,
                attrs: a.as_deref(),
            })
            .collect();
        let page = TestPage::build(0, &clusters);
        let visible = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| GpuVisibleCluster {
                instance: e.instance,
                cluster_slot: page.slots[i],
            })
            .collect();
        BuiltPool {
            bytes: page.pool(),
            quant: page.quant,
            visible,
            page,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_positions_are_within_half_a_lattice_step() {
        let pts = [[-1.0f32, 5.0, 3.0], [4.0, 5.25, 3.5], [2.0, 5.5, 100.0]];
        let mut p = PagePool::default();
        p.push(&pts, &[[0, 1, 2]], 0, 0);
        let b = p.build();
        let step = b.quant.step;
        for (a, d) in pts.iter().zip(&b.page.decoded[0]) {
            for k in 0..3 {
                assert!(
                    (a[k] - d[k]).abs() <= step * 0.5 + 1e-5,
                    "{a:?} {d:?} step {step}"
                );
            }
        }
        assert_eq!(
            b.visible[0].cluster_slot,
            helio_vg_format::cluster_slot(0, 0)
        );
    }

    #[test]
    fn single_point_cluster() {
        let mut p = PagePool::default();
        p.push(&[[7.0, -1.0, 0.5]], &[[0, 0, 0]], 0, 0);
        let b = p.build();
        assert_eq!(b.page.decoded[0][0], [7.0, -1.0, 0.5]);
    }
}
