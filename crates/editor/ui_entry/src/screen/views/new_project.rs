use gpui::prelude::*;
use gpui::*;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName,
};

use crate::screen::EntryScreen;

pub fn render_new_project(
    screen: &mut EntryScreen,
    window: &mut Window,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let project_name_input = screen.inputs().new_project_name.clone();
    let selected_path = screen.state.input.new_project_path.clone();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("New Project"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Create a new Pulsar Engine project from scratch"),
                ),
        )
        .child(
            v_flex()
                .max_w(px(600.))
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Project Name"),
                        )
                        .child(
                            ui::input::Input::new(&project_name_input).w_full(),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Location"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .px_3()
                                        .py_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(theme.border)
                                        .text_sm()
                                        .text_color(if selected_path.is_some() { theme.foreground } else { theme.muted_foreground })
                                        .child(
                                            selected_path
                                                .as_ref()
                                                .map(|p| p.to_string_lossy().to_string())
                                                .unwrap_or_else(|| "Choose a folder...".to_string()),
                                        ),
                                )
                                .child(
                                    Button::new("browse-location")
                                        .label("Browse")
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.browse_project_location(cx);
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .p_4()
                        .gap_2()
                        .rounded_lg()
                        .bg(theme.secondary.opacity(0.12))
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("What\u{2019}s Created"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .line_height(relative(1.6))
                                .child("A Pulsar.toml manifest file\nassets/ scenes/ scripts/ prefabs/ directories\na Git repository initialized"),
                        ),
                )
                .child(
                    h_flex()
                        .pt_2()
                        .child(
                            Button::new("create-project-btn")
                                .label("Create Project")
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.create_new_project(cx);
                                })),
                        ),
                ),
        )
}
