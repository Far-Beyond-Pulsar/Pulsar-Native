use super::SettingsTab;
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon};

/// Render the tab switcher for Global/Project settings
pub fn render_tab_switcher<F>(
    active_tab: SettingsTab,
    on_tab_change: F,
    cx: &mut App,
) -> impl IntoElement
where
    F: Fn(SettingsTab) + 'static + Clone,
{
    let theme = cx.theme();

    h_flex()
        .w_full()
        .gap_2()
        .p_2()
        .bg(theme.background)
        .border_b_1()
        .border_color(theme.border)
        .child(render_tab_button(
            SettingsTab::Global,
            active_tab == SettingsTab::Global,
            on_tab_change.clone(),
            cx,
        ))
        .child(render_tab_button(
            SettingsTab::Project,
            active_tab == SettingsTab::Project,
            on_tab_change,
            cx,
        ))
}

fn render_tab_button<F>(
    tab: SettingsTab,
    is_active: bool,
    on_click: F,
    cx: &mut App,
) -> impl IntoElement
where
    F: Fn(SettingsTab) + 'static,
{
    let theme = cx.theme();

    div()
        .flex_1()
        .px_6()
        .py_3()
        .rounded_lg()
        .cursor_pointer()
        .when(is_active, |this| {
            this.bg(theme.primary)
        })
        .when(!is_active, |this| {
            this.bg(theme.secondary.opacity(0.3))
                .hover(|style| style.bg(theme.secondary.opacity(0.5)))
        })
        .on_mouse_down(MouseButton::Left, move |_event, _, _| {
            on_click(tab);
        })
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(tab.icon())
                        .size(px(20.0))
                        .text_color(if is_active {
                            theme.primary_foreground
                        } else {
                            theme.foreground
                        })
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_base()
                                .font_weight(if is_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if is_active {
                                    theme.primary_foreground
                                } else {
                                    theme.foreground
                                })
                                .child(tab.label())
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_active {
                                    theme.primary_foreground.opacity(0.8)
                                } else {
                                    theme.muted_foreground
                                })
                                .child(tab.description())
                        )
                )
        )
}
