use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Icon, IconName, Sizable,
};

use crate::screen::ModernSettingsScreen;

pub fn render_save_bar(cx: &mut Context<ModernSettingsScreen>) -> impl IntoElement {
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
        .child(
            Button::new("save-settings")
                .primary()
                .small()
                .icon(IconName::Check)
                .label("Save")
                .on_click(cx.listener(|screen, _, _window, cx| {
                    crate::handlers::save_pending_changes(screen, cx);
                })),
        )
}
