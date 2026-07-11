use gpui::{prelude::*, *};
use ui::{ActiveTheme, StyledExt, button::Button, h_flex, input::TextInput};

use crate::FabSearchWindow;

pub fn render_auth_section(
    window: &FabSearchWindow,
    fg: gpui::Hsla,
    muted_fg: gpui::Hsla,
    border_col: gpui::Hsla,
    cx: &mut Context<FabSearchWindow>,
) -> AnyElement {
    let logged_in = window.api_token.is_some();
    let me_name = window.me.as_ref().map(|m| m.display().to_string());
    let me_avatar = window
        .me
        .as_ref()
        .and_then(|m| m.avatar_url(64).map(|s| s.to_string()))
        .and_then(|url| window.image_cache.get(&url).and_then(|o| o.clone()));
    let me_loading = window.me_loading;
    let show_tok = window.show_token_input;
    let account_tier = window.me.as_ref().and_then(|m| m.account.clone());

    if me_loading {
        div()
            .text_xs()
            .text_color(muted_fg)
            .child("Verifying\u{2026}")
            .into_any_element()
    } else if logged_in {
        h_flex()
            .gap_2()
            .items_center()
            .map(|el| {
                if let Some(arc) = me_avatar {
                    el.child(
                        img(gpui::ImageSource::Render(arc))
                            .w_6()
                            .h_6()
                            .rounded_full()
                            .overflow_hidden(),
                    )
                } else {
                    el.child(div().w_6().h_6().rounded_full().bg(border_col))
                }
            })
            .when_some(me_name, |el, name| {
                el.child(div().text_xs().font_medium().text_color(fg).child(name))
            })
            .when_some(account_tier, |el, tier| {
                el.child(
                    div()
                        .text_xs()
                        .px_2()
                        .py(px(2.0))
                        .rounded_full()
                        .bg(gpui::rgb(0x6366F1))
                        .text_color(gpui::rgb(0xFFFFFF))
                        .child(tier),
                )
            })
            .child(
                Button::new("logout-btn")
                    .label("Logout")
                    .on_click(cx.listener(|this, _, _, cx| {
                        crate::handlers::on_clear_token(this, cx);
                    })),
            )
            .into_any_element()
    } else if show_tok {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w(px(300.0))
                    .child(TextInput::new(&window.token_input).w_full()),
            )
            .child(
                Button::new("verify-tok")
                    .label("Verify & Save")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        let tok = this.token_input.read(cx).value().to_string();
                        crate::handlers::on_set_token(this, tok, cx);
                    })),
            )
            .child(
                Button::new("cancel-tok")
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_token_input = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("Get your token at sketchfab.com/settings/password"),
            )
            .into_any_element()
    } else {
        Button::new("login-btn")
            .label("Login with API Token")
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_token_input = true;
                cx.notify();
            }))
            .into_any_element()
    }
}
