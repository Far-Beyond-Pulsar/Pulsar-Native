//! Where an asset's bytes come from.
//!
//! The `.vgeo` writer lives in `helio-vg-build`; this crate only needs the
//! four things in [`VgeoSource`]. [`VgeoBytes`] implements it over a whole
//! `.vgeo` image using nothing but the section offsets in [`VgeoHeader`]:
//!
//! * hierarchy: `asset.node_count` x [`GpuHierarchyNode`] at `hierarchy_offset`
//! * group refs: `asset.group_count` x [`GpuGroupRef`] at `group_refs_offset`
//! * page `i`: `PAGE_BYTES` at `pages_offset + i * PAGE_BYTES`
//!
//! (`page_dir_offset` is not needed: pages are fixed-size and contiguous.)
//! Root pages are page ids `0..asset.root_page_count`.

use helio_vg_format::*;
use std::sync::Arc;

pub trait VgeoSource: Send + Sync + 'static {
    fn header(&self) -> VgeoHeader;
    /// The always-resident hierarchy, `header().asset.node_count` nodes.
    fn hierarchy(&self) -> Result<Vec<GpuHierarchyNode>, String>;
    /// The group-ref table, `header().asset.group_count` entries.
    fn group_refs(&self) -> Result<Vec<GpuGroupRef>, String>;
    /// Bytes of one page (at most `PAGE_BYTES`, starting with a [`PageHeader`]).
    /// May block on disk; called from the SceneDB asset loader threads.
    fn read_page(&self, page_id: u32) -> Result<Vec<u8>, String>;
    /// Pages that must be resident before the asset is usable.
    /// Asset lattice (`p = origin + q * step`), uploaded as `vg_asset_quant`.
    fn quant(&self) -> GpuAssetQuant {
        GpuAssetQuant {
            origin: [0.0; 3],
            step: 1.0,
        }
    }
    /// Per page: pages that must be resident whenever it is (asset-local ids).
    /// The cull is hole-free and overlap-free only on an upward-closed set.
    fn page_deps(&self) -> Result<Vec<Vec<u32>>, String> {
        Ok(vec![Vec::new(); self.header().asset.page_count as usize])
    }
    fn root_pages(&self) -> Vec<u32> {
        (0..self.header().asset.root_page_count).collect()
    }
}

/// Where a [`VgeoBytes`] keeps its bytes.
enum Storage {
    /// The whole file image in memory.
    Memory(Arc<[u8]>),
    /// The file on disk: the header sections are read at open or on demand and
    /// pages are read as the scheduler asks for them, so a multi-gigabyte cache
    /// costs megabytes of resident memory, not gigabytes.
    File {
        file: std::sync::Mutex<std::fs::File>,
        len: u64,
    },
}

/// A `.vgeo` file, in memory or backed by the file on disk.
pub struct VgeoBytes {
    storage: Storage,
    header: VgeoHeader,
}

impl VgeoBytes {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Result<Self, String> {
        let bytes: Arc<[u8]> = bytes.into();
        let hs = std::mem::size_of::<VgeoHeader>();
        if bytes.len() < hs {
            return Err("vgeo: truncated header".into());
        }
        let header: VgeoHeader = bytemuck::pod_read_unaligned(&bytes[..hs]);
        Self::validate(&header, bytes.len() as u64)?;
        Ok(Self {
            storage: Storage::Memory(bytes),
            header,
        })
    }

