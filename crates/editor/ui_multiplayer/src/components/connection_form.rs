use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{
    button::Button, h_flex, input::TextInput, v_flex, ActiveTheme as _, Disableable as _, Icon,
    IconName,
};

use crate::handlers;
use crate::screen::MultiplayerWindow;
use crate::utils::types::ConnectionStatus;

fn field_label(label: &'static str, cx: &Context<MultiplayerWindow>) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

pub fn render_connection_form(
    this: &MultiplayerWindow,
    cx: &mut Context<MultiplayerWindow>,
) -> impl IntoElement {
    let server_missing = this
        .server_address_input
        .read(cx)
        .text()
        .to_string()
        .is_empty();
    let source_build = engine_state::EngineContext::global()
        .map(|ctx| {
            ctx.store
                .get_or_init::<engine_state::DevContext>()
                .read()
                .is_source_build
        })
        .unwrap_or(false);
    let intro = v_flex()
        .gap_1()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::User)
                        .size(px(18.))
                        .text_color(cx.theme().primary),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(cx.theme().primary)
                        .child("REALTIME WORKSPACE"),
                ),
        )
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(cx.theme().foreground)
                .child("Work together"),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Create a session or join your team to synchronize project changes."),
        );
    let server = v_flex()
        .gap_2()
        .p_4()
        .rounded(px(10.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .child(field_label("RELAY SERVER", cx))
        .child(TextInput::new(&this.server_address_input))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Use the server address provided by your team."),
        );
    let create =
        v_flex()
            .gap_4()
            .p_4()
            .rounded(px(10.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Plus)
                            .size(px(16.))
                            .text_color(cx.theme().success),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child("Start a session"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Invite others with the session credentials."),
                            ),
                    ),
            )
            .child(
                Button::new("create-session")
                    .label("Create session")
                    .icon(IconName::Plus)
                    .w_full()
                    .disabled(server_missing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        handlers::on_create_session(this, window, cx)
                    })),
            );
    let join = v_flex()
        .gap_4()
        .p_4()
        .rounded(px(10.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::LogIn)
                        .size(px(16.))
                        .text_color(cx.theme().primary),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("Join a session"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Enter the ID and password from your invite."),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    v_flex()
                        .gap_1()
                        .child(field_label("SESSION ID", cx))
                        .child(TextInput::new(&this.session_id_input)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(field_label("PASSWORD", cx))
                        .child(TextInput::new(&this.session_password_input)),
                ),
        )
        .child(
            Button::new("join-session")
                .label("Join session")
                .icon(IconName::LogIn)
                .w_full()
                .disabled(server_missing)
                .on_click(
                    cx.listener(|this, _, window, cx| handlers::on_join_session(this, window, cx)),
                ),
        );

    v_flex().size_full().items_center().overflow_y_scroll().px_6().py_5()
        .child(v_flex().w_full().max_w(px(520.)).gap_5()
            .child(intro).child(server).child(create).child(join)
            .when(source_build, |form| form.child(
                v_flex().gap_3().p_4().rounded(px(10.)).border_1()
                    .border_color(cx.theme().primary.opacity(0.45)).bg(cx.theme().primary.opacity(0.06))
                    .child(h_flex().items_center().gap_2()
                        .child(Icon::new(IconName::Code).size(px(16.)).text_color(cx.theme().primary))
                        .child(v_flex().gap_0p5()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().foreground).child("Source build preview"))
                            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Explore the collaboration UI with a local fake teammate. No server needed."))))
                    .child(Button::new("source-preview-session").label("Join local preview session")
                        .icon(IconName::Code).w_full()
                        .on_click(cx.listener(|this, _, _, cx| this.join_source_preview(cx))))
            ))
            .when_some(match &this.connection_status {
                ConnectionStatus::Error(message) => Some(message.clone()), _ => None,
            }, |form, message| form.child(
                h_flex().gap_2().items_start().p_3().rounded(px(8.)).border_1()
                    .border_color(cx.theme().danger.opacity(0.55)).bg(cx.theme().danger.opacity(0.08))
                    .child(Icon::new(IconName::TriangleAlert).size(px(15.)).text_color(cx.theme().danger))
                    .child(div().text_xs().text_color(cx.theme().danger).child(message))
            )))
}
