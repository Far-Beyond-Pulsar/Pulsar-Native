//! Memory Explorer: where the process's memory is, by owner.
//!
//! Three views over three sources that no single tool covers:
//!
//! - **Heap** — live allocations of at least a threshold, grouped by call stack,
//!   from the allocation tracker. Click a row for the whole stack.
//! - **GPU** — each device's allocations by name, and how full its memory
//!   blocks are, from wgpu's allocator report.
//! - **Process** — committed address space by type and protection. Write-combined
//!   regions are GPU-visible memory, which the heap tracker cannot see.
//!
//! Tracking is off by default (it captures a backtrace per large allocation).
//! The GPU and Process views work without it.

use crate::utils::gpu_alloc_report::{self, GpuReport};
use crate::utils::memory_report;
use crate::utils::large_allocs::{self, LargeSnapshot};
use crate::utils::process_memory::{self, ProcessMemory, RegionKind};
use crate::utils::tracking_allocator::{disable_tracking, enable_tracking, is_tracking_active};
use gpui::*;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{
    dock::{Panel, PanelEvent},
    h_flex, v_flex, ActiveTheme,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Heap,
    Gpu,
    Process,
}

#[derive(Default)]
struct Data {
    large: LargeSnapshot,
    gpu: GpuReport,
    process: ProcessMemory,
}

/// Thresholds offered in the toolbar, in MiB.
const THRESHOLDS_MIB: [usize; 4] = [4, 16, 64, 256];

pub struct MemoryExplorerPanel {
    focus_handle: FocusHandle,
    view: View,
    data: Arc<RwLock<Data>>,
    expanded: HashSet<u64>,
    /// When the panel last drew; the refresh loop sleeps while it is hidden.
    last_render: Instant,
    /// What the last export did, shown in the toolbar.
    status: Arc<RwLock<String>>,
    _refresh_task: Task<()>,
}

impl MemoryExplorerPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = Arc::new(RwLock::new(Data::default()));
        let refresh = cx.spawn(async move |this, cx| loop {
            smol::Timer::after(Duration::from_secs(1)).await;
            let state = cx
                .update(|cx| {
                    this.upgrade().map(|this| {
                        this.update(cx, |panel, _| {
                            (
                                panel.view,
                                panel.data.clone(),
                                panel.last_render.elapsed() < Duration::from_secs(3),
                            )
                        })
                    })
                });
            let Some((view, data, visible)) = state else {
                break;
            };
            if !visible {
                continue;
            }
            cx.background_executor()
                .spawn(async move {
                    match view {
                        View::Heap => {
                            let snapshot = large_allocs::build_snapshot();
                            data.write().large = snapshot;
                        }
                        View::Gpu => {
                            let report = gpu_alloc_report::collect();
                            data.write().gpu = report;
                        }
                        View::Process => {
                            let memory = process_memory::scan();
                            data.write().process = memory;
                        }
                    }
                })
                .await;
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |_, cx| cx.notify());
                }
            });
        });
        Self {
            focus_handle: cx.focus_handle(),
            view: View::Heap,
            data,
            expanded: HashSet::new(),
            last_render: Instant::now(),
            status: Arc::new(RwLock::new(String::new())),
            _refresh_task: refresh,
        }
    }

    /// Write a Markdown report of all three views and the cache to
    /// `<project>/.pulsar/profiling/memory/`, off the UI thread.
    fn export(&mut self, cx: &mut Context<Self>) {
        let status = self.status.clone();
        *status.write() = "Exporting…".into();
        let tracking = is_tracking_active();
        cx.spawn(async move |this, cx| {
            let message = cx
                .background_executor()
                .spawn(async move {
                    let heap = large_allocs::build_snapshot();
                    let gpu = gpu_alloc_report::collect();
                    let process = process_memory::scan();
                    let unix_time = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |since| since.as_secs());
                    let markdown = memory_report::render(&memory_report::ReportInput {
                        heap: &heap,
                        gpu: &gpu,
                        process: &process,
                        cache: pulsar_scenedb::residency_stats(),
                        derived: pulsar_scenedb::derived_stats(),
                        tracking_was_on: tracking,
                        label: "",
                        unix_time,
                    });
                    let root = engine_state::get_project_path()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(std::env::temp_dir);
                    match memory_report::write(&root, unix_time, &markdown) {
                        Ok(path) => format!("Saved {}", path.display()),
                        Err(error) => format!("Export failed: {error}"),
                    }
                })
                .await;
            *status.write() = message;
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |_, cx| cx.notify());
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    fn toggle_group(&mut self, key: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }
}

