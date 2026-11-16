use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Icon, IconName, Sizable,
};

use crate::screen::ModernSettingsScreen;

/// The bar shown while changes are unsaved; `restart_required` when one of
/// them applies only after the editor restarts.
pub fn render_save_bar(
    restart_required: bool,
    cx: &mut Context<ModernSettingsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();

    h_flex()
        .w_full()
        .px_4()
        .py_2()
        .justify_end()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.sidebar)
        .when(restart_required, |bar| {
            bar.child(
                h_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Info))
                    .child("Restart the editor to apply device settings"),
            )
        })
        .child(
            Button::new("save-settings")
                .primary()
                .small()
                .icon(IconName::Check)
                .label("Save")
                .on_click(cx.listener(|screen, _, window, cx| {
                    crate::handlers::save_pending_changes(screen, window, cx);
                })),
        )
}
