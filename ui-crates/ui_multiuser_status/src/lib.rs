use engine_state::{EngineContext, MultiuserMode, MultiuserStatus};
use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px,
};
use ui::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex};

pub fn render_status_bar_indicator(cx: &App) -> AnyElement {
    let (icon, color, label) = if let Some(engine) = EngineContext::global() {
        if let Some(session) = engine.multiuser() {
            let mode = match session.mode {
                MultiuserMode::CloudProject => "Cloud",
                MultiuserMode::PeerToPeer => "P2P",
            };

            let connected_label = if session.participant_count() > 0 {
                format!("{} · {}", mode, session.participant_count())
            } else {
                mode.to_string()
            };

            match session.status {
                MultiuserStatus::Connected => (IconName::User, cx.theme().success, connected_label),
                MultiuserStatus::Connecting => (
                    IconName::Loader,
                    cx.theme().warning,
                    format!("{mode} · Connecting"),
                ),
                MultiuserStatus::Disconnected => (
                    IconName::Circle,
                    cx.theme().muted_foreground,
                    format!("{mode} · Offline"),
                ),
                MultiuserStatus::Error(_) => (
                    IconName::TriangleAlert,
                    cx.theme().danger,
                    format!("{mode} · Error"),
                ),
            }
        } else {
            idle_state(cx)
        }
    } else {
        idle_state(cx)
    };

    h_flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(px(4.0))
        .child(Icon::new(icon).size(px(14.0)).text_color(color))
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(text_color_for_status(color, cx))
                .child(label),
        )
        .into_any_element()
}

fn idle_state(cx: &App) -> (IconName, Hsla, String) {
    (
        IconName::Circle,
        cx.theme().muted_foreground,
        "Multiuser Off".to_string(),
    )
}

fn text_color_for_status(color: Hsla, cx: &App) -> Hsla {
    if color == cx.theme().muted_foreground {
        cx.theme().muted_foreground
    } else {
        cx.theme().foreground
    }
}
