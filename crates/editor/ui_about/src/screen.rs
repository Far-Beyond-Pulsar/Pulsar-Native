use std::sync::Arc;

use gpui::*;
use ui::{
    ActiveTheme, Icon, IconName, Root, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use ui_common::translate;

use crate::handlers;

static LOGO_PNG: &[u8] = include_bytes!("../../../../assets/images/logo_sqrkl.png");

fn decode_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

pub struct AboutWindow {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) logo: Option<Arc<RenderImage>>,
}

impl AboutWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            logo: decode_png(LOGO_PNG),
        }
    }
}

impl Focusable for AboutWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child(translate("Window.Title.AboutPulsar")))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p_8()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_8()
                            .w_full()
                            .max_w(px(600.0))
                            .p_8()
                            .rounded_xl()
                            .bg(theme.sidebar.opacity(0.5))
                            .border_1()
                            .border_color(theme.border)
                            .shadow_2xl()
                            .child(crate::components::render_logo_section(&self.logo, &theme))
                            .child(crate::components::render_title_version(&theme))
                            .child(crate::components::render_divider(&theme))
                            .child(crate::components::render_description(&theme))
                            .child(crate::components::render_feature_cards(&theme))
                            .child(crate::components::render_copyright(&theme))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Button::new("github-button")
                                            .label("View on GitHub")
                                            .icon(IconName::ExternalLink)
                                            .primary()
                                            .on_click(cx.listener(handlers::on_open_github))
                                    )
                                    .child(
                                        Button::new("docs-button")
                                            .label("Documentation")
                                            .icon(IconName::BookOpen)
                                            .ghost()
                                            .on_click(cx.listener(handlers::on_open_docs))
                                    )
                            )
                    )
            )
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for AboutWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "AboutWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(500.0, 420.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| AboutWindow::new(window, cx))
    }
}

pub fn create_about_window(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let about = cx.new(|cx| AboutWindow::new(window, cx));
    cx.new(|cx| Root::new(about.into(), window, cx))
}
