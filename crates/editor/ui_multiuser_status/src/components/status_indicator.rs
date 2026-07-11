use engine_state::{
    EngineContext, MultiuserParticipant, MultiuserStatus, RelayConnectionMode,
};
use gpui::{AnyElement, App, Hsla, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use ui::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex};

use crate::components::avatar_chip::avatar_chip_with_image;
use crate::utils::{idle_state, participant_avatar_url, participant_label, text_color_for_status};

pub fn render_status_bar_indicator(cx: &App) -> AnyElement {
    let (icon, color, label, detail, participants) = if let Some(engine) = EngineContext::global() {
        if let Some(session) = engine.multiuser() {
            let mode = session.mode_label();

            let mut participants = if !session.participant_profiles.is_empty() {
                session.participant_profiles.clone()
            } else {
                session
                    .participants
                    .iter()
                    .map(|peer_id| MultiuserParticipant {
                        peer_id: peer_id.clone(),
                        display_name: None,
                        avatar_url: None,
                        github_login: None,
                        ping_ms: None,
                    })
                    .collect()
            };
            if let Some(local) = engine.auth_profile() {
                if !participants
                    .iter()
                    .any(|p| p.github_login.as_deref() == Some(local.login.as_str()))
                {
                    participants.insert(
                        0,
                        MultiuserParticipant {
                            peer_id: "local".to_string(),
                            display_name: local.display_name.clone().or(Some(local.login.clone())),
                            avatar_url: local.avatar_url.clone(),
                            github_login: Some(local.login),
                            ping_ms: session.latency_ms,
                        },
                    );
                }
            }

            let participant_count = participants.len();
            let ping_label = session
                .latency_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "latency unknown".to_string());
            let connection_target = if session.session_id.is_empty() {
                session.server_url.clone()
            } else {
                format!("{} / {}", session.server_url, session.session_id)
            };

            match session.status {
                MultiuserStatus::Connected { relay_mode } => {
                    let relay_label = match relay_mode {
                        Some(RelayConnectionMode::DirectP2P) => "Direct P2P",
                        Some(RelayConnectionMode::BinaryProxy) => "Proxy relay",
                        Some(RelayConnectionMode::JsonFallback) => "JSON fallback",
                        None => "Connection negotiated",
                    };
                    let role_label = if session.is_host { "Host" } else { "Joined" };
                    let label = format!("{mode} · Connected");
                    let detail = format!(
                        "{relay_label} · {role_label} · {participant_count} members · {ping_label}"
                    );
                    (
                        IconName::User,
                        cx.theme().success,
                        label,
                        detail,
                        participants,
                    )
                }
                MultiuserStatus::DegradedMode { relay_mode } => {
                    let relay_label = match relay_mode {
                        RelayConnectionMode::DirectP2P => "P2P fallback",
                        RelayConnectionMode::BinaryProxy => "Proxy fallback",
                        RelayConnectionMode::JsonFallback => "JSON fallback",
                    };
                    let label = format!("{mode} · Degraded");
                    let detail =
                        format!("{relay_label} · {participant_count} members · {ping_label}");
                    (
                        IconName::TriangleAlert,
                        cx.theme().warning,
                        label,
                        detail,
                        participants,
                    )
                }
                MultiuserStatus::Connecting => {
                    let label = format!("{mode} · Connecting");
                    let detail = format!("Joining {connection_target} · {ping_label}");
                    (
                        IconName::Loader,
                        cx.theme().warning,
                        label,
                        detail,
                        participants,
                    )
                }
                MultiuserStatus::Disconnected => {
                    let label = format!("{mode} · Offline");
                    let detail = "Not connected".to_string();
                    (
                        IconName::Circle,
                        cx.theme().muted_foreground,
                        label,
                        detail,
                        participants,
                    )
                }
                MultiuserStatus::Error(message) => {
                    let label = format!("{mode} · Error");
                    let detail = message.clone();
                    (
                        IconName::TriangleAlert,
                        cx.theme().danger,
                        label,
                        detail,
                        participants,
                    )
                }
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
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.18))
        .child(Icon::new(icon).size(px(14.0)).text_color(color))
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(text_color_for_status(color, cx))
                .child(label),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(detail),
        )
        .child(
            h_flex().gap_0p5().children(
                participants
                    .iter()
                    .map(|participant| avatar_chip_with_image(participant, cx)),
            ),
        )
        .into_any_element()
}
