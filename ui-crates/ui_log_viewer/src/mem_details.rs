//! Extended memory statistics beyond what sysinfo provides.
//! Uses `GetPerformanceInfo` (Windows) for cache, paged/non-paged pools,
//! and committed memory. Falls back to zeroes on other platforms.

/// Snapshot of detailed system memory state.
#[derive(Clone, Default, Debug)]
pub struct MemorySnapshot {
    /// Total physical RAM in MiB (from sysinfo).
    pub total_mb: u64,
    /// Available physical RAM in MiB (from sysinfo).
    pub available_mb: u64,
    /// RAM actively in use in MiB (total - available).
    pub in_use_mb: u64,
    /// System file cache in MiB.
    pub cached_mb: Option<u64>,
    /// Virtual memory currently committed in MiB.
    pub committed_mb: Option<u64>,
    /// System commit limit in MiB (RAM + all page files).
    pub committed_limit_mb: Option<u64>,
    /// Paged kernel pool in MiB.
    pub paged_pool_mb: Option<u64>,
    /// Non-paged kernel pool in MiB.
    pub non_paged_pool_mb: Option<u64>,
    /// Total page/swap file capacity in MiB (from sysinfo).
    pub swap_total_mb: u64,
    /// Page/swap file currently used in MiB (from sysinfo).
    pub swap_used_mb: u64,
}

/// Collect an extended memory snapshot.
pub fn collect(system: &sysinfo::System) -> MemorySnapshot {
    let total_mb   = system.total_memory()     / (1024 * 1024);
    let avail_mb   = system.available_memory()  / (1024 * 1024);
    let in_use_mb  = total_mb.saturating_sub(avail_mb);
    let swap_total = system.total_swap()        / (1024 * 1024);
    let swap_used  = system.used_swap()         / (1024 * 1024);

    let mut snap = MemorySnapshot {
        total_mb: total_mb,
        available_mb: avail_mb,
        in_use_mb: in_use_mb,
        swap_total_mb: swap_total,
        swap_used_mb: swap_used,
        ..Default::default()
    };

    #[cfg(target_os = "windows")]
    windows_collect(&mut snap);

    snap
}

#[cfg(target_os = "windows")]
fn windows_collect(snap: &mut MemorySnapshot) {
    use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};

    unsafe {
        let mut pi = PERFORMANCE_INFORMATION::default();
        let cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        if GetPerformanceInfo(&mut pi, cb).is_ok() {
            let ps = pi.PageSize as u64;
            snap.cached_mb        = Some(pi.SystemCache    as u64 * ps / (1024 * 1024));
            snap.committed_mb     = Some(pi.CommitTotal    as u64 * ps / (1024 * 1024));
            snap.committed_limit_mb = Some(pi.CommitLimit  as u64 * ps / (1024 * 1024));
            snap.paged_pool_mb    = Some(pi.KernelPaged    as u64 * ps / (1024 * 1024));
            snap.non_paged_pool_mb = Some(pi.KernelNonpaged as u64 * ps / (1024 * 1024));
        }
    }
}
