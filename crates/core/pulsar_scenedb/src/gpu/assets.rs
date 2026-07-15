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
        debug_assert!(
            self.free.iter().all(|&(o, l)| offset + len <= o || o + l <= offset),
            "double-free or overlapping free range"
        );
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
        debug_assert!(offset <= u32::MAX as u64, "arena offset exceeds the u32 C5 contract");
        queue.write_buffer(&self.vertex, offset, bytes);
        Ok(offset as u32)
    }

    /// 4-byte-aligned first-fit alloc + `write_buffer`. Returns the byte
    /// offset (the design §6.1 `index_offset` value). No CPU copy retained.
    pub fn upload_indices(&mut self, queue: &wgpu::Queue, bytes: &[u8]) -> Result<u32, ArenaError> {
        let offset = self.ifree.alloc(bytes.len() as u64, 4).ok_or(ArenaError::Exhausted)?;
        debug_assert!(offset <= u32::MAX as u64, "arena offset exceeds the u32 C5 contract");
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

/// C5 (§6.1): 72-byte mesh metadata record, mirrored 1:1 into the
/// mesh-configurator SSBO. Field order/offsets are load-bearing (see comment
/// column below and the const size assert) — if the size assert ever fails,
/// fix the field order/types, never insert manual padding fields.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshMetadata {
    pub vertex_offset: u32,          // 0
    pub index_offset: u32,           // 4
    pub index_count: u32,            // 8
    pub base_vertex: i32,            // 12
    pub material_index: u32,         // 16
    pub lod_count: u32,              // 20
    pub lod_distances: [f32; 4],     // 24
    pub local_aabb_center: [f32; 3], // 40
    pub cluster_table_offset: u32,   // 52
    pub local_aabb_extents: [f32; 3], // 56
    pub meshlet_count: u32,          // 68
} // = 72 bytes (C5/§6.1)
const _: () = assert!(std::mem::size_of::<MeshMetadata>() == 72);
// SAFETY: `MeshMetadata` is `#[repr(C)]`, `Copy`, every field is itself POD
// (u32/i32/f32 and fixed-size arrays thereof), and the const assert above
// pins the layout to exactly 72 bytes with no hidden padding — matching the
// mesh-configurator SSBO stride byte-for-byte. `Pod` is a marker trait
// (`unsafe trait Pod: Copy {}`) with no methods, so this impl only asserts
// the bit-pattern/layout claim, not any behavior. This impl lives here
// (gpu-gated `assets.rs`), NOT in the graphics-free core (`page.rs`).
unsafe impl crate::page::Pod for MeshMetadata {}

/// Hard mesh-registry errors (§8): surfaced to the caller, never silently
/// coerced or retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshError {
    /// C5 XOR rule: exactly one of `{lod_count, cluster_table_offset}` must
    /// be non-zero — a traditional mesh carries an LOD distance chain, a
    /// virtualized-geometry (VG) mesh carries a cluster table, and a mesh
    /// can't be both or neither.
    XorRule,
    RegistryFull,
}

/// Flat host registry mirrored 1:1 into the mesh-configurator SSBO (design
/// Rev 2 §6.1): registry index `i` is always uploaded at byte offset `i *
/// 72` in `buf`. Append-only for M2b-α — no CPU free list (unregister is out
/// of scope here).
pub struct MeshRegistry {
    buf: wgpu::Buffer,
    entries: Vec<MeshMetadata>,
    max_meshes: u32,
}

impl MeshRegistry {
    pub fn new(ctx: &EngineGpuContext, max_meshes: u32) -> Self {
        let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-registry"),
            size: max_meshes as u64 * 72,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { buf, entries: Vec::new(), max_meshes }
    }

    /// C5 XOR rule: exactly one of `{lod_count, cluster_table_offset}` must
    /// be non-zero (both zero and both non-zero are equally an error). On
    /// success, uploads ONLY the new 72-byte entry — never a bulk re-upload.
    pub fn register(&mut self, queue: &wgpu::Queue, m: MeshMetadata) -> Result<u32, MeshError> {
        if (m.lod_count != 0) == (m.cluster_table_offset != 0) {
            return Err(MeshError::XorRule);
        }
        if self.entries.len() as u32 >= self.max_meshes {
            return Err(MeshError::RegistryFull);
        }
        let index = self.entries.len() as u32;
        queue.write_buffer(&self.buf, index as u64 * 72, super::as_bytes(std::slice::from_ref(&m)));
        self.entries.push(m);
        Ok(index)
    }

    pub fn get(&self, mesh_index: u32) -> &MeshMetadata {
        &self.entries[mesh_index as usize]
    }

    pub fn len(&self) -> u32 {
        self.entries.len() as u32
    }

    /// Test 14 rebuild source: the CPU-authoritative copy of every entry, in
    /// registry-index order (matches the SSBO's byte layout 1:1).
    pub fn entries(&self) -> &[MeshMetadata] {
        &self.entries
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    /// Test 14 (C0 companion gate): bulk re-upload every entry from the
    /// CPU-authoritative `entries` copy — device-loss re-materialization,
    /// same shape as `SceneGpuStore::rebuild`. No-op on an empty registry
    /// (`write_buffer` with a zero-length slice is fine, but skip the call).
    pub fn rebuild(&self, queue: &wgpu::Queue) {
        if self.entries.is_empty() {
            return;
        }
        queue.write_buffer(&self.buf, 0, super::as_bytes(&self.entries));
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

    #[test]
    #[should_panic(expected = "double-free or overlapping free range")]
    fn double_free_panics_in_debug() {
        let mut r = RangeList::new(64);
        let a = r.alloc(32, 1).unwrap();
        r.free(a, 32);
        // Same range freed twice: without the guard this silently corrupts
        // the free list into two overlapping [a, a+32) spans, and a
        // subsequent alloc would hand out overlapping (aliased) offsets.
        r.free(a, 32);
    }
}
