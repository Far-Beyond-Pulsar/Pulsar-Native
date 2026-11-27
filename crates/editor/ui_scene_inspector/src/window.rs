//! The SceneDB Inspector's own window.

use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme, TitleBar};

use crate::screen::SceneInspectorView;

pub struct SceneInspectorWindow {
    inspector: gpui::Entity<SceneInspectorView>,
}

impl Render for SceneInspectorWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                TitleBar::new().child(
                    h_flex().flex_1().items_center().child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("SceneDB Inspector"),
                    ),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.inspector.clone()))
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for SceneInspectorWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "SceneInspectorWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1600.0, 940.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let inspector = cx.new(|cx| SceneInspectorView::new(window, cx));
        cx.new(|_| SceneInspectorWindow { inspector })
    }
}
