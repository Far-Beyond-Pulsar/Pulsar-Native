use crate::page::Pod;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

/// Delta-sync instrumentation: how many `write_buffer` ranges and bytes the
/// last sync issued. The delta-minimality gates assert on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub ranges: u32,
    pub bytes: u64,
}

/// One persistent **row-indexed** scene SSBO plus its row dirty bitmask
/// (M2a §3/§4). Generic over the C5 element type. Allocated once at capacity;
/// never reallocates.
pub struct SceneBuffer<T: Pod> {
    buf: wgpu::Buffer,
    capacity: u32,
    dirty: Vec<AtomicU64>,
    _elem: PhantomData<T>,
}

impl<T: Pod> SceneBuffer<T> {
    pub fn new(device: &wgpu::Device, label: &str, capacity: u32) -> Self {
        let size = capacity as u64 * std::mem::size_of::<T>() as u64;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let words = capacity.div_ceil(64) as usize;
        Self {
            buf,
            capacity,
            dirty: (0..words).map(|_| AtomicU64::new(0)).collect(),
            _elem: PhantomData,
        }
    }

    /// Mark a row for re-upload (writes and compaction moves). Atomic — the
    /// write window may be threaded.
    #[inline]
    pub fn mark_row_dirty(&self, row: u32) {
        debug_assert!(row < self.capacity, "row {row} beyond SSBO capacity {}", self.capacity);
        self.dirty[(row / 64) as usize].fetch_or(1u64 << (row % 64), Ordering::Relaxed);
    }

    #[inline]
    fn is_dirty(&self, row: u32) -> bool {
        self.dirty[(row / 64) as usize].load(Ordering::Relaxed) & (1u64 << (row % 64)) != 0
    }

    /// Coalesce contiguous dirty rows into minimal `write_buffer` ranges,
    /// upload from the CPU column (byte-identical layout, C5 — a straight
    /// memcpy), clear all bits. Ranges stream directly to the queue: no range
    /// list, no mid-frame heap allocation. A zero-mutation frame writes
    /// nothing. Rows ≥ `cpu.len()` (popped by compaction) are only cleared.
    pub fn sync(&self, queue: &wgpu::Queue, cpu: &[T]) -> SyncStats {
        assert!(
            cpu.len() as u32 <= self.capacity,
            "CPU column ({}) exceeds SSBO capacity ({}) — scene buffers never reallocate",
            cpu.len(),
            self.capacity
        );
        let stride = std::mem::size_of::<T>() as u64;
        let n = cpu.len() as u32;
        let mut stats = SyncStats { ranges: 0, bytes: 0 };
        let mut run_start: Option<u32> = None;
        for row in 0..n {
            match (self.is_dirty(row), run_start) {
                (true, None) => run_start = Some(row),
                (false, Some(start)) => {
                    self.flush(queue, cpu, start, row, stride, &mut stats);
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            self.flush(queue, cpu, start, n, stride, &mut stats);
        }
        for word in &self.dirty {
            word.store(0, Ordering::Relaxed);
        }
        stats
    }

    fn flush(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        start: u32,
        end: u32,
        stride: u64,
        stats: &mut SyncStats,
    ) {
        let bytes = super::as_bytes(&cpu[start as usize..end as usize]);
        queue.write_buffer(&self.buf, start as u64 * stride, bytes);
        stats.ranges += 1;
        stats.bytes += bytes.len() as u64;
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}
