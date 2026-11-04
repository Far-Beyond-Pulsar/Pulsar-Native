use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_manager::{DependencyState, DependencyStatus, PluginDetails};
use ui::scroll::ScrollbarAxis;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt,
};

use crate::screen::PluginManagerWindow;

/// Identity, capabilities, dependencies, and provenance for the selected extension.
pub fn render_plugin_details(
    details: &PluginDetails,
    all: &[PluginDetails],
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let metadata = &details.metadata;
    let name_of = |id: &str| -> String {
        all.iter()
            .find(|plugin| plugin.metadata.id.as_str() == id)
            .map(|plugin| plugin.metadata.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let categories = [
        (
            IconName::Code,
            "File types",
            &details.file_types,
            "file types",
        ),
        (IconName::Code, "Editors", &details.editors, "editors"),
        (
            IconName::Box,
            "Components",
            &details.components,
            "components",
        ),
        (
            IconName::Activity,
            "Statusbar widgets",
            &details.statusbar_buttons,
            "widgets",
        ),
        (IconName::Cpu, "AI tools", &details.ai_tools, "tools"),
        (
            IconName::Database,
            "Engine subsystems",
            &details.subsystems,
            "subsystems",
        ),
    ];
    let provides = categories
        .iter()
        .filter(|(_, _, items, _)| !items.is_empty())
        .map(|(icon, title, items, count_label)| {
            capability_section(icon.clone(), *title, items.as_slice(), *count_label, cx)
                .into_any_element()
        })
        .collect::<Vec<_>>();
    let dependency_count = details
        .dependencies
        .iter()
        .filter(|dep| !dep.optional)
        .count();
    let unmet_count = details
        .dependencies
        .iter()
        .filter(|dep| !dep.optional && !matches!(dep.state, DependencyState::Satisfied { .. }))
        .count();
    let contribution_rows = categories
        .iter()
        .map(|(icon, title, items, count_label)| {
            let count = items.len();
            let value = if count == 0 {
                "—".to_string()
            } else {
                count.to_string()
            };
            let note = if count == 0 {
                "No registrations".to_string()
            } else {
                count_label.to_string()
            };
            h_flex()
                .w_full()
                .h(px(38.))
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border.opacity(0.65))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(icon.clone())
                                .size_4()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child(*title),
                        ),
                )
                .child(
                    div()
                        .w(px(52.))
                        .text_xs()
                        .font_family("monospace")
                        .font_semibold()
                        .text_color(if count == 0 {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().foreground
                        })
                        .child(value),
                )
                .child(
                    div()
                        .w(px(112.))
                        .text_right()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();
    let dependencies = if details.dependencies.is_empty() {
        none_row("No plugin dependencies.", cx).into_any_element()
    } else {
        v_flex()
            .gap_2()
            .children(
                details
                    .dependencies
                    .iter()
                    .enumerate()
                    .map(|(index, dependency)| dependency_row(index, dependency, &name_of, cx)),
            )
            .into_any_element()
    };
    let required_by = if details.required_by.is_empty() {
        none_row("No loaded plugins depend on this extension.", cx).into_any_element()
    } else {
        v_flex()
            .gap_1()
            .children(details.required_by.iter().map(|id| {
                bullet(
                    format!("{} ({id})", name_of(id.as_str())),
                    cx.theme().foreground,
                    cx,
                )
            }))
            .into_any_element()
    };

    div()
        .id("plugin-details")
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .child(
            v_flex()
                .w_full()
                .max_w(px(780.))
                .mx_auto()
                .p_5()
                .gap_4()
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap_3()
                        .child(
                            div()
                                .size(px(42.))
                                .flex_shrink_0()
                                .rounded_lg()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().sidebar.opacity(0.65))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Puzzle)
                                        .size_5()
                                        .text_color(cx.theme().primary),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_semibold()
                                                .text_color(cx.theme().foreground)
                                                .child(metadata.name.clone()),
                                        )
                                        .when(details.builtin, |this| {
                                            this.child(
                                                h_flex()
                                                    .px_1p5()
                                                    .py_px()
                                                    .rounded_sm()
                                                    .bg(cx.theme().success.opacity(0.12))
                                                    .text_xs()
                                                    .text_color(cx.theme().success)
                                                    .child("Built-in"),
                                            )
                                        })
                                        .when(!metadata.version.is_empty(), |this| {
                                            this.child(
                                                div()
                                                    .px_1p5()
                                                    .py_px()
                                                    .rounded_sm()
                                                    .bg(cx.theme().secondary.opacity(0.65))
                                                    .text_xs()
                                                    .font_family("monospace")
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(metadata.version.clone()),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family("monospace")
                                        .text_color(cx.theme().muted_foreground)
                                        .child(metadata.id.to_string()),
                                ),
                        ),
                )
                .when(!metadata.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(metadata.description.clone()),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .when(!details.builtin, |this| {
                            this.child(
                                Button::new("plugin-unload")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Refresh)
                                    .label("Restart required to unload")
                                    .disabled(true)
                                    .tooltip(
                                        "Plugin libraries remain loaded until the editor restarts",
                                    ),
                            )
                        })
                        .when(unmet_count > 0, |this| {
                            this.child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .rounded_md()
                                    .bg(cx.theme().warning.opacity(0.13))
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(Icon::new(IconName::TriangleAlert).size_3())
                                    .child(format!("{unmet_count} dependency issues")),
                            )
                        })
                        .when(dependency_count > 0 && unmet_count == 0, |this| {
                            this.child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .rounded_md()
                                    .bg(cx.theme().success.opacity(0.12))
                                    .text_xs()
                                    .text_color(cx.theme().success)
                                    .child(Icon::new(IconName::Check).size_3())
                                    .child(format!("{dependency_count} dependencies satisfied")),
                            )
                        }),
                )
                .child(section(
                    "Contributions",
                    v_flex()
                        .w_full()
                        .children(contribution_rows)
                        .into_any_element(),
                    "What this extension registers with the engine",
                    cx,
                ))
                .when(!provides.is_empty(), |this| {
                    this.child(section(
                        "Registered capabilities",
                        v_flex().gap_3().children(provides).into_any_element(),
                        "Open the corresponding editor or system to inspect each registration.",
                        cx,
                    ))
                })
                .child(section(
                    "Dependencies",
                    dependencies,
                    "Required and optional plugin links",
                    cx,
                ))
                .child(section(
                    "Required by",
                    required_by,
                    "Loaded extensions that depend on this one",
                    cx,
                ))
                .child(section(
                    "Details",
                    v_flex()
                        .gap_2()
                        .child(field("Author", metadata.author.clone(), cx))
                        .when_some(details.library_path.as_ref(), |this, path| {
                            this.child(field("Library", path.display().to_string(), cx))
                        })
                        .into_any_element(),
                    "Extension metadata",
                    cx,
                )),
        )
}

