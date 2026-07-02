use gpui::*;
use gpui::prelude::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Disableable, Icon, IconName};

use crate::screen::EntryScreen;
use super::types::{AvailableTools, ToolInfo};

pub fn render_integrations_tab(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let Some(ref settings) = screen.state.ui.project_settings else {
        return div().into_any_element();
    };
    let tools = AvailableTools::detect();
    let preferred_editor = settings.preferred_editor.clone();
    let preferred_git_tool = settings.preferred_git_tool.clone();
    let project_path = settings.project_path.clone();

    v_flex()
        .gap_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Integrations"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Configure preferred tools for this project"),
        )
        .child(
            render_tool_category(
                "Editors",
                IconName::Code,
                tools.editors,
                preferred_editor,
                project_path.clone(),
                cx,
            ),
        )
        .child(
            render_tool_category(
                "Git Tools",
                IconName::Github,
                tools.git_tools,
                preferred_git_tool,
                project_path.clone(),
                cx,
            ),
        )
        .child(
            render_tool_category(
                "Terminals",
                IconName::Terminal,
                tools.terminals,
                None,
                project_path,
                cx,
            ),
        )
        .into_any_element()
}

fn render_tool_category(
    title: &str,
    icon: IconName,
    tools: Vec<ToolInfo>,
    preferred: Option<String>,
    _project_path: std::path::PathBuf,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let title = title.to_string();

    v_flex()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(icon).size_4().text_color(theme.foreground))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("({} found)", tools.len())),
                ),
        )
        .when(tools.is_empty(), |this| {
            this.child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("No {} detected on your system", title.to_lowercase())),
            )
        })
        .children(tools.into_iter().map(|tool| {
            let name = tool.name.clone();
            let name_id = name.clone();
            let path = tool.path.clone();
            let is_default = preferred.as_deref() == Some(name.as_str());
            let tool_path = tool.path.clone();

            h_flex()
                .w_full()
                .p_3()
                .gap_3()
                .items_center()
                .rounded_md()
                .bg(theme.secondary.opacity(0.08))
                .child(
                    Icon::new(IconName::Check)
                        .size(px(14.))
                        .text_color(if is_default { theme.success_foreground } else { gpui::transparent_white() }),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_default { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                .text_color(theme.foreground)
                                .child(name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(path),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("set-default-{}", name_id)))
                        .compact()
                        .ghost()
                        .label(if is_default { "Default" } else { "Set Default" })
                        .disabled(is_default)
                        .on_click(cx.listener(move |_, _, _, _cx| {})),
                )
                .child(
                    Button::new(SharedString::from(format!("launch-{}", name_id)))
                        .compact()
                        .ghost()
                        .label("Launch")
                        .on_click(cx.listener(move |_, _, _, _cx| {
                            let _ = open::that(&tool_path);
                        })),
                )
        }))
        .into_any_element()
}
