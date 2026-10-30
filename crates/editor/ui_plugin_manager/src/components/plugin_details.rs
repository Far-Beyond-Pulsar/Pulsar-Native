use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_manager::{DependencyState, DependencyStatus, PluginDetails};
use ui::scroll::ScrollbarAxis;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt};

use crate::screen::PluginManagerWindow;

/// The page for one plugin: identity, dependencies and what depends on it,
/// and what it contributes. `all` resolves dependency ids to names.
pub fn render_plugin_details(
    details: &PluginDetails,
    all: &[PluginDetails],
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let metadata = &details.metadata;
    let name_of = |id: &str| -> String {
        all.iter()
            .find(|p| p.metadata.id.as_str() == id)
            .map(|p| p.metadata.name.clone())
            .unwrap_or_else(|| id.to_string())
    };

    let dependencies = if details.dependencies.is_empty() {
        none_row("This plugin does not depend on other plugins.", cx).into_any_element()
    } else {
        v_flex()
            .gap_2()
            .children(
                details
                    .dependencies
                    .iter()
                    .enumerate()
                    .map(|(i, d)| dependency_row(i, d, &name_of, cx)),
            )
            .into_any_element()
    };

    let required_by = if details.required_by.is_empty() {
        none_row("No loaded plugin depends on this one.", cx).into_any_element()
    } else {
        v_flex()
            .gap_1()
            .children(details.required_by.iter().map(|id| {
                bullet(
                    format!("{} ({})", name_of(id.as_str()), id),
                    cx.theme().foreground,
                    cx,
                )
            }))
            .into_any_element()
    };

    let provides: Vec<(&str, Vec<String>)> = vec![
        ("File types", details.file_types.clone()),
        ("Editors", details.editors.clone()),
        ("Components", details.components.clone()),
        (
            "Statusbar buttons",
            if details.statusbar_buttons == 0 {
                Vec::new()
            } else {
                vec![format!("{}", details.statusbar_buttons)]
            },
        ),
    ];
    let provides: Vec<AnyElement> = provides
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(title, items)| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .child(title.to_string()),
                )
                .children(
                    items
                        .into_iter()
                        .map(|item| bullet(item, cx.theme().foreground, cx)),
                )
                .into_any_element()
        })
        .collect();

    div()
        .id("plugin-details")
        .flex_1()
        .w_full()
        .scrollable(ScrollbarAxis::Vertical)
        .child(
            v_flex()
                .w_full()
                .p_4()
                .gap_4()
                .child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child(metadata.name.clone()),
                        )
                        .when(!metadata.version.is_empty(), |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_px()
                                    .rounded(px(4.))
                                    .bg(cx.theme().muted.opacity(0.3))
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(metadata.version.clone()),
                            )
                        })
                        .when(details.builtin, |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_px()
                                    .rounded(px(4.))
                                    .bg(cx.theme().success.opacity(0.14))
                                    .text_xs()
                                    .text_color(cx.theme().success)
                                    .child("Built-in"),
                            )
                        }),
                )
                .when(!metadata.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(metadata.description.clone()),
                    )
                })
                .child(section(
                    "Details",
                    v_flex()
                        .gap_1()
                        .child(field("Id", metadata.id.to_string(), cx))
                        .child(field("Author", metadata.author.clone(), cx))
                        .when_some(details.library_path.as_ref(), |this, path| {
                            this.child(field("Library", path.display().to_string(), cx))
                        })
                        .into_any_element(),
                    cx,
                ))
                .child(section("Depends on", dependencies, cx))
                .child(section("Required by", required_by, cx))
                .when(!provides.is_empty(), |this| {
                    this.child(section(
                        "Provides",
                        v_flex().gap_3().children(provides).into_any_element(),
                        cx,
                    ))
                }),
        )
}

fn section(
    title: &'static str,
    body: AnyElement,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_4()
        .rounded_xl()
        .border_1()
        .border_color(cx.theme().border.opacity(0.65))
        .bg(cx.theme().sidebar.opacity(0.7))
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
        .child(body)
}

fn field(label: &'static str, value: String, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .w(px(72.))
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child(value),
        )
}

fn none_row(text: &'static str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground.opacity(0.8))
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
        .child(div().text_sm().text_color(color).child(text))
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
            cx.theme().danger,
            format!("version {found} does not satisfy {}", dependency.requirement),
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
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
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
                                    .rounded(px(4.))
                                    .bg(cx.theme().muted.opacity(0.3))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("optional"),
                            )
                        }),
                )
                .child(div().text_xs().text_color(color).child(note)),
        )
}