fn capability_section(
    icon: IconName,
    title: &'static str,
    items: &[String],
    count_label: &'static str,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(icon)
                        .size_4()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} {count_label}", items.len())),
                ),
        )
        .children(
            items
                .iter()
                .map(|item| bullet(item.clone(), cx.theme().foreground, cx)),
        )
}

fn section(
    title: &'static str,
    body: AnyElement,
    hint: &'static str,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border.opacity(0.65))
        .bg(cx.theme().sidebar.opacity(0.45))
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                ),
        )
        .child(body)
}

fn field(
    label: &'static str,
    value: String,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(72.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(value),
        )
}

fn none_row(text: &'static str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn bullet(text: String, color: Hsla, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Icon::new(IconName::ChevronRight)
                .size_3()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().text_xs().text_color(color).child(text))
}

fn dependency_row(
    index: usize,
    status: &DependencyStatus,
    name_of: &dyn Fn(&str) -> String,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let dependency = &status.dependency;
    let (icon, color, note) = match &status.state {
        DependencyState::Satisfied { version } => (
            IconName::Check,
            cx.theme().success,
            format!("loaded, version {version}"),
        ),
        DependencyState::VersionMismatch { found } => (
            IconName::TriangleAlert,
            cx.theme().warning,
            format!(
                "version {found} does not satisfy {}",
                dependency.requirement
            ),
        ),
        DependencyState::Missing if status.optional => (
            IconName::Info,
            cx.theme().muted_foreground,
            "not installed (optional)".to_string(),
        ),
        DependencyState::Missing => (
            IconName::CircleX,
            cx.theme().danger,
            "not installed".to_string(),
        ),
    };
    h_flex()
        .id(ElementId::Integer(index as u64))
        .gap_2()
        .items_start()
        .child(Icon::new(icon).size_4().text_color(color))
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(name_of(dependency.id.as_str())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} {}", dependency.id, dependency.requirement)),
                        )
                        .when(status.optional, |this| {
                            this.child(
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(cx.theme().secondary.opacity(0.65))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("optional"),
                            )
                        }),
                )
                .child(div().text_xs().text_color(color).child(note)),
        )
}
