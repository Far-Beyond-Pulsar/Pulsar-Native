use gpui::*;
use ui::{TitleBar, v_flex, ActiveTheme};
use crate::{FlamegraphView, TraceData};

pub struct FlamegraphWindow {
    view: Entity<FlamegraphView>,
}

impl FlamegraphWindow {
    pub fn new(trace_data: TraceData, _window: &mut Window, cx: &mut App) -> Entity<Self> {
        let view = cx.new(|_cx| FlamegraphView::new(trace_data));

        cx.new(|_cx| Self { view })
    }

    pub fn open(trace_data: TraceData, cx: &mut App) -> WindowHandle<Self> {
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(100.0), px(100.0)),
                size: size(px(1200.0), px(800.0)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("Flamegraph Trace Viewer".into()),
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
}

impl Render for FlamegraphWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child("Flamegraph Profiler"))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.view.clone())
            )
    }
}
