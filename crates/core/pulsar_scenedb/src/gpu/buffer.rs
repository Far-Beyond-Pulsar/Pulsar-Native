use crate::page::Pod;
use std::marker::PhantomData;

/// Delta-sync instrumentation: how many `write_buffer` ranges and bytes the
/// last sync issued. The delta-minimality gates assert on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub ranges: u32,
    pub bytes: u64,
}

/// One persistent **row-indexed** scene SSBO (M2a §3/§4; M2b-α §2: dirty
/// state now lives beside the cell, in a caller-supplied `DirtyMask`).
/// Generic over the C5 element type. Allocated once at capacity; never
/// reallocates.
pub struct SceneBuffer<T: Pod> {
    buf: wgpu::Buffer,
    capacity: u32,
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
        Self {
            buf,
            capacity,
            _elem: PhantomData,
        }
    }

    /// Coalescing delta-upload of one CELL REGION (design Rev 2 §2): identical
    /// to the M2a streaming coalescer but offset by `region_base` rows, with
    /// the dirty mask supplied by the cell's `CellGpuState`. Clears the mask.
    pub fn sync_region(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        region_base: u32,
        dirty: &super::DirtyMask,
    ) -> SyncStats {
        assert!(
            region_base as u64 + cpu.len() as u64 <= self.capacity as u64,
            "region [{region_base}, +{}) exceeds SSBO capacity {} — scene buffers never reallocate",
            cpu.len(),
            self.capacity
        );
        assert!(
            dirty.capacity() as u64 >= cpu.len() as u64,
            "dirty mask smaller than the CPU slice — wrong mask for this cell"
        );
        let stride = std::mem::size_of::<T>() as u64;
        let n = cpu.len() as u32;
        let mut stats = SyncStats { ranges: 0, bytes: 0 };
        let mut run_start: Option<u32> = None;
        for row in 0..n {
            match (dirty.is_marked(row), run_start) {
                (true, None) => run_start = Some(row),
                (false, Some(start)) => {
                    self.flush(queue, cpu, region_base, start, row, stride, &mut stats);
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start {
            self.flush(queue, cpu, region_base, start, n, stride, &mut stats);
        }
        dirty.clear_all();
        stats
    }

    /// Unconditional bulk write of a region prefix (registration warm-up /
    /// device-loss rebuild). Not delta-tracked.
    pub fn write_rows(&self, queue: &wgpu::Queue, cpu: &[T], region_base: u32) {
        assert!(region_base as u64 + cpu.len() as u64 <= self.capacity as u64);
        if !cpu.is_empty() {
            queue.write_buffer(&self.buf, region_base as u64 * std::mem::size_of::<T>() as u64, super::as_bytes(cpu));
        }
    }

    fn flush(
        &self,
        queue: &wgpu::Queue,
        cpu: &[T],
        region_base: u32,
        start: u32,
        end: u32,
        stride: u64,
        stats: &mut SyncStats,
    ) {
        let bytes = super::as_bytes(&cpu[start as usize..end as usize]);
        queue.write_buffer(&self.buf, (region_base as u64 + start as u64) * stride, bytes);
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
