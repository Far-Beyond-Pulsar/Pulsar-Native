use gpui::{prelude::*, *};
use ui::{ActiveTheme, StyledExt, v_flex};

/// Renders the stripped-HTML description text in a readable, prose-like layout.
#[derive(IntoElement)]
pub struct DescriptionSection {
    pub text: SharedString,
}

impl DescriptionSection {
    pub fn new(text: SharedString) -> Self {
        Self { text }
    }
}

impl RenderOnce for DescriptionSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(border)
            // section label
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .child("Description"),
            )
            // prose body
            .child(
                div()
                    .text_sm()
                    .text_color(fg)
                    .line_height(relative(1.7))
                    .child(self.text),
            )
    }
}
