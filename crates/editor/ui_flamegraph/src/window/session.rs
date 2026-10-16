//! The loaded-session screen: the dock layout, its toolbar, exports and the
//! saved dock sizes.

use super::FlamegraphWindow;
use crate::components::StatusBar;
use crate::export;
use crate::flamegraph_view::SpanSelected;
use crate::format::format_duration;
use crate::{CallstackPanel, FlamegraphPanel, ScriptsPanel, StatisticsPanel};
use gpui::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockChannel, DockEvent, DockItem, DockPlacement, PanelStyle, PanelView},
    h_flex, ActiveTheme, Icon, IconName,
};

/// What the docks start with when nothing was saved.
const DEFAULT_BOTTOM: f32 = 300.0;
const DEFAULT_RIGHT: f32 = 300.0;
/// Quiet time before a layout change is written out.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

pub(super) struct SessionUi {
    pub(super) dock: Entity<DockArea>,
    pub(super) status_bar: Entity<StatusBar>,
    _dock_events: Subscription,
    _selection: Subscription,
}

#[derive(Clone, Copy)]
pub(super) enum ExportKind {
    StatsCsv,
    ChromeTrace,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct SavedDock {
    size: f32,
    open: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct SavedLayout {
    bottom: Option<SavedDock>,
    right: Option<SavedDock>,
}

fn layout_file() -> Option<PathBuf> {
    engine_state::get_project_path()
        .map(|project| PathBuf::from(project).join(".pulsar/flamegraph_layout.json"))
}

fn load_layout() -> SavedLayout {
    layout_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

impl FlamegraphWindow {
    /// Create the panels and dock them: the timeline in the middle, the
    /// analysis tabs below, the call stack on the right.
    pub(super) fn build_session_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = load_layout();
        let frame = self.trace_data.get_frame();
        let has_scripts = frame.spans.iter().any(|s| s.name.starts_with("script:"));

        let timeline = cx.new(|cx| FlamegraphPanel::new(self.view.clone(), cx));
        let statistics = cx.new(|cx| StatisticsPanel::new(self.trace_data.clone(), window, cx));
        let scripts = has_scripts.then(|| cx.new(|cx| ScriptsPanel::new(self.trace_data.clone(), cx)));
        let callstack = cx.new(|cx| CallstackPanel::new(self.view.clone(), cx));

        // Its own channel keeps tabs dragged here out of other docks.
        let dock = cx.new(|cx| {
            DockArea::new_with_channel("flamegraph-dock", None, DockChannel(7), window, cx)
                .panel_style(PanelStyle::TabBar)
        });
        dock.update(cx, |dock, cx| dock.set_toggle_button_visible(false, cx));
        let weak = dock.downgrade();

        let center = DockItem::tabs(
            vec![Arc::new(timeline) as Arc<dyn PanelView>],
            Some(0),
            &weak,
            window,
            cx,
        );
        let mut analysis: Vec<Arc<dyn PanelView>> = vec![Arc::new(statistics)];
        if let Some(scripts) = scripts {
            analysis.push(Arc::new(scripts));
        }
        let bottom = DockItem::tabs(analysis, Some(0), &weak, window, cx);
        let right = DockItem::tabs(
            vec![Arc::new(callstack) as Arc<dyn PanelView>],
            Some(0),
            &weak,
            window,
            cx,
        );

        let bottom_saved = saved.bottom.unwrap_or(SavedDock {
            size: DEFAULT_BOTTOM,
            open: true,
        });
        let right_saved = saved.right.unwrap_or(SavedDock {
            size: DEFAULT_RIGHT,
            open: false,
        });
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_bottom_dock(bottom, Some(px(bottom_saved.size)), bottom_saved.open, window, cx);
            dock.set_right_dock(right, Some(px(right_saved.size)), right_saved.open, window, cx);
        });

        let events = cx.subscribe_in(
            &dock,
            window,
            |this: &mut Self, _dock, event: &DockEvent, _window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.schedule_layout_save(cx);
                }
            },
        );
        // Clicking a span opens the call stack if its dock is closed.
        let selection = cx.subscribe_in(
            &self.view,
            window,
            |this: &mut Self, _view, _event: &SpanSelected, window, cx| {
                let closed = this.session_ui.as_ref().is_some_and(|ui| {
                    !ui.dock.read(cx).is_dock_open(DockPlacement::Right, cx)
                });
                if closed {
                    this.toggle_dock(DockPlacement::Right, window, cx);
                }
            },
        );
        let status_bar = cx.new(|cx| StatusBar::new(self.view.clone(), cx));
        self.session_ui = Some(SessionUi {
            dock,
            status_bar,
            _dock_events: events,
            _selection: selection,
        });
    }

    /// Write the dock sizes once the layout has been quiet for a moment.
    fn schedule_layout_save(&mut self, cx: &mut Context<Self>) {
        self.layout_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| this.save_layout(cx));
            });
        }));
    }

    fn save_layout(&mut self, cx: &mut Context<Self>) {
        let (Some(ui), Some(path)) = (&self.session_ui, layout_file()) else {
            return;
        };
        let state = ui.dock.read(cx).dump(cx);
        let saved = |dock: Option<&ui::dock::DockState>| {
            dock.map(|dock| SavedDock {
                size: f32::from(dock.size()),
                open: dock.is_open(),
            })
        };
        let layout = SavedLayout {
            bottom: saved(state.bottom_dock.as_ref()),
            right: saved(state.right_dock.as_ref()),
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&layout) {
            let _ = std::fs::write(path, text);
        }
    }

    fn toggle_dock(&mut self, placement: DockPlacement, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ui) = &self.session_ui {
            ui.dock
                .update(cx, |dock, cx| dock.toggle_dock(placement, window, cx));
            cx.notify();
        }
    }

    /// Save the statistics table or the whole trace through a file dialog.
    pub(super) fn export(&mut self, kind: ExportKind, cx: &mut Context<Self>) {
        let frame = self.trace_data.get_frame();
        let stem = self
            .current_db_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .unwrap_or("session")
            .to_string();
        let (title, name, label, extension) = match kind {
            ExportKind::StatsCsv => ("Export statistics", format!("{stem}-statistics.csv"), "CSV", "csv"),
            ExportKind::ChromeTrace => ("Export Chrome trace", format!("{stem}-trace.json"), "Chrome trace", "json"),
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title(title)
            .set_file_name(name)
            .add_filter(label, &[extension]);

        cx.spawn(async move |this, cx| {
            let Some(file) = dialog.save_file().await else {
                return;
            };
            let path = file.path().to_path_buf();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let text = match kind {
                        ExportKind::StatsCsv => export::stats_csv(&frame),
                        ExportKind::ChromeTrace => export::chrome_trace(&frame),
                    };
                    std::fs::write(&path, text).map(|_| path)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.export_status = Some(match result {
                        Ok(path) => format!("Exported {}", path.display()),
                        Err(error) => format!("Export failed: {error}"),
                    });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn render_session_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let frame = self.trace_data.get_frame();
        let theme = cx.theme();
        let (sidebar, border, muted, foreground) = (
            theme.sidebar,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        );
        let file = self
            .current_db_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Session")
            .to_string();
        let summary = format!(
            "{} spans · {} threads · {} frames · {}",
            frame.spans.len(),
            frame.threads.len(),
            frame.frame_boundaries_ns.len().max(frame.frame_times_ms.len()),
            format_duration(frame.duration_ns()),
        );
        let (bottom_open, right_open) = match &self.session_ui {
            Some(ui) => (
                ui.dock.read(cx).is_dock_open(DockPlacement::Bottom, cx),
                ui.dock.read(cx).is_dock_open(DockPlacement::Right, cx),
            ),
            None => (false, false),
        };
        let status = self.export_status.clone();

        let divider = || div().w(px(1.)).h(px(18.)).mx_1().bg(border);
        let toggle = |id: &'static str, icon: IconName, tip: &'static str, open: bool| {
            let button = Button::new(id).icon(icon).compact().tooltip(tip);
            if open { button.primary() } else { button.ghost() }
        };

        h_flex()
            .w_full()
            .h(px(36.))
            .flex_shrink_0()
            .px_3()
            .gap_1()
            .items_center()
            .bg(sidebar)
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::Database).size(px(13.)).text_color(muted))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child(file),
            )
            .child(div().text_size(px(11.)).text_color(muted).ml_2().child(summary))
            .child(div().flex_1())
            .children(status.map(|text| {
                div()
                    .max_w(px(360.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(muted)
                    .mr_2()
                    .child(text)
            }))
            .child(
                Button::new("open-session")
                    .icon(IconName::FolderOpen)
                    .label("Open")
                    .ghost()
                    .compact()
                    .tooltip("Open a saved session")
                    .on_click(cx.listener(|this, _, _, cx| this.open_database_picker(cx))),
            )
            .child(
                Button::new("export-csv")
                    .icon(IconName::Download)
                    .label("CSV")
                    .ghost()
                    .compact()
                    .tooltip("Export the statistics table as CSV")
                    .on_click(cx.listener(|this, _, _, cx| this.export(ExportKind::StatsCsv, cx))),
            )
            .child(
                Button::new("export-trace")
                    .icon(IconName::Download)
                    .label("Trace")
                    .ghost()
                    .compact()
                    .tooltip("Export as a Chrome trace (Perfetto, chrome://tracing)")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.export(ExportKind::ChromeTrace, cx)),
                    ),
            )
            .child(divider())
            .child(
                Button::new("prev-frame")
                    .icon(IconName::ChevronLeft)
                    .ghost()
                    .compact()
                    .tooltip("Previous frame")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view.update(cx, |view, cx| view.step_frame(-1, cx));
                    })),
            )
            .child(
                Button::new("next-frame")
                    .icon(IconName::ChevronRight)
                    .ghost()
                    .compact()
                    .tooltip("Next frame")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view.update(cx, |view, cx| view.step_frame(1, cx));
                    })),
            )
            .child(
                Button::new("zoom-out")
                    .icon(IconName::Minus)
                    .ghost()
                    .compact()
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view.update(cx, |view, cx| view.zoom_by(1.0 / 1.5, cx));
                    })),
            )
            .child(
                Button::new("zoom-in")
                    .icon(IconName::Plus)
                    .ghost()
                    .compact()
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view.update(cx, |view, cx| view.zoom_by(1.5, cx));
                    })),
            )
            .child(
                Button::new("fit-all")
                    .icon(IconName::Expand)
                    .label("Fit")
                    .ghost()
                    .compact()
                    .tooltip("Show the whole session")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.view.update(cx, |view, cx| view.fit_all(cx));
                    })),
            )
            .child(divider())
            .child(
                toggle("toggle-bottom", IconName::PanelBottom, "Analysis panels", bottom_open)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Bottom, window, cx)
                    })),
            )
            .child(
                toggle("toggle-right", IconName::PanelRight, "Call stack", right_open).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.toggle_dock(DockPlacement::Right, window, cx)
                    }),
                ),
            )
    }
}
