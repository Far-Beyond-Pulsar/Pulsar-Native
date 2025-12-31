//! Trait implementations for MultiplayerWindow

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::Button,
    h_flex, v_flex,
    ActiveTheme as _, StyledExt, TitleBar,
};

use super::state::MultiplayerWindow;

impl Focusable for MultiplayerWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}


impl Render for MultiplayerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Process any pending updates that need window access
        self.process_pending_updates(window, cx);

        if self.pending_file_sync.is_some() {
            tracing::debug!("RENDER: pending_file_sync present, FileSync tab should show it");
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(div().text_sm().child("Multiplayer"))
                            .when_some(self.active_session.as_ref(), |this, session| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded(px(3.))
                                                .bg(cx.theme().success)
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().foreground)
                                                .child(format!("{} users", session.connected_users.len()))
                                        )
                                )
                            })
                    )
            )
            .child(
                if let Some(ref session) = self.active_session {
                    self.render_active_session(session, cx).into_any_element()
                } else {
                    self.render_connection_form(cx).into_any_element()
                }
            )
    }
}