    /// Opens `path` without reading it into memory: only the header is read
    /// now; sections and pages are read when asked for.
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let mut file =
            std::fs::File::open(path).map_err(|e| format!("vgeo: {}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let hs = std::mem::size_of::<VgeoHeader>();
        let mut head = vec![0u8; hs];
        file.read_exact(&mut head)
            .map_err(|_| "vgeo: truncated header".to_string())?;
        let header: VgeoHeader = bytemuck::pod_read_unaligned(&head);
        Self::validate(&header, len)?;
        Ok(Self {
            storage: Storage::File {
                file: std::sync::Mutex::new(file),
                len,
            },
            header,
        })
    }

    fn validate(header: &VgeoHeader, len: u64) -> Result<(), String> {
        if header.magic != VGEO_MAGIC {
            return Err("vgeo: bad magic".into());
        }
        if header.version != FORMAT_VERSION {
            return Err(format!(
                "vgeo: version {} (expected {FORMAT_VERSION})",
                header.version
            ));
        }
        let a = &header.asset;
        let check = |name: &str, off: u32, n: u64| {
            if u64::from(off) + n > len {
                Err(format!("vgeo: {name} section out of bounds"))
            } else {
                Ok(())
            }
        };
        check(
            "hierarchy",
            header.hierarchy_offset,
            u64::from(a.node_count) * 96,
        )?;
        check(
            "group refs",
            header.group_refs_offset,
            u64::from(a.group_count) * 8,
        )?;
        check(
            "pages",
            header.pages_offset,
            u64::from(a.page_count) * u64::from(PAGE_BYTES),
        )
    }

    /// `n` bytes at `off`, borrowed from memory or read from the file.
    fn slice(&self, off: usize, n: usize) -> Result<std::borrow::Cow<'_, [u8]>, String> {
        match &self.storage {
            Storage::Memory(bytes) => bytes
                .get(off..off + n)
                .map(std::borrow::Cow::Borrowed)
                .ok_or_else(|| "vgeo: range out of bounds".to_string()),
            Storage::File { file, len } => {
                use std::io::{Read, Seek, SeekFrom};
                if (off + n) as u64 > *len {
                    return Err("vgeo: range out of bounds".into());
                }
                let mut buf = vec![0u8; n];
                let mut f = file
                    .lock()
                    .map_err(|_| "vgeo: file lock poisoned".to_string())?;
                f.seek(SeekFrom::Start(off as u64))
                    .map_err(|e| e.to_string())?;
                f.read_exact(&mut buf).map_err(|e| e.to_string())?;
                Ok(std::borrow::Cow::Owned(buf))
            }
        }
    }
}

impl VgeoSource for VgeoBytes {
    fn header(&self) -> VgeoHeader {
        self.header
    }
    fn hierarchy(&self) -> Result<Vec<GpuHierarchyNode>, String> {
        let off = self.header.hierarchy_offset as usize;
        let n = self.header.asset.node_count as usize;
        Ok(bytemuck::pod_collect_to_vec(&self.slice(off, n * 96)?))
    }
    fn group_refs(&self) -> Result<Vec<GpuGroupRef>, String> {
        let off = self.header.group_refs_offset as usize;
        let n = self.header.asset.group_count as usize;
        Ok(bytemuck::pod_collect_to_vec(&self.slice(off, n * 8)?))
    }
    fn quant(&self) -> GpuAssetQuant {
        let o = VGEO_META_OFFSET;
        self.slice(o, 16)
            .map(|b| bytemuck::pod_read_unaligned(&b))
            .unwrap_or(GpuAssetQuant {
                origin: [0.0; 3],
                step: 1.0,
            })
    }
    /// Reads the builder's optional CSR table (`VgeoMeta::page_deps_offset`,
    /// the last word of the 32-byte meta block at [`VGEO_META_OFFSET`]).
    fn page_deps(&self) -> Result<Vec<Vec<u32>>, String> {
        let n = self.header.asset.page_count as usize;
        let at = VGEO_META_OFFSET + 28;
        let Ok(w) = self.slice(at, 4) else {
            return Ok(vec![Vec::new(); n]);
        };
        let off = u32::from_le_bytes(w[..4].try_into().unwrap()) as usize;
        if off == 0 {
            return Ok(vec![Vec::new(); n]);
        }
        let words = |from: usize, count: usize| -> Result<Vec<u32>, String> {
            let bytes = self
                .slice(off + from * 4, count * 4)
                .map_err(|_| "vgeo: page deps out of bounds".to_string())?;
            Ok(bytes
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect())
        };
        let offsets = words(0, n + 1)?;
        let total = offsets[n] as usize;
        let items = words(n + 1, total)?;
        let mut out = Vec::with_capacity(n);
        for p in 0..n {
            let (a, b) = (offsets[p] as usize, offsets[p + 1] as usize);
            if a > b || b > total {
                return Err("vgeo: bad page deps table".into());
            }
            out.push(items[a..b].to_vec());
        }
        Ok(out)
    }
    fn read_page(&self, page_id: u32) -> Result<Vec<u8>, String> {
        if page_id >= self.header.asset.page_count {
            return Err(format!("vgeo: page {page_id} out of range"));
        }
        let off = self.header.pages_offset as usize + page_id as usize * PAGE_BYTES as usize;
        Ok(self.slice(off, PAGE_BYTES as usize)?.into_owned())
    }
}

