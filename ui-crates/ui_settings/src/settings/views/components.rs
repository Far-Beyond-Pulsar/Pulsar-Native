use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::Button,
    switch::Switch,
    label::Label,
};

// Re-export the components from ui crate for convenience
pub use ui::{SettingCard, SettingRow};

/// Value display component
pub fn render_value_display(value: impl Into<String>, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();

    div()
        .px_3()
        .py_1p5()
        .rounded_md()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .font_family("monospace")
                .text_color(theme.foreground)
                .child(value.into())
        )
}

/// Section header component
pub fn render_section_header(title: impl Into<String>, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();

    div()
        .w_full()
        .pb_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child(title.into())
        )
}