fn fmt_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_age(secs: f64) -> String {
    if secs >= 3600.0 {
        format!("{:.1} h", secs / 3600.0)
    } else if secs >= 60.0 {
        format!("{:.0} min", secs / 60.0)
    } else {
        format!("{secs:.0} s")
    }
}

impl EventEmitter<PanelEvent> for MemoryExplorerPanel {}

ui_common::panel_boilerplate!(MemoryExplorerPanel);

impl Panel for MemoryExplorerPanel {
    fn panel_name(&self) -> &'static str {
        "memory_explorer"
    }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Memory Explorer".into_any_element()
    }
}

impl Render for MemoryExplorerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Instant::now();
        let theme = cx.theme().clone();
        let tracking = is_tracking_active();

        let toolbar = h_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .flex_wrap()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(self.chip("view-heap", "Heap", self.view == View::Heap, cx, |this, cx| {
                this.set_view(View::Heap, cx)
            }))
            .child(self.chip("view-gpu", "GPU", self.view == View::Gpu, cx, |this, cx| {
                this.set_view(View::Gpu, cx)
            }))
            .child(self.chip(
                "view-process",
                "Process",
                self.view == View::Process,
                cx,
                |this, cx| this.set_view(View::Process, cx),
            ))
            .child(div().w(px(12.0)))
            .child(
                div()
                    .px_3()
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(if tracking {
                        theme.success.opacity(0.2)
                    } else {
                        theme.muted.opacity(0.3)
                    })
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if tracking {
                        theme.success
                    } else {
                        theme.muted_foreground
                    })
                    .cursor_pointer()
                    .id("tracking-toggle")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            if is_tracking_active() {
                                disable_tracking();
                            } else {
                                enable_tracking();
                            }
                            cx.notify();
                        }),
                    )
                    .child(if tracking {
                        "Heap tracking: ON"
                    } else {
                        "Heap tracking: OFF"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("record from"),
            )
            .children(THRESHOLDS_MIB.iter().map(|&mib| {
                let active = large_allocs::threshold() == mib * 1024 * 1024;
                self.chip(
                    SharedString::from(format!("threshold-{mib}")),
                    format!("{mib} MiB"),
                    active,
                    cx,
                    move |_, cx| {
                        large_allocs::set_threshold(mib * 1024 * 1024);
                        cx.notify();
                    },
                )
            }))
            .child(self.chip("clear", "Clear", false, cx, |_, cx| {
                large_allocs::clear();
                cx.notify();
            }))
            .child(self.chip("export", "Export report", false, cx, |this, cx| {
                this.export(cx)
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.status.read().clone()),
            );

        let body = match self.view {
            View::Heap => self.render_heap(tracking, cx),
            View::Gpu => self.render_gpu(cx),
            View::Process => self.render_process(cx),
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(toolbar)
            .child(
                div()
                    .id("memory-explorer-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

impl MemoryExplorerPanel {
    fn chip(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id(id)
            .px_2()
            .py(px(3.0))
            .rounded(px(4.0))
            .text_xs()
            .cursor_pointer()
            .bg(if active {
                theme.accent.opacity(0.25)
            } else {
                theme.muted.opacity(0.2)
            })
            .text_color(if active {
                theme.accent
            } else {
                theme.muted_foreground
            })
            .hover(|s| s.bg(theme.accent.opacity(0.15)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            )
            .child(label.into())
    }

    fn stat(&self, label: &str, value: String, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .px_3()
            .py_1()
            .rounded(px(4.0))
            .bg(theme.muted.opacity(0.15))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(value),
            )
    }

    fn render_heap(&mut self, tracking: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let data = self.data.read();
        let snapshot = &data.large;

        let summary = h_flex()
            .gap_2()
            .p_3()
            .flex_wrap()
            .child(self.stat("Live (≥ threshold)", fmt_bytes(snapshot.live_bytes), cx))
            .child(self.stat("Blocks live", snapshot.live_count.to_string(), cx))
            .child(self.stat("Peak", fmt_bytes(snapshot.peak_bytes), cx))
            .child(self.stat(
                "Allocated / freed",
                format!("{} / {}", snapshot.total_allocs, snapshot.total_frees),
                cx,
            ))
            .child(self.stat("Threshold", fmt_bytes(snapshot.threshold.max(large_allocs::threshold()) as u64), cx))
            .child({
                // SceneDB's budgeted cache of rebuildable data (decoded
                // images): resident bytes stay within the budget, and
                // evictions/rebuilds show the cache working.
                let cache = pulsar_scenedb::residency_stats();
                self.stat(
                    "Rebuildable-data cache",
                    format!(
                        "{} / {} · {} values · {} evicted · {} rebuilt · {} prefetched · {} joined",
                        fmt_bytes(cache.resident_bytes as u64),
                        fmt_bytes(cache.budget as u64),
                        cache.resident_values,
                        cache.evictions,
                        cache.rebuilds,
                        cache.prefetches,
                        cache.joined,
                    ),
                    cx,
                )
            })
            .child({
                // SceneDB's derived-data cache on disk: a hit is a file read,
                // a miss is a cook (decode, mips, encode).
                let derived = pulsar_scenedb::derived_stats();
                self.stat(
                    "Derived-data cache (disk)",
                    format!(
                        "{} hits · {} cooked · {} read · {} written",
                        derived.hits,
                        derived.misses,
                        fmt_bytes(derived.bytes_read),
                        fmt_bytes(derived.bytes_written),
                    ),
                    cx,
                )
            });

        let hint = (!tracking).then(|| {
            div()
                .mx_3()
                .mb_2()
                .p_2()
                .rounded(px(4.0))
                .bg(theme.warning.opacity(0.12))
                .text_xs()
                .text_color(theme.warning)
                .child(
                    "Heap tracking is off. Turn it on, then load or place the assets you want to \
                     inspect. Allocations made before tracking was enabled are not listed.",
                )
        });

        let rows: Vec<AnyElement> = snapshot
            .groups
            .iter()
            .map(|group| {
                let key = group.key;
                let open = self.expanded.contains(&key);
                let mut row = v_flex().w_full().child(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py(px(5.0))
                        .gap_2()
                        .items_center()
                        .cursor_pointer()
                        .id(SharedString::from(format!("group-{key}")))
                        .hover(|s| s.bg(theme.accent.opacity(0.1)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.toggle_group(key, cx)),
                        )
                        .child(
                            div()
                                .w(px(14.0))
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(theme.foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(group.label.clone()),
                        )
                        .child(cell(format!("{}×", group.count), theme.muted_foreground))
                        .child(cell(fmt_bytes(group.largest), theme.muted_foreground))
                        .child(cell(fmt_bytes(group.live_bytes), theme.danger))
                        .child(cell(fmt_age(group.oldest_secs), theme.muted_foreground)),
                );
                if open {
                    row = row.child(
                        v_flex()
                            .mx_3()
                            .mb_2()
                            .p_2()
                            .gap(px(1.0))
                            .rounded(px(4.0))
                            .bg(theme.muted.opacity(0.12))
                            .children(group.stack.iter().enumerate().map(|(i, frame)| {
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(if frame == &group.label {
                                        theme.accent
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(format!("{i:>2}  {frame}"))
                            }))
                            .child(
                                div()
                                    .pt_1()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!(
                                        "threads: {}",
                                        group
                                            .threads
                                            .iter()
                                            .map(|t| t.to_string())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )),
                            ),
                    );
                }
                row.into_any_element()
            })
            .collect();

        let empty = rows.is_empty().then(|| {
            div()
                .p_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No live allocations at or above the threshold.")
        });

        v_flex()
            .child(summary)
            .children(hint)
            .child(header_row(&theme, &["Call site (attributed frame)", "Count", "Largest", "Live", "Oldest"]))
            .children(rows)
            .children(empty)
            .into_any_element()
    }

    fn render_gpu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let data = self.data.read();
        let report = &data.gpu;

        if report.devices.is_empty() {
            return div()
                .p_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(if report.unsupported > 0 {
                    "This graphics backend does not provide an allocator report."
                } else {
                    "No engine GPU device has reported yet."
                })
                .into_any_element();
        }

        v_flex()
            .children(report.devices.iter().map(|device| {
                v_flex()
                    .child(
                        h_flex()
                            .gap_2()
                            .p_3()
                            .flex_wrap()
                            .child(self.stat("Device", format!("#{}", device.index), cx))
                            .child(self.stat("Allocated", fmt_bytes(device.allocated), cx))
                            .child(self.stat("Reserved", fmt_bytes(device.reserved), cx))
                            .child(self.stat("Blocks", device.blocks.len().to_string(), cx)),
                    )
                    .child(header_row(&theme, &["Allocation name", "Count", "Largest", "Total", ""]))
                    .children(device.groups.iter().map(|group| {
                        h_flex()
                            .w_full()
                            .px_3()
                            .py(px(4.0))
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(group.name.clone()),
                            )
                            .child(cell(format!("{}×", group.count), theme.muted_foreground))
                            .child(cell(fmt_bytes(group.largest), theme.muted_foreground))
                            .child(cell(fmt_bytes(group.bytes), theme.danger))
                            .child(cell(String::new(), theme.muted_foreground))
                    }))
                    .child(
                        div()
                            .px_3()
                            .pt_3()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child("Memory blocks (used of reserved)"),
                    )
                    .children(device.blocks.iter().map(|block| {
                        let used = if block.size == 0 {
                            0.0
                        } else {
                            block.used as f32 / block.size as f32
                        };
                        h_flex()
                            .w_full()
                            .px_3()
                            .py(px(3.0))
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(8.0))
                                    .rounded(px(2.0))
                                    .bg(theme.muted.opacity(0.25))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(2.0))
                                            .bg(theme.accent)
                                            .w(relative(used.clamp(0.0, 1.0))),
                                    ),
                            )
                            .child(cell(
                                format!("{} / {}", fmt_bytes(block.used), fmt_bytes(block.size)),
                                theme.muted_foreground,
                            ))
                            .child(cell(format!("{}×", block.allocations), theme.muted_foreground))
                    }))
            }))
            .into_any_element()
    }

    fn render_process(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let data = self.data.read();
        let memory = &data.process;

        if memory.committed == 0 {
            return div()
                .p_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Scanning the address space… (not available on this platform)")
                .into_any_element();
        }

        v_flex()
            .child(
                h_flex()
                    .gap_2()
                    .p_3()
                    .flex_wrap()
                    .child(self.stat("Committed", fmt_bytes(memory.committed), cx))
                    .child(self.stat("Private", fmt_bytes(memory.private), cx))
                    .child(self.stat(
                        "Write-combined (GPU-visible)",
                        fmt_bytes(memory.write_combined),
                        cx,
                    )),
            )
            .child(header_row(&theme, &["Type · protection", "Regions", "", "Size", ""]))
            .children(memory.totals.iter().take(24).map(|total| {
                h_flex()
                    .w_full()
                    .px_3()
                    .py(px(4.0))
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(if total.write_combined {
                                theme.warning
                            } else {
                                theme.foreground
                            })
                            .child(format!("{} · {}", total.kind.label(), total.protection)),
                    )
                    .child(cell(total.regions.to_string(), theme.muted_foreground))
                    .child(cell(String::new(), theme.muted_foreground))
                    .child(cell(fmt_bytes(total.bytes), theme.danger))
                    .child(cell(String::new(), theme.muted_foreground))
            }))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("Largest regions"),
            )
            .children(memory.largest.iter().map(|region| {
                h_flex()
                    .w_full()
                    .px_3()
                    .py(px(3.0))
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_family("monospace")
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{:#018x}  {}  {}",
                                region.base,
                                region.kind.label(),
                                process_memory::describe_protection(region.protect)
                            )),
                    )
                    .child(cell(fmt_bytes(region.bytes), if region.kind == RegionKind::Private {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    }))
            }))
            .into_any_element()
    }
}

fn header_row(theme: &ui::Theme, labels: &[&str]) -> impl IntoElement {
    let mut row = h_flex()
        .w_full()
        .px_3()
        .py_1()
        .gap_2()
        .bg(theme.sidebar)
        .border_y_1()
        .border_color(theme.border);
    for (i, label) in labels.iter().enumerate() {
        let cell = div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .child(label.to_string());
        row = row.child(if i == 0 {
            cell.flex_1()
        } else {
            cell.w(px(84.0))
        });
    }
    row
}

fn cell(text: String, color: Hsla) -> impl IntoElement {
    div()
        .w(px(84.0))
        .text_xs()
        .text_color(color)
        .overflow_hidden()
        .text_ellipsis()
        .child(text)
}
