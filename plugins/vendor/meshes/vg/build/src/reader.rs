//! `.vgeo` reader and the CPU reference of the runtime DAG cut.
//!
//! The reader mirrors what `helio-vg-stream` and the GPU passes do with the
//! file, so tests can check the *serialized* data (not just the in-memory
//! DAG): header/hierarchy/group refs are parsed eagerly (they are always
//! resident at runtime), pages are decoded on demand.

use std::fmt;

use bytemuck::Pod;
use helio_vg_format::*;

use crate::file::*;

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Truncated,
    BadMagic,
    BadVersion(u32),
    BadOffsets,
    PageOutOfRange,
    BadCluster,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ParseError {}

fn read_pod<T: Pod>(b: &[u8], off: usize) -> Result<T, ParseError> {
    let end = off
        .checked_add(std::mem::size_of::<T>())
        .ok_or(ParseError::Truncated)?;
    b.get(off..end)
        .map(bytemuck::pod_read_unaligned)
        .ok_or(ParseError::Truncated)
}

fn read_vec<T: Pod>(b: &[u8], off: usize, count: usize) -> Result<Vec<T>, ParseError> {
    let sz = std::mem::size_of::<T>();
    let end = count
        .checked_mul(sz)
        .and_then(|n| n.checked_add(off))
        .ok_or(ParseError::Truncated)?;
    let s = b.get(off..end).ok_or(ParseError::Truncated)?;
    Ok(s.chunks_exact(sz)
        .map(bytemuck::pod_read_unaligned)
        .collect())
}

pub struct VgeoReader<'a> {
    bytes: &'a [u8],
    pub header: VgeoHeader,
    pub meta: VgeoMeta,
    pub nodes: Vec<GpuHierarchyNode>,
    pub group_refs: Vec<GpuGroupRef>,
    pub page_dir: Vec<PageDirEntry>,
    /// Pages that must be resident before page `i` may be (parents of its clusters).
    pub page_deps: Vec<Vec<u32>>,
}

/// One 128 KiB page.
pub struct PageView<'a> {
    bytes: &'a [u8],
    pub header: PageHeader,
    meta: VgeoMeta,
}

