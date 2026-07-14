// M2a §3: slot-indexed SSBO mirroring HandleRegistry::generations()

/// The lone **slot-indexed** buffer (M2a §3): mirrors
/// `HandleRegistry::generations()` so the GPU validates handles against VRAM
/// exclusively (C6). Sized to max slots ever allocated — can exceed live
/// count after churn; `u32::MAX` tombstones upload as-is and are never
/// reissued.
pub struct GenerationBuffer {
    buf: wgpu::Buffer,
    max_slots: u32,
}

impl GenerationBuffer {
    pub fn new(device: &wgpu::Device, max_slots: u32) -> Self {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scenedb-generations"),
            size: max_slots as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { buf, max_slots }
    }

    /// Retirement write: the new generation must land here BEFORE the slot
    /// returns to the free pool (C6) — `GpuStore::retire` owns that ordering.
    pub fn write(&self, queue: &wgpu::Queue, slot: u32, generation: u32) {
        assert!(slot < self.max_slots, "slot {slot} beyond generation-buffer capacity {}", self.max_slots);
        queue.write_buffer(&self.buf, slot as u64 * 4, &generation.to_le_bytes());
    }

    /// Bulk upload from the CPU-authoritative registry (init / Test 14).
    pub fn rebuild(&self, queue: &wgpu::Queue, generations: &[u32]) {
        assert!(generations.len() as u32 <= self.max_slots);
        queue.write_buffer(&self.buf, 0, super::as_bytes(generations));
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    pub fn max_slots(&self) -> u32 {
        self.max_slots
    }
}
