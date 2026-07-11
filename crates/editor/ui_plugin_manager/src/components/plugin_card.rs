use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::PluginMetadata;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};

pub fn render_plugin_item(
    plugin: &PluginMetadata,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let plugin_id = plugin.id.clone();
    let plugin_name = plugin.name.clone();
    let plugin_version = plugin.version.clone();
    let plugin_author = plugin.author.clone();

    h_flex()
        .w_full()
        .p_4()
        .gap_4()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar.opacity(0.5))
        .hover(|style| style.bg(cx.theme().sidebar))
        .child(
            div()
                .flex_shrink_0()
                .size(px(48.))
                .rounded_lg()
                .bg(cx.theme().primary.opacity(0.1))
                .border_1()
                .border_color(cx.theme().primary.opacity(0.3))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Puzzle)
                        .size(px(24.))
                        .text_color(cx.theme().primary),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_base()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(plugin_name),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_px()
                                .rounded(px(4.))
                                .bg(cx.theme().muted.opacity(0.3))
                                .text_xs()
                                .font_family("monospace")
                                .text_color(cx.theme().muted_foreground)
                                .child(plugin_version),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("by {}", plugin_author)),
                )
                .when(!plugin.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground.opacity(0.8))
                            .child(plugin.description.clone()),
                    )
                }),
        )
        .child(
            v_flex()
                .flex_shrink_0()
                .gap_2()
                .justify_center()
                .child({
                    let id_for_button = plugin_id.clone();
                    Button::new(SharedString::from(format!("unload-{}", plugin_id)))
                        .label("Unload")
                        .icon(IconName::Trash)
                        .danger()
                        .on_click(cx.listener(move |_this, _, _window, _cx| {
                            todo!(
                                "Implement plugin unloading in PluginManager and call it here (plugin_id: {})",
                                id_for_button
                            );
                        }))
                }),
        )
}
