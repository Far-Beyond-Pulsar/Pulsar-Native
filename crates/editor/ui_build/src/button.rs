//! The toolbar's Build control.
//!
//! A split button: the left half runs the selected build configuration, the
//! chevron opens [`BuildPicker`] to choose another or edit them. While a build
//! runs the group is replaced by a Cancel button; while the game it launched
//! runs there is a Stop button beside it.

use engine_state::build_config::{BuildConfigurations, build_configurations};
use engine_state::playback::{PlayPhase, PlaybackState, game_process, playback};
use gpui::{
    App, Corner, Entity, IntoElement, ParentElement as _, Styled as _,
    prelude::FluentBuilder as _,
};
use ui::button::{Button, ButtonVariants as _};
use ui::popover::Popover;
use ui::{Disableable as _, IconName, h_flex};

use crate::picker::{BuildPicker, config_icon};
use crate::runner::{cancel_build, run_configuration};

/// Longest name shown on the button before it is cut with an ellipsis.
const MAX_LABEL: usize = 22;

fn shorten(name: &str) -> String {
    if name.chars().count() <= MAX_LABEL {
        return name.to_owned();
    }
    let kept: String = name.chars().take(MAX_LABEL - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Render the Build control for the current state.
pub fn build_button(
    state: &PlaybackState,
    configs: &BuildConfigurations,
    picker: &Entity<BuildPicker>,
    _cx: &mut App,
) -> impl IntoElement {
    // Building: the only sensible action is to cancel.
    if state.build_running {
        return h_flex()
            .child(
                Button::new("build-cancel")
                    .icon(IconName::Square)
                    .label("Cancel Build")
                    .tooltip("Stop the build")
                    .on_click(|_, _, _| cancel_build()),
            )
            .into_any_element();
    }

    let selected = configs.selected().cloned();
    // Not while a play session or the launched game is using the project.
    let busy = state.phase != PlayPhase::Stopped || state.game_running;
    let (icon, label, tooltip) = match &selected {
        Some(config) => (config_icon(config), shorten(&config.name), config.subtitle()),
        None => (IconName::Hammer, "No configuration".to_owned(), "Choose a build configuration".to_owned()),
    };

    let run = Button::new("build-run")
        .icon(icon)
        .label(label)
        .tooltip(tooltip)
        .disabled(busy || selected.is_none())
        .on_click(|_, window, cx| {
            let selected = build_configurations().read().selected().cloned();
            if let Some(config) = selected {
                run_configuration(&config, window, cx);
            }
        });

    let picker = picker.clone();
    let chooser = Popover::<BuildPicker>::new("build-picker-popover")
        .anchor(Corner::TopRight)
        .trigger(
            Button::new("build-choose")
                .ghost()
                .icon(IconName::ChevronDown)
                .tooltip("Choose build configuration")
                .disabled(busy),
        )
        .content(move |_, _| picker.clone());

    h_flex()
        .gap_1()
        .items_center()
        .child(h_flex().gap_px().items_center().child(run).child(chooser))
        .when(state.game_running, |el| {
            el.child(
                Button::new("build-stop-game")
                    .icon(IconName::Square)
                    .label("Stop")
                    .tooltip("Stop the running game")
                    .on_click(|_, _, _| {
                        let game = game_process();
                        if let Some(mut child) = game.read().0.lock().take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        playback().update(|s| s.game_running = false);
                    }),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_are_untouched() {
        assert_eq!(shorten("Build & Run"), "Build & Run");
    }

    #[test]
    fn long_names_are_cut_with_an_ellipsis() {
        let cut = shorten("A really quite long configuration name");
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= MAX_LABEL);
    }

    #[test]
    fn cutting_respects_multibyte_characters() {
        let name = "ビルド".repeat(20);
        assert!(shorten(&name).chars().count() <= MAX_LABEL);
    }
}
