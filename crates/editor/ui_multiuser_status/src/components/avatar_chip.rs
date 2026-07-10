use engine_state::MultiuserParticipant;
use gpui::{AnyElement, App, ImageSource, IntoElement, ObjectFit, ParentElement, Styled, StyledImage, div, img, px};
use std::sync::Arc;
use ui::{ActiveTheme as _, StyledExt};

use crate::utils::{
    avatar_cache, fetch_avatar_image, participant_avatar_url, participant_label,
};

pub fn avatar_chip_with_image(participant: &MultiuserParticipant, cx: &App) -> AnyElement {
    let name = participant_label(participant);

    if let Some(avatar_url) = participant_avatar_url(participant) {
        let cache = avatar_cache();

        if let Some(cached_image) = cache.read().get(&avatar_url) {
            if cached_image.frame_count() > 0 {
                return img(ImageSource::Render(cached_image))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .flex_shrink()
                    .into_any_element();
            }
        } else {
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
                        cache_clone.write().insert(
                            url.clone(),
                            Arc::new(gpui::RenderImage::new(smallvec::smallvec![])),
                        );
                    }
                }
            });
        }
    }

    avatar_chip_initials(name, cx)
}

pub fn avatar_chip_initials(name: String, cx: &App) -> AnyElement {
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
