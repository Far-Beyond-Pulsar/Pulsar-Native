use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _,
};

use crate::screen::EntryScreen;

pub fn render_plugin_card(
    plugin: &crate::PluginRegistry,
    installed: bool,
    on_install: Option<Box<dyn Fn(&mut Window, &mut Context<EntryScreen>)>>,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let name = plugin.name.clone();
    let url = plugin.url.clone();
    let url_card = url.clone();

    v_flex()
        .id(SharedString::from(format!("plugin-{}", url)))
        .w_full()
        .p_3()
        .gap_2()
        .rounded_lg()
        .bg(theme.secondary.opacity(0.12))
        .border_1()
        .border_color(if installed {
            theme.success_foreground.opacity(0.3)
        } else {
            theme.border
        })
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .truncate()
                                .child(name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(url),
                        ),
                )
                .when_some(on_install, |this, install| {
                    if installed {
                        this.child(
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded_full()
                                .bg(theme.success_foreground.opacity(0.15))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.success_foreground)
                                .child("Installed"),
                        )
                    } else {
                        this.child(
                            Button::new(SharedString::from(format!("install-{}", url_card)))
                                .primary()
                                .label("Install")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    install(window, cx);
                                    cx.notify();
                                })),
                        )
                    }
                }),
        )
        .into_any_element()
}
