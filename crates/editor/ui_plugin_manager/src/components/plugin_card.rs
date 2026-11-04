use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_manager::{DependencyState, PluginDetails};
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt};

pub fn render_plugin_item(
    details: &PluginDetails,
    selected: bool,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let plugin = &details.metadata;
    let plugin_id = plugin.id.clone();
    let title = plugin.name.clone();
    let author = plugin.author.clone();
    let contributions = [
        (details.file_types.len(), "files"),
        (details.editors.len(), "editors"),
        (details.components.len(), "components"),
        (details.statusbar_buttons.len(), "statusbar widgets"),
        (details.ai_tools.len(), "AI tools"),
        (details.subsystems.len(), "subsystems"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect::<Vec<_>>()
    .join(" · ");
    let dependency_count = details
        .dependencies
        .iter()
        .filter(|dep| !dep.optional)
        .count();
    let has_unmet_dependencies = details
        .dependencies
        .iter()
        .any(|dep| !dep.optional && !matches!(dep.state, DependencyState::Satisfied { .. }));
    let source = if details.builtin {
        "Built-in".to_string()
    } else if plugin.version.is_empty() {
        "Loaded extension".to_string()
    } else {
        format!("v{}", plugin.version)
    };
    let open_id = plugin_id.clone();

    h_flex()
        .id(SharedString::from(format!("plugin-{plugin_id}")))
        .w_full()
        .h(px(62.))
        .px_2()
        .gap_2()
        .items_center()
        .justify_start()
        .rounded_md()
        .border_1()
        .border_color(if selected {
            cx.theme().border
        } else {
            cx.theme().transparent
        })
        .bg(if selected {
            cx.theme().secondary.opacity(0.55)
        } else {
            cx.theme().transparent
        })
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().secondary.opacity(0.4)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select(Some(open_id.clone()), cx);
        }))
        .child(
            div()
                .size(px(30.))
                .flex_shrink_0()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Puzzle)
                        .size_4()
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(title),
                        )
                        .when(dependency_count > 0, |this| {
                            this.child(
                                Icon::new(if has_unmet_dependencies {
                                    IconName::TriangleAlert
                                } else {
                                    IconName::Check
                                })
                                .size_3()
                                .text_color(
                                    if has_unmet_dependencies {
                                        cx.theme().warning
                                    } else {
                                        cx.theme().success
                                    },
                                ),
                            )
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if contributions.is_empty() {
                            format!("by {author} · no contributions reported")
                        } else {
                            format!("by {author} · {contributions}")
                        }),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .px_1p5()
                .py_px()
                .rounded_sm()
                .bg(if details.builtin {
                    cx.theme().success.opacity(0.12)
                } else {
                    cx.theme().secondary.opacity(0.55)
                })
                .text_xs()
                .text_color(if details.builtin {
                    cx.theme().success
                } else {
                    cx.theme().muted_foreground
                })
                .child(source),
        )
}
