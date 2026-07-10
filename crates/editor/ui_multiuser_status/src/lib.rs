//! Multi-user status bar indicator with profile picture display

mod avatar_cache;

pub use avatar_cache::{AvatarCache, fetch_avatar_image};

use engine_state::{
    EngineContext, MultiuserParticipant, MultiuserStatus, RelayConnectionMode, ResourceHandle,
};
use gpui::{
    AnyElement, App, Hsla, ImageSource, IntoElement, ObjectFit, ParentElement, Styled, StyledImage,
    div, img, prelude::FluentBuilder, px,
};
use std::sync::Arc;
use ui::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex};

/// Get or create the global avatar cache
fn avatar_cache() -> ResourceHandle<AvatarCache> {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .get_or_init::<AvatarCache>()
}

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

fn idle_state(cx: &App) -> (IconName, Hsla, String, String, Vec<MultiuserParticipant>) {
    let label = "Multiuser Off".to_string();
    (
        IconName::Circle,
        cx.theme().muted_foreground,
        label.clone(),
        "No session connected".to_string(),
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

fn participant_avatar_url(participant: &MultiuserParticipant) -> Option<String> {
    participant.avatar_url.clone().or_else(|| {
        participant
            .github_login
            .as_ref()
            .map(|login| format!("https://github.com/{login}.png?size=64"))
    })
}

/// Render avatar chip with profile picture if available, otherwise show initials
fn avatar_chip_with_image(participant: &MultiuserParticipant, cx: &App) -> AnyElement {
    let name = participant_label(participant);

    // Try to fetch and render profile picture if URL is available
    if let Some(avatar_url) = participant_avatar_url(participant) {
        let cache = avatar_cache();

        // Check if we have a cached image
        if let Some(cached_image) = cache.read().get(&avatar_url) {
            // Only render if it's a valid image (not the empty placeholder used for failed fetches)
            if cached_image.frame_count() > 0 {
                return img(ImageSource::Render(cached_image))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .flex_shrink()
                    .into_any_element();
            }
            // If it's the empty placeholder, fall through to initials
        } else {
            // Try to fetch if not in cache and not already fetching
            let url = avatar_url.clone();
            let cache_clone = cache.clone();
            std::thread::spawn(move || {
                match fetch_avatar_image(&url) {
                    Ok(image) => {
                        cache_clone.write().insert(url.clone(), image);
                        tracing::debug!("Fetched avatar from {}", url);
                    }
                    Err(e) => {
                        tracing::debug!("Failed to fetch avatar from {}: {}", url, e);
                        // Mark as attempted (store empty to avoid retrying)
                        cache_clone.write().insert(
                            url.clone(),
                            Arc::new(gpui::RenderImage::new(smallvec::smallvec![])),
                        );
                    }
                }
            });
        }
    }

    // Fallback to initials
    avatar_chip_initials(name, cx)
}

/// Render avatar chip with initials
fn avatar_chip_initials(name: String, cx: &App) -> AnyElement {
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
