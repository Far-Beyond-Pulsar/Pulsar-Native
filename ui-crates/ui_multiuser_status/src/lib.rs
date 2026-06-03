use engine_state::{EngineContext, MultiuserMode, MultiuserParticipant, MultiuserStatus};
use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px,
};
use ui::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex};

pub fn render_status_bar_indicator(cx: &App) -> AnyElement {
    let (icon, color, label, ping_label, participants) = if let Some(engine) = EngineContext::global() {
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
                if !participants.iter().any(|p| p.github_login.as_deref() == Some(local.login.as_str()))
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

            let ping_label = session
                .latency_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "--ms".to_string());

            match session.status {
                MultiuserStatus::Connected => (
                    IconName::User,
                    cx.theme().success,
                    connected_label,
                    ping_label,
                    participants,
                ),
                MultiuserStatus::Connecting => (
                    IconName::Loader,
                    cx.theme().warning,
                    format!("{mode} · Connecting"),
                    ping_label,
                    participants,
                ),
                MultiuserStatus::Disconnected => (
                    IconName::Circle,
                    cx.theme().muted_foreground,
                    format!("{mode} · Offline"),
                    ping_label,
                    participants,
                ),
                MultiuserStatus::Error(_) => (
                    IconName::TriangleAlert,
                    cx.theme().danger,
                    format!("{mode} · Error"),
                    ping_label,
                    participants,
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
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(ping_label),
        )
        .children(participants.iter().take(6).map(|participant| {
            avatar_chip(participant_label(participant), cx)
        }))
        .into_any_element()
}

fn idle_state(cx: &App) -> (IconName, Hsla, String, String, Vec<MultiuserParticipant>) {
    (
        IconName::Circle,
        cx.theme().muted_foreground,
        "Multiuser Off".to_string(),
        "--ms".to_string(),
        Vec::new(),
    )
}

fn text_color_for_status(color: Hsla, cx: &App) -> Hsla {
    if color == cx.theme().muted_foreground {
        cx.theme().muted_foreground
    } else {
        cx.theme().foreground
    }
}

fn participant_label(participant: &MultiuserParticipant) -> String {
    participant
        .display_name
        .clone()
        .or_else(|| participant.github_login.clone())
        .unwrap_or_else(|| participant.peer_id.clone())
}

fn avatar_chip(name: String, cx: &App) -> AnyElement {
    let initials = name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let shown = if initials.is_empty() {
        name.chars().take(2).collect::<String>().to_uppercase()
    } else {
        initials
    };

    div()
        .w(px(16.0))
        .h(px(16.0))
        .rounded_full()
        .bg(cx.theme().secondary)
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().secondary_foreground)
        .flex()
        .items_center()
        .justify_center()
        .child(shown)
        .into_any_element()
}
