use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, divider::Divider, ActiveTheme as _, IconName,
};
use super::{types::{ProjectSettings, format_size}, helpers::{render_info_section, render_size_bar}};
use crate::entry_screen::EntryScreen;

pub fn render_disk_info_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let project_size = settings.disk_size.unwrap_or(0);
    let git_size = settings.git_repo_size.unwrap_or(0);
    let working_files_size = if project_size > git_size { project_size - git_size } else { 0 };
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Disk Usage")
        )
        .child(Divider::horizontal())
        .child(render_info_section("Total Size", vec![
            ("Project Size", format_size(Some(project_size))),
            ("Git Repository", format_size(Some(git_size))),
            ("Working Files", format_size(Some(working_files_size))),
        ], &theme))
        .child(
            v_flex()
                .gap_3()
                .p_4()
                .border_1()
                .border_color(theme.border)
                .rounded_lg()
                .bg(theme.sidebar)
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .mb_3()
                        .child("Size Breakdown")
                )
                .child(render_size_bar("Working Files", working_files_size, project_size, theme.accent, &theme))
                .child(render_size_bar("Git Data", git_size, project_size, theme.primary, &theme))
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    Button::new("refresh-disk")
                        .label("Refresh Disk Info")
                        .icon(IconName::ArrowUp)
                        .w_full()
                        .with_variant(ui::button::ButtonVariant::Primary)
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.refresh_project_settings(cx);
                        }))
                )
                .child(
                    Button::new("clean-project")
                        .label("Clean Project (Git GC)")
                        .icon(IconName::Trash)
                        .w_full()
                        .with_variant(ui::button::ButtonVariant::Secondary)
                        .on_click({
                            let path = settings.project_path.clone();
                            move |_, _, _| {
                                let _ = std::process::Command::new("git")
                                    .args(&["gc", "--aggressive", "--prune=now"])
                                    .current_dir(&path)
                                    .spawn();
                            }
                        })
                )
        )
}
