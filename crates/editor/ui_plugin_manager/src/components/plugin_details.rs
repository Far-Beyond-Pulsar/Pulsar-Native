use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::scroll::ScrollbarAxis;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt};

use crate::model::{Entry, EntryKind, Extensions, Group, RowState};
use crate::screen::PluginManagerWindow;

/// The selected entry: identity, state, problems, contracts and every
/// contribution by category, all from the catalog snapshot.
pub fn render_plugin_details(
    entry: &Entry,
    model: &Extensions,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    let name_of = |id: &str| -> String {
        model
            .entry(id)
            .map(|entry| format!("{} ({id})", entry.name))
            .unwrap_or_else(|| id.to_string())
    };
    let state = match (entry.kind, entry.enabled) {
        (EntryKind::Engine, _) => "Always present",
        (EntryKind::Library, _) => "Loaded",
        (_, true) => "Enabled by this project",
        (_, false) => "Disabled for this project",
    };
    let requirements = if entry.requirements.is_empty() {
        muted("Requires nothing from other plugins.", cx).into_any_element()
    } else {
        v_flex()
            .gap_2()
            .children(entry.requirements.iter().map(|requirement| {
                let (icon, color) = match requirement.state {
                    RowState::Met => (IconName::Check, cx.theme().success),
                    RowState::Absent => (IconName::Info, cx.theme().muted_foreground),
                    RowState::Inactive => (IconName::Info, cx.theme().muted_foreground),
                    RowState::Unmet => (IconName::CircleX, cx.theme().danger),
                };
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(Icon::new(icon).size_4().text_color(color))
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("monospace")
                                            .text_color(cx.theme().foreground)
                                            .child(requirement.contract.clone()),
                                    )
                                    .when(requirement.optional, |this| {
                                        this.child(tag("optional", cx))
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color)
                                    .child(requirement.note.clone()),
                            ),
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
                .max_w(px(820.))
                .mx_auto()
                .p_5()
                .gap_4()
                .child(
                    v_flex()
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
                                        .child(entry.name.clone()),
                                )
                                .child(tag(entry.kind.label(), cx))
                                .child(tag(state, cx)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} · {}", entry.id, entry.source)),
                        ),
                )
                .when(!entry.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(entry.description.clone()),
                    )
                })
                .when(!entry.problems.is_empty(), |this| {
                    this.child(section(
                        "Problems",
                        v_flex()
                            .gap_1()
                            .children(
                                entry
                                    .problems
                                    .iter()
                                    .map(|problem| line(problem.clone(), cx.theme().danger, cx)),
                            )
                            .into_any_element(),
                        cx,
                    ))
                })
                .child(section("Requires", requirements, cx))
                .when(!entry.provides.is_empty(), |this| {
                    this.child(section(
                        "Provides",
                        v_flex()
                            .gap_1()
                            .children(
                                entry.provides.iter().map(|contract| {
                                    line(contract.clone(), cx.theme().foreground, cx)
                                }),
                            )
                            .into_any_element(),
                        cx,
                    ))
                })
                .when(!entry.required_by.is_empty(), |this| {
                    this.child(section(
                        "Required by",
                        v_flex()
                            .gap_1()
                            .children(
                                entry
                                    .required_by
                                    .iter()
                                    .map(|id| line(name_of(id), cx.theme().foreground, cx)),
                            )
                            .into_any_element(),
                        cx,
                    ))
                })
                .children(
                    entry
                        .groups
                        .iter()
                        .map(|group| contribution_group(group, cx)),
                )
                .when(entry.groups.is_empty(), |this| {
                    this.child(muted("Registers nothing.", cx))
                }),
        )
}

fn contribution_group(group: &Group, cx: &mut Context<PluginManagerWindow>) -> AnyElement {
    section_titled(
        format!("{} · {}", group.title, group.items.len()),
        v_flex()
            .gap_2()
            .children(group.items.iter().map(|item| {
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(item.label.clone()),
                            )
                            .when(item.label != item.id, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .font_family("monospace")
                                        .text_color(cx.theme().muted_foreground)
                                        .child(item.id.clone()),
                                )
                            }),
                    )
                    .children(
                        item.details
                            .iter()
                            .map(|detail| line(detail.clone(), cx.theme().muted_foreground, cx)),
                    )
            }))
            .into_any_element(),
        cx,
    )
    .into_any_element()
}

fn section(
    title: &'static str,
    body: AnyElement,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    section_titled(title.to_string(), body, cx)
}

fn section_titled(
    title: String,
    body: AnyElement,
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
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
        .child(body)
}

fn tag(text: &str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .px_1p5()
        .py_px()
        .rounded_sm()
        .bg(cx.theme().secondary.opacity(0.65))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

fn muted(text: &'static str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn line(text: String, color: Hsla, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            Icon::new(IconName::ChevronRight)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        )
        .child(div().text_xs().text_color(color).child(text))
}
