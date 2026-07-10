use engine_state::{EngineContext, MultiuserParticipant, ResourceHandle};
use gpui::{App, Hsla};
use ui::{ActiveTheme, IconName};

use super::cache::AvatarCache;

/// Get or create the global avatar cache
pub fn avatar_cache() -> ResourceHandle<AvatarCache> {
    EngineContext::global()
        .expect("EngineContext not initialized")
        .store
        .get_or_init::<AvatarCache>()
}

pub fn idle_state(cx: &App) -> (IconName, Hsla, String, String, Vec<MultiuserParticipant>) {
    let label = "Multiuser Off".to_string();
    (
        IconName::Circle,
        cx.theme().muted_foreground,
        label.clone(),
        "No session connected".to_string(),
        Vec::new(),
    )
}

pub fn text_color_for_status(color: Hsla, cx: &App) -> Hsla {
    if color == cx.theme().muted_foreground {
        cx.theme().muted_foreground
    } else {
        cx.theme().foreground
    }
}

pub fn participant_label(participant: &MultiuserParticipant) -> String {
    participant
        .display_name
        .clone()
        .or_else(|| participant.github_login.clone())
        .unwrap_or_else(|| participant.peer_id.clone())
}

pub fn participant_avatar_url(participant: &MultiuserParticipant) -> Option<String> {
    participant.avatar_url.clone().or_else(|| {
        participant
            .github_login
            .as_ref()
            .map(|login| format!("https://github.com/{login}.png?size=64"))
    })
}
