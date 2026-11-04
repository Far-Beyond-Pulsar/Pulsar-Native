//! Test/bench support: build a real-format page (header, cluster records,
//! position blocks on an asset lattice, index and attribute streams) from
//! plain triangle lists, so pass crates can test against the same bytes the
//! builder writes without depending on `helio-vg-build`.

use crate::*;

/// One cluster to place in a [`TestPage`].
pub struct TestCluster<'a> {
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u8; 3]],
    pub material: u32,
    /// Optional attribute rows ([`ATTR_STRIDE`] bytes per vertex). When absent
    /// no attribute bytes are stored (position-only tests keep pages small).
    pub attrs: Option<&'a [[u8; 16]]>,
}

/// A single page in pool slot `pool_slot`.
pub struct TestPage {
    /// `PAGE_BYTES` bytes.
    pub bytes: Vec<u8>,
    pub quant: GpuAssetQuant,
    pub pool_slot: u32,
    /// `cluster_slot` of each cluster, in input order.
    pub slots: Vec<u32>,
    /// Lattice-decoded positions of each cluster (what the GPU must produce).
    pub decoded: Vec<Vec<[f32; 3]>>,
}

fn put_bits(words: &mut [u32], bitpos: usize, value: u32, nbits: u32) {
    if nbits == 0 {
        return;
    }
    let (w, o) = (bitpos / 32, (bitpos % 32) as u32);
    words[w] |= value << o;
    if o + nbits > 32 {
        words[w + 1] |= value >> (32 - o);
    }
}

fn bounding_sphere(p: &[[f32; 3]]) -> [f32; 4] {
    let mut c = [0.0f32; 3];
    for v in p {
        for a in 0..3 {
            c[a] += v[a] / p.len() as f32;
        }
    }
    let r = p
        .iter()
        .map(|v| ((v[0] - c[0]).powi(2) + (v[1] - c[1]).powi(2) + (v[2] - c[2]).powi(2)).sqrt())
        .fold(0.0, f32::max);
    [c[0], c[1], c[2], r]
}

/// Decode a lattice coordinate exactly like the shader and `VgeoReader`.
pub fn dequantize(q: [u32; 3], quant: &GpuAssetQuant) -> [f32; 3] {
    [
        quant.origin[0] + q[0] as f32 * quant.step,
        quant.origin[1] + q[1] as f32 * quant.step,
        quant.origin[2] + q[2] as f32 * quant.step,
    ]
}

