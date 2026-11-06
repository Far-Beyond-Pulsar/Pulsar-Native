//! Standalone shader editor — opens in a bare window without the Pulsar engine.
//! Useful for iterating on the WGSL shader graph, compiler, and preview viewport
//! without a full engine build.
//!
//! Run with:
//!   cargo run --example standalone

use gpui::*;
use materials_graph_editor::ShaderEditorPanel;
use ui::{Assets, Root, Theme, ThemeMode};

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        ui::init(cx);
        ui::themes::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point {
                        x: px(60.0),
                        y: px(60.0),
                    },
                    size: Size {
                        width: px(1600.0),
                        height: px(960.0),
                    },
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("Shader Editor".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let panel = cx.new(|cx| {
                    let mut p = ShaderEditorPanel::new(window, cx);
                    p.start_compilation(cx);
                    p
                });
                cx.new(|cx| Root::new(panel.into(), window, cx))
            },
        )
        .expect("failed to open window");
    });
}
