use gpui::*;
use ui::{
    ActiveTheme, Root, Sizable, StyledExt, TitleBar, v_flex, h_flex,
    button::{Button, ButtonVariants as _},
    Icon, IconName,
};
use ui_common::translate;
use chrono::Datelike;

pub struct AboutWindow {
    focus_handle: FocusHandle,
}

impl AboutWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
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
        let current_year = chrono::Local::now().year();

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
                            // Logo/Icon section
                            .child(
                                div()
                                    .w(px(120.0))
                                    .h(px(120.0))
                                    .rounded_2xl()
                                    .bg(theme.accent.opacity(0.15))
                                    .border_2()
                                    .border_color(theme.accent.opacity(0.3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .shadow_lg()
                                    .child(
                                        img("images/logo_sqrkl.png")
                                            .w(px(100.0))
                                            .h(px(100.0))
                                            .object_fit(gpui::ObjectFit::Contain)
                                    )
                            )
                            // Title and version
                            .child(
                                v_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_3xl()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .child("Pulsar Engine")
                                    )
                                    .child(
                                        div()
                                            .px_4()
                                            .py_1p5()
                                            .rounded_full()
                                            .bg(theme.accent.opacity(0.15))
                                            .border_1()
                                            .border_color(theme.accent.opacity(0.3))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.accent)
                                                    .child("Version 0.1.47")
                                            )
                                    )
                            )
                            // Divider
                            .child(
                                div()
                                    .w_full()
                                    .h_px()
                                    .bg(theme.border.opacity(0.5))
                            )
                            // Description card
                            .child(
                                v_flex()
                                    .w_full()
                                    .gap_3()
                                    .p_6()
                                    .rounded_lg()
                                    .bg(theme.background.opacity(0.5))
                                    .border_1()
                                    .border_color(theme.border.opacity(0.3))
                                    .child(
                                        div()
                                            .text_base()
                                            .text_center()
                                            .line_height(rems(1.6))
                                            .text_color(theme.foreground.opacity(0.9))
                                            .child("A modern, high-performance game engine built with Rust. Designed for flexibility, speed, and developer experience.")
                                    )
                            )
                            // Features/highlights
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_4()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_2()
                                            .p_4()
                                            .rounded_lg()
                                            .bg(theme.sidebar)
                                            .border_1()
                                            .border_color(theme.border.opacity(0.5))
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Activity)
                                                    .size_5()
                                                    .text_color(theme.accent)
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("High Performance")
                                            )
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_2()
                                            .p_4()
                                            .rounded_lg()
                                            .bg(theme.sidebar)
                                            .border_1()
                                            .border_color(theme.border.opacity(0.5))
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Code)
                                                    .size_5()
                                                    .text_color(theme.accent)
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Rust Powered")
                                            )
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap_2()
                                            .p_4()
                                            .rounded_lg()
                                            .bg(theme.sidebar)
                                            .border_1()
                                            .border_color(theme.border.opacity(0.5))
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Globe)
                                                    .size_5()
                                                    .text_color(theme.accent)
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Open Source")
                                            )
                                    )
                            )
                            // Copyright
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("© {} Pulsar Engine Contributors", current_year))
                            )
                            // Action buttons
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
                                            .on_click(|_, _, cx| {
                                                cx.open_url("https://github.com/Far-Beyond-Pulsar/Pulsar-Native")
                                            })
                                    )
                                    .child(
                                        Button::new("docs-button")
                                            .label("Documentation")
                                            .icon(IconName::BookOpen)
                                            .ghost()
                                            .on_click(|_, _, cx| {
                                                cx.open_url("https://docs.pulsarengine.dev")
                                            })
                                    )
                            )
                    )
            )
    }
}

/// Helper to create the about window with Root wrapper
pub fn create_about_window(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let about = cx.new(|cx| AboutWindow::new(window, cx));
    cx.new(|cx| Root::new(about.into(), window, cx))
}
