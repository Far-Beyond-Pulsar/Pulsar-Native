//! The VRAM Map's own window.

use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme, TitleBar};

use crate::VramMapView;

pub struct VramMapWindow {
    map: Entity<VramMapView>,
}

impl Render for VramMapWindow {
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
                            .child("VRAM Map"),
                    ),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.map.clone()))
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for VramMapWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "VramMapWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1280.0, 820.0)
    }

    fn build(_: (), _window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let map = cx.new(VramMapView::new);
        cx.new(|_| VramMapWindow { map })
    }
}
