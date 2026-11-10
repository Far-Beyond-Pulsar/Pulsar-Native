//! Mission Control - Engine monitoring and logging interface

#![warn(clippy::undocumented_unsafe_blocks)]

rust_i18n::i18n!("locales", fallback = "en");

mod components;
mod screen;
mod utils;

pub use components::log_drawer::LogDrawer;
pub use components::panels::{
    AdvancedMetricsPanel, CallerSitesPanel, GpuMetricsPanel, LogsPanel, MemoryBreakdownPanel,
    MemoryExplorerPanel, ResourceMonitorPanel, SystemInfoPanel,
};
pub use screen::MissionControlPanel;
pub use utils::atomic_memory_tracking::{AllocationEntry, SizeBucket, ATOMIC_MEMORY_COUNTERS};
pub use utils::large_allocs::{
    build_snapshot as large_allocation_snapshot, LargeGroup, LargeSnapshot,
};
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

// Mission Control's status-bar button and Tools menu entry
// (`ui_common::editor_tools`): the editor binary links this crate (it is also
// the editor's log and memory tracking), the shell names none of it.
inventory::submit! {
    ui_common::editor_tools::EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.mission_control",
        label: || rust_i18n::t!("MissionControl.Title").to_string(),
        icon: ui::IconName::Page,
        order: 20,
        open: |_, cx| {
            use gpui::UpdateGlobal as _;
            window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                registry.open("MissionControlPanel", cx)
            })
        },
        is_open: None,
        init: None,
    }
}
