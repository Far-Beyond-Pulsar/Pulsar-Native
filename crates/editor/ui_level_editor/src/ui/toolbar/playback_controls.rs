use engine_state::playback::{PlayPhase, PlaybackState};
use pulsar_events::{PlaybackCommand, publish_playback_command};
use gpui::*;
use rust_i18n::t;
use ui::{
    IconName, Selectable,
    button::{Button, ButtonVariants as _},
};

/// Play / Pause / Step / Stop for the engine's play session.
///
/// Purely a view of [`PlaybackState`] that sends [`PlaybackCommand`]s; whichever
/// editor is the playback host does the work.
pub struct PlaybackControls;

impl PlaybackControls {
    pub fn render(state: &PlaybackState) -> impl IntoElement {
        let stopped = state.phase == PlayPhase::Stopped;
        let can_control = state.phase == PlayPhase::Playing && state.supports_control;
        let paused = state.paused;

        ui::h_flex()
            .gap_1p5()
            .items_center()
            .child(if stopped {
                Button::new("play")
                    .icon(IconName::Play)
                    .tooltip(t!("LevelEditor.Toolbar.StartSimulation"))
                    .on_click(|_, _, _| send(PlaybackCommand::Play))
                    .into_any_element()
            } else {
                // Native hot reload (#653): Play while a game runs rebuilds it
                // and swaps the dylib without stopping the world.
                Button::new("play_active")
                    .icon(IconName::Play)
                    .tooltip(t!("LevelEditor.Toolbar.ReloadSimulation"))
                    .selected(true)
                    .on_click(|_, _, _| send(PlaybackCommand::Play))
                    .into_any_element()
            })
            .child({
                let btn = Button::new("pause")
                    .icon(if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .tooltip(if paused {
                        t!("LevelEditor.Toolbar.ResumeSimulation")
                    } else {
                        t!("LevelEditor.Toolbar.PauseSimulation")
                    })
                    .ghost()
                    .selected(paused)
                    .on_click(|_, _, _| send(PlaybackCommand::TogglePause));
                if can_control {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                let btn = Button::new("step")
                    .icon(IconName::SkipNext)
                    .tooltip(t!("LevelEditor.Toolbar.StepSimulation"))
                    .ghost()
                    .on_click(|_, _, _| send(PlaybackCommand::Step));
                if can_control && paused {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                let btn = Button::new("stop")
                    .icon(IconName::Square)
                    .tooltip(t!("LevelEditor.Toolbar.StopSimulation"))
                    .on_click(|_, _, _| send(PlaybackCommand::Stop));
                if stopped {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
    }
}

fn send(command: PlaybackCommand) {
    publish_playback_command(command);
}
