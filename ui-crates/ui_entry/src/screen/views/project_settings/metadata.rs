use gpui::*;
use gpui::prelude::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName};

use crate::screen::EntryScreen;

pub fn render_metadata_tab(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };
    let project_path = settings.project_path.clone();
    let project_name = settings.project_name.clone();
    let toml_path = project_path.join("Pulsar.toml");
    let toml_content = std::fs::read_to_string(&toml_path).unwrap_or_else(|_| "Pulsar.toml not found".to_string());

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Metadata"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Project configuration stored in Pulsar.toml"),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("Pulsar.toml"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("edit-toml")
                                        .compact()
                                        .ghost()
                                        .on_click(cx.listener(move |_, _, _, _cx| {
                                            let _ = open::that(&toml_path);
                                        })),
                                )
                                .child(
                                    Button::new("validate-project")
                                        .compact()
                                        .ghost()
                                        .on_click(cx.listener(move |_, _, _, _cx| {})),
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .p_4()
                        .rounded_md()
                        .bg(gpui::black().opacity(0.3))
                        .text_sm()
                        .font_family(SharedString::from("monospace"))
                        .text_color(theme.foreground)
                        .child(toml_content),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child("Actions"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("create-template-from-project")
                                .ghost()
                                .on_click(cx.listener(|_, _, _, _cx| {})),
                        )
                        .child(
                            Button::new("create-missing-folders")
                                .ghost()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    for dir in &["assets", "scenes", "scripts", "prefabs"] {
                                        let _ = std::fs::create_dir_all(project_path.join(dir));
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .into_any_element()
}
