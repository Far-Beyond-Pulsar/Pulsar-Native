use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName};

pub fn render_feature_cards(theme: &ui::Theme) -> impl IntoElement {
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
                .child(Icon::new(IconName::Activity).size_5())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("High Performance"),
                ),
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
                .child(Icon::new(IconName::Code).size_5())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Rust Powered"),
                ),
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
                .child(Icon::new(IconName::Globe).size_5())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Open Source"),
                ),
        )
}
