//! Trait implementations for MultiplayerWindow

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    h_flex,
    v_flex,
    ActiveTheme as _,
    Icon,
    IconName,
    StyledExt as _,
    TitleBar,
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

        let kick_reason = engine_state::EngineContext::global().and_then(|ctx| {
            ctx.multiuser().and_then(|multiuser| match multiuser.status {
                engine_state::MultiuserStatus::Error(ref message)
                    if message.contains("Kicked from session") =>
                {
                    Some(message.clone())
                }
                _ => None,
            })
        });

        if self.pending_file_sync.is_some() {
            tracing::debug!("RENDER: pending_file_sync present, FileSync tab should show it");
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child(
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
                                            .bg(cx.theme().success),
                                    )
                                    .child(
                                        div().text_xs().text_color(cx.theme().foreground).child(
                                            format!("{} users", session.connected_users.len()),
                                        ),
                                    ),
                            )
                        }),
                ),
            )
            .child(if let Some(ref session) = self.active_session {
                self.render_active_session(session, cx).into_any_element()
            } else {
                self.render_connection_form(cx).into_any_element()
            })
            .when_some(kick_reason, |this, reason| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::rgba(0x000000dd))
                        .child(
                            v_flex()
                                .w(px(520.))
                                .gap_4()
                                .p_6()
                                .rounded(px(12.))
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .shadow_lg()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(24.))
                                                .text_color(cx.theme().danger),
                                        )
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_bold()
                                                .text_color(cx.theme().foreground)
                                                .child("Session ended"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(reason),
                                )
                                .child(
                                    Button::new("dismiss-kick")
                                        .label("Close")
                                        .w_full()
                                        .on_click(cx.listener(|_, _, _window, cx| {
                                            if let Some(ctx) = engine_state::EngineContext::global() {
                                                ctx.clear_multiuser();
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
    }
}

impl window_manager::PulsarWindow for MultiplayerWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "MultiplayerWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(500.0, 600.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let path = engine_state::get_project_path().map(std::path::PathBuf::from);
        cx.new(|cx| MultiplayerWindow::new(path, window, cx))
    }
}
