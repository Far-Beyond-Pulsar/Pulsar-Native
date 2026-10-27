//! The process's own address space, summarised by region type and protection.
//!
//! The heap tracker sees only what goes through Rust's allocator. A process
//! that "uses 12 GB" can be mostly driver-mapped GPU memory, which never does;
//! this is the view that says so. On Windows the walk is `VirtualQuery` over the
//! whole address space (what VMMap does); elsewhere it is empty.

/// What backs a committed region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionKind {
    /// Allocated by the process or a driver (heap, `VirtualAlloc`, GPU memory).
    Private,
    /// A mapped file or shared section.
    Mapped,
    /// A loaded executable or library.
    Image,
}

impl RegionKind {
    pub fn label(self) -> &'static str {
        match self {
            RegionKind::Private => "Private",
            RegionKind::Mapped => "Mapped",
            RegionKind::Image => "Image",
        }
    }
}

/// One committed region as the OS reports it.
#[derive(Clone, Copy, Debug)]
pub struct RawRegion {
    pub base: usize,
    pub bytes: u64,
    pub kind: RegionKind,
    /// Win32 `PAGE_*` protection flags.
    pub protect: u32,
}

const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE: u32 = 0x10;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
const PAGE_GUARD: u32 = 0x100;
const PAGE_NOCACHE: u32 = 0x200;
const PAGE_WRITECOMBINE: u32 = 0x400;

/// Write-combined memory is how GPU-visible memory (VRAM windows, upload
/// heaps) appears in the process: ordinary heap is never write-combined.
pub fn is_write_combined(protect: u32) -> bool {
    protect & PAGE_WRITECOMBINE != 0
}

/// A short human description of Win32 protection flags.
pub fn describe_protection(protect: u32) -> String {
    let base = match protect & 0xFF {
        PAGE_NOACCESS => "no access",
        PAGE_READONLY => "read",
        PAGE_READWRITE => "read/write",
        PAGE_WRITECOPY => "copy-on-write",
        PAGE_EXECUTE => "execute",
        PAGE_EXECUTE_READ => "execute/read",
        PAGE_EXECUTE_READWRITE => "execute/read/write",
        PAGE_EXECUTE_WRITECOPY => "execute/copy-on-write",
        _ => "other",
    };
    let mut text = base.to_string();
    if protect & PAGE_GUARD != 0 {
        text.push_str(" +guard");
    }
    if protect & PAGE_NOCACHE != 0 {
        text.push_str(" +no-cache");
    }
    if protect & PAGE_WRITECOMBINE != 0 {
        text.push_str(" +write-combined (GPU-visible)");
    }
    text
}

/// Committed bytes of one kind and protection.
#[derive(Clone, Debug)]
pub struct RegionTotal {
    pub kind: RegionKind,
    pub protection: String,
    pub write_combined: bool,
    pub bytes: u64,
    pub regions: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessMemory {
    pub totals: Vec<RegionTotal>,
    /// The largest committed regions, biggest first.
    pub largest: Vec<RawRegion>,
    pub committed: u64,
    pub private: u64,
    pub write_combined: u64,
}

/// Regions at least this big are listed individually.
pub const LARGE_REGION: u64 = 32 * 1024 * 1024;
const LARGEST_KEPT: usize = 64;

/// Summarise `regions`.
pub fn summarize(regions: impl IntoIterator<Item = RawRegion>) -> ProcessMemory {
    let mut totals: std::collections::HashMap<(RegionKind, u32), RegionTotal> = Default::default();
    let mut memory = ProcessMemory::default();
    for region in regions {
        memory.committed += region.bytes;
        if region.kind == RegionKind::Private {
            memory.private += region.bytes;
        }
        if is_write_combined(region.protect) {
            memory.write_combined += region.bytes;
        }
        if region.bytes >= LARGE_REGION {
            memory.largest.push(region);
        }
        let total = totals
            .entry((region.kind, region.protect))
            .or_insert_with(|| RegionTotal {
                kind: region.kind,
                protection: describe_protection(region.protect),
                write_combined: is_write_combined(region.protect),
                bytes: 0,
                regions: 0,
            });
        total.bytes += region.bytes;
        total.regions += 1;
    }
    memory.totals = totals.into_values().collect();
    memory.totals.sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes));
    memory
        .largest
        .sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes));
    memory.largest.truncate(LARGEST_KEPT);
    memory
}

/// Walk this process's address space. Blocking; call it off the UI thread.
#[cfg(windows)]
pub fn scan() -> ProcessMemory {
    use windows::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEM_PRIVATE,
    };

    let mut regions = Vec::new();
    let mut address = 0usize;
    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `info` is a valid out-buffer of the size passed.
        let written = unsafe {
            VirtualQuery(
                Some(address as *const _),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 {
            break;
        }
        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        if info.State == MEM_COMMIT {
            let kind = match info.Type {
                MEM_PRIVATE => Some(RegionKind::Private),
                MEM_MAPPED => Some(RegionKind::Mapped),
                MEM_IMAGE => Some(RegionKind::Image),
                _ => None,
            };
            if let Some(kind) = kind {
                regions.push(RawRegion {
                    base,
                    bytes: size as u64,
                    kind,
                    protect: info.Protect.0,
                });
            }
        }
        match base.checked_add(size) {
            Some(next) if next > address => address = next,
            _ => break,
        }
    }
    summarize(regions)
}

#[cfg(not(windows))]
pub fn scan() -> ProcessMemory {
    ProcessMemory::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn write_combined_regions_are_counted_apart_from_heap() {
        let memory = summarize([
            RawRegion {
                base: 0x1000,
                bytes: 256 * MB,
                kind: RegionKind::Private,
                protect: 0x404,
            },
            RawRegion {
                base: 0x2000,
                bytes: 64 * MB,
                kind: RegionKind::Private,
                protect: 0x04,
            },
            RawRegion {
                base: 0x3000,
                bytes: 4 * MB,
                kind: RegionKind::Image,
                protect: 0x20,
            },
        ]);
        assert_eq!(memory.committed, 324 * MB);
        assert_eq!(memory.private, 320 * MB);
        assert_eq!(memory.write_combined, 256 * MB);
        assert_eq!(memory.largest.len(), 2, "only regions of 32 MB or more");
        assert_eq!(memory.largest[0].bytes, 256 * MB);
        assert!(memory.totals[0].write_combined);
    }

    #[test]
    fn protection_flags_read_as_words() {
        assert_eq!(describe_protection(0x04), "read/write");
        assert!(describe_protection(0x404).contains("write-combined"));
        assert!(describe_protection(0x104).contains("guard"));
    }

    #[cfg(windows)]
    #[test]
    fn scanning_this_process_finds_committed_memory() {
        let block = vec![1u8; 8 * 1024 * 1024];
        let memory = scan();
        assert!(memory.committed > block.len() as u64);
        assert!(memory.private > 0);
        std::hint::black_box(&block);
    }
}