/// Deterministic fake assets for tests (here and in pass crates).
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// An asset of `page_count` pages whose first `root_pages` are roots.
    /// Page `i` is `PAGE_BYTES` long: a valid [`PageHeader`], then every u32
    /// word after it equals `fill(content, i)` so uploads are checkable.
    pub struct FakeSource {
        pub content: u64,
        pub nodes: u32,
        pub groups: u32,
        pub pages: u32,
        pub roots: u32,
        pub reads: AtomicU32,
        pub fail_page: Option<u32>,
    }

    impl FakeSource {
        pub fn new(content: u64, pages: u32, roots: u32) -> Self {
            Self {
                content,
                nodes: 3,
                groups: 4,
                pages,
                roots,
                reads: AtomicU32::new(0),
                fail_page: None,
            }
        }
        pub fn fill(content: u64, page: u32) -> u32 {
            (content as u32).wrapping_mul(0x9e37_79b1)
                ^ page.wrapping_mul(0x85eb_ca6b)
                ^ 0xdead_0000
        }
    }

    impl VgeoSource for FakeSource {
        fn header(&self) -> VgeoHeader {
            VgeoHeader {
                magic: VGEO_MAGIC,
                version: FORMAT_VERSION,
                content_id: [self.content, 0],
                asset: GpuAssetHeader {
                    root_node: 0,
                    node_count: self.nodes,
                    group_count: self.groups,
                    page_count: self.pages,
                    cluster_count: self.pages,
                    root_page_count: self.roots,
                    triangle_count: 128,
                    flags: 0,
                    ..Default::default()
                },
                ..Default::default()
            }
        }
        fn hierarchy(&self) -> Result<Vec<GpuHierarchyNode>, String> {
            Ok((0..self.nodes)
                .map(|i| GpuHierarchyNode {
                    children: [
                        NODE_LEAF_BIT | (i % self.groups),
                        NODE_INVALID,
                        NODE_INVALID,
                        NODE_INVALID,
                    ],
                    ..Default::default()
                })
                .collect())
        }
        fn group_refs(&self) -> Result<Vec<GpuGroupRef>, String> {
            Ok((0..self.groups)
                .map(|g| GpuGroupRef::new(g % self.pages, 0, 1))
                .collect())
        }
        fn read_page(&self, page_id: u32) -> Result<Vec<u8>, String> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_page == Some(page_id) {
                return Err("injected failure".into());
            }
            let mut words = vec![Self::fill(self.content, page_id); (PAGE_BYTES / 4) as usize];
            let hdr = PageHeader {
                magic: PAGE_MAGIC,
                version: FORMAT_VERSION,
                page_id,
                cluster_count: 1,
                clusters_offset: 32,
                ..Default::default()
            };
            words[..8].copy_from_slice(bytemuck::cast_slice(&[hdr]));
            Ok(bytemuck::cast_slice(&words).to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeSource;
    use super::*;

    #[test]
    fn vgeo_bytes_parses_sections() {
        let f = FakeSource::new(7, 3, 1);
        let nodes = f.hierarchy().unwrap();
        let refs = f.group_refs().unwrap();
        let mut hdr = f.header();
        let hs = std::mem::size_of::<VgeoHeader>() as u32;
        hdr.hierarchy_offset = hs;
        hdr.group_refs_offset = hs + nodes.len() as u32 * 96;
        hdr.pages_offset = hdr.group_refs_offset + refs.len() as u32 * 8;
        let mut img = bytemuck::bytes_of(&hdr).to_vec();
        img.extend_from_slice(bytemuck::cast_slice(&nodes));
        img.extend_from_slice(bytemuck::cast_slice(&refs));
        for p in 0..3 {
            img.extend_from_slice(&f.read_page(p).unwrap());
        }
        let v = VgeoBytes::new(img.clone()).unwrap();
        assert_eq!(v.hierarchy().unwrap(), nodes);
        assert_eq!(v.group_refs().unwrap(), refs);
        assert_eq!(v.read_page(2).unwrap(), f.read_page(2).unwrap());
        assert!(v.read_page(3).is_err());
        assert_eq!(v.root_pages(), vec![0]);
        img.truncate(img.len() - 1);
        assert!(VgeoBytes::new(img).is_err());
    }
}
