use gpui::prelude::FluentBuilder;
use gpui::*;
use plugin_editor_api::PluginMetadata;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt,
};

pub fn render_plugin_item(
    plugin: &PluginMetadata,
    is_builtin: bool,
    cx: &mut Context<crate::screen::PluginManagerWindow>,
) -> impl IntoElement {
    let plugin_id = plugin.id.clone();
    let plugin_name = plugin.name.clone();
    let plugin_version = plugin.version.clone();
    let plugin_author = plugin.author.clone();

    let action = if is_builtin {
        h_flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().success.opacity(0.14))
            .text_xs()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(cx.theme().success)
            .child(Icon::new(IconName::Check).size_3())
            .child("Built-in")
            .into_any_element()
    } else {
        // A plugin's library is never unloaded (see the permanent library
        // pattern in `.agents/PLUGIN_SYSTEM.md`): its types and vtables may be
        // held anywhere in the editor. The button says so instead of acting.
        Button::new(SharedString::from(format!("unload-{}", plugin_id)))
            .label("Unload")
            .icon(IconName::Trash)
            .danger()
            .disabled(true)
            .tooltip("Plugins stay loaded until the editor restarts")
            .into_any_element()
    };

    h_flex()
        .items_start()
        .w_full()
        .p_5()
        .gap_4()
        .rounded_xl()
        .border_1()
        .border_color(cx.theme().border.opacity(0.65))
        .bg(cx.theme().sidebar.opacity(0.7))
        .hover(|style| style.bg(cx.theme().sidebar).border_color(cx.theme().border))
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
                        .when(!is_builtin, |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_px()
                                    .rounded(px(4.))
                                    .bg(cx.theme().muted.opacity(0.3))
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().muted_foreground)
                                    .child(plugin_version),
                            )
                        }),
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
        .child(v_flex().flex_shrink_0().justify_center().child(action))
}
