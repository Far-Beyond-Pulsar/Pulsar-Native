use gpui::*;
use ui::{TitleBar, v_flex, h_flex, ActiveTheme, StyledExt};
use crate::{FlamegraphView, TraceData, InstrumentationCollector};
use std::sync::Arc;

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
    collector: Option<Arc<InstrumentationCollector>>,
    trace_data: Arc<TraceData>,
    is_profiling: bool,
}

impl FlamegraphWindow {
    pub fn new(trace_data: Arc<TraceData>, _window: &mut Window, cx: &mut App) -> Entity<Self> {
        // Clone the Arc so window and view share the same TraceData
        let view_trace_data = Arc::clone(&trace_data);
        let view = cx.new(move |_cx| FlamegraphView::new((*view_trace_data).clone()));
        
        cx.new(|_cx| Self { 
            view,
            collector: None,
            trace_data,
            is_profiling: false,
        })
    }

    pub fn open(cx: &mut App) -> WindowHandle<Self> {
        let trace_data = Arc::new(TraceData::new());
        
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(100.0), px(100.0)),
                size: size(px(1200.0), px(800.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("Flamegraph Profiler (Instrumentation)".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            is_minimizable: true,
            is_resizable: true,
            window_decorations: None,
            display_id: None,
            window_min_size: Some(size(px(600.0), px(400.0))),
            tabbing_identifier: None,
            app_id: None,
        };

        cx.open_window(window_options, |_window, cx| {
            Self::new(trace_data, _window, cx)
        }).unwrap()
    }

    fn start_profiling(&mut self, _cx: &mut Context<Self>) {
        if self.is_profiling {
            return;
        }

        println!("[PROFILER] Starting instrumentation collector");

        // Create instrumentation collector
        let collector = Arc::new(InstrumentationCollector::new(
            Arc::clone(&self.trace_data),
            100, // Update UI every 100ms
        ));
        
        collector.start();
        self.collector = Some(collector);
        self.is_profiling = true;

        println!("[PROFILER] Instrumentation profiling started");
        _cx.notify();
    }

    fn stop_profiling(&mut self, _cx: &mut Context<Self>) {
        if !self.is_profiling {
            return;
        }

        if let Some(collector) = &self.collector {
            collector.stop();
        }

        self.collector = None;
        self.is_profiling = false;
        
        println!("[PROFILER] Instrumentation profiling stopped");
        _cx.notify();
    }
}

impl Render for FlamegraphWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_profiling = self.is_profiling;
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new()
                    .child("Flamegraph Profiler - Instrumentation (Unreal Insights Style)")
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                if !is_profiling {
                                    div()
                                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                            this.start_profiling(cx);
                                        }))
                                        .child("▶ Start")
                                        .px_3()
                                        .py_1()
                                        .bg(gpui::green())
                                        .rounded(px(4.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .text_size(px(12.0))
                                } else {
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _event, _window, cx| {
                                                    this.stop_profiling(cx);
                                                }))
                                                .child("⏹ Stop")
                                                .px_3()
                                                .py_1()
                                                .bg(gpui::red())
                                                .rounded(px(4.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .text_size(px(12.0))
                                        )
                                        .child(
                                            div()
                                                .child("● Recording")
                                                .text_color(gpui::red())
                                                .text_size(px(12.0))
                                        )
                                }
                            )
                    )
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.view.clone())
            )
    }
}
