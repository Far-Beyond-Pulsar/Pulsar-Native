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

#[cfg(test)]
mod tests {
    use super::*;

    fn participant(
        display: Option<&str>,
        login: Option<&str>,
        avatar: Option<&str>,
    ) -> MultiuserParticipant {
        MultiuserParticipant {
            peer_id: "peer-1".into(),
            display_name: display.map(String::from),
            avatar_url: avatar.map(String::from),
            github_login: login.map(String::from),
            ping_ms: None,
        }
    }

    #[test]
    fn label_prefers_display_name_then_login_then_peer_id() {
        assert_eq!(
            participant_label(&participant(Some("Ann"), Some("ann"), None)),
            "Ann"
        );
        assert_eq!(
            participant_label(&participant(None, Some("ann"), None)),
            "ann"
        );
        assert_eq!(participant_label(&participant(None, None, None)), "peer-1");
    }

    #[test]
    fn avatar_url_prefers_explicit_url_then_github_login() {
        let explicit = participant(None, Some("ann"), Some("https://img.test/a.png"));
        assert_eq!(
            participant_avatar_url(&explicit).as_deref(),
            Some("https://img.test/a.png")
        );
        let from_login = participant(None, Some("ann"), None);
        assert_eq!(
            participant_avatar_url(&from_login).as_deref(),
            Some("https://github.com/ann.png?size=64")
        );
        assert_eq!(participant_avatar_url(&participant(None, None, None)), None);
    }
}
