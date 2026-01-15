use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, divider::Divider,
    input::TextInput,
};
use crate::entry_screen::EntryScreen;

pub fn render_new_project(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let project_name_empty = screen.new_project_name.is_empty();
    let project_path_display = screen.new_project_path.as_ref()
        .and_then(|p| p.to_str())
        .unwrap_or("Click Browse to select location...")
        .to_string();

    v_flex()
        .size_full()
        .p_12()
        .gap_8()
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Create New Project")
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Set up a new Pulsar Engine project with your custom configuration")
                )
        )
        .child(
            v_flex()
                .max_w(px(700.))
                .gap_8()
                .p_8()
                .border_1()
                .border_color(theme.border)
                .rounded_xl()
                .bg(theme.sidebar)
                .shadow_sm()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Project Name")
                        )
                        .child(TextInput::new(&screen.new_project_name_input))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("The name of your new Pulsar project")
                        )
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Project Location")
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .px_3()
                                        .py_2()
                                        .border_1()
                                        .border_color(theme.border)
                                        .rounded_md()
                                        .bg(theme.background)
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(project_path_display)
                                )
                                .child(
                                    Button::new("browse-location")
                                        .label("Browse")
                                        .with_variant(ui::button::ButtonVariant::Secondary)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.browse_project_location(cx);
                                        }))
                                )
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Choose where to create your project folder")
                        )
                )
                .child(
                    div()
                        .p_3()
                        .rounded_lg()
                        .bg(theme.accent.opacity(0.1))
                        .border_1()
                        .border_color(theme.accent.opacity(0.3))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.accent)
                                        .child("What's Created")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• Pulsar.toml configuration file")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• Project folders (assets, scenes, scripts, prefabs)")
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("• Git repository initialization")
                                )
                        )
                )
                .child(
                    h_flex()
                        .justify_end()
                        .child(
                            Button::new("create-project")
                                .label("Create Project")
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.create_new_project(window, cx);
                                }))
                        )
                )
        )
}
