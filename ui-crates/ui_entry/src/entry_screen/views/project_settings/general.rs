use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, divider::Divider, ActiveTheme as _, IconName,
};
use super::{types::ProjectSettings, helpers::render_info_section};
use crate::entry_screen::EntryScreen;

pub fn render_general_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("General Settings")
        )
        .child(Divider::horizontal())
        .child(render_info_section("Project Information", vec![
            ("Name", settings.project_name.clone()),
            ("Path", settings.project_path.to_string_lossy().to_string()),
            ("Type", "Pulsar Native Game Project".to_string()),
        ], &theme))
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Project Actions")
                )
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            Button::new("open-in-explorer")
                                .label("Open in File Manager")
                                .icon(IconName::FolderOpen)
                                .w_full()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        use crate::entry_screen::integration_launcher;
                                        let _ = integration_launcher::launch_file_manager(&path);
                                    }
                                })
                        )
                        .child(
                            Button::new("open-in-terminal")
                                .label("Open in Terminal")
                                .icon(IconName::Terminal)
                                .w_full()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        use crate::entry_screen::integration_launcher;
                                        let _ = integration_launcher::launch_terminal("default", &path);
                                    }
                                })
                        )
                        .child(
                            Button::new("copy-path")
                                .label("Copy Project Path")
                                .icon(IconName::Copy)
                                .w_full()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.to_string_lossy().to_string();
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()));
                                    }
                                })
                        )
                )
        )
}
