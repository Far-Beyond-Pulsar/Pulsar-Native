//! GPU memory by allocation, from wgpu's allocator report.
//!
//! Every engine device can say what it has allocated from the driver: each
//! memory block and, inside it, each named allocation (`Scene Texture`, a
//! buffer's label, …). That is the only place VRAM use is visible by name; the
//! heap tracker never sees it.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct GpuBlock {
    pub size: u64,
    /// Bytes of the block that are allocated.
    pub used: u64,
    pub allocations: usize,
}

/// Allocations that share a name.
#[derive(Clone, Debug)]
pub struct GpuNameGroup {
    pub name: String,
    pub count: usize,
    pub bytes: u64,
    pub largest: u64,
}

#[derive(Clone, Debug, Default)]
pub struct GpuDeviceReport {
    pub index: usize,
    pub allocated: u64,
    pub reserved: u64,
    pub blocks: Vec<GpuBlock>,
    pub groups: Vec<GpuNameGroup>,
}

#[derive(Clone, Debug, Default)]
pub struct GpuReport {
    pub devices: Vec<GpuDeviceReport>,
    /// Devices that cannot report (the backend has no allocator report).
    pub unsupported: usize,
}

/// Convert wgpu's report into blocks and per-name groups.
pub fn from_allocator_report(index: usize, report: &wgpu::AllocatorReport) -> GpuDeviceReport {
    let blocks = report
        .blocks
        .iter()
        .map(|block| {
            let range = block.allocations.clone();
            let slice = report.allocations.get(range.clone()).unwrap_or(&[]);
            GpuBlock {
                size: block.size,
                used: slice.iter().map(|a| a.size).sum(),
                allocations: slice.len(),
            }
        })
        .collect();

    let mut by_name: HashMap<&str, GpuNameGroup> = HashMap::new();
    for allocation in &report.allocations {
        let group = by_name
            .entry(allocation.name.as_str())
            .or_insert_with(|| GpuNameGroup {
                name: if allocation.name.is_empty() {
                    "<unnamed>".into()
                } else {
                    allocation.name.clone()
                },
                count: 0,
                bytes: 0,
                largest: 0,
            });
        group.count += 1;
        group.bytes += allocation.size;
        group.largest = group.largest.max(allocation.size);
    }
    let mut groups: Vec<GpuNameGroup> = by_name.into_values().collect();
    groups.sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes));

    GpuDeviceReport {
        index,
        allocated: report.total_allocated_bytes,
        reserved: report.total_reserved_bytes,
        blocks,
        groups,
    }
}

/// Ask every engine device for its allocator report. Walks every allocation,
/// so call it off the UI thread.
pub fn collect() -> GpuReport {
    let mut report = GpuReport::default();
    for (index, device) in engine_backend::services::device_loss::watched_devices()
        .iter()
        .enumerate()
    {
        match device.generate_allocator_report() {
            Some(raw) => report.devices.push(from_allocator_report(index, &raw)),
            None => report.unsupported += 1,
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::wgt::{AllocationReport, AllocatorReport, MemoryBlockReport};

    fn allocation(name: &str, size: u64) -> AllocationReport {
        AllocationReport {
            name: name.into(),
            offset: 0,
            size,
        }
    }

    #[test]
    fn allocations_group_by_name_and_blocks_report_their_use() {
        let mb = 1024 * 1024;
        let raw = AllocatorReport {
            allocations: vec![
                allocation("Scene Texture", 64 * mb),
                allocation("Scene Texture", 64 * mb),
                allocation("Scene Texture", 64 * mb),
                allocation("", 1 * mb),
            ],
            blocks: vec![
                MemoryBlockReport {
                    size: 256 * mb,
                    allocations: 0..3,
                },
                MemoryBlockReport {
                    size: 64 * mb,
                    allocations: 3..4,
                },
            ],
            total_allocated_bytes: 193 * mb,
            total_reserved_bytes: 320 * mb,
        };
        let report = from_allocator_report(0, &raw);
        assert_eq!(report.groups[0].name, "Scene Texture");
        assert_eq!(report.groups[0].count, 3);
        assert_eq!(report.groups[0].bytes, 192 * mb);
        assert_eq!(report.groups[1].name, "<unnamed>");
        assert_eq!(report.blocks[0].used, 192 * mb);
        assert_eq!(report.blocks[0].allocations, 3);
        assert_eq!(report.reserved, 320 * mb);
    }
}