impl<'a> VgeoReader<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ParseError> {
        let header: VgeoHeader = read_pod(bytes, 0)?;
        if header.magic != VGEO_MAGIC {
            return Err(ParseError::BadMagic);
        }
        if header.version != FORMAT_VERSION {
            return Err(ParseError::BadVersion(header.version));
        }
        let meta: VgeoMeta = read_pod(bytes, std::mem::size_of::<VgeoHeader>())?;
        let a = &header.asset;
        let nodes = read_vec(
            bytes,
            header.hierarchy_offset as usize,
            a.node_count as usize,
        )?;
        let group_refs = read_vec(
            bytes,
            header.group_refs_offset as usize,
            a.group_count as usize,
        )?;
        let page_dir: Vec<PageDirEntry> = read_vec(
            bytes,
            header.page_dir_offset as usize,
            a.page_count as usize,
        )?;
        let pages_end = header.pages_offset as usize + a.page_count as usize * PAGE_BYTES as usize;
        if pages_end > bytes.len() || a.root_page_count > a.page_count {
            return Err(ParseError::BadOffsets);
        }
        let mut page_deps = vec![Vec::new(); a.page_count as usize];
        if meta.page_deps_offset != 0 {
            let csr: Vec<u32> = read_vec(
                bytes,
                meta.page_deps_offset as usize,
                a.page_count as usize + 1,
            )?;
            let flat: Vec<u32> = read_vec(
                bytes,
                meta.page_deps_offset as usize + csr.len() * 4,
                *csr.last().unwrap_or(&0) as usize,
            )?;
            for p in 0..a.page_count as usize {
                page_deps[p] = flat[csr[p] as usize..csr[p + 1] as usize].to_vec();
            }
        }
        Ok(Self {
            bytes,
            header,
            meta,
            nodes,
            group_refs,
            page_dir,
            page_deps,
        })
    }

    pub fn page(&self, id: u32) -> Result<PageView<'a>, ParseError> {
        let e = self
            .page_dir
            .get(id as usize)
            .ok_or(ParseError::PageOutOfRange)?;
        let off = e.file_offset as usize;
        let bytes = self
            .bytes
            .get(off..off + PAGE_BYTES as usize)
            .ok_or(ParseError::Truncated)?;
        let header: PageHeader = read_pod(bytes, 0)?;
        if header.magic != PAGE_MAGIC {
            return Err(ParseError::BadMagic);
        }
        Ok(PageView {
            bytes,
            header,
            meta: self.meta,
        })
    }

    /// Clusters of a hierarchy leaf.
    pub fn group_clusters(&self, group: u32) -> Result<Vec<GpuCluster>, ParseError> {
        let r = self
            .group_refs
            .get(group as usize)
            .ok_or(ParseError::BadOffsets)?;
        let page = self.page(r.page_id)?;
        (r.first_cluster()..r.first_cluster() + r.cluster_count())
            .map(|i| page.cluster(i))
            .collect()
    }

    /// CPU reference of the runtime cut: walk the hierarchy, enter a child iff
    /// its worst `parent_error` still projects above `tau`, and emit every
    /// cluster with `project(self) <= tau && project(parent) > tau`. No
    /// frustum or occlusion tests; this is the pure LOD decision.
    pub fn select_clusters(
        &self,
        tau: f32,
        proj: &Projection,
    ) -> Result<Vec<SelectedCluster>, ParseError> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return Ok(out);
        }
        let mut stack = vec![self.header.asset.root_node];
        while let Some(n) = stack.pop() {
            let node = self.nodes.get(n as usize).ok_or(ParseError::BadOffsets)?;
            for i in 0..NODE_FANOUT {
                let child = node.children[i];
                if child == NODE_INVALID {
                    continue;
                }
                if proj.project(node.max_parent_error[i], node.spheres[i]) <= tau {
                    continue;
                }
                if child & NODE_LEAF_BIT == 0 {
                    stack.push(child);
                    continue;
                }
                let g = self
                    .group_refs
                    .get((child & !NODE_LEAF_BIT) as usize)
                    .ok_or(ParseError::BadOffsets)?;
                let page = self.page(g.page_id)?;
                for k in g.first_cluster()..g.first_cluster() + g.cluster_count() {
                    let c = page.cluster(k)?;
                    if proj.project(c.self_error, c.sphere) <= tau
                        && proj.project(c.parent_error, c.parent_sphere) > tau
                    {
                        out.push(SelectedCluster {
                            page_id: g.page_id,
                            cluster_index: k,
                            cluster: c,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn decode(&self, sel: &SelectedCluster) -> Result<DecodedCluster<'a>, ParseError> {
        self.page(sel.page_id)?.decode_cluster(sel.cluster_index)
    }
}

impl<'a> PageView<'a> {
    pub fn cluster(&self, i: u32) -> Result<GpuCluster, ParseError> {
        if i >= self.header.cluster_count {
            return Err(ParseError::BadCluster);
        }
        read_pod(
            self.bytes,
            self.header.clusters_offset as usize + i as usize * std::mem::size_of::<GpuCluster>(),
        )
    }

    pub fn clusters(&self) -> Result<Vec<GpuCluster>, ParseError> {
        (0..self.header.cluster_count)
            .map(|i| self.cluster(i))
            .collect()
    }

    pub fn decode_cluster(&self, i: u32) -> Result<DecodedCluster<'a>, ParseError> {
        let c = self.cluster(i)?;
        let (nv, nt) = (c.vert_count() as usize, c.tri_count() as usize);
        let po = c.vertex_offset as usize;
        let w = |k: usize| -> Result<u32, ParseError> { read_pod(self.bytes, po + k * 4) };
        let anchor = [w(0)?, w(1)?, w(2)?];
        let packed = w(3)?;
        let bits = [packed & 0xff, (packed >> 8) & 0xff, (packed >> 16) & 0xff];
        let attr_off = w(4)? as usize;
        let per_vert = (bits[0] + bits[1] + bits[2]) as usize;
        let total_words = (per_vert * nv + 31) / 32;
        let stream: Vec<u32> = read_vec(self.bytes, po + POS_BLOCK_HEADER_WORDS * 4, total_words)?;
        let get = |bitpos: usize, n: u32| -> u32 {
            if n == 0 {
                return 0;
            }
            let (wi, o) = (bitpos / 32, (bitpos % 32) as u32);
            let mut v = stream[wi] >> o;
            if o + n > 32 {
                v |= stream[wi + 1] << (32 - o);
            }
            if n < 32 {
                v &= (1 << n) - 1;
            }
            v
        };
        let mut lattice = Vec::with_capacity(nv);
        let mut positions = Vec::with_capacity(nv);
        let mut bitpos = 0;
        for _ in 0..nv {
            let mut q = [0u32; 3];
            let mut p = [0.0f32; 3];
            for a in 0..3 {
                q[a] = anchor[a] + get(bitpos, bits[a]);
                bitpos += bits[a] as usize;
                p[a] = self.meta.origin[a] + q[a] as f32 * self.meta.step;
            }
            lattice.push(q);
            positions.push(p);
        }
        let io = c.index_offset as usize;
        let raw = self
            .bytes
            .get(io..io + nt * 3)
            .ok_or(ParseError::Truncated)?;
        let tris = raw.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
        let stride = self.meta.attribute_stride as usize;
        let attributes = self
            .bytes
            .get(attr_off..attr_off + nv * stride)
            .ok_or(ParseError::Truncated)?;
        Ok(DecodedCluster {
            lattice,
            positions,
            tris,
            attributes,
        })
    }
}
