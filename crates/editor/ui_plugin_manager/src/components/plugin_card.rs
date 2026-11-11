use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt};

use crate::model::{Entry, EntryKind};

/// One row of the list: name, what it contributes, its kind and state.
pub fn render_plugin_item(
    entry: &Entry,
    selected: bool,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let open_id = entry.id.clone();
    let summary = entry
        .groups
        .iter()
        .map(|group| format!("{} {}", group.items.len(), group.title.to_lowercase()))
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    let attention = entry.needs_attention();
    let badge = if entry.kind == EntryKind::Engine {
        "Engine".to_string()
    } else if entry.enabled {
        entry.kind.label().to_string()
    } else {
        format!("{} · off", entry.kind.label())
    };

    h_flex()
        .id(SharedString::from(format!("plugin-{}", entry.id)))
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
        .when(!entry.enabled, |row| row.opacity(0.6))
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
                    Icon::new(match entry.kind {
                        EntryKind::Engine => IconName::Cpu,
                        EntryKind::Library => IconName::Box,
                        _ => IconName::Puzzle,
                    })
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
                                .child(entry.name.clone()),
                        )
                        .when(attention, |this| {
                            this.child(
                                Icon::new(IconName::TriangleAlert)
                                    .size_3()
                                    .text_color(cx.theme().warning),
                            )
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if summary.is_empty() {
                            format!("{} · contributes nothing", entry.id)
                        } else {
                            summary
                        }),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .px_1p5()
                .py_px()
                .rounded_sm()
                .bg(if entry.enabled {
                    cx.theme().success.opacity(0.12)
                } else {
                    cx.theme().secondary.opacity(0.55)
                })
                .text_xs()
                .text_color(if entry.enabled {
                    cx.theme().success
                } else {
                    cx.theme().muted_foreground
                })
                .child(badge),
        )
}
