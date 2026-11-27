//! The toolbar's Play control.
//!
//! A split button: the left half does what is selected, the chevron opens
//! [`BuildPicker`] to choose or to edit the build configurations. The
//! selection is either the built-in Play in Editor (it publishes a Play
//! command; the editor hosting play sessions runs the game) or a build
//! configuration, which it runs. While a build runs the control is a Cancel
//! button. Stop is the playback controls'.

use engine_state::build_config::{BuildConfigurations, build_configurations};
use engine_state::playback::{PlayPhase, PlaybackState};
use gpui::{App, Corner, Entity, IntoElement, ParentElement as _, Styled as _};
use pulsar_events::{PlaybackCommand, publish_playback_command};
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
use ui::popover::Popover;
use ui::{Disableable as _, IconName, Selectable as _, h_flex};

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

/// Render the Play control for the current state.
pub fn play_button(
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
                    .label(t!("Build.Button.Cancel").to_string())
                    .tooltip(t!("Build.Button.CancelTooltip").to_string())
                    .on_click(|_, _, _| cancel_build()),
            )
            .into_any_element();
    }

    let playing = state.phase != PlayPhase::Stopped;
    // Not while the launched game is using the project.
    let run = match configs.selected() {
        Some(config) => Button::new("build-run")
            .icon(config_icon(config))
            .label(shorten(&config.name))
            .tooltip(crate::text::subtitle(config))
            .disabled(playing || state.game_running)
            .on_click(|_, window, cx| {
                let selected = build_configurations().read().selected().cloned();
                if let Some(config) = selected {
                    run_configuration(&config, window, cx);
                }
            }),
        // Pressed again while playing, it reloads the running game.
        None => Button::new("play-in-editor")
            .icon(IconName::Play)
            .label(t!("Build.Button.PlayInEditor").to_string())
            .tooltip(if playing {
                t!("Build.Button.ReloadPlayTooltip").to_string()
            } else {
                t!("Build.Button.PlayInEditorTooltip").to_string()
            })
            .selected(playing)
            .disabled(state.game_running)
            .on_click(|_, _, _| publish_playback_command(PlaybackCommand::Play)),
    };

    let picker = picker.clone();
    let chooser = Popover::<BuildPicker>::new("build-picker-popover")
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new("build-choose")
                .ghost()
                .icon(IconName::ChevronDown)
                .tooltip(t!("Build.Button.ChooseTooltip").to_string())
                .disabled(playing || state.game_running),
        )
        .content(move |_, _| picker.clone());

    h_flex()
        .gap_px()
        .items_center()
        .child(run)
        .child(chooser)
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
