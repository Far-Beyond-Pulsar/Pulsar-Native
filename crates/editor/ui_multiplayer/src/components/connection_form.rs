use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button,
    clipboard::Clipboard,
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt,
};

use crate::screen::MultiplayerWindow;
use crate::utils::types::ConnectionStatus;
use crate::handlers;

pub fn render_connection_form(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    v_flex()
        .gap_4()
        .p_4()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::User)
                        .size(px(24.))
                        .text_color(cx.theme().primary),
                )
                .child(
                    div()
                        .text_lg()
                        .font_bold()
                        .text_color(cx.theme().foreground)
                        .child("Multiplayer Collaboration"),
                ),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(cx.theme().foreground)
                        .child("Server Address"),
                )
                .child(TextInput::new(&this.server_address_input)),
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .text_color(cx.theme().muted_foreground)
                        .child("CREATE NEW SESSION"),
                )
                .child(
                    Button::new("create-session")
                        .label("Create New Session")
                        .icon(IconName::Plus)
                        .w_full()
                        .disabled(
                            this.server_address_input
                                .read(cx)
                                .text()
                                .to_string()
                                .is_empty(),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            handlers::on_create_session(this, window, cx);
                        })),
                ),
        )
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(div().flex_1().h(px(1.)).bg(cx.theme().border))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("OR"),
                )
                .child(div().flex_1().h(px(1.)).bg(cx.theme().border)),
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .text_color(cx.theme().muted_foreground)
                        .child("JOIN EXISTING SESSION"),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child("Session ID"),
                        )
                        .child(TextInput::new(&this.session_id_input)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child("Password"),
                        )
                        .child(TextInput::new(&this.session_password_input)),
                )
                .child(
                    Button::new("join-session")
                        .label("Join Session")
                        .icon(IconName::LogIn)
                        .w_full()
                        .disabled(
                            this.server_address_input
                                .read(cx)
                                .text()
                                .to_string()
                                .is_empty(),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            handlers::on_join_session(this, window, cx);
                        })),
                ),
        )
        .when_some(
            match &this.connection_status {
                ConnectionStatus::Error(msg) => Some(msg.clone()),
                _ => None,
            },
            |this, error_msg| {
                this.child(
                    div()
                        .p_3()
                        .rounded(px(6.))
                        .bg(cx.theme().danger.opacity(0.1))
                        .border_1()
                        .border_color(cx.theme().danger)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(16.))
                                        .text_color(cx.theme().danger),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().danger)
                                        .child(error_msg),
                                ),
                        ),
                )
            },
        )
}
