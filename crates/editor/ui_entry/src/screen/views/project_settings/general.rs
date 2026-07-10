use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _};

use super::helpers::render_info_section;
use crate::screen::EntryScreen;

pub fn render_general_tab(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };
    let project_name = settings.project_name.clone();
    let project_path = settings.project_path.clone();
    let path_str = project_path.to_string_lossy().to_string();

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("General"),
        )
        .child(render_info_section(
            vec![
                ("Name".to_string(), project_name),
                ("Path".to_string(), path_str.clone()),
                ("Type".to_string(), "Pulsar Engine Project".to_string()),
            ],
            cx,
        ))
        .child(
            h_flex()
                .gap_2()
                .child({
                    let project_path = project_path.clone();
                    Button::new("open-file-manager")
                        .label("Open in File Manager")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, _cx| {
                            let _ = open::that(&project_path);
                        }))
                })
                .child({
                    let project_path = project_path.clone();
                    Button::new("open-terminal")
                        .label("Open Terminal")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, _cx| {
                            let _ = open::that(&project_path);
                        }))
                })
                .child(
                    Button::new("copy-path")
                        .label("Copy Path")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                path_str.clone(),
                            ));
                        })),
                ),
        )
        .into_any_element()
}
