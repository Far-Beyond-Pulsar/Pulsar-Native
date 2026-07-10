//! Mission Control - Engine monitoring and logging interface

mod components;
mod screen;
mod utils;

pub use screen::MissionControlPanel;
pub use components::log_drawer::LogDrawer;
pub use components::panels::{
    AdvancedMetricsPanel, CallerSitesPanel, GpuMetricsPanel, LogsPanel, MemoryBreakdownPanel,
    ResourceMonitorPanel, SystemInfoPanel,
};
pub use utils::atomic_memory_tracking::{AllocationEntry, SizeBucket, ATOMIC_MEMORY_COUNTERS};
pub use utils::live_logs::{publish_live_log, subscribe_live_logs};
pub use utils::memory_tracking::{
    create_memory_tracker, MemoryCategory, MemoryStatsSnapshot, MemoryTracker, SharedMemoryTracker,
};
pub use utils::performance_metrics::{
    create_shared_metrics, PerformanceMetrics, SharedPerformanceMetrics,
};
pub use utils::system_info::{create_shared_info, SharedSystemInfo, SystemInfo};
pub use utils::tracking_allocator::{
    disable_tracking, enable_tracking, is_tracking_active, MemoryCategoryGuard, TrackingAllocator,
};
pub use utils::type_tracking::{AllocationSite, TYPE_TRACKER};

pub use screen::LogViewerWindow;
