//! Asset GPU store (design Rev 2 §3): write-once-at-load geometry residency.
//! Unlike the per-frame scene SSBOs (`SceneGpuStore`), assets are uploaded
//! once at load and freed only on unload — no per-frame churn — so a simple
//! first-fit byte-range suballocator with free-span coalescing is sufficient.
//! The arena retains no CPU copy of geometry; it is residency-only (the asset
//! system owns the source blobs for any future re-upload).

use super::EngineGpuContext;

/// Hard arena-exhaustion error (§8): surfaced to the caller, never a realloc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaError {
    Exhausted,
}

/// First-fit byte-range suballocator over one buffer (design Rev 2 §3):
/// whole-mesh allocations at load, frees only on asset unload — no per-frame
/// churn, so first-fit with free-span coalescing is sufficient.
struct RangeList {
    /// Sorted, non-adjacent free spans: (offset, len).
    free: Vec<(u64, u64)>,
}

impl RangeList {
    fn new(total: u64) -> Self {
        Self { free: vec![(0, total)] }
    }

    fn alloc(&mut self, len: u64, align: u64) -> Option<u64> {
        debug_assert!(align.is_power_of_two());
        for i in 0..self.free.len() {
            let (off, span) = self.free[i];
            let aligned = (off + align - 1) & !(align - 1);
            let pad = aligned - off;
            if pad + len <= span {
                // Split: [off, aligned) stays free (alignment pad),
                // [aligned+len, off+span) stays free (tail).
                let tail = span - pad - len;
                self.free.remove(i);
                if tail > 0 {
                    self.free.insert(i, (aligned + len, tail));
                }
                if pad > 0 {
                    self.free.insert(i, (off, pad));
                }
                return Some(aligned);
            }
        }
        None
    }

    fn free(&mut self, offset: u64, len: u64) {
        let idx = self.free.partition_point(|&(o, _)| o < offset);
        self.free.insert(idx, (offset, len));
        // Coalesce with next, then with previous.
        if idx + 1 < self.free.len() && self.free[idx].0 + self.free[idx].1 == self.free[idx + 1].0 {
            self.free[idx].1 += self.free[idx + 1].1;
            self.free.remove(idx + 1);
        }
        if idx > 0 && self.free[idx - 1].0 + self.free[idx - 1].1 == self.free[idx].0 {
            self.free[idx - 1].1 += self.free[idx].1;
            self.free.remove(idx);
        }
    }
}

/// Global vertex + index buffers for all resident geometry (design Rev 2 §3):
/// write-once-at-load uploads, byte-range suballocated. No CPU copy is
/// retained here — residency only; the asset system owns source blobs for
/// any future re-upload (e.g. Test 14's asset half, a later task).
pub struct GeometryArena {
    vertex: wgpu::Buffer,
    vfree: RangeList,
    index: wgpu::Buffer,
    ifree: RangeList,
}

impl GeometryArena {
    pub fn new(ctx: &EngineGpuContext, vertex_bytes: u64, index_bytes: u64) -> Self {
        let vertex = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("geometry-arena-vertex"),
            size: vertex_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let index = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("geometry-arena-index"),
            size: index_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::INDEX,
            mapped_at_creation: false,
        });
        Self {
            vertex,
            vfree: RangeList::new(vertex_bytes),
            index,
            ifree: RangeList::new(index_bytes),
        }
    }

    /// 4-byte-aligned first-fit alloc + `write_buffer`. Returns the byte
    /// offset (the design §6.1 `vertex_offset` value). No CPU copy retained.
    pub fn upload_vertices(&mut self, queue: &wgpu::Queue, bytes: &[u8]) -> Result<u32, ArenaError> {
        let offset = self.vfree.alloc(bytes.len() as u64, 4).ok_or(ArenaError::Exhausted)?;
        queue.write_buffer(&self.vertex, offset, bytes);
        Ok(offset as u32)
    }

    /// 4-byte-aligned first-fit alloc + `write_buffer`. Returns the byte
    /// offset (the design §6.1 `index_offset` value). No CPU copy retained.
    pub fn upload_indices(&mut self, queue: &wgpu::Queue, bytes: &[u8]) -> Result<u32, ArenaError> {
        let offset = self.ifree.alloc(bytes.len() as u64, 4).ok_or(ArenaError::Exhausted)?;
        queue.write_buffer(&self.index, offset, bytes);
        Ok(offset as u32)
    }

    /// Asset-unload path: return a previous `upload_vertices` range to the
    /// free list (coalesced with adjacent free spans).
    pub fn free_vertices(&mut self, offset: u32, len: u32) {
        self.vfree.free(offset as u64, len as u64);
    }

    /// Asset-unload path: return a previous `upload_indices` range to the
    /// free list (coalesced with adjacent free spans).
    pub fn free_indices(&mut self, offset: u32, len: u32) {
        self.ifree.free(offset as u64, len as u64);
    }

    pub fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex
    }

    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_returns_disjoint_offsets_from_a_single_span() {
        let mut r = RangeList::new(1024);
        let a = r.alloc(100, 4).unwrap();
        let b = r.alloc(200, 4).unwrap();
        assert_eq!(a, 0, "first alloc starts at 0");
        assert_eq!(b, 100, "second alloc packed right after the first");
    }

    #[test]
    fn alignment_padding_is_inserted_as_free_space() {
        let mut r = RangeList::new(1024);
        let a = r.alloc(10, 4).unwrap(); // offset 0, consumes [0,10)
        assert_eq!(a, 0);
        // Next alloc at align 16 must skip to 16, leaving [10,16) as a
        // reclaimable pad rather than being silently lost.
        let b = r.alloc(8, 16).unwrap();
        assert_eq!(b, 16, "aligned alloc skips the pad rather than starting at 10");
        // A small alloc that fits exactly in the [10,16) pad must succeed,
        // proving the pad was tracked as free space (not leaked).
        let c = r.alloc(6, 1).unwrap();
        assert_eq!(c, 10, "pad space is still allocatable");
    }

    #[test]
    fn coalescing_merges_both_neighbors_on_free() {
        let mut r = RangeList::new(300);
        let a = r.alloc(100, 1).unwrap(); // [0,100)
        let b = r.alloc(100, 1).unwrap(); // [100,200)
        let c = r.alloc(100, 1).unwrap(); // [200,300)
        assert_eq!((a, b, c), (0, 100, 200));
        r.free(a, 100);
        r.free(c, 100);
        // Freeing the middle span must coalesce with BOTH neighbors into one
        // [0,300) span — provable by a single alloc of the full size.
        r.free(b, 100);
        let whole = r.alloc(300, 1);
        assert_eq!(whole, Some(0), "all three adjacent frees coalesced into one span");
    }

    #[test]
    fn exhausted_arena_returns_none() {
        let mut r = RangeList::new(16);
        assert!(r.alloc(16, 1).is_some());
        assert_eq!(r.alloc(1, 1), None, "no space left");
    }

    #[test]
    fn free_then_realloc_reuses_the_space() {
        let mut r = RangeList::new(64);
        let a = r.alloc(32, 1).unwrap();
        r.free(a, 32);
        let b = r.alloc(32, 1).unwrap();
        assert_eq!(a, b, "freed space reused by the next alloc of the same size");
    }
}
