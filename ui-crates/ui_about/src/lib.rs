use gpui::*;
use ui::{ActiveTheme, Root, Sizable, StyledExt, TitleBar, v_flex, button::{Button, ButtonVariants as _}};

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

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child("About Pulsar Engine"))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_6()
                    .p_8()
                    .rounded_lg()
                    .bg(theme.sidebar)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .child(
                        div()
                            .text_3xl()
                            .font_bold()
                            .text_color(theme.foreground)
                            .child("Pulsar Engine")
                    )
                    .child(
                        div()
                            .text_lg()
                            .text_color(theme.muted_foreground)
                            .child("Version 0.1.47")
                    )
                    .child(
                        div()
                            .w_96()
                            .h_px()
                            .bg(theme.border)
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("A modern game engine built with Rust")
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("© 2024 Pulsar Engine Contributors")
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                Button::new("github-button")
                                    .label("GitHub")
                                    .primary()
                                    .on_click(|_, _, cx| {
                                        cx.open_url("https://github.com/Far-Beyond-Pulsar/Pulsar-Native")
                                    })
                            )
                            .child(
                                Button::new("docs-button")
                                    .label("Documentation")
                                    .outline()
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
