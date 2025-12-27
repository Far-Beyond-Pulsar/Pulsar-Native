use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, divider::Divider, ActiveTheme as _,
};
use super::{types::ProjectSettings, helpers::render_info_section};
use crate::entry_screen::EntryScreen;

pub fn render_git_ci_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Git CI/CD Integration")
        )
        .child(Divider::horizontal())
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Continuous Integration and Deployment workflows for your project")
        )
        .child(render_info_section("GitHub Actions", vec![
            ("Workflow Files", settings.workflow_files.len().to_string()),
            ("Status", if settings.workflow_files.is_empty() { "Not configured" } else { "Active" }.to_string()),
        ], &theme))
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Detected Workflows")
                )
                .children(if settings.workflow_files.is_empty() {
                    vec![
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("No workflow files found in .github/workflows/")
                            .into_any_element()
                    ]
                } else {
                    settings.workflow_files.iter().map(|workflow| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .px_3()
                            .py_2()
                            .border_1()
                            .border_color(theme.border)
                            .rounded_md()
                            .bg(theme.sidebar)
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(16.))
                                    .text_color(theme.accent)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(workflow.clone())
                            )
                            .into_any_element()
                    }).collect()
                })
        )
        .child(
            v_flex()
                .gap_3()
                .mt_4()
                .child(
                    Button::new("create-workflow")
                        .label("Create New Workflow")
                        .icon(IconName::Plus)
                        .w_full()
                        .with_variant(ui::button::ButtonVariant::Primary)
                        .on_click({
                            let path = settings.project_path.clone();
                            move |_, _, _| {
                                let workflows_dir = path.join(".github").join("workflows");
                                let _ = std::fs::create_dir_all(&workflows_dir);
                                use crate::entry_screen::integration_launcher;
                                let _ = integration_launcher::launch_file_manager(&workflows_dir);
                            }
                        })
                )
                .child(
                    Button::new("view-actions")
                        .label("View on GitHub")
                        .icon(IconName::GitHub)
                        .w_full()
                        .with_variant(ui::button::ButtonVariant::Secondary)
                        .on_click({
                            let remote = settings.remote_url.clone();
                            move |_, _, _| {
                                if let Some(url) = &remote {
                                    let actions_url = url
                                        .trim_end_matches(".git")
                                        .to_string() + "/actions";
                                    let _ = open::that(actions_url);
                                }
                            }
                        })
                )
        )
}