impl TestPage {
    pub fn build(pool_slot: u32, clusters: &[TestCluster<'_>]) -> Self {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for c in clusters {
            for p in c.positions {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
        }
        let ext = (0..3).map(|i| (hi[i] - lo[i]) as f64).fold(0.0, f64::max);
        let step = if ext > 0.0 {
            (ext * (1.0 + 1e-6) / LATTICE_MAX as f64) as f32
        } else {
            1.0
        };
        let quant = GpuAssetQuant { origin: lo, step };

        let n = clusters.len();
        assert!(n as u32 <= PAGE_CLUSTER_MASK + 1);
        let clusters_off = std::mem::size_of::<PageHeader>();
        let mut pos_blocks: Vec<Vec<u32>> = Vec::new();
        let mut attr_blocks: Vec<Vec<u8>> = Vec::new();
        let mut idx_blocks: Vec<Vec<u8>> = Vec::new();
        let mut decoded = Vec::new();
        for c in clusters {
            let q: Vec<[u32; 3]> = c
                .positions
                .iter()
                .map(|p| {
                    let mut q = [0u32; 3];
                    for i in 0..3 {
                        let v =
                            ((p[i] as f64 - quant.origin[i] as f64) / quant.step as f64).round();
                        q[i] = v.clamp(0.0, LATTICE_MAX as f64) as u32;
                    }
                    q
                })
                .collect();
            let mut mn = [u32::MAX; 3];
            let mut mx = [0u32; 3];
            for p in &q {
                for i in 0..3 {
                    mn[i] = mn[i].min(p[i]);
                    mx[i] = mx[i].max(p[i]);
                }
            }
            let bits: Vec<u32> = (0..3)
                .map(|i| 32 - (mx[i] - mn[i]).leading_zeros())
                .collect();
            let per = (bits[0] + bits[1] + bits[2]) as usize;
            let mut words = vec![0u32; POS_BLOCK_HEADER_WORDS + (per * q.len() + 31) / 32];
            words[0..3].copy_from_slice(&mn);
            words[3] = bits[0] | (bits[1] << 8) | (bits[2] << 16);
            let mut bp = 0;
            for p in &q {
                for i in 0..3 {
                    put_bits(
                        &mut words[POS_BLOCK_HEADER_WORDS..],
                        bp,
                        p[i] - mn[i],
                        bits[i],
                    );
                    bp += bits[i] as usize;
                }
            }
            pos_blocks.push(words);
            decoded.push(q.iter().map(|&q| dequantize(q, &quant)).collect());
            let mut idx: Vec<u8> = c.tris.iter().flatten().copied().collect();
            while idx.len() % 4 != 0 {
                idx.push(0);
            }
            idx_blocks.push(idx);
            let mut a = Vec::new();
            if let Some(rows) = c.attrs {
                for v in 0..c.positions.len() {
                    a.extend_from_slice(&rows[v]);
                }
            }
            attr_blocks.push(a);
        }
        let pos_off = clusters_off + n * std::mem::size_of::<GpuCluster>();
        let attr_off = pos_off + pos_blocks.iter().map(|b| b.len() * 4).sum::<usize>();
        let idx_off = attr_off + attr_blocks.iter().map(|b| b.len()).sum::<usize>();
        let end = idx_off + idx_blocks.iter().map(|b| b.len()).sum::<usize>();
        assert!(
            end <= PAGE_BYTES as usize,
            "test page overflows {PAGE_BYTES} bytes"
        );

        let mut page = vec![0u8; PAGE_BYTES as usize];
        let hdr = PageHeader {
            magic: PAGE_MAGIC,
            version: FORMAT_VERSION,
            page_id: 0,
            cluster_count: n as u32,
            clusters_offset: clusters_off as u32,
            position_stream_offset: pos_off as u32,
            attribute_stream_offset: attr_off as u32,
            index_stream_offset: idx_off as u32,
        };
        page[..clusters_off].copy_from_slice(bytemuck::bytes_of(&hdr));
        let (mut po, mut ao, mut io) = (pos_off, attr_off, idx_off);
        let mut slots = Vec::new();
        for (i, c) in clusters.iter().enumerate() {
            pos_blocks[i][4] = ao as u32;
            let pb: &[u8] = bytemuck::cast_slice(&pos_blocks[i]);
            page[po..po + pb.len()].copy_from_slice(pb);
            page[ao..ao + attr_blocks[i].len()].copy_from_slice(&attr_blocks[i]);
            page[io..io + idx_blocks[i].len()].copy_from_slice(&idx_blocks[i]);
            let sphere = bounding_sphere(c.positions);
            let rec = GpuCluster {
                sphere,
                parent_sphere: sphere,
                cone: pack_cone([0.0, 0.0, 1.0], 1.0),
                self_error: 0.0,
                parent_error: f32::INFINITY,
                counts_material: GpuCluster::pack_counts(
                    c.tris.len() as u32,
                    c.positions.len() as u32,
                    c.material,
                ),
                vertex_offset: po as u32,
                index_offset: io as u32,
                group_id: NODE_INVALID,
                flags: GpuCluster::FLAG_LEAF | GpuCluster::FLAG_ROOT,
            };
            let at = clusters_off + i * std::mem::size_of::<GpuCluster>();
            page[at..at + 64].copy_from_slice(bytemuck::bytes_of(&rec));
            po += pb.len();
            ao += attr_blocks[i].len();
            io += idx_blocks[i].len();
            slots.push(cluster_slot(pool_slot, i as u32));
        }
        // The page is placed in pool slot `pool_slot`: callers upload `bytes`
        // at `pool_slot * PAGE_BYTES` (use `pool()` for a ready buffer image).
        Self {
            bytes: page,
            quant,
            pool_slot,
            slots,
            decoded,
        }
    }

    /// Byte image of a pool whose slot `pool_slot` holds this page.
    pub fn pool(&self) -> Vec<u8> {
        let mut v = vec![0u8; (self.pool_slot as usize + 1) * PAGE_BYTES as usize];
        let o = self.pool_slot as usize * PAGE_BYTES as usize;
        v[o..].copy_from_slice(&self.bytes);
        v
    }
}
